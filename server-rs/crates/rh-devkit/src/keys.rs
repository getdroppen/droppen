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
        let ctx = context::machine_key_under_user_key(&self.account, machine, mk_epoch, uk_epoch);
        let wrapped = wrap::wrap_under(&self.user_keys[&uk_epoch], &ctx, mk, &mut OsRng).map_err(err("wrap"))?;
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
    /// every user key epoch, MAC'd with `secret`.
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
        self.add_and_deliver(new_device, answer.sign_key, answer.dh_key, &secret).await
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
    /// to the agents (in a channel, or [`Device::mail_machine_key`]).
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
        let chain = self.chain.clone().ok_or("no chain")?;
        for d in chain.admins().filter(|d| d.info.id != self.id) {
            let ctx = SealContext {
                purpose: Purpose::UserKey,
                account: self.account.clone(),
                sender: self.id.clone(),
                recipient: d.info.id.clone(),
                epoch: uk_epoch,
            };
            let sealed = wrap::seal_to(&self.dh, &d.info.dh_key, &ctx, &self.user_keys[&uk_epoch], &mut OsRng).map_err(err("seal"))?;
            self.mail(&d.info.id, MAIL_USER_KEY, uk_epoch, &sealed.encode()).await?;
        }
        Ok((uk_epoch, rotated))
    }

    /// Chain v3, S1-4: remove `device` and, in the same batch, turn every
    /// remaining current host's machine key over (`[RemoveDevice3,
    /// KeyEpochs]` — what the web writes for a device that held no user key
    /// but could control hosts: their keys, never the user key), each new key
    /// wrapped into the keyring. `(host, epoch, key)` for the caller to hand
    /// to the agents.
    pub async fn remove3_and_rotate_hosts(&mut self, device: &str) -> Result<Vec<(String, u32, [u8; 32])>> {
        use rh_proto::chain::{machine_key_commit, KeyCommit, KeyCommits};
        self.sync_chain().await?;
        let chain = self.chain.clone().ok_or("no chain")?;
        let hosts: Vec<(String, [u8; 32])> = chain
            .hosts()
            .filter(|m| m.is_current_host() && m.info.id != device)
            .map(|m| (m.info.id.clone(), crate::random_key()))
            .collect();
        let commits = KeyCommits {
            user: None,
            machines: hosts
                .iter()
                .map(|(id, mk)| {
                    let epoch = chain.machine_key_epochs(id);
                    (
                        id.clone(),
                        KeyCommit {
                            epoch,
                            hash: machine_key_commit(&self.account, id, epoch, mk),
                        },
                    )
                })
                .collect(),
        };
        let epochs: Vec<u32> = commits.machines.iter().map(|(_, c)| c.epoch).collect();
        self.append_batch(vec![Action::RemoveDevice3(device.into()), Action::KeyEpochs(commits)])
            .await?;
        let mut rotated = Vec::new();
        for ((id, mk), epoch) in hosts.into_iter().zip(epochs) {
            if self.current_user_epoch().is_some() {
                self.wrap_machine_key(&id, epoch, &mk).await?;
            }
            rotated.push((id, epoch, mk));
        }
        Ok(rotated)
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
                let ctx = wrap::HostKeyContext {
                    account: self.account.clone(),
                    sender: self.id.clone(),
                    recipient: to.into(),
                    host: host.clone(),
                    epoch,
                };
                let sealed = wrap::seal_host_key(&self.dh, &dh, &ctx, &*mk, &mut OsRng).map_err(err("seal"))?;
                self.mail(to, wrap::MAIL_HOST_KEY, epoch, &wrap::encode_host_key_mail(&host, &sealed))
                    .await?;
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
