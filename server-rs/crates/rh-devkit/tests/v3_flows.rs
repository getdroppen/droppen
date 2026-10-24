//! The headless device's chain v3 flows against an in-memory hub that
//! verifies every batch with the Rust verifier, as a hub does: an account
//! made on v3 from its genesis, a device paired by offer (an admin and a
//! limited one) and by code, a host enrolled with its first machine key, a
//! removal that rotates the keys in the same batch — and, at the end, the
//! whole chain as the hub holds it verified again from its genesis, every
//! key that went out opened by its recipient against it. And the v2 flows
//! on an account made before chain v3 still write v2.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rh_devkit::{hex, unhex, Device, HubUrl};
use rh_proto::chain::{Accept, Action, ChainClock, ChainState, Entry, VERSION, VERSION_3};
use rh_proto::identity::{EnrollCode, MachineIdentity};
use rh_proto::keys::DhKeypair;
use rh_proto::wrap::{self, Purpose, SealContext, SealedKey};
use serde_json::{json, Value};

const SECS: Duration = Duration::from_secs(10);
const EMAIL: &str = "v3@example.com";
const PASSWORD: &str = "correct horse battery staple";

#[derive(Clone)]
struct Mail {
    id: i64,
    recipient: String,
    sender: String,
    purpose: u8,
    epoch: u32,
    sealed: String,
}

#[derive(Default)]
struct Hub {
    account: String,
    devices: u32,
    entries: Vec<Entry>,
    /// Each `POST /v1/chain` taken: its entries' kinds, in order.
    batches: Vec<Vec<&'static str>>,
    chain: Option<ChainState>,
    mail: Vec<Mail>,
    mail_seq: i64,
    keys: BTreeMap<String, String>,
    /// Enrollment token (hex) → the agent's proof, once it "proved".
    enrollments: BTreeMap<String, Option<Value>>,
    pairings: BTreeMap<String, (String, Vec<(String, String)>)>,
    pairing_seq: u32,
    /// What came in, in order: `put <name>`, `mail <purpose> <recipient>`.
    log: Vec<String>,
    /// Mail for a recipient refused once, with this message (400 `bad_request`).
    refuse_mail: BTreeMap<String, String>,
}

type Shared = Arc<Mutex<Hub>>;

fn fail(status: StatusCode, code: &str, message: impl Into<String>) -> Response {
    (status, Json(json!({"error": code, "message": message.into()}))).into_response()
}

/// The device a session names (`tok-<device>`); a login token names none.
fn caller(headers: &HeaderMap) -> Option<String> {
    let bearer = headers.get("authorization")?.to_str().ok()?.strip_prefix("Bearer ")?;
    bearer.strip_prefix("tok-").map(String::from)
}

async fn signup(State(hub): State<Shared>) -> Json<Value> {
    let account = hub.lock().unwrap().account.clone();
    Json(json!({"account": account, "token": "login"}))
}

async fn register(State(hub): State<Shared>) -> Json<Value> {
    let mut h = hub.lock().unwrap();
    h.devices += 1;
    Json(json!({"device": format!("d-{}", h.devices)}))
}

async fn challenge() -> Json<Value> {
    Json(json!({"nonce": hex(&[7u8; 32])}))
}

async fn session(Json(body): Json<Value>) -> Json<Value> {
    Json(json!({"token": format!("tok-{}", body["device"].as_str().unwrap_or_default())}))
}

