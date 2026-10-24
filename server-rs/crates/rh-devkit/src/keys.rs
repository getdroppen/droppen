//! The M2 part B flows a headless device runs, as the browser does: signing
//! in as a second device, user keys and the account keyring, pairing by QR
//! offer and by numeric code (CPace through the hub), recovery with `RK`, and
//! removing a device with the key rotation that follows.

use std::collections::BTreeMap;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use rand::RngCore;
use rh_proto::chain::{recovery_keys, Action, ChainClock, ChainState, Entry, Signer};
use rh_proto::identity::fingerprint;
use rh_proto::keys::DhKeypair;
use rh_proto::pairing::{channel_id, Answer, Delivery, Hello, Offer};
use rh_proto::pake::{Cpace, Role};
use rh_proto::wrap::{self, context, Purpose, SealContext, SealedKey, WrappedKey};
use serde_json::{json, Value};
use zeroize::Zeroizing;

use crate::{err, get, hex, now_ms, post, unhex, Device, HubUrl, Result};

/// Mailbox purposes.
pub const MAIL_USER_KEY: u8 = 1;
pub const MAIL_MACHINE_KEY: u8 = 2;
pub const MAIL_PAIRING: u8 = 3;

impl Device {
    /// Sign in to an existing account as a new device (registered at the hub,
    /// not in the chain yet).
    pub async fn log_in(hub: &HubUrl, email: &str, password: &str) -> Result<Device> {
        let http = reqwest::Client::builder().no_proxy().build().map_err(err("http client"))?;
        let v: Value = post(
            &http,
            &format!("{}/v1/login", hub.http),
            None,
            &json!({"email": email, "password": password}),
        )
        .await?;
        let account = v["account"].as_str().ok_or("no account")?.to_string();
        let login = v["token"].as_str().ok_or("no token")?.to_string();
        let sign = SigningKey::generate(&mut OsRng);
        let dh = DhKeypair::generate(&mut OsRng);
        let (id, token) = crate::register_device(&http, hub, &account, &login, &sign, &dh).await?;
        Ok(Device {
            hub: hub.clone(),
            http,
            account,
            token,
            id,
            sign,
            dh,
            chain: None,
            clock: ChainClock::live(now_ms(), 0),
            user_keys: BTreeMap::new(),
        })
    }

    /// Fetch the chain from the hub and verify it (from genesis the first
    /// time, the new entries after that).
    pub async fn sync_chain(&mut self) -> Result<()> {
        let from = self.chain.as_ref().map_or(0, |c| c.seq() + 1);
        let v: Value = get(&self.http, &format!("{}/v1/chain?from={from}", self.hub.http), &self.token).await?;
        let entries: Vec<Entry> = v["entries"]
            .as_array()
            .ok_or("no entries")?
            .iter()
            .map(|e| unhex(e.as_str().unwrap_or_default()).and_then(|b| Entry::decode(&b).map_err(|e| e.to_string())))
            .collect::<Result<_>>()?;
        self.clock.set_now(now_ms());
        match &mut self.chain {
            Some(chain) => {
                chain.merge(&entries, &mut self.clock).map_err(err("chain"))?;
            }
            None if !entries.is_empty() => {
                self.clock = ChainClock::live(now_ms(), entries.len() as u64);
                self.chain = Some(ChainState::verify(&entries, None, &mut self.clock).map_err(err("chain"))?);
            }
            None => {}
        }
        Ok(())
    }

    /// Commit new key epochs in the chain (a `KeyEpochs` entry): a new user
    /// key, and a new key for each of `machines` — each the epoch after the
    /// chain's last, never one picked from anywhere else. The user key is
    /// kept. Returns the epochs: `(user, [(machine, epoch)])`.
    pub async fn commit_keys(&mut self, user: Option<[u8; 32]>, machines: &[(String, [u8; 32])]) -> Result<(Option<u32>, Vec<(String, u32)>)> {
        use rh_proto::chain::{machine_key_commit, user_key_commit, KeyCommit, KeyCommits};
        self.sync_chain().await?;
        let chain = self.chain.as_ref().ok_or("no chain")?;
        let user_epoch = user.map(|_| chain.user_key_epochs());
        let machine_epochs: Vec<(String, u32)> = machines.iter().map(|(m, _)| (m.clone(), chain.machine_key_epochs(m))).collect();
        let commits = KeyCommits {
            user: user.zip(user_epoch).map(|(key, epoch)| KeyCommit {
                epoch,
                hash: user_key_commit(&self.account, epoch, &key),
            }),
            machines: machines
                .iter()
                .zip(&machine_epochs)
                .map(|((m, key), (_, epoch))| {
                    (
                        m.clone(),
                        KeyCommit {
                            epoch: *epoch,
                            hash: machine_key_commit(&self.account, m, *epoch, key),
                        },
                    )
                })
                .collect(),
        };
        self.append(Action::KeyEpochs(commits)).await?;
        if let (Some(key), Some(epoch)) = (user, user_epoch) {
            self.user_keys.insert(epoch, key);
        }
        Ok((user_epoch, machine_epochs))
    }

    /// A fresh user key epoch, committed in the chain; returns it.
    pub async fn new_user_key(&mut self) -> Result<u32> {
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        Ok(self.commit_keys(Some(key), &[]).await?.0.expect("a user key"))
    }

    /// A fresh key epoch for `machine`, committed in the chain and wrapped
    /// into the keyring under the current user key (if this device has one):
    /// `(epoch, key)`, for the caller to hand to the agent.
    pub async fn issue_machine_key(&mut self, machine: &str) -> Result<(u32, [u8; 32])> {
        let mk = crate::random_key();
        let (_, epochs) = self.commit_keys(None, &[(machine.to_string(), mk)]).await?;
        let epoch = epochs[0].1;
        if self.current_user_epoch().is_some() {
            self.wrap_machine_key(machine, epoch, &mk).await?;
        }
        Ok((epoch, mk))
    }

    /// `SetAccept` for `host`. When it narrows who may control the host
    /// (chain v3 review, D2), the host's next machine key epoch goes in the
    /// same batch and is wrapped into the keyring: `Some((epoch, key))` for
    /// the caller to hand to the agent; `None` when nothing narrowed.
    pub async fn set_accept(&mut self, host: &str, from: rh_proto::chain::Accept) -> Result<Option<(u32, [u8; 32])>> {
        use rh_proto::chain::{machine_key_commit, KeyCommit, KeyCommits};
        self.sync_chain().await?;
        let chain = self.chain.as_ref().ok_or("no chain")?;
        let action = Action::SetAccept { host: host.into(), from };
        let mut probe = chain.clone();
        let entry = probe.draft(Signer::Device(self.id.clone()), action.clone(), now_ms()).sign(&self.sign);
        probe.apply(&entry, &mut ChainClock::history()).map_err(err("own entry"))?;
        if probe.host_key_due().is_none() {
            self.append(action).await?;
            return Ok(None);
        }
        let mk = crate::random_key();
        let epoch = chain.machine_key_epochs(host);
        let commit = KeyCommits {
            user: None,
            machines: vec![(
                host.into(),
                KeyCommit {
                    epoch,
                    hash: machine_key_commit(&self.account, host, epoch, &mk),
                },
            )],
        };
        self.append_batch(vec![action, Action::KeyEpochs(commit)]).await?;
        if self.current_user_epoch().is_some() {
            self.wrap_machine_key(host, epoch, &mk).await?;
        }
        Ok(Some((epoch, mk)))
    }

    pub fn current_user_epoch(&self) -> Option<u32> {
        self.user_keys.keys().next_back().copied()
    }

    /// Store a keyring blob; names are written once, so `Ok(false)` when
    /// the name already holds something (the same or another blob).
    pub async fn put_key(&self, name: &str, blob: &[u8]) -> Result<bool> {
        let (url, body) = (format!("{}/v1/keys/{name}", self.hub.http), json!({"blob": hex(blob)}));
        crate::traffic::http_out("PUT", &url, &body);
        let res = self
            .http
            .put(&url)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .map_err(err("request"))?;
        let status = res.status();
        let v: Value = res.json().await.unwrap_or(Value::Null);
        crate::traffic::http_in("PUT", &url, status.as_u16(), &v);
        if status == reqwest::StatusCode::CONFLICT {
            return Ok(false);
        }
        if !status.is_success() {
            return Err(format!("PUT /v1/keys/{name}: {status}"));
        }
        if v.is_null() {
            return Err("body: not JSON".into());
        }
        Ok(v["stored"].as_bool().unwrap_or(false))
    }

