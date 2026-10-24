//! rottest: an end-to-end correctness run of identity-key renewal
//! (docs/host-identity-rotation.md) against a real hub, as an account's admin
//! device — by hand, step by step, with real agents (`remote-hand-server agent
//! run` in throwaway HOMEs) as the hosts. Every HTTP status, close code and
//! chain entry seen is logged with a UTC timestamp.
//!
//! **Opt-in only.** It does nothing unless `ROTTEST_STATE` names its state
//! file, and it talks only to the hub that `signup` was given (kept in that
//! file). It is never run by CI or by `cargo test`: it creates a real account
//! (with an invite from the hub's operator), enrolls machines and renews their
//! identity keys. Use a throwaway account and throwaway agent instances only.
//!
//! The state file holds the admin device's secret keys, the user key and the
//! machine keys it issued (mode 0600): it is the only way back into the
//! account, so keep it for as long as the account is to be reused, and delete
//! it (and the account's agents) when done.
//!
//! ```text
//! export ROTTEST_STATE=$PWD/rottest-state.json
//! cargo run -p rh-devkit --example rottest -- signup https://<hub> <email> <invite.json> <password-file>
//! cargo run -p rh-devkit --example rottest -- enroll A        # prints CODE …; run `agent enroll <code> --hub …` on the host
//! cargo run -p rh-devkit --example rottest -- handover A      # its first machine key, over a channel
//! cargo run -p rh-devkit --example rottest -- policy          # SetRenewalPolicy(false): renewals may start
//! cargo run -p rh-devkit --example rottest -- rotate A 0      # an automatic renewal, end to end
//! cargo run -p rh-devkit --example rottest -- rotate A 1 <code-file>   # confirmed on the device: the code it shows, typed into <code-file>
//! cargo run -p rh-devkit --example rottest -- status | oldkey | asdevice | accept | pause | control | hostkeys | put | redispute | watch …
//! cargo run -p rh-devkit --example rottest -- remove A B      # at the end: the hosts leave the account (it is kept)
//! ```
//!
//! `rotate` with `ROTTEST_DEVICE_SEED=<the host's machine-key.json>` also signs
//! in as the host's device role while its renewal is under way (refused:
//! `rotation_pending`), and takes a device-role relay ticket before the
//! request (`ROTTEST_DEVICE_TICKET=<file saved by asdevice>`, else one minted
//! then): it signs in on the relay with it before the request (and holds one
//! such connection open), again and again from the request on until the
//! relay refuses it as revoked (4403; the time from the attempt's
//! registration is logged), once staged and after the commit — and, once
//! staged, opens a channel to the host over its relay (its agent's tickets
//! are left alone).

use std::collections::BTreeMap;
use std::io::Write as _;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ed25519_dalek::SigningKey;
use futures_util::{SinkExt, StreamExt};
use rand::RngCore;
use rh_devkit::{hex, now_ms, rpc, unhex, Device, HubUrl, Link, OpenedChannel};
use rh_proto::chain::{Accept, Action, Assurance, ChainState, Entry, HostRotation, KeyCommit, KeyCommits, PausePhase};
use rh_proto::frame::{Frame, AGENT_PATH};
use rh_proto::identity::MachineIdentity;
use rh_proto::keys::DhKeypair;
use rh_proto::sig::{HubLogin, LoginRole};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio_tungstenite::tungstenite::Message;

type R<T> = Result<T, String>;

fn ts() -> String {
    let ms = now_ms();
    let s = ms / 1000;
    format!("{:02}:{:02}:{:02}.{:03}Z", (s / 3600) % 24, (s / 60) % 60, s % 60, ms % 1000)
}

macro_rules! log {
    ($($a:tt)*) => {{
        println!("[{}] {}", ts(), format!($($a)*));
        let _ = std::io::stdout().flush();
    }};
}

fn short(v: &Value, n: usize) -> String {
    let s = v.to_string();
    if s.len() > n {
        format!("{}…", &s[..n])
    } else {
        s
    }
}

fn k32(s: &str) -> R<[u8; 32]> {
    unhex(s)?.try_into().map_err(|_| "not 32 bytes".to_string())
}
fn k64(s: &str) -> R<[u8; 64]> {
    unhex(s)?.try_into().map_err(|_| "not 64 bytes".to_string())
}
fn k16(s: &str) -> R<[u8; 16]> {
    unhex(s)?.try_into().map_err(|_| "not 16 bytes".to_string())
}
fn fp(k: &[u8; 32]) -> String {
    hex(&k[..6])
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct State {
    hub: String,
    account: String,
    id: String,
    sign: String,
    dh: String,
    user_keys: BTreeMap<u32, String>,
    mks: BTreeMap<String, BTreeMap<u32, String>>,
    labels: BTreeMap<String, String>,
    pause_op: Option<String>,
}

fn state_path() -> String {
    std::env::var("ROTTEST_STATE").expect("checked in main")
}
fn load() -> R<State> {
    serde_json::from_str(&std::fs::read_to_string(state_path()).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}
fn save(st: &State) -> R<()> {
    let p = state_path();
    if let Some(dir) = std::path::Path::new(&p).parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut f = std::fs::OpenOptions::new();
    f.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut f, 0o600);
    f.open(&p).and_then(|mut f| f.write_all(serde_json::to_string_pretty(st).unwrap().as_bytes())).map_err(|e| e.to_string())
}
impl State {
    fn host(&self, label: &str) -> String {
        self.labels.get(label).cloned().unwrap_or_else(|| label.to_string())
    }
    fn label(&self, id: &str) -> String {
        self.labels.iter().find(|(_, v)| *v == id).map(|(k, _)| k.clone()).unwrap_or_else(|| id.to_string())
    }
}

async fn device(st: &State) -> R<Device> {
    let hub = HubUrl::parse(&st.hub)?;
    let sign = SigningKey::from_bytes(&k32(&st.sign)?);
    let dh = DhKeypair::from_secret(k32(&st.dh)?);
    let mut d = Device::from_keys(&hub, &st.account, &st.id, sign, dh).await?;
    for (e, k) in &st.user_keys {
        d.user_keys.insert(*e, k32(k)?);
    }
    Ok(d)
}

/// A host's identity from its agent's `machine-key.json` (or a staged file).
fn seed_identity(path: &str) -> R<MachineIdentity> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?).map_err(|e| e.to_string())?;
    Ok(MachineIdentity::from_seed(&k32(v["seed"].as_str().ok_or("no seed")?)?))
}

async fn call(base: &str, token: Option<&str>, method: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
    let http = reqwest::Client::new();
    let mut req = http.request(method, format!("{base}{path}"));
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    if let Some(b) = body {
        req = req.json(&b);
    }
    match req.send().await {
        Ok(r) => {
            let s = r.status().as_u16();
            (s, r.json().await.unwrap_or(Value::Null))
        }
        Err(e) => (0, json!({"network": e.to_string()})),
    }
}

/// The hub link, with every control message logged (and kept).
struct Watch {
    link: Arc<Link>,
    seen: Arc<Mutex<Vec<(u64, Value)>>>,
}

fn log_push(v: &Value, labels: &BTreeMap<String, String>) {
    let name = |id: &str| labels.iter().find(|(_, v)| *v == id).map(|(k, _)| k.clone()).unwrap_or_else(|| id.to_string());
    if v["t"] == "presence" {
        let ms: Vec<String> = v["machines"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|m| {
                let via: Vec<String> = m["via"].as_array().into_iter().flatten().filter_map(|x| x["relay"].as_str().map(String::from)).collect();
                format!("{}:{}{}", name(m["id"].as_str().unwrap_or("?")), m["state"].as_str().or(m["presence"].as_str()).unwrap_or(if m["online"] == true { "online" } else { "?" }), if via.is_empty() { String::new() } else { format!(" via {via:?}") })
            })
            .collect();
        log!("PUSH presence {}", ms.join(", "));
    } else {
        log!("PUSH {}", short(v, 600));
    }
}

async fn watch(d: &Device, labels: &BTreeMap<String, String>) -> R<Watch> {
    let link = Arc::new(d.connect().await?);
    if let Ok(att) = d.attestation() {
        link.attest(&att)?;
    }
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (l2, s2, labels) = (link.clone(), seen.clone(), labels.clone());
    tokio::spawn(async move {
        loop {
            match l2.next_control(Duration::from_secs(5)).await {
                Some(v) => {
                    log_push(&v, &labels);
                    s2.lock().unwrap().push((now_ms(), v));
                }
                None => {
                    if l2.is_closed() {
                        log!("admin hub link closed");
                        break;
                    }
                }
            }
        }
    });
    Ok(Watch { link, seen })
}