/// `POST /v1/chain`: the batch verified against the chain held, with the
/// Rust verifier, appended whole or not at all.
async fn push_chain(State(hub): State<Shared>, Json(body): Json<Value>) -> Response {
    let entries: Vec<Entry> = match body["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| unhex(e.as_str().unwrap_or_default()).and_then(|b| Entry::decode(&b).map_err(|e| e.to_string())))
        .collect::<Result<Vec<Entry>, String>>()
    {
        Ok(e) if !e.is_empty() => e,
        _ => return fail(StatusCode::BAD_REQUEST, "bad_request", "no entries"),
    };
    let mut h = hub.lock().unwrap();
    let next = h.entries.len() as u64;
    if entries[0].draft.seq != next {
        return fail(StatusCode::CONFLICT, "chain_conflict", format!("expected entry {next}"));
    }
    let mut clock = ChainClock::history();
    let verified = match &h.chain {
        None => ChainState::verify(&entries, None, &mut clock).map_err(|e| e.to_string()),
        Some(chain) => {
            let mut chain = chain.clone();
            entries
                .iter()
                .try_for_each(|e| chain.apply(e, &mut clock).map_err(|e| e.to_string()))
                .map(|()| chain)
        }
    };
    let chain = match verified {
        Ok(c) => c,
        Err(e) => return fail(StatusCode::UNPROCESSABLE_ENTITY, "invalid_entry", e),
    };
    if let Some(host) = chain.host_key_due() {
        return fail(StatusCode::UNPROCESSABLE_ENTITY, "invalid_entry", format!("{host} is owed its machine key epoch"));
    }
    h.batches.push(entries.iter().map(|e| kind(&e.draft.action)).collect());
    h.entries.extend(entries);
    h.chain = Some(chain);
    Json(json!({"seq": h.entries.len() - 1})).into_response()
}

async fn read_chain(State(hub): State<Shared>, Query(q): Query<BTreeMap<String, String>>) -> Json<Value> {
    let from: usize = q.get("from").and_then(|f| f.parse().ok()).unwrap_or(0);
    let h = hub.lock().unwrap();
    let entries: Vec<String> = h.entries.iter().skip(from).map(|e| hex(&e.encode())).collect();
    Json(json!({"entries": entries}))
}

async fn send_mail(State(hub): State<Shared>, Json(b): Json<Value>) -> Response {
    let mut h = hub.lock().unwrap();
    let recipient = b["recipient"].as_str().unwrap_or_default().to_string();
    if let Some(message) = h.refuse_mail.remove(&recipient) {
        return fail(StatusCode::BAD_REQUEST, "bad_request", message);
    }
    h.log.push(format!("mail {} {recipient}", b["purpose"]));
    h.mail_seq += 1;
    let mail = Mail {
        id: h.mail_seq,
        recipient: b["recipient"].as_str().unwrap_or_default().into(),
        sender: b["sender"].as_str().unwrap_or_default().into(),
        purpose: b["purpose"].as_u64().unwrap_or_default() as u8,
        epoch: b["epoch"].as_u64().unwrap_or_default() as u32,
        sealed: b["sealed"].as_str().unwrap_or_default().into(),
    };
    h.mail.push(mail.clone());
    Json(json!({"id": mail.id})).into_response()
}

async fn my_mail(State(hub): State<Shared>, headers: HeaderMap, Query(q): Query<BTreeMap<String, String>>) -> Json<Value> {
    let me = caller(&headers).unwrap_or_default();
    let after: i64 = q.get("after").and_then(|a| a.parse().ok()).unwrap_or(0);
    let h = hub.lock().unwrap();
    let mail: Vec<Value> = h
        .mail
        .iter()
        .filter(|m| m.recipient == me && m.id > after)
        .take(64)
        .map(|m| json!({"id": m.id, "sender": m.sender, "purpose": m.purpose, "epoch": m.epoch, "sealed": m.sealed}))
        .collect();
    Json(json!({"mail": mail}))
}

async fn ack_mail(State(hub): State<Shared>, Path(id): Path<i64>) -> Json<Value> {
    hub.lock().unwrap().mail.retain(|m| m.id != id);
    Json(json!({}))
}

async fn put_key(State(hub): State<Shared>, Path(name): Path<String>, Json(b): Json<Value>) -> Response {
    let mut h = hub.lock().unwrap();
    if h.keys.contains_key(&name) {
        return fail(StatusCode::CONFLICT, "exists", "written once");
    }
    h.log.push(format!("put {name}"));
    h.keys.insert(name, b["blob"].as_str().unwrap_or_default().into());
    Json(json!({"stored": true})).into_response()
}

async fn keyring(State(hub): State<Shared>) -> Json<Value> {
    let h = hub.lock().unwrap();
    let keys: Vec<Value> = h.keys.iter().map(|(name, blob)| json!({"name": name, "blob": blob})).collect();
    Json(json!({"keys": keys}))
}

async fn announce(State(hub): State<Shared>, Json(b): Json<Value>) -> Json<Value> {
    let token = b["token"].as_str().unwrap_or_default().to_string();
    hub.lock().unwrap().enrollments.insert(token, None);
    Json(json!({"expires_ms": 0}))
}

async fn enrollment(State(hub): State<Shared>, Path(token): Path<String>) -> Response {
    match hub.lock().unwrap().enrollments.get(&token) {
        Some(Some(proved)) => Json(proved.clone()).into_response(),
        Some(None) => Json(json!({"state": "waiting"})).into_response(),
        None => fail(StatusCode::NOT_FOUND, "not_found", "no such enrollment"),
    }
}

async fn open_pairing(State(hub): State<Shared>, Json(b): Json<Value>) -> Json<Value> {
    let mut h = hub.lock().unwrap();
    h.pairing_seq += 1;
    let id = format!("p-{}", h.pairing_seq);
    h.pairings.insert(id.clone(), (b["device"].as_str().unwrap_or_default().into(), Vec::new()));
    Json(json!({"id": id}))
}

async fn pairings(State(hub): State<Shared>) -> Json<Value> {
    let h = hub.lock().unwrap();
    let list: Vec<Value> = h.pairings.iter().map(|(id, (device, _))| json!({"id": id, "device": device})).collect();
    Json(json!({"pairings": list}))
}

async fn pairing_post(State(hub): State<Shared>, Path(id): Path<String>, Json(b): Json<Value>) -> Response {
    let mut h = hub.lock().unwrap();
    let Some((_, messages)) = h.pairings.get_mut(&id) else {
        return fail(StatusCode::NOT_FOUND, "not_found", "no such pairing");
    };
    messages.push((b["from"].as_str().unwrap_or_default().into(), b["body"].as_str().unwrap_or_default().into()));
    Json(json!({})).into_response()
}

async fn pairing_messages(State(hub): State<Shared>, Path(id): Path<String>) -> Json<Value> {
    let h = hub.lock().unwrap();
    let messages: Vec<Value> = h
        .pairings
        .get(&id)
        .map(|(_, m)| m.iter().map(|(from, body)| json!({"from": from, "body": body})).collect())
        .unwrap_or_default();
    Json(json!({"messages": messages}))
}

async fn close_pairing(State(hub): State<Shared>, Path(id): Path<String>) -> Json<Value> {
    hub.lock().unwrap().pairings.remove(&id);
    Json(json!({}))
}

/// A hub for one account on a loopback port.
async fn start_hub(account: &str) -> (HubUrl, Shared) {
    let hub: Shared = Arc::new(Mutex::new(Hub {
        account: account.into(),
        ..Default::default()
    }));
    let app = Router::new()
        .route("/v1/signup", post(signup))
        .route("/v1/login", post(signup))
        .route("/v1/devices", post(register))
        .route("/v1/session/challenge", post(challenge))
        .route("/v1/session", post(session))
        .route("/v1/chain", post(push_chain).get(read_chain))
        .route("/v1/mailbox", post(send_mail).get(my_mail))
        .route("/v1/mailbox/{id}", axum::routing::delete(ack_mail))
        .route("/v1/keys", get(keyring))
        .route("/v1/keys/{*name}", axum::routing::put(put_key))
        .route("/v1/enrollments", post(announce))
        .route("/v1/enrollments/{token}", get(enrollment))
        .route("/v1/pairings", post(open_pairing).get(pairings))
        .route("/v1/pairings/{id}", axum::routing::delete(close_pairing))
        .route("/v1/pairings/{id}/messages", post(pairing_post).get(pairing_messages))
        .with_state(hub.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (HubUrl::parse(&format!("http://{addr}")).unwrap(), hub)
}

/// The agent's half of an enrollment: it proves `code` for its own keys.
fn prove(hub: &Shared, code: &EnrollCode, account: &str, id: &str, agent: &MachineIdentity) {
    let proof = code.proof(account, id, &agent.sign_public(), &agent.dh_public());
    hub.lock().unwrap().enrollments.insert(
        hex(&code.token()),
        Some(json!({
            "state": "proved",
            "machine": id,
            "sign_key": hex(&agent.sign_public()),
            "dh_key": hex(&agent.dh_public()),
            "proof": hex(&proof),
        })),
    );
}

/// The mail for `recipient` of `purpose`, as the hub holds it now.
fn mail_for(hub: &Shared, recipient: &str, purpose: u8) -> Vec<Mail> {
    hub.lock()
        .unwrap()
        .mail
        .iter()
        .filter(|m| m.recipient == recipient && m.purpose == purpose)
        .cloned()
        .collect()
}

/// The hub's chain verified afresh from its genesis, every entry through
/// its encoding.
fn verify_from_genesis(hub: &Shared) -> ChainState {
    let entries: Vec<Entry> = hub
        .lock()
        .unwrap()
        .entries
        .iter()
        .map(|e| Entry::decode(&e.encode()).unwrap())
        .collect();
    ChainState::verify(&entries, None, &mut ChainClock::history()).unwrap()
}

fn kind(action: &Action) -> &'static str {
    match action {
        Action::Genesis3 { .. } => "Genesis3",
        Action::KeyEpochs(_) => "KeyEpochs",
        Action::AddDevice3(_) => "AddDevice3",
        Action::RemoveDevice3(_) => "RemoveDevice3",
        Action::SetAccept { .. } => "SetAccept",
        Action::SetAdmin { .. } => "SetAdmin",
        Action::RecoveryJoin3(_) => "RecoveryJoin3",
        Action::Genesis { .. } => "Genesis",
        Action::AddDevice(_) => "AddDevice",
        _ => "other",
    }
}

fn kinds(chain: &ChainState) -> Vec<(u8, &'static str)> {
    chain.entries().iter().map(|e| (e.draft.version, kind(&e.draft.action))).collect()
}

/// The last `POST /v1/chain` the hub took, by kind.
fn last_batch(hub: &Shared) -> Vec<&'static str> {
    hub.lock().unwrap().batches.last().cloned().unwrap_or_default()
}