    pub async fn keyring(&self) -> Result<Vec<(String, Vec<u8>)>> {
        let v: Value = get(&self.http, &format!("{}/v1/keys", self.hub.http), &self.token).await?;
        v["keys"]
            .as_array()
            .ok_or("no keys")?
            .iter()
            .map(|k| {
                Ok((
                    k["name"].as_str().unwrap_or_default().to_string(),
                    unhex(k["blob"].as_str().unwrap_or_default())?,
                ))
            })
            .collect()
    }

    /// Put machine key `mk` (epoch `mk_epoch`) into the keyring, wrapped under
    /// the current user key.
    pub async fn wrap_machine_key(&self, machine: &str, mk_epoch: u32, mk: &[u8; 32]) -> Result<()> {
        let uk_epoch = self.current_user_epoch().ok_or("no user key")?;
        self.wrap_machine_key_under(machine, mk_epoch, uk_epoch, mk).await
    }

    /// [`Device::wrap_machine_key`] under user key epoch `uk_epoch`.
    pub async fn wrap_machine_key_under(&self, machine: &str, mk_epoch: u32, uk_epoch: u32, mk: &[u8; 32]) -> Result<()> {
        let uk = self.user_keys.get(&uk_epoch).ok_or("no such user key")?;
        let ctx = context::machine_key_under_user_key(&self.account, machine, mk_epoch, uk_epoch);
        let wrapped = wrap::wrap_under(uk, &ctx, mk, &mut OsRng).map_err(err("wrap"))?;
        // Names end with their writer, and the hub takes them only for committed epochs.
        self.put_key(&format!("mk/{machine}/{mk_epoch}/{uk_epoch}/{}", self.id), &wrapped.encode())
            .await?;
        Ok(())
    }

    /// Every machine key epoch of `machine` this device can unwrap — each
    /// exactly the key the chain committed (a blob planted under a stolen old
    /// user key, or any other, is skipped). Every copy the hub serves is
    /// tried, whoever wrote it ([`open_machine_copies`]).
    pub async fn machine_keys(&self, machine: &str) -> Result<BTreeMap<u32, Zeroizing<[u8; 32]>>> {
        let chain = self.chain.as_ref().ok_or("no chain")?;
        Ok(open_machine_copies(chain, &self.user_keys, machine, &self.keyring().await?))
    }

    /// Seal user key `epoch` into the recovery package (if the chain names a recovery key).
    pub async fn seal_recovery_package(&self, epoch: u32) -> Result<bool> {
        let chain = self.chain.as_ref().ok_or("no chain")?;
        let Some(rsk) = chain.recovery_key() else { return Ok(false) };
        let sealed =
            wrap::seal_for_recovery(&self.dh, &self.id, &self.account, &rsk, epoch, &self.user_keys[&epoch], &mut OsRng).map_err(err("seal"))?;
        // Written once: a package this device sealed before stands. The name
        // carries the recovery key's tag and ends with its writer (the sender).
        let tag: String = rsk[..8].iter().map(|b| format!("{b:02x}")).collect();
        self.put_key(&format!("rk/{epoch}/{}/{tag}/{}", self.id, self.id), &sealed.encode())
            .await?;
        Ok(true)
    }

    /// Create the account with a recovery key, as accounts were made before
    /// chain v3: a v2 genesis naming its RSK, user key 0 and its recovery
    /// package. Returns `RK` (what the words encode).
    #[deprecated(note = "chain v2: every new account is v3 from its genesis — use Device::create_genesis3(true); this stands only for an account made before chain v3")]
    pub async fn create_genesis_with_recovery(&mut self) -> Result<[u8; 16]> {
        let mut rk = [0u8; 16];
        OsRng.fill_bytes(&mut rk);
        let keys = recovery_keys(&rk);
        let ak = SigningKey::generate(&mut OsRng);
        let ak_pub = ak.verifying_key().to_bytes();
        let recovery = rh_proto::chain::RecoveryKey::new(&keys.signing, &self.account, &ak_pub);
        let entry = rh_proto::chain::genesis_draft(&self.account, ak_pub, self.info(), Some(recovery), now_ms()).sign(&ak);
        drop(ak);
        self.push_entries(std::slice::from_ref(&entry)).await?;
        self.clock.set_now(now_ms());
        self.chain = Some(ChainState::verify(&[entry], None, &mut self.clock).map_err(err("genesis"))?);
        let epoch = self.new_user_key().await?;
        self.seal_recovery_package(epoch).await?;
        Ok(rk)
    }

    /// Create the account as the web app does (`createGenesis`): on chain v3
    /// from its genesis — `Genesis3` naming this device the first admin (a
    /// device that runs no AI) and user key epoch 0 as a v3 `KeyEpochs`, in
    /// one batch — with a recovery key (and its package) when
    /// `with_recovery`. Returns `RK` (what the words encode), if any.
    /// [`Device::create_genesis`] and [`Device::create_genesis_with_recovery`]
    /// stand for an account made before chain v3.
    pub async fn create_genesis3(&mut self, with_recovery: bool) -> Result<Option<[u8; 16]>> {
        use rh_proto::chain::{genesis3_draft, user_key_commit, KeyCommit, KeyCommits, RecoveryKey};
        let rk = with_recovery.then(|| {
            let mut rk = [0u8; 16];
            OsRng.fill_bytes(&mut rk);
            rk
        });
        let ak = SigningKey::generate(&mut OsRng);
        let ak_pub = ak.verifying_key().to_bytes();
        let recovery = rk.map(|rk| RecoveryKey::new(&recovery_keys(&rk).signing, &self.account, &ak_pub));
        let genesis = genesis3_draft(&self.account, ak_pub, self.device_v3(false, true), recovery, now_ms()).sign(&ak);
        drop(ak);
        self.clock.set_now(now_ms());
        let mut chain = ChainState::verify(std::slice::from_ref(&genesis), None, &mut self.clock).map_err(err("genesis"))?;
        let mut uk = [0u8; 32];
        OsRng.fill_bytes(&mut uk);
        let first_key = KeyCommits {
            user: Some(KeyCommit {
                epoch: 0,
                hash: user_key_commit(&self.account, 0, &uk),
            }),
            machines: Vec::new(),
        };
        let commit = chain
            .draft(Signer::Device(self.id.clone()), Action::KeyEpochs(first_key), now_ms())
            .sign(&self.sign);
        chain.apply(&commit, &mut self.clock).map_err(err("own entry"))?;
        self.push_entries(&[genesis, commit]).await?;
        self.chain = Some(chain);
        self.user_keys.insert(0, uk);
        if rk.is_some() {
            self.seal_recovery_package(0).await?;
        }
        Ok(rk)
    }

    async fn push_entries(&self, entries: &[Entry]) -> Result<u64> {
        let v: Value = post(
            &self.http,
            &format!("{}/v1/chain", self.hub.http),
            Some(&self.token),
            &json!({"entries": entries.iter().map(|e| hex(&e.encode())).collect::<Vec<_>>()}),
        )
        .await?;
        v["seq"].as_u64().ok_or_else(|| "no seq".into())
    }

    pub async fn mail(&self, recipient: &str, purpose: u8, epoch: u32, sealed: &[u8]) -> Result<()> {
        let _: Value = post(
            &self.http,
            &format!("{}/v1/mailbox", self.hub.http),
            Some(&self.token),
            &json!({"recipient": recipient, "sender": self.id, "purpose": purpose, "epoch": epoch, "sealed": hex(sealed)}),
        )
        .await?;
        Ok(())
    }

    /// This device's mail, every page: `(id, sender, purpose, epoch, sealed)`.
    pub async fn my_mail(&self) -> Result<Vec<(i64, String, u8, u32, Vec<u8>)>> {
        let mut out = Vec::new();
        let mut after = 0i64;
        loop {
            let v: Value = get(&self.http, &format!("{}/v1/mailbox?after={after}", self.hub.http), &self.token).await?;
            let page = v["mail"].as_array().ok_or("no mail")?;
            for m in page {
                after = after.max(m["id"].as_i64().unwrap_or_default());
                out.push((
                    m["id"].as_i64().unwrap_or_default(),
                    m["sender"].as_str().unwrap_or_default().to_string(),
                    m["purpose"].as_u64().unwrap_or_default() as u8,
                    m["epoch"].as_u64().unwrap_or_default() as u32,
                    unhex(m["sealed"].as_str().unwrap_or_default())?,
                ));
            }
            if page.len() < 64 {
                return Ok(out);
            }
        }
    }