impl Watch {
    fn attest(&self, d: &Device) {
        match d.attestation().and_then(|a| self.link.attest(&a)) {
            Ok(()) => log!("attested head seq {}", d.chain().map_or(0, |c| c.seq())),
            Err(e) => log!("attest failed: {e}"),
        }
    }
    async fn wait_push(&self, since: u64, within: Duration, pred: impl Fn(&Value) -> bool) -> Option<Value> {
        let deadline = now_ms() + within.as_millis() as u64;
        loop {
            if let Some((_, v)) = self.seen.lock().unwrap().iter().find(|(t, v)| *t >= since && pred(v)) {
                return Some(v.clone());
            }
            if now_ms() > deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

/// A relay ticket for `d` (`relay`: the account set's first, or the first listed).
async fn ticket(base: &str, token: &str, who: &str) -> Option<(String, String, String)> {
    ticket_for(base, token, who, None).await
}

/// [`ticket`], for the relay `want` when given.
async fn ticket_for(base: &str, token: &str, who: &str, want: Option<&str>) -> Option<(String, String, String)> {
    let (s, relays) = call(base, Some(token), reqwest::Method::GET, "/v1/relays", None).await;
    if s != 200 {
        log!("{who}: GET /v1/relays {s} {}", short(&relays, 300));
        return None;
    }
    let pick = match want {
        Some(w) => w.to_string(),
        None => relays["account_set"][0]["id"].as_str().or(relays["relays"][0]["id"].as_str())?.to_string(),
    };
    let (s, t) = call(base, Some(token), reqwest::Method::POST, "/v1/relays/tickets", Some(json!({"relays": [pick]}))).await;
    let tk = &t["tickets"][0];
    if s != 200 || tk.is_null() {
        log!("{who}: POST /v1/relays/tickets [{pick}] → {s} {}", short(&t, 400));
        return None;
    }
    let text = tk["ticket"].as_str()?.to_string();
    match rh_proto::ticket::SignedTicket::parse(&text) {
        Ok(p) => log!(
            "{who}: ticket for relay {pick} → {s}: role {:?} id {} sign_key {} peers {:?} issued {} expires {}",
            p.ticket.role,
            p.ticket.id,
            fp(&p.ticket.sign_key),
            p.ticket.peers,
            p.ticket.issued_ms,
            p.ticket.expires_ms
        ),
        Err(e) => log!("{who}: ticket does not parse: {e:?}"),
    }
    Some((pick, tk["origin"].as_str()?.to_string(), text))
}

/// A channel from `d` to `host`: over a relay first, else over the hub link.
async fn open_to(d: &mut Device, hub_link: Option<&Link>, host: &str, who: &str) -> R<(Option<Link>, OpenedChannel)> {
    if let Some((relay, origin, text)) = ticket(&d.hub.http.clone(), &d.token.clone(), who).await {
        match d.connect_relay(&origin, &text).await {
            Ok(link) => match d.open_channel_info(&link, host).await {
                Ok(o) => {
                    log!("{who}: channel to {host} over relay {relay}: open (paths {}, direct {})", o.paths, o.direct);
                    return Ok((Some(link), o));
                }
                Err(e) => log!("{who}: channel to {host} over relay {relay}: {e}"),
            },
            Err(e) => log!("{who}: relay {relay} sign-in: {e}"),
        }
    }
    if let Some(link) = hub_link {
        match d.open_channel_info(link, host).await {
            Ok(o) => {
                log!("{who}: channel to {host} over the hub: open");
                return Ok((None, o));
            }
            Err(e) => {
                log!("{who}: channel to {host} over the hub: {e}");
                return Err(e);
            }
        }
    }
    Err(format!("{who}: no channel to {host}"))
}

fn kinds(chain: &ChainState, from: u64) -> Vec<String> {
    chain
        .entries()
        .iter()
        .filter(|e| e.seq() >= from)
        .map(|e| format!("{}:{}", e.seq(), describe(&e)))
        .collect()
}

fn describe(e: &Entry) -> String {
    let a = &e.draft.action;
    let k = a.kind();
    match a {
        Action::RotateHostKey(r) => format!(
            "RotateHostKey(20){{host {}, new sign {}, staged_seq {}, assurance {}}}",
            r.host,
            fp(&r.sign_key),
            r.staged_seq,
            r.assurance.to_byte()
        ),
        Action::KeyEpochs(c) => format!(
            "KeyEpochs({k}){{user {:?}, machines {:?}}}",
            c.user.as_ref().map(|u| u.epoch),
            c.machines.iter().map(|(m, c)| (m.clone(), c.epoch)).collect::<Vec<_>>()
        ),
        Action::SetRenewalPolicy { always_assured } => format!("SetRenewalPolicy(21){{always_assured {always_assured}}}"),
        Action::RenewalPause { op, phase, removals } => format!("RenewalPause(22){{op {}, {:?}, {:?}}}", hex(&op[..4]), phase, removals),
        Action::SetAccept { host, from } => format!("SetAccept({k}){{{host}, {from:?}}}"),
        Action::AddDevice3(dv) => format!("AddDevice3({k}){{{}, host {}, admin {}}}", dv.id, dv.flags.host, dv.flags.admin),
        other => format!("kind {k} {}", format!("{other:?}").chars().take(80).collect::<String>()),
    }
}

async fn rotations(d: &Device) -> Value {
    let (s, v) = call(&d.hub.http, Some(&d.token), reqwest::Method::GET, "/v1/rotations", None).await;
    if s != 200 {
        log!("GET /v1/rotations → {s} {}", short(&v, 300));
    }
    v
}
fn row<'a>(v: &'a Value, host: &str) -> &'a Value {
    v["hosts"].as_array().and_then(|a| a.iter().find(|r| r["host"] == host)).unwrap_or(&Value::Null)
}

async fn me_machine(d: &Device, host: &str) -> Value {
    let (_, me) = call(&d.hub.http, Some(&d.token), reqwest::Method::GET, "/v1/me", None).await;
    me["machines"].as_array().and_then(|a| a.iter().find(|m| m["id"] == host)).cloned().unwrap_or(Value::Null)
}

fn summarize_machine(m: &Value) -> String {
    format!(
        "sign_key {} presence {} rotates {} reads_v3 {} last_seen {}",
        m["sign_key"].as_str().map(|s| &s[..12.min(s.len())]).unwrap_or("-"),
        short(&m["presence"], 200),
        m["rotates"],
        m["reads_v3"],
        m["last_seen_ms"]
    )
}

// ---------------------------------------------------------------- commands

async fn signup(args: &[String]) -> R<()> {
    let [hub, email, invite_file, pw_file] = args else { return Err("signup <hub> <email> <invite-file> <password-file>".into()) };
    let hub = HubUrl::parse(hub)?;
    let invite: Value = serde_json::from_str(&std::fs::read_to_string(invite_file).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let password = std::fs::read_to_string(pw_file).map_err(|e| e.to_string())?.trim().to_string();
    let (s, v) = call(&hub.http, None, reqwest::Method::POST, "/v1/signup", Some(json!({"invite": invite["code"], "email": email, "password": password}))).await;
    log!("POST /v1/signup → {s}: account {} email_verified {} verification {}", v["account"], v["email_verified"], v["verification"]);
    if s != 200 {
        return Err(format!("signup: {s} {v}"));
    }
    let account = v["account"].as_str().ok_or("no account")?.to_string();
    let login = v["token"].as_str().ok_or("no token")?.to_string();
    let sign = SigningKey::generate(&mut rand::rngs::OsRng);
    let dh = DhKeypair::generate(&mut rand::rngs::OsRng);
    let (s, v) = call(
        &hub.http,
        Some(&login),
        reqwest::Method::POST,
        "/v1/devices",
        Some(json!({"sign_key": hex(&sign.verifying_key().to_bytes()), "dh_key": hex(&dh.public())})),
    )
    .await;
    log!("POST /v1/devices → {s}: {}", short(&v, 200));
    let id = v["device"].as_str().ok_or("no device")?.to_string();
    let mut st = State {
        hub: hub.http.clone(),
        account: account.clone(),
        id: id.clone(),
        sign: hex(&sign.to_bytes()),
        dh: hex(dh.secret()),
        ..Default::default()
    };
    save(&st)?;
    let mut d = Device::from_keys(&hub, &account, &id, sign, dh).await?;
    d.create_genesis3(false).await?;
    st.user_keys = d.user_keys.iter().map(|(e, k)| (*e, hex(k))).collect();
    save(&st)?;
    let chain = d.chain().ok_or("no chain")?;
    log!("genesis written: account {account}, admin device {id}, chain v3 {}, entries {:?}", chain.is_v3(), kinds(chain, 0));
    Ok(())
}

async fn enroll(args: &[String]) -> R<()> {
    let [label] = args else { return Err("enroll <label>".into()) };
    let mut st = load()?;
    let mut d = device(&st).await?;
    let code = d.new_enrollment().await?;
    println!("CODE {code}");
    log!("enrollment code issued for {label}; waiting for the agent's proof");
    let mk = rh_devkit::random_key();
    let mut tries = 0;
    let machine = loop {
        match d.complete_enrollment_v3(&code, false, &mk, Duration::from_secs(600)).await {
            Ok(m) => break m,
            Err(e) if tries < 12 && e.contains("409") => {
                log!("complete enrollment: {e} — again in 5 s");
                tries += 1;
                tokio::time::sleep(Duration::from_secs(5)).await;
                d.sync_chain().await?;
            }
            Err(e) => return Err(e),
        }
    };
    st.labels.insert(label.clone(), machine.id.clone());
    st.mks.entry(machine.id.clone()).or_default().insert(0, hex(&mk));
    save(&st)?;
    d.wrap_machine_key(&machine.id, 0, &mk).await?;
    log!("MACHINE {label} {} sign {} added; entries {:?}", machine.id, fp(&machine.sign_key), kinds(d.chain().unwrap(), d.chain().unwrap().seq().saturating_sub(1)));
    Ok(())
}

async fn handover(args: &[String]) -> R<()> {
    let [label] = args else { return Err("handover <host>".into()) };
    let st = load()?;
    let host = st.host(label);
    let mut d = device(&st).await?;
    let w = watch(&d, &st.labels).await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let (epoch, key) = st.mks.get(&host).and_then(|m| m.iter().next_back()).map(|(e, k)| (*e, k.clone())).ok_or("no key")?;
    let (_relay, o) = open_to(&mut d, Some(&w.link), &host, "admin").await?;
    let r = rpc(&o.channel, "POST", "/rh/machine-key", Some(&json!({"epoch": epoch, "key": key, "reencrypt": false}))).await?;
    log!("POST /rh/machine-key epoch {epoch} → {} {}", r.status, String::from_utf8_lossy(&r.body));
    let r = rpc(&o.channel, "GET", "/rh/info", None).await?;
    log!("GET /rh/info → {} {}", r.status, String::from_utf8_lossy(&r.body).chars().take(300).collect::<String>());
    Ok(())
}

async fn policy(_: &[String]) -> R<()> {
    let st = load()?;
    let mut d = device(&st).await?;
    match d.append(Action::SetRenewalPolicy { always_assured: false }).await {
        Ok(seq) => log!("SetRenewalPolicy(false) → seq {seq}; entries {:?}", kinds(d.chain().unwrap(), seq)),
        Err(e) => log!("SetRenewalPolicy refused: {e}"),
    }
    Ok(())
}

async fn status(_: &[String]) -> R<()> {
    let st = load()?;
    let d = device(&st).await?;
    let (s, me) = call(&d.hub.http, Some(&d.token), reqwest::Method::GET, "/v1/me", None).await;
    log!("GET /v1/me → {s}: plan {} chain_v3 {} rotations {}", short(&me["plan"]["id"], 40), short(&me["chain_v3"], 120), short(&me["rotations"], 500));
    for m in me["machines"].as_array().into_iter().flatten() {
        log!("  machine {} ({}): {}", m["id"].as_str().unwrap_or("?"), st.label(m["id"].as_str().unwrap_or("?")), summarize_machine(m));
    }
    let v = rotations(&d).await;
    log!("GET /v1/rotations: enabled {} window_ms {} now_ms {}", v["enabled"], v["window_ms"], v["now_ms"]);
    for r in v["hosts"].as_array().into_iter().flatten() {
        log!("  rotation {}: {}", st.label(r["host"].as_str().unwrap_or("?")), short(r, 700));
    }
    let chain = d.chain().ok_or("no chain")?;
    for dsp in v["disputes"].as_array().into_iter().flatten() {
        let host = dsp["host"].as_str().unwrap_or("");
        let verified = if dsp["kind"] == "disputed" {
            match (dsp["seq"].as_u64(), dsp["sig"].as_str().map(k64)) {
                (Some(seq), Some(Ok(sig))) => format!("verified by this device: {}", chain.verify_rotate_dispute(host, seq, &sig)),
                _ => "no sig".into(),
            }
        } else {
            String::new()
        };
        log!("  dispute {}: {} {verified}", st.label(host), short(dsp, 500));
    }
    let (s, r) = call(&d.hub.http, Some(&d.token), reqwest::Method::GET, "/v1/relays", None).await;
    log!("GET /v1/relays → {s}: account_set {} agents {} relays {}", short(&r["account_set"], 200), short(&r["agents"], 300), r["relays"].as_array().map_or(0, |a| a.len()));
    log!("chain seq {} entries {:?}", chain.seq(), kinds(chain, 0));
    for h in chain.hosts() {
        log!(
            "  host {} ({}): sign {} dh {} rotated_seq {:?} retired {:?} accept {:?} paused_edges {:?}",
            h.info.id,
            st.label(&h.info.id),
            fp(&h.info.sign_key),
            fp(&h.info.dh_key),
            h.rotated_seq,
            h.retired.iter().map(|r| (fp(&r.sign_key), r.until_seq, r.by_assured)).collect::<Vec<_>>(),
            chain.base_accept(&h.info.id),
            chain.paused_edges(&h.info.id)
        );
    }
    for p in chain.open_pauses() {
        log!("  open pause op {} start {} by {} removals {:?}", hex(&p.op[..4]), p.start_seq, p.signer, p.removals);
    }
    Ok(())
}

async fn watch_cmd(args: &[String]) -> R<()> {
    let secs: u64 = args.first().and_then(|s| s.parse().ok()).unwrap_or(60);
    let st = load()?;
    let d = device(&st).await?;
    let _w = watch(&d, &st.labels).await?;
    log!("watching the admin link for {secs} s");
    tokio::time::sleep(Duration::from_secs(secs)).await;
    Ok(())
}

async fn rotate(args: &[String]) -> R<()> {
    let (label, assurance, sas_file) = match args {
        [l, a] => (l, a, None),
        [l, a, f] => (l, a, Some(f.clone())),
        _ => return Err("rotate <host> <0|1> [sas-file]".into()),
    };
    let assurance = Assurance::from_byte(assurance.parse().map_err(|_| "assurance")?).map_err(|e| e.to_string())?;
    let mut st = load()?;
    let host = st.host(label);
    let mut d = device(&st).await?;
    let w = watch(&d, &st.labels).await?;
    let t0 = now_ms();
    let before = d.chain().ok_or("no chain")?.clone();
    let old = before.machine(&host).ok_or("not a current host")?.info.clone();
    log!("renewing {label} ({host}) assurance {}: old sign {} dh {}", assurance.to_byte(), fp(&old.sign_key), fp(&old.dh_key));
    log!("  before: {}", summarize_machine(&me_machine(&d, &host).await));
    let mut nonce = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let eph = DhKeypair::generate(&mut rand::rngs::OsRng);
    let nonce_hash = hex(&Sha256::digest(nonce));
    let (before, minted) = probe_device_role(&st, &host, "before the request", None).await;
    // A device-role relay ticket from before the request: good now, and a
    // connection on it held open across the request.
    let old_tk = old_ticket(minted);
    let held: Held = Arc::default();
    if let Some(t) = &old_tk {
        t.try_signin(&st, &host, "before the request", None).await;
        t.try_signin(&st, &host, "before the request, held open", Some(held.clone())).await;
    }
    let t_put0 = now_ms();
    let (s, v) = call(
        &d.hub.http,
        Some(&d.token),
        reqwest::Method::PUT,
        &format!("/v1/rotations/{host}"),
        Some(json!({"nonce_hash": nonce_hash, "assurance": assurance.to_byte()})),
    )
    .await;
    log!("PUT /v1/rotations/{label} {{nonce_hash, assurance {}}} → {s} {}", assurance.to_byte(), short(&v, 500));
    if s != 200 {
        return Err(format!("attempt refused: {s}"));
    }
    let t_put = now_ms();
    log!("attempt registered: PUT took {} ms", t_put - t_put0);
    // The gap: the ticket from before, tried again until the relay refuses it
    // (at most 20 s: the attempt has its own clock).
    if let Some(t) = &old_tk {
        let mut n = 0;
        loop {
            let t_try = now_ms();
            let r = t.try_signin(&st, &host, &format!("attempt, try {n} at +{} ms", t_try - t_put), None).await;
            if matches!(&r, RelayIn::Closed(4403, _)) {
                log!("GAP CHECK: the ticket from before is refused as revoked {} ms after the attempt was registered (try {n}, begun at +{} ms)", now_ms() - t_put, t_try - t_put);
                break;
            }
            if now_ms() - t_put > 20_000 {
                log!("GAP CHECK: the ticket from before is NOT refused 20 s after the attempt was registered: {r:?}");
                break;
            }
            n += 1;
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        held_report(&held, t_put, "the attempt");
    }
    probe_device_role(&st, &host, "attempt", before.as_deref()).await;
    let withdraw = |d: &Device| {
        let (base, token, host) = (d.hub.http.clone(), d.token.clone(), host.clone());
        async move {
            let (s, v) = call(&base, Some(&token), reqwest::Method::DELETE, &format!("/v1/rotations/{host}"), None).await;
            log!("DELETE /v1/rotations/{host} → {s} {}", short(&v, 200));
        }
    };
    // The view the proof is made against.
    let mut answer = Value::Null;
    let mut view = (0u64, [0u8; 32]);
    // Patient, as a page may be: until 30 s before the attempt runs out (hub clock).
    let (_, rows) = call(&d.hub.http, Some(&d.token), reqwest::Method::GET, "/v1/rotations", None).await;
    let offset = rows["now_ms"].as_u64().unwrap_or(now_ms()) as i64 - now_ms() as i64;
    let give_up = (v["attempt_expires_ms"].as_u64().unwrap_or(now_ms() + 120_000) as i64 - offset - 30_000) as u64;
    let mut attempt = 0;
    while now_ms() < give_up {
        attempt += 1;
        d.sync_chain().await?;
        w.attest(&d);
        tokio::time::sleep(Duration::from_secs(2)).await;
        let chain = d.chain().unwrap();
        view = (chain.seq(), chain.head());
        let (_relay, o) = match open_to(&mut d, Some(&w.link), &host, "admin").await {
            Ok(x) => x,
            Err(e) => {
                log!("no channel ({e}); try {attempt}, giving up at {}", give_up);
                tokio::time::sleep(Duration::from_secs(10)).await;
                continue;
            }
        };
        let body = json!({"nonce": hex(&nonce), "eph": hex(&eph.public()), "chainSeq": view.0, "chainHead": hex(&view.1), "assurance": assurance.to_byte()});
        let r = rpc(&o.channel, "POST", "/rh/rotate", Some(&body)).await?;
        let j = r.json().unwrap_or(Value::Null);
        log!("POST /rh/rotate (chainSeq {}) → {} {}", view.0, r.status, short(&j, 700));
        if r.status == 200 {
            answer = j;
            break;
        }
        if r.status == 409 || r.status == 412 || r.status == 504 {
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }
        break;
    }
    if answer.is_null() {
        withdraw(&d).await;
        return Err("no answer to /rh/rotate".into());
    }
    let chain = d.chain().unwrap().clone();
    let get = |k: &str| answer[k].as_str().unwrap_or("").to_string();
    let rot = HostRotation {
        host: host.clone(),
        sign_key: k32(&get("signKey"))?,
        dh_key: k32(&get("dhKey"))?,
        staged_seq: view.0,
        staged_head: view.1,
        nonce,
        assurance,
        old_sig: k64(&get("oldSig"))?,
        new_sig: k64(&get("newSig"))?,
    };
    let body = rot.body(&st.account, &old.sign_key, &old.dh_key);
    let mac = k32(&get("mac"))?;
    let checks = [
        ("staged view is the one asked", answer["stagedSeq"].as_u64() == Some(view.0) && get("stagedHead") == hex(&view.1)),
        ("old and new keys signed the proof", rot.verify(&st.account, &old.sign_key, &old.dh_key)),
        ("MAC proves the new DH key", rh_proto::chain::check_rotate_confirm(&eph, &rot.dh_key, &body, &mac)),
        ("new keys never named by the chain", !chain.ever_named(&rot.sign_key) && !chain.ever_named(&rot.dh_key)),
    ];
    for (what, ok) in checks {
        log!("  check: {what}: {}", if ok { "ok" } else { "FAILED" });
    }
    if checks.iter().any(|(_, ok)| !ok) {
        withdraw(&d).await;
        return Err("the answer does not check out".into());
    }
    log!("new sign {} dh {} readyMs {}", fp(&rot.sign_key), fp(&rot.dh_key), answer["readyMs"]);
    if assurance == Assurance::OnDevice {
        let want = rh_proto::chain::rotate_sas(&body).replace('-', "");
        let path = sas_file.ok_or("assurance 1 needs a sas-file")?;
        log!("waiting for the code shown on {label} to be typed into {path}");
        let deadline = now_ms() + 600_000;
        let typed = loop {
            if let Ok(t) = std::fs::read_to_string(&path) {
                if !t.trim().is_empty() {
                    break t;
                }
            }
            if now_ms() > deadline {
                withdraw(&d).await;
                return Err("no code typed".into());
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        };
        let typed: String = typed.to_uppercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        let ok = typed == want;
        log!("  check: typed code {} the proof's code: {}", if ok { "matches" } else { "DOES NOT MATCH" }, if ok { "ok" } else { "FAILED" });
        if !ok {
            withdraw(&d).await;
            return Err("wrong code".into());
        }
    }
    // Staged, and its window.
    let mut last = String::new();
    let (ready_ms, hub_offset) = loop {
        let v = rotations(&d).await;
        let r = row(&v, &host).clone();
        let st_ = format!("{}/{}", r["state"], r["failure"]);
        if st_ != last {
            log!("rotation row: {}", short(&r, 600));
            last = st_;
        }
        if r["state"] == "staged" {
            let hub_now = v["now_ms"].as_u64().unwrap_or(now_ms());
            break (r["ready_ms"].as_u64().unwrap_or(0), hub_now as i64 - now_ms() as i64);
        }
        if !r["failure"].is_null() || r["state"] == "pending" || r.is_null() {
            return Err(format!("not staged: {r}"));
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    };
    let staged_row = row(&rotations(&d).await, &host).clone();
    log!(
        "staged: staged_ms {} ready_ms {} (window {} ms) hub clock offset {} ms",
        staged_row["staged_ms"],
        ready_ms,
        ready_ms.saturating_sub(staged_row["staged_ms"].as_u64().unwrap_or(0)),
        hub_offset
    );
    probe_device_role(&st, &host, "staged", before.as_deref()).await;
    if let Some(t) = &old_tk {
        let r = t.try_signin(&st, &host, &format!("staged, +{} ms", now_ms() - t_put), None).await;
        log!("GAP CHECK (staged): the ticket from before → {r:?}");
        held_report(&held, t_put, "the staging");
        // The host's agent, meanwhile, still on its relay.
        match open_to(&mut d, None, &host, "admin (staged)").await {
            Ok((Some(_l), o)) => match rpc(&o.channel, "GET", "/rh/info", None).await {
                Ok(r) => log!("AGENT CHECK (staged): channel to {label} over its relay → GET /rh/info {}", r.status),
                Err(e) => log!("AGENT CHECK (staged): channel to {label} over its relay, GET /rh/info: {e}"),
            },
            Ok((None, _)) => log!("AGENT CHECK (staged): no relay channel, the hub's"),
            Err(e) => log!("AGENT CHECK (staged): no channel to {label} over a relay: {e}"),
        }
    }
    let mk = rh_devkit::random_key();
    let epoch = chain.machine_key_epochs(&host);
    let batch = |rot: &HostRotation| {
        vec![
            Action::RotateHostKey(rot.clone()),
            Action::KeyEpochs(KeyCommits {
                user: None,
                machines: vec![(host.clone(), KeyCommit { epoch, hash: rh_proto::chain::machine_key_commit(&st.account, &host, epoch, &mk) })],
            }),
        ]
    };
    let hub_now = || (now_ms() as i64 + hub_offset) as u64;
    if ready_ms > hub_now() + 15_000 {
        log!("early commit {} ms before ready_ms (expect 409 rotation_not_ready window)", ready_ms - hub_now());
        d.sync_chain().await?;
        match d.append_batch(batch(&rot)).await {
            Ok(seq) => log!("EARLY COMMIT TAKEN at seq {seq} (deviation)"),
            Err(e) => log!("early commit → {e}"),
        }
    }
    // Watch the window: every sign-in owes a confirmation.
    loop {
        let left = ready_ms as i64 - hub_now() as i64;
        if left <= -1000 {
            break;
        }
        tokio::time::sleep(Duration::from_millis((left + 1500).clamp(100, 10_000) as u64)).await;
        let r = row(&rotations(&d).await, &host).clone();
        if r["state"] != "staged" {
            log!("rotation row left staged in the window: {}", short(&r, 600));
            return Err("voided in the window".into());
        }
    }
    let mut seq = 0;
    for tries in 0..6 {
        d.sync_chain().await?;
        match d.append_batch(batch(&rot)).await {
            Ok(s) => {
                seq = s;
                break;
            }
            Err(e) => {
                log!("commit → {e}");
                if tries < 5 && e.contains("window") {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
                return Err(e);
            }
        }
    }
    let t_commit = now_ms();
    log!(
        "COMMITTED: POST /v1/chain [RotateHostKey, KeyEpochs] → seq {seq} ({} ms after the attempt was registered, {} ms after the command started)",
        t_commit - t_put,
        t_commit - t0
    );
    st.mks.entry(host.clone()).or_default().insert(epoch, hex(&mk));
    save(&st)?;
    if let Err(e) = d.wrap_machine_key(&host, epoch, &mk).await {
        log!("wrap mk: {e}");
    }
    let chain = d.chain().unwrap().clone();
    log!("chain after: {:?}", kinds(&chain, seq.saturating_sub(1)));
    let rec = chain.machine(&host).unwrap();
    log!(
        "chain record of {label}: sign {} dh {} rotated_seq {:?} retired {:?} keys_at(seq-1) sign {}",
        fp(&rec.info.sign_key),
        fp(&rec.info.dh_key),
        rec.rotated_seq,
        rec.retired.iter().map(|r| (fp(&r.sign_key), fp(&r.dh_key), r.until_seq, r.by_assured)).collect::<Vec<_>>(),
        chain.keys_at(&host, seq.saturating_sub(2)).map(|k| fp(&k.sign_key)).unwrap_or_default()
    );
    log!("rotation row after commit: {}", short(row(&rotations(&d).await, &host), 600));
    match w.wait_push(t_commit.saturating_sub(2000), Duration::from_secs(10), |v| v["t"] == "rotation" && v["host"] == host.as_str() && v["state"] == "committed").await {
        Some(v) => log!("push seen: {v}"),
        None => log!("no `rotation committed` push within 10 s"),
    }
    // The host back under its new key.
    let want = hex(&rot.sign_key);
    let deadline = now_ms() + 180_000;
    loop {
        let m = me_machine(&d, &host).await;
        let online = m["presence"].to_string().contains("online") || m["presence"]["online"] == true;
        if m["sign_key"] == want.as_str() && online {
            log!("{label} signed in again under its new key {} ms after the commit: {}", now_ms() - t_commit, summarize_machine(&m));
            break;
        }
        if now_ms() > deadline {
            log!("{label} not back under its new key within 180 s: {}", summarize_machine(&m));
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    if let Some(t) = &old_tk {
        let r = t.try_signin(&st, &host, &format!("after the commit, +{} ms", now_ms() - t_put), None).await;
        log!("GAP CHECK (after the commit): the ticket from before → {r:?}");
        held_report(&held, t_put, "the end");
    }
    w.attest(&d);
    tokio::time::sleep(Duration::from_secs(3)).await;
    for attempt in 0..6 {
        match open_to(&mut d, Some(&w.link), &host, "admin").await {
            Ok((_relay, o)) => {
                let r = rpc(&o.channel, "POST", "/rh/machine-key", Some(&json!({"epoch": epoch, "key": hex(&mk), "reencrypt": true}))).await?;
                log!("POST /rh/machine-key epoch {epoch} reencrypt → {} {} (channel to the new DH key)", r.status, String::from_utf8_lossy(&r.body));
                let r = rpc(&o.channel, "GET", "/rh/info", None).await?;
                log!("GET /rh/info over the new key → {} {}", r.status, String::from_utf8_lossy(&r.body).chars().take(200).collect::<String>());
                break;
            }
            Err(e) => {
                log!("hand-over channel try {attempt}: {e}");
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        }
    }
    match d.send_host_keys(&host).await {
        Ok(n) => log!("sealed {n} host key(s) to {label}'s device role (purpose 5)"),
        Err(e) => log!("send_host_keys({label}): {e}"),
    }
    log!("after: {}", summarize_machine(&me_machine(&d, &host).await));
    tokio::time::sleep(Duration::from_secs(3)).await;
    Ok(())
}

/// With `ROTTEST_DEVICE_SEED`: the host's device role (its own key) asks for
/// a session and a relay ticket while its renewal is under way — both should
/// be refused (`rotation_pending`).
async fn probe_device_role(st: &State, host: &str, when: &str, before: Option<&str>) -> (Option<String>, Option<OldTicket>) {
    let Ok(seed) = std::env::var("ROTTEST_DEVICE_SEED") else { return (None, None) };
    let id = match seed_identity(&seed) {
        Ok(id) => id,
        Err(e) => {
            log!("device-role probe: {e}");
            return (None, None);
        }
    };
    let Ok(hub) = HubUrl::parse(&st.hub) else { return (None, None) };
    let who = format!("{} device role ({when})", st.label(host));
    let fresh = session_signin(&st.hub, &hub, &st.account, host, id.signing_key(), &who).await;
    // A session made before the renewal was asked for, used now.
    if let Some(token) = before {
        ticket(&st.hub, token, &format!("{who}, with a session from before")).await;
    }
    let mut minted = None;
    if let Some(token) = &fresh {
        if let Some((relay, origin, text)) = ticket(&st.hub, token, &who).await {
            minted = Some(OldTicket { relay, origin, text, key: id.signing_key().clone() });
        }
    }
    (fresh, minted)
}

fn held_report(held: &Held, t_put: u64, by: &str) {
    match held.lock().unwrap().as_ref() {
        Some((at, code, reason)) => log!("HELD CHECK (by {by}): the device-role connection from before was closed {code} {reason:?} at {:+} ms from the attempt's registration", *at as i64 - t_put as i64),
        None => log!("HELD CHECK (by {by}): the device-role connection from before is STILL OPEN"),
    }
}

/// The device-role ticket from before the request: `ROTTEST_DEVICE_TICKET`
/// (saved by `asdevice`), else the one just minted.
fn old_ticket(minted: Option<OldTicket>) -> Option<OldTicket> {
    let Ok(file) = std::env::var("ROTTEST_DEVICE_TICKET") else { return minted };
    let seed = std::env::var("ROTTEST_DEVICE_SEED").ok()?;
    let read = || -> R<OldTicket> {
        let t: Value = serde_json::from_str(&std::fs::read_to_string(&file).map_err(|e| format!("{file}: {e}"))?).map_err(|e| e.to_string())?;
        let s = |k: &str| t[k].as_str().unwrap_or("").to_string();
        Ok(OldTicket { relay: s("relay"), origin: s("origin"), text: s("ticket"), key: seed_identity(&seed)?.signing_key().clone() })
    };
    match read() {
        Ok(t) => {
            if let Ok(p) = rh_proto::ticket::SignedTicket::parse(&t.text) {
                log!("device-role ticket from {file}: relay {} role {:?} id {} issued {} expires {}", t.relay, p.ticket.role, p.ticket.id, p.ticket.issued_ms, p.ticket.expires_ms);
            }
            Some(t)
        }
        Err(e) => {
            log!("ROTTEST_DEVICE_TICKET: {e}");
            minted
        }
    }
}

/// Sign in with a key, raw, on the hub's `/v1/agent`; then one `chain_sub`.
async fn agent_signin(hub: &HubUrl, account: &str, id: &str, key: &SigningKey, what: &str) {
    let ws = match tokio_tungstenite::connect_async(format!("{}{AGENT_PATH}", hub.ws)).await {
        Ok((ws, _)) => ws,
        Err(e) => return log!("{what}: dial: {e}"),
    };
    let (mut sink, mut stream) = ws.split();
    let mut nonce = [0u8; 32];
    while let Some(Ok(m)) = stream.next().await {
        if let Message::Text(t) = m {
            let v: Value = serde_json::from_str(t.as_str()).unwrap_or_default();
            nonce = k32(v["nonce"].as_str().unwrap_or("")).unwrap_or_default();
            break;
        }
    }
    let time_ms = now_ms();
    let sig = HubLogin { role: LoginRole::Agent, account: account.into(), id: id.into(), hub: hub.origin.clone(), nonce, time_ms }.sign(key).unwrap();
    let login = json!({"t": "login", "role": "agent", "account": account, "id": id, "time_ms": time_ms, "sig": hex(&sig)});
    let _ = sink.send(Message::Text(login.to_string().into())).await;
    let t0 = now_ms();
    let mut subbed = false;
    let mut chain_entries = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(20), stream.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                let v: Value = serde_json::from_str(t.as_str()).unwrap_or_default();
                log!("{what}: hub said {}", short(&v, 300));
                if v["t"] == "welcome" {
                    log!("{what}: SIGNED IN (welcome) — closing");
                    let _ = sink.close().await;
                    return;
                }
                if v["t"] == "error" && !subbed {
                    subbed = true;
                    let _ = sink.send(Message::Text(json!({"t": "chain_sub", "from": 0}).to_string().into())).await;
                    log!("{what}: sent chain_sub {{from: 0}}");
                }
            }
            Ok(Some(Ok(Message::Binary(b)))) => {
                if let Ok(Frame::Chain { entries, .. }) = Frame::decode(&b) {
                    for e in entries {
                        if let Ok(e) = Entry::decode(&e) {
                            chain_entries.push(format!("{}:{}", e.seq(), e.draft.action.kind()));
                        }
                    }
                }
            }
            Ok(Some(Ok(Message::Close(c)))) => {
                log!(
                    "{what}: closed {} {:?} after {} ms; CHAIN entries served (seq:kind): {:?}",
                    c.as_ref().map_or(0, |c| u16::from(c.code)),
                    c.as_ref().map(|c| c.reason.to_string()),
                    now_ms() - t0,
                    chain_entries
                );
                return;
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => return log!("{what}: socket error {e}; CHAIN entries {:?}", chain_entries),
            Ok(None) => return log!("{what}: socket ended; CHAIN entries {:?}", chain_entries),
            Err(_) => return log!("{what}: nothing for 20 s"),
        }
    }
}

async fn session_signin(base: &str, hub: &HubUrl, account: &str, id: &str, key: &SigningKey, what: &str) -> Option<String> {
    let (s, v) = call(base, None, reqwest::Method::POST, "/v1/session/challenge", Some(json!({"account": account, "device": id}))).await;
    if s != 200 {
        log!("{what}: POST /v1/session/challenge → {s} {}", short(&v, 200));
        return None;
    }
    let nonce = k32(v["nonce"].as_str().unwrap_or("")).ok()?;
    let time_ms = now_ms();
    let sig = HubLogin { role: LoginRole::DeviceSession, account: account.into(), id: id.into(), hub: hub.origin.clone(), nonce, time_ms }.sign(key).ok()?;
    let (s, v) = call(
        base,
        None,
        reqwest::Method::POST,
        "/v1/session",
        Some(json!({"account": account, "device": id, "time_ms": time_ms, "nonce": hex(&nonce), "signature": hex(&sig)})),
    )
    .await;
    let mut shown = v.clone();
    if shown.get("token").is_some() {
        shown["token"] = json!("<redacted>");
    }
    log!("{what}: POST /v1/session → {s} {}", short(&shown, 300));
    v["token"].as_str().map(String::from)
}

/// What a relay sign-in came to.
#[derive(Debug, Clone, PartialEq)]
enum RelayIn {
    Welcome,
    Closed(u16, String),
    Failed(String),
}

/// When a held relay connection was closed: (local ms, code, reason).
type Held = Arc<Mutex<Option<(u64, u16, String)>>>;

async fn relay_signin(origin: &str, ticket: &str, account: &str, id: &str, key: &SigningKey, role: LoginRole, what: &str) -> RelayIn {
    relay_signin_hold(origin, ticket, account, id, key, role, what, None).await
}

/// [`relay_signin`]; with `hold`, a connection signed in is kept open (read
/// until the relay closes it, which is noted in `hold`) instead of closed.
#[allow(clippy::too_many_arguments)]
async fn relay_signin_hold(origin: &str, ticket: &str, account: &str, id: &str, key: &SigningKey, role: LoginRole, what: &str, hold: Option<Held>) -> RelayIn {
    let ws = match tokio_tungstenite::connect_async(format!("{origin}{}", rh_devkit::CONNECT_PATH)).await {
        Ok((ws, _)) => ws,
        Err(e) => {
            log!("{what}: dial {origin}: {e}");
            return RelayIn::Failed(format!("dial: {e}"));
        }
    };
    let (mut sink, mut stream) = ws.split();
    let mut nonce = [0u8; 32];
    while let Some(Ok(m)) = stream.next().await {
        if let Message::Text(t) = m {
            let v: Value = serde_json::from_str(t.as_str()).unwrap_or_default();
            nonce = k32(v["nonce"].as_str().unwrap_or("")).unwrap_or_default();
            break;
        }
    }
    let time_ms = now_ms();
    let sig = HubLogin { role, account: account.into(), id: id.into(), hub: origin.to_string(), nonce, time_ms }.sign(key).unwrap();
    let _ = sink.send(Message::Text(json!({"t": "login", "ticket": ticket, "time_ms": time_ms, "sig": hex(&sig)}).to_string().into())).await;
    loop {
        match tokio::time::timeout(Duration::from_secs(10), stream.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                let v: Value = serde_json::from_str(t.as_str()).unwrap_or_default();
                log!("{what}: relay said {}", short(&v, 300));
                if v["t"] == "welcome" {
                    let Some(held) = hold else {
                        log!("{what}: SIGNED IN on the relay — closing");
                        let _ = sink.close().await;
                        return RelayIn::Welcome;
                    };
                    log!("{what}: SIGNED IN on the relay — holding the connection open");
                    let what = what.to_string();
                    tokio::spawn(async move {
                        let _sink = sink;
                        let (code, reason) = loop {
                            match stream.next().await {
                                Some(Ok(Message::Close(c))) => break (c.as_ref().map_or(0, |c| u16::from(c.code)), c.as_ref().map(|c| c.reason.to_string()).unwrap_or_default()),
                                Some(Ok(Message::Text(t))) => log!("{what}: relay said {}", short(&serde_json::from_str::<Value>(t.as_str()).unwrap_or_default(), 300)),
                                Some(Ok(_)) => {}
                                Some(Err(e)) => break (0, format!("socket error {e}")),
                                None => break (0, "socket ended".into()),
                            }
                        };
                        log!("{what}: HELD CONNECTION CLOSED by the relay: {code} {reason:?}");
                        *held.lock().unwrap() = Some((now_ms(), code, reason));
                    });
                    return RelayIn::Welcome;
                }
            }
            Ok(Some(Ok(Message::Close(c)))) => {
                let (code, reason) = (c.as_ref().map_or(0, |c| u16::from(c.code)), c.as_ref().map(|c| c.reason.to_string()).unwrap_or_default());
                log!("{what}: relay closed {code} {reason:?}");
                return RelayIn::Closed(code, reason);
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => {
                log!("{what}: socket error {e}");
                return RelayIn::Failed(e.to_string());
            }
            Ok(None) => {
                log!("{what}: socket ended");
                return RelayIn::Failed("socket ended".into());
            }
            Err(_) => {
                log!("{what}: nothing for 10 s");
                return RelayIn::Failed("timeout".into());
            }
        }
    }
}

/// A device-role relay ticket of the host from before its renewal was asked
/// for (`ROTTEST_DEVICE_TICKET`, saved by `asdevice`, or one minted with the
/// session from before), and the key it is signed in with.
struct OldTicket {
    relay: String,
    origin: String,
    text: String,
    key: SigningKey,
}

impl OldTicket {
    async fn try_signin(&self, st: &State, host: &str, what: &str, hold: Option<Held>) -> RelayIn {
        relay_signin_hold(
            &self.origin,
            &self.text,
            &st.account,
            host,
            &self.key,
            LoginRole::RelayDevice,
            &format!("{} device role on relay {} with a ticket from before the request ({what})", st.label(host), self.relay),
            hold,
        )
        .await
    }
}

/// A host's device role, from its seed: a session and a relay ticket (saved to `out`).
async fn asdevice(args: &[String]) -> R<()> {
    let (label, seed, out, relay) = match args {
        [l, s, o] => (l, s, o, None),
        [l, s, o, r] => (l, s, o, Some(r.as_str())),
        _ => return Err("asdevice <host> <seed-file> <out> [relay]".into()),
    };
    let st = load()?;
    let host = st.host(label);
    let id = seed_identity(seed)?;
    let hub = HubUrl::parse(&st.hub)?;
    log!("{label} device role with key {}", fp(&id.sign_public()));
    let Some(token) = session_signin(&st.hub, &hub, &st.account, &host, id.signing_key(), &format!("{label} device session")).await else { return Ok(()) };
    if let Some((relay, origin, text)) = ticket_for(&st.hub, &token, &format!("{label} as device"), relay).await {
        std::fs::write(out, json!({"relay": relay, "origin": origin, "ticket": text}).to_string()).map_err(|e| e.to_string())?;
        log!("saved {label}'s device ticket for {relay} to {out}");
    }
    Ok(())
}

/// Sign-ins with a host's retired key: agent socket, device session, relay (with a ticket from before).
async fn oldkey(args: &[String]) -> R<()> {
    let (label, seed, ticket_file) = match args {
        [l, s] => (l, s, None),
        [l, s, t] => (l, s, Some(t)),
        _ => return Err("oldkey <host> <old-seed-file> [ticket-file]".into()),
    };
    let st = load()?;
    let host = st.host(label);
    let id = seed_identity(seed)?;
    let hub = HubUrl::parse(&st.hub)?;
    log!("{label}: sign-ins with key {}", fp(&id.sign_public()));
    agent_signin(&hub, &st.account, &host, id.signing_key(), &format!("{label} agent (key {})", fp(&id.sign_public()))).await;
    session_signin(&st.hub, &hub, &st.account, &host, id.signing_key(), &format!("{label} device session (key {})", fp(&id.sign_public()))).await;
    if let Some(f) = ticket_file {
        let t: Value = serde_json::from_str(&std::fs::read_to_string(f).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        relay_signin(
            t["origin"].as_str().unwrap_or(""),
            t["ticket"].as_str().unwrap_or(""),
            &st.account,
            &host,
            id.signing_key(),
            LoginRole::RelayDevice,
            &format!("{label} on relay {} with a ticket from before (key {})", t["relay"], fp(&id.sign_public())),
        )
        .await;
    }
    Ok(())
}

/// A host's device role on a relay with a ticket saved by `asdevice`, signed with `seed`.
async fn relaylogin(args: &[String]) -> R<()> {
    let [label, seed, ticket_file] = args else { return Err("relaylogin <host> <seed-file> <ticket-file>".into()) };
    let st = load()?;
    let host = st.host(label);
    let id = seed_identity(seed)?;
    let t: Value = serde_json::from_str(&std::fs::read_to_string(ticket_file).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    relay_signin(
        t["origin"].as_str().unwrap_or(""),
        t["ticket"].as_str().unwrap_or(""),
        &st.account,
        &host,
        id.signing_key(),
        LoginRole::RelayDevice,
        &format!("{label} device role on relay {} with a ticket saved earlier (key {})", t["relay"], fp(&id.sign_public())),
    )
    .await;
    Ok(())
}

async fn accept(args: &[String]) -> R<()> {
    let [label, rest @ ..] = args else { return Err("accept <host> admins|<host>...".into()) };
    let st = load()?;
    let host = st.host(label);
    let mut d = device(&st).await?;
    let from = if rest.first().map(String::as_str) == Some("admins") {
        Accept::Admins
    } else {
        let mut ids: Vec<String> = rest.iter().map(|l| st.host(l)).collect();
        ids.sort();
        Accept::Devices(ids)
    };
    match d.set_accept(&host, from.clone()).await {
        Ok(k) => log!("SetAccept {label} ← {from:?} → ok (machine key due: {})", k.is_some()),
        Err(e) => log!("SetAccept {label} refused: {e}"),
    }
    log!("entries {:?}", kinds(d.chain().unwrap(), d.chain().unwrap().seq().saturating_sub(1)));
    Ok(())
}

async fn pause(args: &[String]) -> R<()> {
    let [what, rest @ ..] = args else { return Err("pause start <host>:<id>,<id> ... | pause completed|cancelled".into()) };
    let mut st = load()?;
    let mut d = device(&st).await?;
    let (op, phase, removals) = if what == "start" {
        let mut op = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut op);
        let mut removals: Vec<(String, Vec<String>)> = rest
            .iter()
            .map(|spec| {
                let (h, ids) = spec.split_once(':').unwrap_or((spec, ""));
                let mut ids: Vec<String> = ids.split(',').filter(|s| !s.is_empty()).map(|l| st.host(l)).collect();
                ids.sort();
                (st.host(h), ids)
            })
            .collect();
        removals.sort();
        (op, PausePhase::Start, removals)
    } else {
        let op = k16(st.pause_op.as_deref().ok_or("no pause op")?)?;
        let phase = if what == "completed" { PausePhase::Completed } else { PausePhase::Cancelled };
        (op, phase, Vec::new())
    };
    let t = now_ms();
    match d.append(Action::RenewalPause { op, phase, removals: removals.clone() }).await {
        Ok(seq) => {
            log!("RenewalPause {phase:?} op {} removals {removals:?} → seq {seq} (POST /v1/chain {} ms)", hex(&op[..4]), now_ms() - t);
            if phase == PausePhase::Start {
                st.pause_op = Some(hex(&op));
                save(&st)?;
            }
            let chain = d.chain().unwrap();
            log!("entries {:?}; open pauses {}", kinds(chain, seq), chain.open_pauses().len());
            let w = watch(&d, &st.labels).await?;
            w.attest(&d);
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
        Err(e) => log!("RenewalPause {phase:?} refused: {e}"),
    }
    Ok(())
}

/// Control from one host (its device role, its seed) to another.
async fn control(args: &[String]) -> R<()> {
    let (from, seed, to, hold) = match args {
        [f, s, t] => (f, s, t, 0u64),
        [f, s, t, h] => (f, s, t, h.parse().unwrap_or(0)),
        _ => return Err("control <from-host> <seed-file> <to-host> [hold-secs]".into()),
    };
    let st = load()?;
    let (from_id, to_id) = (st.host(from), st.host(to));
    let id = seed_identity(seed)?;
    let hub = HubUrl::parse(&st.hub)?;
    let who = format!("{from}→{to}");
    let sign = id.signing_key().clone();
    let dh = DhKeypair::from_secret(*id.dh().secret());
    let mut d = match Device::from_keys(&hub, &st.account, &from_id, sign, dh).await {
        Ok(d) => d,
        Err(e) => return Ok(log!("{who}: {from}'s device role cannot sign in (key {}): {e}", fp(&id.sign_public()))),
    };
    let chain = d.chain().unwrap();
    log!(
        "{who}: {from}'s chain view: may_control({from},{to}) = {:?}; {to}'s base accept {:?}, paused edges {:?}",
        chain.may_control(&from_id, &to_id).map_err(|e| e.to_string()),
        chain.base_accept(&to_id),
        chain.paused_edges(&to_id)
    );
    let (relay_link, o) = match open_to(&mut d, None, &to_id, &who).await {
        Ok(x) => x,
        Err(_) => {
            // Over the hub too, for the record.
            match d.connect().await {
                Ok(link) => match d.open_channel_info(&link, &to_id).await {
                    Ok(_) => log!("{who}: over the hub: OPEN"),
                    Err(e) => log!("{who}: over the hub: {e}"),
                },
                Err(e) => log!("{who}: hub link: {e}"),
            }
            return Ok(());
        }
    };
    let r = rpc(&o.channel, "GET", "/rh/info", None).await;
    match &r {
        Ok(r) => log!("{who}: GET /rh/info → {} {}", r.status, String::from_utf8_lossy(&r.body).chars().take(160).collect::<String>()),
        Err(e) => log!("{who}: GET /rh/info: {e}"),
    }
    if hold > 0 {
        log!("{who}: holding the channel open up to {hold} s");
        let end = now_ms() + hold * 1000;
        let mut n = 0;
        while now_ms() < end {
            tokio::time::sleep(Duration::from_secs(1)).await;
            n += 1;
            if o.channel.is_closed() {
                log!("{who}: CHANNEL CLOSED by the far end");
                break;
            }
            if relay_link.as_ref().is_some_and(|l| l.is_closed()) {
                log!("{who}: relay link closed");
                break;
            }
            if n % 5 == 0 {
                match tokio::time::timeout(Duration::from_secs(8), rpc(&o.channel, "GET", "/rh/info", None)).await {
                    Ok(Ok(r)) if r.status == 200 => {}
                    Ok(Ok(r)) => log!("{who}: GET /rh/info → {}", r.status),
                    Ok(Err(e)) => {
                        log!("{who}: GET /rh/info: {e}");
                        break;
                    }
                    Err(_) => {
                        log!("{who}: GET /rh/info: no answer in 8 s");
                        break;
                    }
                }
            }
        }
        log!("{who}: hold over (channel closed: {})", o.channel.is_closed());
    }
    Ok(())
}

async fn hostkeys(args: &[String]) -> R<()> {
    let [label] = args else { return Err("hostkeys <host>".into()) };
    let st = load()?;
    let host = st.host(label);
    let mut d = device(&st).await?;
    match d.answer_key_requests().await {
        Ok(who) => log!("answered key requests (purpose 4) from {:?}", who.iter().map(|w| st.label(w)).collect::<Vec<_>>()),
        Err(e) => log!("answer_key_requests: {e}"),
    }
    match d.send_host_keys(&host).await {
        Ok(n) => log!("sealed {n} host key(s) to {label}'s device role (purpose 5): hosts admitting it {:?}", d.chain().unwrap().hosts_admitting(&host).map(|h| st.label(&h.info.id)).collect::<Vec<_>>()),
        Err(e) => log!("send_host_keys({label}): {e}"),
    }
    Ok(())
}

async fn put(args: &[String]) -> R<()> {
    let [label, a] = args else { return Err("put <host> <0|1>".into()) };
    let st = load()?;
    let host = st.host(label);
    let d = device(&st).await?;
    let mut nonce = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let (s, v) = call(
        &d.hub.http,
        Some(&d.token),
        reqwest::Method::PUT,
        &format!("/v1/rotations/{host}"),
        Some(json!({"nonce_hash": hex(&Sha256::digest(nonce)), "assurance": a.parse::<u8>().unwrap_or(0)})),
    )
    .await;
    log!("PUT /v1/rotations/{label} {{assurance {a}}} → {s} {}", short(&v, 400));
    if s == 200 {
        let (s, v) = call(&d.hub.http, Some(&d.token), reqwest::Method::DELETE, &format!("/v1/rotations/{host}"), None).await;
        log!("DELETE /v1/rotations/{label} → {s} {}", short(&v, 200));
    }
    if a == "1" {
        if let Some(_t) = ticket(&d.hub.http, &d.token, "admin (with the attempt open)").await {}
    }
    Ok(())
}

async fn redispute(args: &[String]) -> R<()> {
    let [label] = args else { return Err("redispute <host>".into()) };
    let st = load()?;
    let host = st.host(label);
    let d = device(&st).await?;
    let v = rotations(&d).await;
    let Some(dsp) = v["disputes"].as_array().and_then(|a| a.iter().find(|x| x["host"] == host.as_str() && x["kind"] == "disputed")).cloned() else {
        return Err("no disputed statement listed".into());
    };
    let body = json!({"seq": dsp["seq"], "entry_hash": dsp["entry_hash"], "sig": dsp["sig"]});
    let (s, r) = call(&d.hub.http, None, reqwest::Method::POST, &format!("/v1/rotations/{host}/disputes"), Some(body)).await;
    log!("POST /v1/rotations/{label}/disputes (the statement again, seq {}) → {s} {}", dsp["seq"], short(&r, 300));
    log!("row: {}", short(row(&rotations(&d).await, &host), 600));
    Ok(())
}

/// Remove hosts from the account (the end of a run: the free plan holds
/// three, and an account kept for later runs should not keep dead ones).
async fn remove(args: &[String]) -> R<()> {
    if args.is_empty() {
        return Err("remove <host>...".into());
    }
    let mut st = load()?;
    let mut d = device(&st).await?;
    let ids: Vec<String> = args.iter().map(|l| st.host(l)).collect();
    let others = d.chain().ok_or("no chain")?.hosts().filter(|h| h.is_current_host() && !ids.contains(&h.info.id)).count();
    if others == 0 {
        match d.append_batch(ids.iter().map(|id| Action::RemoveDevice3(id.clone())).collect()).await {
            Ok(seq) => log!("removed {:?} → seq {seq}", args),
            Err(e) => return Err(format!("remove: {e}")),
        }
    } else {
        for id in &ids {
            let turned = d.remove3_and_rotate_hosts(id).await?;
            log!("removed {} (the remaining hosts' machine keys turned over: {:?})", st.label(id), turned.machines.iter().map(|(h, e, _)| (st.label(h), *e)).collect::<Vec<_>>());
            if let Some(e) = turned.uk_epoch {
                st.user_keys.insert(e, hex(&d.user_keys[&e]));
            }
            for (h, e, k) in turned.machines {
                st.mks.entry(h).or_default().insert(e, hex(&k));
            }
        }
    }
    for id in &ids {
        st.labels.retain(|_, v| v != id);
        st.mks.remove(id);
    }
    save(&st)
}

async fn tickets_cmd(_: &[String]) -> R<()> {
    let st = load()?;
    let d = device(&st).await?;
    ticket(&d.hub.http, &d.token, "admin").await;
    Ok(())
}

async fn channel_cmd(args: &[String]) -> R<()> {
    let [label] = args else { return Err("channel <host>".into()) };
    let st = load()?;
    let host = st.host(label);
    let mut d = device(&st).await?;
    let w = watch(&d, &st.labels).await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    match open_to(&mut d, Some(&w.link), &host, "admin").await {
        Ok((_l, o)) => match rpc(&o.channel, "GET", "/rh/info", None).await {
            Ok(r) => log!("admin→{label}: GET /rh/info → {}", r.status),
            Err(e) => log!("admin→{label}: GET /rh/info: {e}"),
        },
        Err(e) => log!("admin→{label}: {e}"),
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if std::env::var_os("ROTTEST_STATE").is_none_or(|v| v.is_empty()) {
        eprintln!("rottest is opt-in: set ROTTEST_STATE to its state file (see the docs at the top of examples/rottest.rs)");
        std::process::exit(2);
    }
    let Some((cmd, rest)) = args.split_first() else {
        eprintln!("rottest signup|enroll|handover|policy|status|watch|rotate|asdevice|oldkey|accept|pause|control|hostkeys|put|redispute|tickets|channel|remove|relaylogin …");
        std::process::exit(2);
    };
    let r = match cmd.as_str() {
        "signup" => signup(rest).await,
        "enroll" => enroll(rest).await,
        "handover" => handover(rest).await,
        "policy" => policy(rest).await,
        "status" => status(rest).await,
        "watch" => watch_cmd(rest).await,
        "rotate" => rotate(rest).await,
        "asdevice" => asdevice(rest).await,
        "oldkey" => oldkey(rest).await,
        "accept" => accept(rest).await,
        "pause" => pause(rest).await,
        "control" => control(rest).await,
        "hostkeys" => hostkeys(rest).await,
        "put" => put(rest).await,
        "redispute" => redispute(rest).await,
        "tickets" => tickets_cmd(rest).await,
        "channel" => channel_cmd(rest).await,
        "remove" => remove(rest).await,
        "relaylogin" => relaylogin(rest).await,
        other => Err(format!("unknown command {other}")),
    };
    if let Err(e) = r {
        log!("ERROR {e}");
        std::process::exit(1);
    }
}