/// The key epochs the hub's chain committed last (`KeyEpochs` of the last batch): `(user epoch, [(host, epoch)])`.
fn last_commits(hub: &Shared) -> (Option<u32>, Vec<(String, u32)>) {
    let h = hub.lock().unwrap();
    let n = h.batches.last().map_or(0, Vec::len);
    let mut user = None;
    let mut machines = Vec::new();
    for e in &h.entries[h.entries.len() - n..] {
        if let Action::KeyEpochs(k) = &e.draft.action {
            user = user.or(k.user.as_ref().map(|u| u.epoch));
            machines.extend(k.machines.iter().map(|(m, c)| (m.clone(), c.epoch)));
        }
    }
    (user, machines)
}

#[tokio::test]
async fn a_v3_account_pairs_enrolls_and_removes_as_the_web_does() {
    let (url, hub) = start_hub("a-v3").await;
    let mut a = Device::sign_up(&url, "invite", EMAIL, PASSWORD).await.unwrap();
    let rk = a.create_genesis3(true).await.unwrap().expect("recovery words");

    // An admin paired by offer gets every user key epoch.
    let mut b = Device::log_in(&url, EMAIL, PASSWORD).await.unwrap();
    let offer = b.offer();
    a.add_and_deliver3(&offer.device, offer.sign_key, offer.dh_key, &offer.secret, true)
        .await
        .unwrap();
    assert_eq!(b.await_delivery(&offer.secret, SECS).await.unwrap(), 1);
    assert_eq!(b.user_keys.get(&0), a.user_keys.get(&0));

    // A host: `[AddDevice3{host}, KeyEpochs]`, its first key into the keyring and its mailbox.
    let agent = MachineIdentity::from_seed(&[4; 32]);
    let code = a.new_enrollment().await.unwrap();
    prove(&hub, &code, &a.account, "m-1", &agent);
    let mk0 = rh_devkit::random_key();
    a.complete_enrollment_v3(&code, false, &mk0, SECS).await.unwrap();
    a.wrap_machine_key("m-1", 0, &mk0).await.unwrap();
    a.mail_machine_key("m-1", 0, &mk0).await.unwrap();

    // A limited device: no user key in its delivery; once the host accepts
    // it, the host's keys by mail (purpose 5).
    const C_DH: [u8; 32] = [0x33; 32];
    let c_sign = ed25519_dalek::SigningKey::from_bytes(&[0x34; 32]);
    let mut c = Device::from_keys(&url, &a.account, "d-limited", c_sign, DhKeypair::from_secret(C_DH))
        .await
        .unwrap();
    let offer = c.offer();
    a.add_and_deliver3(&offer.device, offer.sign_key, offer.dh_key, &offer.secret, false)
        .await
        .unwrap();
    assert_eq!(c.await_delivery(&offer.secret, SECS).await.unwrap(), 0);
    assert!(c.user_keys.is_empty());
    assert_eq!(a.set_accept("m-1", Accept::Devices(vec![c.id.clone()])).await.unwrap(), None);
    assert_eq!(a.send_host_keys(&c.id).await.unwrap(), 1);

    // Pairing by code follows the chain's version: a v3 admin.
    let mut d = Device::log_in(&url, EMAIL, PASSWORD).await.unwrap();
    let d_id = d.id.clone();
    let (paired, entered) = tokio::join!(d.pair_by_code("12345678", SECS), a.pair_with_code(&d_id, "12345678", SECS));
    entered.unwrap();
    assert_eq!(paired.unwrap(), 1);

    // Removing b: one batch `[RemoveDevice3, KeyEpochs{UK 1, m-1 MK 1}]`.
    let rotation = a.remove_and_rotate3(&b.id).await.unwrap();
    assert_eq!(last_batch(&hub), ["RemoveDevice3", "KeyEpochs"], "one POST");
    assert_eq!(rotation.uk_epoch, Some(1));
    assert!(rotation.failed.is_empty(), "{:?}", rotation.failed);
    assert_eq!(rotation.machines.len(), 1);
    let (host, mk_epoch, mk1) = rotation.machines[0].clone();
    assert_eq!((host.as_str(), mk_epoch), ("m-1", 1));

    let chain = verify_from_genesis(&hub);
    assert_eq!(chain.v3_from(), Some(0));
    assert_eq!(
        kinds(&chain),
        vec![
            (VERSION_3, "Genesis3"),
            (VERSION_3, "KeyEpochs"),
            (VERSION_3, "AddDevice3"),
            (VERSION_3, "AddDevice3"),
            (VERSION_3, "KeyEpochs"),
            (VERSION_3, "AddDevice3"),
            (VERSION_3, "SetAccept"),
            (VERSION_3, "AddDevice3"),
            (VERSION_3, "RemoveDevice3"),
            (VERSION_3, "KeyEpochs"),
        ]
    );
    assert!(chain.device(&b.id).is_none(), "b is removed");
    assert!(chain.admin(&d.id).is_some(), "d paired by code manages the account");
    let limited = chain.device(&c.id).expect("c is a current device");
    assert!(!limited.admin && !limited.host);
    assert!(chain.machine("m-1").is_some_and(|m| !m.admin));
    assert_eq!((chain.user_key_epochs(), chain.machine_key_epochs("m-1")), (2, 2));
    for (epoch, key) in &a.user_keys {
        chain.check_user_key(*epoch, key).unwrap();
    }
    chain.check_machine_key("m-1", 0, &mk0).unwrap();
    chain.check_machine_key("m-1", 1, &mk1).unwrap();

    // The other admin takes the new user key from its mail and reads both machine key epochs.
    assert_eq!(d.take_user_key_mail().await.unwrap(), 1);
    assert_eq!(d.user_keys.get(&1), a.user_keys.get(&1));
    let keys = d.machine_keys("m-1").await.unwrap();
    assert_eq!((keys.get(&0).map(|k| **k), keys.get(&1).map(|k| **k)), (Some(mk0), Some(mk1)));
    // The removed device was mailed nothing new.
    assert!(mail_for(&hub, &b.id, wrap::MAIL_USER_KEY).is_empty());

    // The limited device was mailed each of the host's epochs, never a user key.
    assert!(mail_for(&hub, &c.id, wrap::MAIL_USER_KEY).is_empty());
    let mut host_keys = BTreeMap::new();
    for m in mail_for(&hub, &c.id, wrap::MAIL_HOST_KEY) {
        let (host, sealed) = wrap::decode_host_key_mail(&unhex(&m.sealed).unwrap()).unwrap();
        let ctx = wrap::HostKeyContext {
            account: chain.account().into(),
            sender: m.sender.clone(),
            recipient: c.id.clone(),
            host: host.clone(),
            epoch: m.epoch,
        };
        let key = wrap::open_host_key(&DhKeypair::from_secret(C_DH), &c.id, &chain, &ctx, &sealed).unwrap();
        host_keys.insert((host, m.epoch), key.to_vec());
    }
    assert_eq!(
        host_keys,
        BTreeMap::from([(("m-1".to_string(), 0), mk0.to_vec()), (("m-1".to_string(), 1), mk1.to_vec())])
    );

    // The agent opens its first key from its mailbox against the chain.
    let mails = mail_for(&hub, "m-1", wrap::MAIL_MACHINE_KEY);
    assert_eq!(mails.len(), 1);
    let ctx = SealContext {
        purpose: Purpose::MachineKey,
        account: chain.account().into(),
        sender: mails[0].sender.clone(),
        recipient: "m-1".into(),
        epoch: 0,
    };
    let sealed = SealedKey::decode(&unhex(&mails[0].sealed).unwrap()).unwrap();
    assert_eq!(&*wrap::open_machine_key(agent.dh(), "m-1", &chain, &ctx, &sealed).unwrap(), &mk0[..]);

    // The recovery words open both user key epochs, and the join is v3.
    let mut e = Device::log_in(&url, EMAIL, PASSWORD).await.unwrap();
    assert_eq!(e.recover(&rk).await.unwrap(), 2);
    assert_eq!(e.user_keys, a.user_keys);
    let chain = verify_from_genesis(&hub);
    assert_eq!(kinds(&chain).last(), Some(&(VERSION_3, "RecoveryJoin3")));

    // A v2 entry is refused on a v3 chain before anything reaches the hub.
    #[allow(deprecated)]
    let refused = a.remove_and_rotate(&c.id).await;
    assert!(refused.is_err());
    assert_eq!(verify_from_genesis(&hub).seq(), chain.seq());
}