    async fn ack(&self, id: i64) -> Result<()> {
        let url = format!("{}/v1/mailbox/{id}", self.hub.http);
        let res = self.http.delete(&url).bearer_auth(&self.token).send().await.map_err(err("ack"))?;
        crate::traffic::http_in("DELETE", &url, res.status().as_u16(), &Value::Null);
        Ok(())
    }

    // ------------------------------------------------------------ pairing

    /// What this (new) device shows: its keys and a fresh secret.
    pub fn offer(&self) -> Offer {
        let mut secret = [0u8; 32];
        OsRng.fill_bytes(&mut secret);
        Offer {
            account: self.account.clone(),
            device: self.id.clone(),
            sign_key: self.sign.verifying_key().to_bytes(),
            dh_key: self.dh.public(),
            secret,
        }
    }

    /// An existing full device adds the device behind `offer` and mails it
    /// every user key epoch, MAC'd with `secret` — as chain v2's `AddDevice`,
    /// which a v3 chain refuses ([`Device::add_and_deliver3`]).
    #[deprecated(note = "chain v2's AddDevice: on a chain v3 account use Device::add_and_deliver3; this stands only for an account made before chain v3")]
    pub async fn add_and_deliver(&mut self, device: &str, sign_key: [u8; 32], dh_key: [u8; 32], secret: &[u8; 32]) -> Result<()> {
        self.sync_chain().await?;
        self.append(Action::AddDevice(rh_proto::chain::DeviceInfo {
            id: device.into(),
            sign_key,
            dh_key,
            full: true,
        }))
        .await?;
        let chain = self.chain.as_ref().ok_or("no chain")?;
        let uks: Vec<(u32, &[u8])> = self.user_keys.iter().map(|(e, k)| (*e, &k[..])).collect();
        let delivery = Delivery::create(
            chain,
            &self.id,
            self.sign.verifying_key().to_bytes(),
            &self.dh,
            device,
            &dh_key,
            &uks,
            secret,
            &mut OsRng,
        )
        .map_err(err("delivery"))?;
        self.mail(device, MAIL_PAIRING, 0, &delivery.encode()).await
    }

    /// Chain v3 (as the web's `pairDevice`): an admin adds the device behind
    /// `offer` — `AddDevice3`, a device that runs no AI, managing the account
    /// when `admin` — and mails it its pairing delivery, MAC'd with `secret`:
    /// every user key epoch for an admin, none for a device that is not one
    /// (it holds no user key, §3.3), which is mailed the machine keys of the
    /// hosts that accept it instead ([`Device::send_host_keys`], purpose 5).
    /// Only on a chain that holds a v3 entry.
    pub async fn add_and_deliver3(&mut self, device: &str, sign_key: [u8; 32], dh_key: [u8; 32], secret: &[u8; 32], admin: bool) -> Result<()> {
        self.sync_chain().await?;
        if !self.chain.as_ref().ok_or("no chain")?.is_v3() {
            return Err("the chain holds no v3 entry: a v3 device is added only to an account on chain v3".into());
        }
        self.append(Action::AddDevice3(rh_proto::chain::DeviceV3 {
            id: device.into(),
            sign_key,
            dh_key,
            flags: rh_proto::chain::Flags { host: false, admin },
        }))
        .await?;
        let chain = self.chain.as_ref().ok_or("no chain")?;
        let uks: Vec<(u32, &[u8])> = if admin {
            self.user_keys.iter().map(|(e, k)| (*e, &k[..])).collect()
        } else {
            Vec::new()
        };
        let delivery = Delivery::create(
            chain,
            &self.id,
            self.sign.verifying_key().to_bytes(),
            &self.dh,
            device,
            &dh_key,
            &uks,
            secret,
            &mut OsRng,
        )
        .map_err(err("delivery"))?;
        self.mail(device, MAIL_PAIRING, 0, &delivery.encode()).await?;
        // The pairing stands whatever happens to the host keys (as the web's
        // `pairDevice`): the device asks for what it lacks (purpose 4).
        if !admin {
            if let Err(e) = self.send_host_keys(device).await {
                eprintln!("rh-devkit: could not send {device} the hosts' keys (it asks again): {e}");
            }
        }
        Ok(())
    }