#[tokio::test]
#[allow(deprecated)]
async fn an_account_made_before_chain_v3_still_writes_v2() {
    let (url, hub) = start_hub("a-v2").await;
    let mut a = Device::sign_up(&url, "invite", EMAIL, PASSWORD).await.unwrap();
    a.create_genesis().await.unwrap();
    a.new_user_key().await.unwrap();
    let mut d = Device::log_in(&url, EMAIL, PASSWORD).await.unwrap();
    let d_id = d.id.clone();
    let (paired, entered) = tokio::join!(d.pair_by_code("87654321", SECS), a.pair_with_code(&d_id, "87654321", SECS));
    entered.unwrap();
    assert_eq!(paired.unwrap(), 1);
    // The v3 flows refuse a v2 chain.
    let b = Device::log_in(&url, EMAIL, PASSWORD).await.unwrap();
    let offer = b.offer();
    assert!(a
        .add_and_deliver3(&offer.device, offer.sign_key, offer.dh_key, &offer.secret, true)
        .await
        .is_err());
    assert!(a.remove_and_rotate3(&d_id).await.is_err());
    let chain = verify_from_genesis(&hub);
    assert!(!chain.is_v3());
    assert_eq!(kinds(&chain), vec![(VERSION, "Genesis"), (VERSION, "KeyEpochs"), (VERSION, "AddDevice")]);
}

/// The host key mails (purpose 5) `to` holds, as `(host, epoch)`.
fn host_mail(hub: &Shared, to: &str) -> Vec<(String, u32)> {
    let mut out: Vec<(String, u32)> = mail_for(hub, to, wrap::MAIL_HOST_KEY)
        .into_iter()
        .map(|m| (wrap::decode_host_key_mail(&unhex(&m.sealed).unwrap()).unwrap().0, m.epoch))
        .collect();
    out.sort();
    out
}

/// Pair a fresh device by offer, an admin or a limited one.
async fn pair(url: &HubUrl, a: &mut Device, admin: bool) -> Device {
    let mut d = Device::log_in(url, EMAIL, PASSWORD).await.unwrap();
    let offer = d.offer();
    a.add_and_deliver3(&offer.device, offer.sign_key, offer.dh_key, &offer.secret, admin)
        .await
        .unwrap();
    d.await_delivery(&offer.secret, SECS).await.unwrap();
    d
}

/// Enroll host `id` as the web does, with its first key `mk`.
async fn enroll(hub: &Shared, a: &mut Device, id: &str, seed: u8, mk: &[u8; 32]) -> rh_devkit::keys::Enrolled3 {
    let code = a.new_enrollment().await.unwrap();
    prove(hub, &code, &a.account, id, &MachineIdentity::from_seed(&[seed; 32]));
    let enrolled = a.enroll_host3(&code, mk, SECS).await.unwrap();
    assert!(enrolled.failed.is_empty(), "{:?}", enrolled.failed);
    assert_eq!((enrolled.machine.id.as_str(), enrolled.mk_epoch), (id, 0));
    enrolled
}