    /// The new device waits for its pairing delivery and opens it against the
    /// verified chain; returns how many user key epochs it got.
    pub async fn await_delivery(&mut self, secret: &[u8; 32], timeout: Duration) -> Result<usize> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            for (id, _, purpose, _, sealed) in self.my_mail().await? {
                if purpose != MAIL_PAIRING {
                    continue;
                }
                let delivery = Delivery::decode(&sealed).map_err(err("delivery"))?;
                delivery.check_tag(secret).map_err(err("delivery"))?;
                self.sync_chain().await?;
                let chain = self.chain.as_ref().ok_or("no chain")?;
                let keys = delivery
                    .open(secret, chain, &self.id, &self.sign.verifying_key().to_bytes(), &self.dh)
                    .map_err(err("delivery"))?;
                for (epoch, key) in &keys {
                    self.user_keys
                        .insert(*epoch, key.as_slice().try_into().map_err(|_| "a user key is 32 bytes")?);
                }
                self.ack(id).await?;
                return Ok(keys.len());
            }
            if tokio::time::Instant::now() > deadline {
                return Err("no pairing delivery arrived".into());
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    async fn pairing_post(&self, id: &str, from: &str, body: &[u8]) -> Result<()> {
        let _: Value = post(
            &self.http,
            &format!("{}/v1/pairings/{id}/messages", self.hub.http),
            Some(&self.token),
            &json!({"from": from, "body": hex(body)}),
        )
        .await?;
        Ok(())
    }

    async fn pairing_wait(&self, id: &str, from: &str, timeout: Duration) -> Result<Vec<u8>> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let v: Value = get(&self.http, &format!("{}/v1/pairings/{id}/messages", self.hub.http), &self.token).await?;
            if let Some(m) = v["messages"].as_array().into_iter().flatten().find(|m| m["from"] == from) {
                return unhex(m["body"].as_str().unwrap_or_default());
            }
            if tokio::time::Instant::now() > deadline {
                return Err(format!("no {from} message"));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    /// The new device's half of a numeric-code pairing: open the rendezvous,
    /// answer one run for `code`, then wait for the delivery MAC'd with the
    /// run's secret. The code is spent whatever happens.
    pub async fn pair_by_code(&mut self, code: &str, timeout: Duration) -> Result<usize> {
        let v: Value = post(
            &self.http,
            &format!("{}/v1/pairings", self.hub.http),
            Some(&self.token),
            &json!({"device": self.id}),
        )
        .await?;
        let id = v["id"].as_str().ok_or("no pairing id")?.to_string();
        let hello = Hello::decode(&self.pairing_wait(&id, "initiator", timeout).await?).map_err(err("hello"))?;
        let ci = channel_id(&self.account, &self.id);
        let (run, share) = Cpace::start(Role::Responder, code.as_bytes(), &ci, &hello.sid, &mut OsRng).map_err(err("cpace"))?;
        let keys = run.finish(&hello.share).map_err(err("cpace"))?;
        let answer = Answer::new(&hello, share, &self.id, self.sign.verifying_key().to_bytes(), self.dh.public(), &keys);
        self.pairing_post(&id, "responder", &answer.encode()).await?;
        // One run per code: whatever comes next is not answered, and the
        // rendezvous goes once the delivery is in (or never came).
        let secret = *keys.pair_secret;
        let got = self.await_delivery(&secret, timeout).await;
        self.pairing_close(&id).await;
        got
    }

    async fn pairing_close(&self, id: &str) {
        let _ = self
            .http
            .delete(format!("{}/v1/pairings/{id}", self.hub.http))
            .bearer_auth(&self.token)
            .send()
            .await;
    }

    /// The existing device's half: find the new device's rendezvous, run
    /// CPace with the code typed in, check the answer (a wrong code ends it),
    /// add the device and deliver.
    pub async fn pair_with_code(&mut self, new_device: &str, code: &str, timeout: Duration) -> Result<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        let id = loop {
            let v: Value = get(&self.http, &format!("{}/v1/pairings", self.hub.http), &self.token).await?;
            if let Some(p) = v["pairings"].as_array().into_iter().flatten().find(|p| p["device"] == new_device) {
                break p["id"].as_str().unwrap_or_default().to_string();
            }
            if tokio::time::Instant::now() > deadline {
                return Err("the new device never opened a pairing".into());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        };
        let mut sid = [0u8; 16];
        OsRng.fill_bytes(&mut sid);
        let ci = channel_id(&self.account, new_device);
        let (run, share) = Cpace::start(Role::Initiator, code.as_bytes(), &ci, &sid, &mut OsRng).map_err(err("cpace"))?;
        let hello = Hello { sid, share };
        self.pairing_post(&id, "initiator", &hello.encode()).await?;
        // The new device closes the rendezvous (only its creator may); it
        // answers this one run and no other.
        let answer = Answer::decode(&self.pairing_wait(&id, "responder", timeout).await?).map_err(err("answer"))?;
        let keys = run.finish(&answer.share).map_err(err("cpace"))?;
        answer.verify(&hello, &keys).map_err(err("answer"))?;
        if answer.device != new_device {
            return Err("the answer names another device".into());
        }
        let secret = *keys.pair_secret;
        // The chain's own version: a v3 admin on a v3 chain, v2's full device
        // on an account made before chain v3.
        self.sync_chain().await?;
        if self.chain.as_ref().ok_or("no chain")?.is_v3() {
            self.add_and_deliver3(new_device, answer.sign_key, answer.dh_key, &secret, true).await
        } else {
            #[allow(deprecated)]
            self.add_and_deliver(new_device, answer.sign_key, answer.dh_key, &secret).await
        }
    }

    // ------------------------------------------------------------ recovery

    /// A fresh device recovers with `RK`: opens the recovery package and
    /// joins as a pending device (signed with RSK).
    pub async fn recover(&mut self, rk: &[u8]) -> Result<usize> {
        self.sync_chain().await?;
        let keys = recovery_keys(rk);
        let chain = self.chain.as_ref().ok_or("no chain")?;
        let mut got = 0;
        for (name, blob) in self.keyring().await? {
            if let Some((epoch, uk)) = open_recovery_package(&keys, chain, &name, &blob) {
                self.user_keys
                    .insert(epoch, uk.as_slice().try_into().map_err(|_| "a user key is 32 bytes")?);
                got += 1;
            }
        }
        // On a v3 chain the join is v3 too (no v2 entry after a v3 one): a
        // device that comes back to manage the account, never a host.
        let join = if chain.is_v3() {
            Action::RecoveryJoin3(self.device_v3(false, true))
        } else {
            Action::RecoveryJoin(self.info())
        };
        let entry = chain.draft(Signer::Recovery, join, now_ms()).sign(&keys.signing);
        self.push_entries(&[entry]).await?;
        self.sync_chain().await?;
        Ok(got)
    }

    pub async fn approve(&mut self, device: &str) -> Result<u64> {
        self.sync_chain().await?;
        self.append(Action::ApproveDevice(device.into())).await
    }

    // ------------------------------------------------------------ revocation

    /// Remove `device` and rotate: a new user key epoch sealed to every
    /// remaining full device (mail) and to the recovery key, and a new machine
    /// key for every active machine, wrapped into the keyring. Returns the
    /// new `(uk_epoch, [(machine, mk_epoch, mk)])` for the caller to deliver
    /// to the agents (in a channel, or [`Device::mail_machine_key`]). Chain
    /// v2's `RemoveDevice`, which a v3 chain refuses
    /// ([`Device::remove_and_rotate3`]).
    #[deprecated(note = "chain v2's RemoveDevice: on a chain v3 account use Device::remove_and_rotate3 (or remove3_and_rotate_hosts); this stands only for an account made before chain v3")]
    pub async fn remove_and_rotate(&mut self, device: &str) -> Result<(u32, Vec<(String, u32, [u8; 32])>)> {
        self.sync_chain().await?;
        self.append(Action::RemoveDevice(device.into())).await?;
        self.rotate().await
    }

    /// Rotate: a new user key and a new key for every machine, committed in
    /// one `KeyEpochs` entry, then (in this order, so a device that refreshes
    /// on the mail finds them) the keyring blobs — the recovery package of
    /// every user key epoch this device holds, and each machine key under the
    /// new user key — then the new user key mailed to the other full devices.
    /// The machine keys are returned for the caller to hand to the agents.
    pub async fn rotate(&mut self) -> Result<(u32, Vec<(String, u32, [u8; 32])>)> {
        let mut uk = [0u8; 32];
        OsRng.fill_bytes(&mut uk);
        let chain = self.chain.clone().ok_or("no chain")?;
        let machines: Vec<(String, [u8; 32])> = chain
            .hosts()
            .filter(|m| m.is_current_host())
            .map(|m| (m.info.id.clone(), crate::random_key()))
            .collect();
        let (uk_epoch, epochs) = self.commit_keys(Some(uk), &machines).await?;
        let uk_epoch = uk_epoch.expect("a user key");
        for epoch in self.user_keys.keys().copied().collect::<Vec<_>>() {
            self.seal_recovery_package(epoch).await?;
        }
        let mut rotated = Vec::new();
        for ((machine, mk), (_, epoch)) in machines.iter().zip(&epochs) {
            self.wrap_machine_key(machine, *epoch, mk).await?;
            rotated.push((machine.clone(), *epoch, *mk));
        }
        if let Some((to, why)) = self.mail_user_key(uk_epoch).await?.into_iter().next() {
            return Err(format!("the new user key did not go to {to}: {why}"));
        }
        Ok((uk_epoch, rotated))
    }

    /// Seal user key `epoch` to every other current admin and mail it; the
    /// ones it did not reach, `(device, why)`.
    async fn mail_user_key(&self, epoch: u32) -> Result<Vec<(String, String)>> {
        let chain = self.chain.as_ref().ok_or("no chain")?;
        let uk = self.user_keys.get(&epoch).ok_or("no such user key")?;
        let mut failed = Vec::new();
        for d in chain.admins().filter(|d| d.info.id != self.id) {
            let ctx = SealContext {
                purpose: Purpose::UserKey,
                account: self.account.clone(),
                sender: self.id.clone(),
                recipient: d.info.id.clone(),
                epoch,
            };
            let sent = match wrap::seal_to(&self.dh, &d.info.dh_key, &ctx, uk, &mut OsRng) {
                Ok(sealed) => self.mail(&d.info.id, MAIL_USER_KEY, epoch, &sealed.encode()).await,
                Err(e) => Err(format!("seal: {e}")),
            };
            if let Err(why) = sent {
                failed.push((d.info.id.clone(), why));
            }
        }
        Ok(failed)
    }

    /// Whether a batch that issues machine keys may wrap them under the
    /// newest user key, as the web's `commitHostKey` and `rotate` decide
    /// before anything is committed: `Some(epoch)`, the newest epoch, which
    /// this device holds as the chain committed it; `None`, the newest is
    /// stale ([`user_key_stale`]: a device that held it no longer manages the
    /// account), so a new one must come in the same batch. An error when this
    /// device is not an admin, or lacks the newest user key (take it from
    /// the mail first). On this device's view of the chain: sync it first.
    pub fn usable_user_key(&self) -> Result<Option<u32>> {
        let chain = self.chain.as_ref().ok_or("no chain")?;
        if chain.admin(&self.id).is_none() {
            return Err("only a device that manages the account issues keys".into());
        }
        if user_key_stale(chain) {
            return Ok(None);
        }
        let latest = chain.latest_user_epoch().ok_or("the account has no user key yet")?;
        let uk = self
            .user_keys
            .get(&latest)
            .ok_or_else(|| format!("this device does not hold user key epoch {latest}, the newest (take it from the mail first)"))?;
        chain
            .check_user_key(latest, uk)
            .map_err(|e| format!("user key epoch {latest}: {e}"))?;
        Ok(Some(latest))
    }

    /// Keep a user key epoch just committed (as the web's `keepUserKey`):
    /// held here, and the recovery package of every epoch held. Not mailed
    /// yet ([`Device::share_user_key`], once the keyring holds what goes
    /// under it). What did not go is said in `failed`, never undoing the
    /// commit.
    async fn keep_user_key(&mut self, epoch: u32, uk: [u8; 32], failed: &mut Vec<String>) {
        self.user_keys.insert(epoch, uk);
        for e in self.user_keys.keys().copied().collect::<Vec<_>>() {
            if let Err(why) = self.seal_recovery_package(e).await {
                failed.push(format!("recovery package of user key epoch {e}: {why}"));
            }
        }
    }

    /// Mail user key `epoch` to the other admins (as the web's
    /// `mailUserKey`) — after the keyring copies that go under it, so a
    /// device that reads the keyring when the mail lands finds them all.
    async fn share_user_key(&self, epoch: u32, failed: &mut Vec<String>) {
        match self.mail_user_key(epoch).await {
            Ok(missed) => failed.extend(missed.into_iter().map(|(to, why)| format!("user key epoch {epoch} to {to}: {why}"))),
            Err(why) => failed.push(format!("user key epoch {epoch}: {why}")),
        }
    }

    /// Chain v3, as the web's `rotate`: `prefix` (a removal) and, in the same
    /// batch, a new machine key epoch for every current host the batch keeps,
    /// with a new user key epoch when `user_key`, or when the newest is
    /// stale. Machine keys alone go under the newest user key, which this
    /// device must hold ([`Device::usable_user_key`]: checked before anything
    /// is committed). Then, in this order, the new user key kept (recovery
    /// packages), each machine key wrapped into the keyring, the new user key
    /// mailed to the other admins, and each machine key sealed to the devices
    /// that are not admins and that its host accepts (purpose 5). Nothing
    /// after the commit undoes it:
    /// what did not go is logged and said in [`Rotation3::failed`].
    async fn rotate3(&mut self, prefix: Vec<Action>, user_key: bool) -> Result<Rotation3> {
        use rh_proto::chain::{machine_key_commit, user_key_commit, KeyCommit};
        self.sync_chain().await?;
        let chain = self.chain.clone().ok_or("no chain")?;
        if !chain.is_v3() {
            return Err("the chain holds no v3 entry: this rotation goes only on an account on chain v3".into());
        }
        if chain.admin(&self.id).is_none() {
            return Err("only a device that manages the account rotates the keys".into());
        }
        // Machine keys never go under a stale user key: a new one comes with them then.
        let with_user_key = user_key || user_key_stale(&chain);
        let wrap_epoch = if with_user_key { None } else { self.usable_user_key()? };
        // The hosts the batch takes out get no new key.
        let gone: Vec<&str> = prefix
            .iter()
            .filter_map(|a| match a {
                Action::RemoveDevice3(id) => Some(id.as_str()),
                _ => None,
            })
            .collect();
        let fresh = with_user_key.then(crate::random_key);
        let uk_epoch = chain.user_key_epochs();
        let machines: Vec<(String, u32, [u8; 32])> = chain
            .hosts()
            .filter(|h| h.is_current_host() && !gone.contains(&h.info.id.as_str()))
            .map(|h| (h.info.id.clone(), chain.machine_key_epochs(&h.info.id), crate::random_key()))
            .collect();
        let user = fresh.map(|uk| KeyCommit {
            epoch: uk_epoch,
            hash: user_key_commit(&self.account, uk_epoch, &uk),
        });
        let commits = machines
            .iter()
            .map(|(id, epoch, mk)| {
                (
                    id.clone(),
                    KeyCommit {
                        epoch: *epoch,
                        hash: machine_key_commit(&self.account, id, *epoch, mk),
                    },
                )
            })
            .collect();
        let mut actions = prefix;
        actions.extend(key_epochs(user, commits));
        self.append_batch(actions).await?;
        // Committed: now the keys may be used.
        let mut failed = Vec::new();
        if let Some(uk) = fresh {
            self.keep_user_key(uk_epoch, uk, &mut failed).await;
        }
        let wrap_under = if fresh.is_some() { uk_epoch } else { wrap_epoch.expect("checked") };
        for (host, epoch, mk) in &machines {
            if let Err(why) = self.wrap_machine_key_under(host, *epoch, wrap_under, mk).await {
                failed.push(format!("keyring copy of {host}'s key epoch {epoch}: {why}"));
            }
        }
        if fresh.is_some() {
            self.share_user_key(uk_epoch, &mut failed).await;
        }
        for (host, epoch, mk) in &machines {
            let mail = self.mail_host_key(host, *epoch, mk).await?;
            failed.extend(mail.failed.into_iter().map(|(to, why)| format!("{host}'s key epoch {epoch} to {to}: {why}")));
        }
        for why in &failed {
            eprintln!("rh-devkit: after the rotation: {why}");
        }
        Ok(Rotation3 {
            uk_epoch: fresh.map(|_| uk_epoch),
            machines,
            failed,
        })
    }

    /// Chain v3, as the web's `removeDevice`: remove `device` and, in the same
    /// batch, turn the keys over (`[RemoveDevice3, KeyEpochs…]`, as the web's
    /// `rotate`): a new machine key epoch for every host the
    /// account keeps, and a new user key epoch when `device` ever held the
    /// user key ([`held_user_key`], by the chain's history, not its flags
    /// now) or the newest is stale — never for a limited device or a host
    /// that never held it. The machine keys are in the report, for the caller
    /// to hand to the agents (in a channel, or [`Device::mail_machine_key`]).
    /// Only on a chain that holds a v3 entry.
    pub async fn remove_and_rotate3(&mut self, device: &str) -> Result<Rotation3> {
        self.sync_chain().await?;
        let held = held_user_key(self.chain.as_ref().ok_or("no chain")?, device);
        self.rotate3(vec![Action::RemoveDevice3(device.into())], held).await
    }

    /// Chain v3, as the web's `completeEnrollment`: wait for the agent's
    /// proof of `code`, check it, and add the host with exactly the keys it
    /// covers, its first machine key epoch `mk` in the same batch
    /// (`[AddDevice3{host}, KeyEpochs]`; never an admin, review D1). Before
    /// anything is asked of the hub, this device must hold a usable user key
    /// ([`Device::usable_user_key`]); while the newest is stale the batch
    /// carries a new one, which is kept as a rotation keeps it. Then, in the
    /// web's order, `mk` is wrapped into the keyring, a new user key mailed
    /// to the other admins, and `mk` sealed to the devices that are not
    /// admins and that the host accepts (none until an admin lets one). What
    /// did not go after the commit is in [`Enrolled3::failed`] (and logged):
    /// the host is in the chain either way. The caller hands `mk` to the
    /// agent ([`Device::mail_machine_key`], or a channel).
    /// [`Device::complete_enrollment_v3`] is the bare batch.
    pub async fn enroll_host3(&mut self, code: &rh_proto::identity::EnrollCode, mk: &[u8; 32], timeout: Duration) -> Result<Enrolled3> {
        use rh_proto::chain::{machine_key_commit, user_key_commit, DeviceV3, Flags, KeyCommit};
        self.sync_chain().await?;
        if !self.chain.as_ref().ok_or("no chain")?.is_v3() {
            return Err("the chain holds no v3 entry: a host joins this way only on an account on chain v3".into());
        }
        let wrap_epoch = self.usable_user_key()?;
        let machine = self.await_proof(code, timeout).await?;
        self.sync_chain().await?;
        let chain = self.chain.clone().ok_or("no chain")?;
        // On the chain as it stands now: a new user key when its newest is stale, or is not the one checked above.
        let fresh = (user_key_stale(&chain) || wrap_epoch.is_none() || chain.latest_user_epoch() != wrap_epoch).then(crate::random_key);
        let uk_epoch = chain.user_key_epochs();
        let mk_epoch = chain.machine_key_epochs(&machine.id);
        let user = fresh.map(|uk| KeyCommit {
            epoch: uk_epoch,
            hash: user_key_commit(&self.account, uk_epoch, &uk),
        });
        let commit = KeyCommit {
            epoch: mk_epoch,
            hash: machine_key_commit(&self.account, &machine.id, mk_epoch, mk),
        };
        let mut actions = vec![Action::AddDevice3(DeviceV3 {
            id: machine.id.clone(),
            sign_key: machine.sign_key,
            dh_key: machine.dh_key,
            flags: Flags { host: true, admin: false },
        })];
        actions.extend(key_epochs(user, vec![(machine.id.clone(), commit)]));
        self.append_batch(actions).await?;
        let mut failed = Vec::new();
        if let Some(uk) = fresh {
            self.keep_user_key(uk_epoch, uk, &mut failed).await;
        }
        let wrap_under = if fresh.is_some() { uk_epoch } else { wrap_epoch.expect("checked") };
        if let Err(why) = self.wrap_machine_key_under(&machine.id, mk_epoch, wrap_under, mk).await {
            failed.push(format!("keyring copy of {}'s key epoch {mk_epoch}: {why}", machine.id));
        }
        if fresh.is_some() {
            self.share_user_key(uk_epoch, &mut failed).await;
        }
        let mail = self.mail_host_key(&machine.id, mk_epoch, mk).await?;
        failed.extend(mail.failed.into_iter().map(|(to, why)| format!("{}'s key epoch {mk_epoch} to {to}: {why}", machine.id)));
        for why in &failed {
            eprintln!("rh-devkit: after adding {}: {why}", machine.id);
        }
        Ok(Enrolled3 {
            machine,
            mk_epoch,
            uk_epoch: fresh.map(|_| uk_epoch),
            failed,
        })
    }

    /// Chain v3, S1-4 (as the web's `mailHostKey`): seal host `host`'s key
    /// epoch `epoch` to every current device that is not an admin and that
    /// the host accepts control from, and mail it (purpose 5). A host's
    /// device role that has not signed in at the hub yet is `waiting` (it
    /// asks for its keys once it does). A mail that does not go is said in
    /// `failed` (and logged), the others still sent: the device asks for it
    /// again (purpose 4).
    pub async fn mail_host_key(&self, host: &str, epoch: u32, mk: &[u8; 32]) -> Result<HostKeyMail> {
        let chain = self.chain.as_ref().ok_or("no chain")?;
        let mut out = HostKeyMail::default();
        for d in chain.devices() {
            let id = &d.info.id;
            if !d.is_active() || d.admin || *id == self.id || chain.may_control(id, host).is_err() {
                continue;
            }
            match self.seal_host_key_to(id, &d.info.dh_key, host, epoch, mk).await {
                Ok(()) => out.sent.push(id.clone()),
                // The hub's refusal for a host's device role that never signed in (docs/hub-api/http.md, `POST /v1/mailbox`).
                Err(e) if d.host && e.contains("gets it once it signs in as a device") => out.waiting.push(id.clone()),
                Err(e) => {
                    eprintln!("rh-devkit: could not mail {host}'s key epoch {epoch} to {id}: {e}");
                    out.failed.push((id.clone(), e));
                }
            }
        }
        Ok(out)
    }

    async fn seal_host_key_to(&self, to: &str, to_dh: &[u8; 32], host: &str, epoch: u32, mk: &[u8; 32]) -> Result<()> {
        let ctx = wrap::HostKeyContext {
            account: self.account.clone(),
            sender: self.id.clone(),
            recipient: to.into(),
            host: host.into(),
            epoch,
        };
        let sealed = wrap::seal_host_key(&self.dh, to_dh, &ctx, mk, &mut OsRng).map_err(err("seal"))?;
        self.mail(to, wrap::MAIL_HOST_KEY, epoch, &wrap::encode_host_key_mail(host, &sealed))
            .await
    }

    /// Chain v3, S1-4: remove `device`, which never held the user key (a
    /// limited device, or a host's device role), and in the same batch turn
    /// every remaining host's machine key over — as the web's `rotate` with
    /// `userKey: false`: the user key stays, unless the
    /// newest is stale, when a new one comes with them. A device that ever
    /// held the user key is refused here: [`Device::remove_and_rotate3`]
    /// turns that over too.
    pub async fn remove3_and_rotate_hosts(&mut self, device: &str) -> Result<Rotation3> {
        self.sync_chain().await?;
        if held_user_key(self.chain.as_ref().ok_or("no chain")?, device) {
            return Err(format!("{device} held the user key: remove it with remove_and_rotate3, which turns that over too"));
        }
        self.rotate3(vec![Action::RemoveDevice3(device.into())], false).await
    }

    /// Chain v3, S1-4 (as the web's `sendHostKeys`): seal every committed
    /// machine-key epoch this admin can unwrap of every host that accepts
    /// control from `to` — a device that is not an admin, a host's device
    /// role included — and mail them (purpose 5). How many went.
    pub async fn send_host_keys(&mut self, to: &str) -> Result<usize> {
        self.sync_chain().await?;
        let chain = self.chain.clone().ok_or("no chain")?;
        let recipient = chain.device(to).ok_or("not a current device")?;
        if recipient.admin {
            return Err(format!("{to} manages the account: it reads the hosts' keys with the user key"));
        }
        let dh = recipient.info.dh_key;
        let hosts: Vec<String> = chain.hosts_admitting(to).map(|h| h.info.id.clone()).collect();
        let mut sent = 0;
        for host in hosts {
            for (epoch, mk) in self.machine_keys(&host).await? {
                self.seal_host_key_to(to, &dh, &host, epoch, &mk).await?;
                sent += 1;
            }
        }
        Ok(sent)
    }

    /// Chain v3, S1-4 (as the web's automatic answer): the key requests
    /// (purpose 4) in this admin's mailbox from current devices that are not
    /// admins are answered with [`Device::send_host_keys`] — never a user key
    /// — and acknowledged. Who was answered.
    pub async fn answer_key_requests(&mut self) -> Result<Vec<String>> {
        self.sync_chain().await?;
        let mut answered = Vec::new();
        for (id, sender, purpose, _, _) in self.my_mail().await? {
            if purpose != wrap::MAIL_KEY_REQUEST {
                continue;
            }
            self.ack(id).await?;
            let asks = self.chain.as_ref().and_then(|c| c.device(&sender)).is_some_and(|d| !d.admin);
            if asks && !answered.contains(&sender) {
                self.send_host_keys(&sender).await?;
                answered.push(sender);
            }
        }
        Ok(answered)
    }

    /// Take user key epochs other devices mailed this one (after a rotation).
    pub async fn take_user_key_mail(&mut self) -> Result<usize> {
        self.sync_chain().await?;
        let chain = self.chain.clone().ok_or("no chain")?;
        let mut got = 0;
        for (id, sender, purpose, epoch, sealed) in self.my_mail().await? {
            if purpose != MAIL_USER_KEY {
                continue;
            }
            let ctx = SealContext {
                purpose: Purpose::UserKey,
                account: self.account.clone(),
                sender,
                recipient: self.id.clone(),
                epoch,
            };
            let Ok(sealed) = SealedKey::decode(&sealed) else { continue };
            if let Ok(uk) = wrap::open_user_key(&self.dh, &self.id, &chain, &ctx, &sealed) {
                self.user_keys
                    .insert(epoch, uk.as_slice().try_into().map_err(|_| "a user key is 32 bytes")?);
                got += 1;
            }
            self.ack(id).await?;
        }
        Ok(got)
    }

    /// The account key's fingerprint as this device sees it.
    pub fn account_fingerprint(&self) -> Option<[u8; 16]> {
        self.chain.as_ref().map(|c| fingerprint(&c.account_key()))
    }
}

/// What a chain v3 rotation committed ([`Device::remove_and_rotate3`],
/// [`Device::remove3_and_rotate_hosts`]).
#[derive(Debug, Clone, Default)]
pub struct Rotation3 {
    /// The new user key epoch, if one was committed.
    pub uk_epoch: Option<u32>,
    /// `(host, epoch, key)`: each host's new machine key, for the caller to
    /// hand to its agent.
    pub machines: Vec<(String, u32, [u8; 32])>,
    /// What did not go after the commit (a mail, a keyring copy, a recovery
    /// package), in words: none of it undoes the rotation.
    pub failed: Vec<String>,
}

/// What [`Device::enroll_host3`] committed.
#[derive(Debug, Clone)]
pub struct Enrolled3 {
    /// The host, with exactly the keys its proof covers.
    pub machine: rh_proto::chain::MachineInfo,
    /// The epoch its first machine key was committed as.
    pub mk_epoch: u32,
    /// A new user key epoch the batch carried (the newest was stale).
    pub uk_epoch: Option<u32>,
    /// What did not go after the commit, in words (see [`Rotation3::failed`]).
    pub failed: Vec<String>,
}

/// Where one host key epoch went ([`Device::mail_host_key`]).
#[derive(Debug, Clone, Default)]
pub struct HostKeyMail {
    pub sent: Vec<String>,
    /// `(device, why)`.
    pub failed: Vec<(String, String)>,
    /// Hosts' device roles that have not signed in at the hub yet.
    pub waiting: Vec<String>,
}

/// `KeyEpochs` entries for `user` and `machines` (as the web's `rotate`):
/// the user key with the first [`rh_proto::chain::MAX_KEY_COMMITS`] machine
/// keys, the rest in entries of their own; none when there is nothing.
pub(crate) fn key_epochs(user: Option<rh_proto::chain::KeyCommit>, machines: Vec<(String, rh_proto::chain::KeyCommit)>) -> Vec<Action> {
    use rh_proto::chain::{KeyCommits, MAX_KEY_COMMITS};
    if user.is_none() && machines.is_empty() {
        return Vec::new();
    }
    let mut chunks = machines.chunks(MAX_KEY_COMMITS);
    let mut out = vec![Action::KeyEpochs(KeyCommits {
        user,
        machines: chunks.next().map(<[_]>::to_vec).unwrap_or_default(),
    })];
    out.extend(chunks.map(|c| {
        Action::KeyEpochs(KeyCommits {
            user: None,
            machines: c.to_vec(),
        })
    }));
    out
}

/// Whether device `id` ever held the user key, by `chain`'s entries (as the
/// web's `heldUserKey`, review 1) — not by its flags now: it was ever an
/// admin (a v2 full device, a v3 admin, made one by `SetAdmin`) or came
/// back with the recovery words.
pub fn held_user_key(chain: &ChainState, id: &str) -> bool {
    chain.entries().iter().any(|e| match &e.draft.action {
        Action::Genesis { device, .. } | Action::AddDevice(device) => device.id == id && device.full,
        Action::Genesis3 { device, .. } | Action::AddDevice3(device) => device.id == id && device.flags.admin,
        Action::RecoveryJoin(device) => device.id == id,
        Action::RecoveryJoin3(device) => device.id == id,
        Action::SetAdmin { id: who, admin } => who == id && *admin,
        _ => false,
    })
}

/// Whether the newest user key is still one a device that no longer manages
/// the account held (as the web's `userKeyStale`): no `KeyEpochs` with a
/// user key since the chain's last removal of a device that ever held it
/// ([`held_user_key`]) or its last `SetAdmin false`. Nothing new is wrapped
/// under it: a batch that issues machine keys then carries a new user key.
pub fn user_key_stale(chain: &ChainState) -> bool {
    let entries = chain.entries();
    let lost = entries.iter().rposition(|e| match &e.draft.action {
        Action::RemoveDevice(id) | Action::RemoveDevice3(id) => held_user_key(chain, id),
        Action::SetAdmin { admin, .. } => !*admin,
        _ => false,
    });
    let Some(lost) = lost.filter(|&seq| seq > 0) else { return false };
    !entries[lost + 1..]
        .iter()
        .any(|e| matches!(&e.draft.action, Action::KeyEpochs(k) if k.user.is_some()))
}

/// The machine key epochs of `machine` that `entries` (the keyring, `(name,
/// blob)`) hold under a user key in `user_keys` — each exactly the key
/// `chain` committed for its epoch. Every copy is tried, whoever wrote it: a
/// current admin, or one demoted or removed since (the hub serves
/// them all, and cannot tell a good one from a bad one); the commitment, not
/// the writer, is what makes one the account's key.
pub fn open_machine_copies(
    chain: &ChainState,
    user_keys: &BTreeMap<u32, [u8; 32]>,
    machine: &str,
    entries: &[(String, Vec<u8>)],
) -> BTreeMap<u32, Zeroizing<[u8; 32]>> {
    let mut out = BTreeMap::new();
    for (name, blob) in entries {
        let parts: Vec<&str> = name.split('/').collect();
        // `mk/<machine>/<mk epoch>/<uk epoch>[/<writer>]`
        let (["mk", m, mk_epoch, uk_epoch] | ["mk", m, mk_epoch, uk_epoch, _]) = parts.as_slice() else {
            continue;
        };
        if *m != machine {
            continue;
        }
        let (Ok(mk_epoch), Ok(uk_epoch)) = (mk_epoch.parse::<u32>(), uk_epoch.parse::<u32>()) else {
            continue;
        };
        if out.contains_key(&mk_epoch) {
            continue;
        }
        let Some(uk) = user_keys.get(&uk_epoch) else { continue };
        let ctx = context::machine_key_under_user_key(chain.account(), machine, mk_epoch, uk_epoch);
        let Ok(wrapped) = WrappedKey::decode(blob) else { continue };
        if let Ok(mk) = wrap::unwrap_under(uk, &ctx, &wrapped) {
            if let (true, Ok(mk)) = (
                chain.check_machine_key(machine, mk_epoch, &mk).is_ok(),
                <[u8; 32]>::try_from(mk.as_slice()),
            ) {
                out.insert(mk_epoch, Zeroizing::new(mk));
            }
        }
    }
    out
}

/// A recovery package `name` (`rk/<epoch>/<sender>`, `rk/<epoch>/<sender>/<RSK
/// prefix>`, or today's `rk/<epoch>/<sender>/<RSK prefix>/<writer>`) opened
/// with `RK`'s keys: `(epoch, user key)`, or `None`.
///
/// Sealed by any device `chain` names — a current admin, or one demoted or
/// removed since — and exactly the user key the chain committed for
/// that epoch ([`ChainState::check_user_key`]). The commitment is what makes
/// it the account's key: a device removed since could seal only a key the
/// chain already names, never one of its own nor an epoch out of reach. And
/// a package sealed by an admin no longer current can be the only way back
/// in for its epoch (the hub keeps one per sender and epoch for that), so it
/// must open — as [`wrap::open_from_recovery`], which this parses the name for.
pub fn open_recovery_package(keys: &rh_proto::chain::RecoveryKeys, chain: &ChainState, name: &str, blob: &[u8]) -> Option<(u32, Zeroizing<Vec<u8>>)> {
    let parts: Vec<&str> = name.split('/').collect();
    let (epoch, sender) = match parts.as_slice() {
        ["rk", epoch, sender] | ["rk", epoch, sender, _] | ["rk", epoch, sender, _, _] => (*epoch, *sender),
        _ => return None,
    };
    let epoch = epoch.parse::<u32>().ok()?;
    let ctx = SealContext {
        purpose: Purpose::UserKey,
        account: chain.account().to_string(),
        sender: sender.into(),
        recipient: wrap::RECOVERY_RECIPIENT.into(),
        epoch,
    };
    let sealed = SealedKey::decode(blob).ok()?;
    let key = wrap::open_from_recovery(keys, chain, &ctx, &sealed).ok()?;
    Some((epoch, key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rh_proto::chain::{
        genesis3_draft, genesis_draft, machine_key_commit, user_key_commit, DeviceInfo, DeviceV3, Flags, KeyCommit, KeyCommits, RecoveryKey,
        RecoveryKeys,
    };
    use rh_proto::identity::MachineIdentity;

    const ACCOUNT: &str = "a-1";
    const UK0: [u8; 32] = [0x10; 32];
    const MK0: [u8; 32] = [0x20; 32];

    struct Fixture {
        chain: ChainState,
        rk: RecoveryKeys,
        d1: MachineIdentity,
        d2: MachineIdentity,
    }

    fn apply(chain: &mut ChainState, by: &MachineIdentity, id: &str, action: Action) {
        let e = chain.draft(Signer::Device(id.into()), action, 2).sign(by.signing_key());
        chain.apply(&e, &mut ChainClock::history()).unwrap();
    }

    /// `d-1` founds the account with recovery words, admin `d-2` joins, host
    /// `m-1` gets machine key epoch 0 and the account user key epoch 0 (v3
    /// when `v3`).
    fn fixture(v3: bool) -> Fixture {
        let ak = MachineIdentity::from_seed(&[1; 32]);
        let (d1, d2, m) = (
            MachineIdentity::from_seed(&[2; 32]),
            MachineIdentity::from_seed(&[3; 32]),
            MachineIdentity::from_seed(&[4; 32]),
        );
        let rk = recovery_keys(&[9; 16]);
        let recovery = Some(RecoveryKey::new(&rk.signing, ACCOUNT, &ak.sign_public()));
        let v3_device = |id: &str, k: &MachineIdentity, host: bool| DeviceV3 {
            id: id.into(),
            sign_key: k.sign_public(),
            dh_key: k.dh_public(),
            flags: Flags { host, admin: !host },
        };
        let genesis = if v3 {
            genesis3_draft(ACCOUNT, ak.sign_public(), v3_device("d-1", &d1, false), recovery, 1)
        } else {
            let info = DeviceInfo {
                id: "d-1".into(),
                sign_key: d1.sign_public(),
                dh_key: d1.dh_public(),
                full: true,
            };
            genesis_draft(ACCOUNT, ak.sign_public(), info, recovery, 1)
        }
        .sign(ak.signing_key());
        let mut chain = ChainState::verify(&[genesis], None, &mut ChainClock::history()).unwrap();
        let keys = KeyCommits {
            user: Some(KeyCommit {
                epoch: 0,
                hash: user_key_commit(ACCOUNT, 0, &UK0),
            }),
            machines: vec![(
                "m-1".into(),
                KeyCommit {
                    epoch: 0,
                    hash: machine_key_commit(ACCOUNT, "m-1", 0, &MK0),
                },
            )],
        };
        let actions = if v3 {
            vec![
                Action::AddDevice3(v3_device("d-2", &d2, false)),
                Action::AddDevice3(v3_device("m-1", &m, true)),
                Action::KeyEpochs(keys),
            ]
        } else {
            let info = DeviceInfo {
                id: "d-2".into(),
                sign_key: d2.sign_public(),
                dh_key: d2.dh_public(),
                full: true,
            };
            vec![Action::AddDevice(info), Action::AddMachine(m.info("m-1")), Action::KeyEpochs(keys)]
        };
        for action in actions {
            apply(&mut chain, &d1, "d-1", action);
        }
        Fixture { chain, rk, d1, d2 }
    }

    fn package(f: &Fixture, by: &MachineIdentity, sender: &str, epoch: u32, uk: &[u8; 32]) -> Vec<u8> {
        let rsk = f.chain.recovery_key().unwrap();
        wrap::seal_for_recovery(by.dh(), sender, ACCOUNT, &rsk, epoch, uk, &mut OsRng)
            .unwrap()
            .encode()
    }

    fn mk_copy(mk: &[u8; 32]) -> Vec<u8> {
        let ctx = context::machine_key_under_user_key(ACCOUNT, "m-1", 0, 0);
        wrap::wrap_under(&UK0, &ctx, mk, &mut OsRng).unwrap().encode()
    }

    /// The readers: what a writer no longer current left — a recovery
    /// package, a machine-key copy — is tried, and taken when it is the key
    /// the chain committed (and refused when it is not).
    fn check_readers(f: &Fixture) {
        assert!(f.chain.admin("d-2").is_none() && f.chain.device_record("d-2").is_some());
        let good = package(f, &f.d2, "d-2", 0, &UK0);
        let name = format!("rk/0/d-2/{}/d-2", hex(&f.chain.recovery_key().unwrap()[..8]));
        let (epoch, uk) = open_recovery_package(&f.rk, &f.chain, &name, &good).expect("the only way back in opens");
        assert_eq!((epoch, uk.as_slice()), (0, &UK0[..]));
        // The proto's opener agrees (the readers are one rule).
        let ctx = SealContext {
            purpose: Purpose::UserKey,
            account: ACCOUNT.into(),
            sender: "d-2".into(),
            recipient: wrap::RECOVERY_RECIPIENT.into(),
            epoch: 0,
        };
        assert_eq!(
            wrap::open_from_recovery(&f.rk, &f.chain, &ctx, &SealedKey::decode(&good).unwrap())
                .unwrap()
                .as_slice(),
            &UK0[..]
        );
        // A key of its own choosing, an epoch not committed, another sender's
        // name, a device the chain never named, other words: refused.
        assert!(open_recovery_package(&f.rk, &f.chain, &name, &package(f, &f.d2, "d-2", 0, &[0x77; 32])).is_none());
        assert!(open_recovery_package(&f.rk, &f.chain, "rk/1/d-2", &package(f, &f.d2, "d-2", 1, &UK0)).is_none());
        assert!(open_recovery_package(&f.rk, &f.chain, "rk/0/d-1", &good).is_none());
        let stranger = MachineIdentity::from_seed(&[7; 32]);
        assert!(open_recovery_package(&f.rk, &f.chain, "rk/0/d-9", &package(f, &stranger, "d-9", 0, &UK0)).is_none());
        assert!(open_recovery_package(&recovery_keys(&[8; 16]), &f.chain, &name, &good).is_none());
        // A current admin's bad copy first, the good one d-2 left after it:
        // the good one is still tried, and taken.
        let user_keys = BTreeMap::from([(0, UK0)]);
        let entries = vec![
            ("mk/m-1/0/0/d-1".to_string(), mk_copy(&[0x66; 32])),
            ("mk/m-1/0/0/d-2".to_string(), mk_copy(&MK0)),
        ];
        let keys = open_machine_copies(&f.chain, &user_keys, "m-1", &entries);
        assert_eq!(keys.get(&0).map(|k| **k), Some(MK0));
        assert!(
            open_machine_copies(&f.chain, &user_keys, "m-1", &entries[..1]).is_empty(),
            "a bad copy alone opens nothing"
        );
        let only_bad = vec![("mk/m-1/0/0/d-2".to_string(), mk_copy(&[0x66; 32]))];
        assert!(open_machine_copies(&f.chain, &user_keys, "m-1", &only_bad).is_empty());
    }

    #[test]
    fn who_held_the_user_key_and_when_it_is_stale_follow_the_chains_history() {
        let mut f = fixture(true);
        assert!(held_user_key(&f.chain, "d-1") && held_user_key(&f.chain, "d-2"));
        assert!(!held_user_key(&f.chain, "m-1") && !held_user_key(&f.chain, "d-9"));
        assert!(!user_key_stale(&f.chain));
        // A demotion leaves the user key stale; it still held it.
        apply(
            &mut f.chain,
            &f.d1,
            "d-1",
            Action::SetAdmin {
                id: "d-2".into(),
                admin: false,
            },
        );
        assert!(user_key_stale(&f.chain) && held_user_key(&f.chain, "d-2"));
        // A machine key alone does not freshen it; a new user key does.
        let commit = |user: bool| KeyCommits {
            user: user.then(|| KeyCommit {
                epoch: 1,
                hash: user_key_commit(ACCOUNT, 1, &[0x11; 32]),
            }),
            machines: vec![(
                "m-1".into(),
                KeyCommit {
                    epoch: 1,
                    hash: machine_key_commit(ACCOUNT, "m-1", 1, &[0x21; 32]),
                },
            )],
        };
        apply(&mut f.chain, &f.d1, "d-1", Action::KeyEpochs(commit(false)));
        assert!(user_key_stale(&f.chain));
        let mut fresh = commit(true);
        fresh.machines.clear();
        apply(&mut f.chain, &f.d1, "d-1", Action::KeyEpochs(fresh));
        assert!(!user_key_stale(&f.chain));
        // Removing a device that held it leaves it stale again; a host's removal does not.
        let mut g = fixture(true);
        apply(&mut g.chain, &g.d1, "d-1", Action::RemoveDevice3("m-1".into()));
        assert!(!user_key_stale(&g.chain));
        apply(&mut g.chain, &g.d1, "d-1", Action::RemoveDevice3("d-2".into()));
        assert!(user_key_stale(&g.chain));
    }

    #[test]
    fn key_epochs_split_past_the_entrys_limit_as_the_web_does() {
        use rh_proto::chain::MAX_KEY_COMMITS;
        let user = || {
            Some(KeyCommit {
                epoch: 3,
                hash: [1; 32],
            })
        };
        let machines = |n: usize| -> Vec<(String, KeyCommit)> {
            (0..n)
                .map(|i| (format!("m-{i}"), KeyCommit { epoch: 0, hash: [2; 32] }))
                .collect()
        };
        assert!(key_epochs(None, Vec::new()).is_empty());
        let shape = |actions: Vec<Action>| -> Vec<(bool, usize)> {
            actions
                .into_iter()
                .map(|a| match a {
                    Action::KeyEpochs(k) => (k.user.is_some(), k.machines.len()),
                    _ => panic!("only KeyEpochs"),
                })
                .collect()
        };
        assert_eq!(shape(key_epochs(user(), Vec::new())), [(true, 0)]);
        assert_eq!(shape(key_epochs(None, machines(3))), [(false, 3)]);
        assert_eq!(
            shape(key_epochs(user(), machines(MAX_KEY_COMMITS + 44))),
            [(true, MAX_KEY_COMMITS), (false, 44)]
        );
        assert_eq!(
            shape(key_epochs(None, machines(2 * MAX_KEY_COMMITS + 1))),
            [(false, MAX_KEY_COMMITS), (false, MAX_KEY_COMMITS), (false, 1)]
        );
    }

    #[test]
    fn what_a_removed_v2_device_left_is_tried_and_taken_only_as_committed() {
        let mut f = fixture(false);
        apply(&mut f.chain, &f.d1, "d-1", Action::RemoveDevice("d-2".into()));
        check_readers(&f);
    }

    #[test]
    fn what_a_demoted_or_removed_v3_admin_left_is_tried_and_taken_only_as_committed() {
        for action in [
            Action::SetAdmin {
                id: "d-2".into(),
                admin: false,
            },
            Action::RemoveDevice3("d-2".into()),
        ] {
            let mut f = fixture(true);
            apply(&mut f.chain, &f.d1, "d-1", action);
            check_readers(&f);
        }
    }
}