/// Where in the hub's log `event` came last, after `from`.
fn last_at(hub: &Shared, from: usize, pred: impl Fn(&str) -> bool) -> Option<usize> {
    let h = hub.lock().unwrap();
    h.log.iter().enumerate().skip(from).filter(|(_, e)| pred(e)).map(|(i, _)| i).last()
}

fn log_len(hub: &Shared) -> usize {
    hub.lock().unwrap().log.len()
}

#[tokio::test]
async fn removals_and_enrollments_turn_over_what_the_web_turns_over() {
    let (url, hub) = start_hub("a-turns").await;
    let mut a = Device::sign_up(&url, "invite", EMAIL, PASSWORD).await.unwrap();
    a.create_genesis3(false).await.unwrap();
    let mks: Vec<[u8; 32]> = (0..3).map(|_| rh_devkit::random_key()).collect();
    enroll(&hub, &mut a, "m-1", 11, &mks[0]).await;
    assert_eq!(last_batch(&hub), ["AddDevice3", "KeyEpochs"]);
    assert_eq!(last_commits(&hub), (None, vec![("m-1".into(), 0)]));
    enroll(&hub, &mut a, "m-2", 12, &mks[1]).await;

    // m-1 accepts every device: a limited device paired now is handed m-1's
    // key at once (and m-2's never: it accepts only admins).
    assert_eq!(a.set_accept("m-1", Accept::All).await.unwrap(), None);
    let c = pair(&url, &mut a, false).await;
    assert_eq!(host_mail(&hub, &c.id), [("m-1".to_string(), 0)]);
    let b = pair(&url, &mut a, true).await;
    let mut d = pair(&url, &mut a, true).await;

    // A limited device never held the user key: only the hosts' keys turn over.
    let r = a.remove_and_rotate3(&c.id).await.unwrap();
    assert_eq!(last_batch(&hub), ["RemoveDevice3", "KeyEpochs"], "one POST");
    assert_eq!(last_commits(&hub), (None, vec![("m-1".into(), 1), ("m-2".into(), 1)]));
    assert_eq!(r.uk_epoch, None);
    assert!(r.failed.is_empty(), "{:?}", r.failed);
    assert_eq!(host_mail(&hub, &c.id), [("m-1".to_string(), 0)], "nothing new for the removed device");
    // m-1's new key went to the other device m-1 accepts: m-2's device role.
    assert!(host_mail(&hub, "m-2").contains(&("m-1".to_string(), 1)));

    // A host: the others' keys turn over, it gets none, the user key stays.
    let r = a.remove_and_rotate3("m-2").await.unwrap();
    assert_eq!(last_batch(&hub), ["RemoveDevice3", "KeyEpochs"]);
    assert_eq!((r.uk_epoch, r.machines.iter().map(|(h, e, _)| (h.as_str(), *e)).collect::<Vec<_>>()), (None, vec![("m-1", 2)]));

    // An admin: the user key too, mailed to the other admin — after the
    // keyring holds what goes under it (a device reading the keyring as the
    // mail lands finds it whole).
    let from = log_len(&hub);
    let r = a.remove_and_rotate3(&b.id).await.unwrap();
    assert_eq!(last_commits(&hub), (Some(1), vec![("m-1".into(), 3)]));
    assert_eq!(r.uk_epoch, Some(1));
    let wrapped = last_at(&hub, from, |e| e.starts_with("put mk/m-1/3/1/")).expect("m-1's new key in the keyring");
    let mailed = last_at(&hub, from, |e| e == format!("mail 1 {}", d.id)).expect("the user key mailed to d");
    assert!(wrapped < mailed, "the keyring first, then the mail");
    assert_eq!(d.take_user_key_mail().await.unwrap(), 1);

    // d demoted: the newest user key is stale (d holds it).
    a.append(Action::SetAdmin {
        id: d.id.clone(),
        admin: false,
    })
    .await
    .unwrap();
    assert!(rh_devkit::keys::user_key_stale(a.chain().unwrap()));
    assert_eq!(a.usable_user_key().unwrap(), None);
    // It held the user key: it is not removed with the hosts' keys alone.
    assert!(a.remove3_and_rotate_hosts(&d.id).await.is_err());
    // A limited device removed while the user key is stale: it never held
    // one, but no machine key goes under a stale one, so a new one comes.
    let x = pair(&url, &mut a, false).await;
    let r = a.remove_and_rotate3(&x.id).await.unwrap();
    assert_eq!(last_batch(&hub), ["RemoveDevice3", "KeyEpochs"]);
    assert_eq!(last_commits(&hub), (Some(2), vec![("m-1".into(), 4)]));
    assert_eq!(r.uk_epoch, Some(2));
    assert!(!rh_devkit::keys::user_key_stale(a.chain().unwrap()));
    assert!(mail_for(&hub, &d.id, wrap::MAIL_USER_KEY).is_empty(), "the demoted device gets no new user key");

    // Another admin demoted: a host enrolled now carries a new user key in
    // its own batch, its key wrapped under that.
    let g = pair(&url, &mut a, true).await;
    a.append(Action::SetAdmin {
        id: g.id.clone(),
        admin: false,
    })
    .await
    .unwrap();
    let enrolled = enroll(&hub, &mut a, "m-3", 13, &mks[2]).await;
    assert_eq!(last_batch(&hub), ["AddDevice3", "KeyEpochs"]);
    assert_eq!(last_commits(&hub), (Some(3), vec![("m-3".into(), 0)]));
    assert_eq!(enrolled.uk_epoch, Some(3));
    assert!(!rh_devkit::keys::user_key_stale(a.chain().unwrap()));
    assert_eq!(a.usable_user_key().unwrap(), Some(3));

    // A limited device removed with the hosts' keys: every current host,
    // the user key kept. A mail the hub refuses after the commit loses
    // nothing: said in `failed`, the keys still returned and kept; a host's
    // device role that never signed in is waited for, not a failure.
    let e = pair(&url, &mut a, false).await;
    {
        let mut h = hub.lock().unwrap();
        h.refuse_mail.insert(d.id.clone(), "the mailbox is full".into());
        h.refuse_mail.insert("m-3".into(), "m-3 gets it once it signs in as a device".into());
    }
    let r = a.remove3_and_rotate_hosts(&e.id).await.unwrap();
    assert_eq!(last_batch(&hub), ["RemoveDevice3", "KeyEpochs"]);
    assert_eq!(last_commits(&hub), (None, vec![("m-1".into(), 5), ("m-3".into(), 1)]));
    assert_eq!(r.uk_epoch, None);
    assert_eq!(r.machines.len(), 2);
    assert_eq!(r.failed.len(), 1, "{:?}", r.failed);
    assert!(r.failed[0].contains(&d.id) && r.failed[0].contains("the mailbox is full"), "{:?}", r.failed);
    assert!(!host_mail(&hub, &d.id).contains(&("m-1".to_string(), 5)));
    hub.lock()
        .unwrap()
        .refuse_mail
        .insert("m-3".into(), "m-3 gets it once it signs in as a device".into());
    let (_, epoch, mk) = r.machines.iter().find(|(h, ..)| h == "m-1").unwrap();
    let sent = a.mail_host_key("m-1", *epoch, mk).await.unwrap();
    assert_eq!((sent.sent, sent.waiting, sent.failed.len()), (vec![d.id.clone(), g.id.clone()], vec!["m-3".to_string()], 0));

    // Every key committed is in the keyring, as the chain committed it.
    let chain = verify_from_genesis(&hub);
    assert!(kinds(&chain).iter().all(|(v, _)| *v == VERSION_3));
    a.sync_chain().await.unwrap();
    let m1 = a.machine_keys("m-1").await.unwrap();
    assert_eq!(m1.keys().copied().collect::<Vec<_>>(), [0, 1, 2, 3, 4, 5]);
    assert_eq!(m1.get(&0).map(|k| **k), Some(mks[0]));
    let m3 = a.machine_keys("m-3").await.unwrap();
    assert_eq!(m3.keys().copied().collect::<Vec<_>>(), [0, 1]);
    assert_eq!(m3.get(&0).map(|k| **k), Some(mks[2]));
}
