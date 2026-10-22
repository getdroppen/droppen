//! The relay's control link to the Hub: `wss://<hub>/v1/relay` (§2.5, §2.10).
//!
//! The relay signs in with its key (`login`); a relay the Hub does not know
//! registers with its token on the same connection (`register`), the Hub
//! checking that it answers at its origin first. A sign-in the Hub refuses
//! for good — the key is not the pinned one, the relay was retired, there is
//! no token to register with — stops the relay with the reason: it never
//! quietly becomes another relay.
//!
//! Signed in, the relay sends `health` every 20 s and its usage reports
//! ([`crate::meter`]) until each is acknowledged, and takes the Hub's
//! `keys`, `revoke` (scoped to one ticket role when the Hub says so: the
//! relay names `revoke-role/1` in its sign-in), `throttle` and `drain`; on
//! every sign-in the Hub replays
//! the revocations still in force (`revoke` each, then `replayed`) and its
//! whole throttle table (`throttles`). The link reconnects with
//! jittered backoff; while it is down the relay keeps working — tickets
//! verify offline — and established connections outlive their tickets for up
//! to two hours ([`crate::server::Server::in_outage`]).

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use rh_proto::sig::{HubLogin, LoginRole};
use rh_proto::ticket::TicketRole;
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::config::{hex, unhex, Config};
use crate::server::{now_ms, Server};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;
pub type Sink = SplitSink<Ws, Message>;
pub type Source = SplitStream<Ws>;

const STEP: Duration = Duration::from_secs(20);
/// What this relay takes from the Hub beyond the first control link,
/// named in its `login` and `register` (`features`): `revoke-role/1`, a
/// `revoke` scoped to one ticket role (`role: "device" | "agent"`). A Hub
/// sends a scoped revocation only to a relay that names it.
pub const FEATURE_REVOKE_ROLE: &str = "revoke-role/1";
const BACKOFF_MIN: Duration = Duration::from_millis(500);
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Why the link ended.
#[derive(Debug)]
pub enum Ended {
    /// Try again (the network, a Hub restart).
    Retry(String),
    /// Stop the relay: the Hub refused it for good, or it was retired.
    Fatal(String),
}

/// Keep the control link up until a fatal refusal (the reason) or `stop`.
pub async fn run(server: Arc<Server>) -> String {
    let mut backoff = BACKOFF_MIN;
    loop {
        let started = Instant::now();
        let ended = tokio::select! {
            e = session(&server) => e,
            () = server.shutdown.cancelled() => return "shutting down".into(),
        };
        server.set_link(None);
        match ended {
            Ended::Fatal(reason) => return reason,
            Ended::Retry(why) => tracing::warn!(%why, "the Hub's control link went down; reconnecting"),
        }
        if started.elapsed() > Duration::from_secs(60) {
            backoff = BACKOFF_MIN;
        }
        let wait = rand::thread_rng().gen_range(Duration::ZERO..=backoff);
        backoff = (backoff * 2).min(server.config.timing.uplink_backoff_max.max(BACKOFF_MIN));
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = server.shutdown.cancelled() => return "shutting down".into(),
        }
    }
}

async fn next_json(source: &mut Source) -> Result<Value, Ended> {
    loop {
        match tokio::time::timeout(STEP, source.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => return serde_json::from_str(crate::traffic::got(t.as_str())).map_err(|e| Ended::Retry(e.to_string())),
            Ok(Some(Ok(Message::Close(frame)))) => {
                return Err(Ended::Retry(format!(
                    "closed: {}",
                    frame.map(|f| f.reason.to_string()).unwrap_or_default()
                )))
            }
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(e))) => return Err(Ended::Retry(e.to_string())),
            Ok(None) => return Err(Ended::Retry("closed".into())),
            Err(_) => return Err(Ended::Retry("the Hub did not answer in time".into())),
        }
    }
}

async fn send(sink: &mut Sink, value: &Value) -> Result<(), Ended> {
    sink.send(Message::Text(crate::traffic::sent(value.to_string()).into()))
        .await
        .map_err(|e| Ended::Retry(e.to_string()))
}

/// The signature of a relay sign-in: `HubLogin {role: Relay, account: "",
/// id, hub, nonce, time}` under `key`.
pub fn login_sig(key: &ed25519_dalek::SigningKey, id: &str, hub: &str, nonce: [u8; 32], time_ms: u64) -> String {
    let sig = HubLogin {
        role: LoginRole::Relay,
        account: String::new(),
        id: id.into(),
        hub: hub.into(),
        nonce,
        time_ms,
    }
    .sign(key)
    .expect("a relay login encodes");
    hex(&sig)
}

/// The TLS provider for the link to the Hub (ring). Idempotent.
pub fn tls_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Dial the Hub and read its challenge: the socket and the nonce.
pub async fn dial(config: &Config) -> Result<(Sink, Source, [u8; 32]), Ended> {
    tls_provider();
    let hub = config.hub.as_deref().ok_or_else(|| Ended::Fatal("no DROPPEN_RELAY_HUB".into()))?;
    let url = format!("{hub}/v1/relay");
    let (socket, _) = tokio::time::timeout(STEP, tokio_tungstenite::connect_async(url.as_str()))
        .await
        .map_err(|_| Ended::Retry(format!("{url}: timed out")))?
        .map_err(|e| Ended::Retry(format!("{url}: {e}")))?;
    let (sink, mut source) = socket.split();
    let challenge = next_json(&mut source).await?;
    if challenge["t"] != "challenge" {
        return Err(Ended::Retry(format!("no challenge from the Hub: {}", challenge["t"])));
    }
    if challenge["hub"].as_str() != Some(hub) {
        return Err(Ended::Fatal(format!(
            "DROPPEN_RELAY_HUB is {hub}, but the Hub says its origin is {}: set DROPPEN_RELAY_HUB to that",
            challenge["hub"]
        )));
    }
    let nonce = challenge["nonce"]
        .as_str()
        .and_then(unhex)
        .and_then(|n| <[u8; 32]>::try_from(n).ok())
        .ok_or_else(|| Ended::Retry("a challenge without a nonce".into()))?;
    Ok((sink, source, nonce))
}

/// Sign in (registering first when the Hub does not know this relay and a
/// token is configured): the Hub's ticket keys.
pub async fn sign_in(config: &Config, sink: &mut Sink, source: &mut Source, nonce: [u8; 32]) -> Result<Vec<(u8, [u8; 32])>, Ended> {
    let hub = config.hub.clone().unwrap_or_default();
    let time_ms = now_ms();
    send(
        sink,
        &json!({
            "t": "login", "id": config.id, "time_ms": time_ms, "sig": login_sig(&config.key, &config.id, &hub, nonce, time_ms), "version": VERSION,
            "features": [FEATURE_REVOKE_ROLE],
        }),
    )
    .await?;
    let mut answer = next_json(source).await?;
    if answer["t"] == "error" && answer["code"] == "unknown_relay" {
        let Some(token) = &config.registration_token else {
            return Err(Ended::Fatal(format!(
                "the Hub does not know relay {} ({}): ask the Hub's operator for a registration token for relay {} and set DROPPEN_RELAY_REGISTRATION_TOKEN to it",
                config.id, answer["message"], config.id
            )));
        };
        let time_ms = now_ms();
        tracing::info!(id = %config.id, origin = %config.origin, "registering with the Hub");
        send(
            sink,
            &json!({
                "t": "register", "token": token, "sign_key": hex(&config.key.verifying_key().to_bytes()), "origin": config.origin,
                "version": VERSION, "time_ms": time_ms, "sig": login_sig(&config.key, &config.id, &hub, nonce, time_ms),
                "features": [FEATURE_REVOKE_ROLE],
            }),
        )
        .await?;
        answer = next_json(source).await?;
    }
    match answer["t"].as_str() {
        Some("welcome" | "registered") => {
            if answer["t"] == "registered" {
                tracing::info!(id = %config.id, "registered: the Hub pinned this relay's key");
            }
            Ok(keys_of(&answer["ticket_keys"]))
        }
        // The Hub could not reach this relay at its origin: often a deploy
        // whose new instance the domain does not route to yet. Try again
        // (the token is not spent by a failed proof).
        Some("error") if answer["code"] == "origin_proof" => Err(Ended::Retry(format!(
            "the Hub could not check this relay's origin yet: {}",
            answer["message"].as_str().unwrap_or_default()
        ))),
        Some("error") => Err(Ended::Fatal(format!(
            "the Hub refused relay {}: {} ({})",
            config.id,
            answer["message"].as_str().unwrap_or_default(),
            answer["code"].as_str().unwrap_or_default()
        ))),
        _ => Err(Ended::Retry(format!("an unexpected answer to the sign-in: {}", answer["t"]))),
    }
}

fn keys_of(v: &Value) -> Vec<(u8, [u8; 32])> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|k| {
            let kid = u8::try_from(k["kid"].as_u64()?).ok()?;
            let key = <[u8; 32]>::try_from(unhex(k["key"].as_str()?)?).ok()?;
            Some((kid, key))
        })
        .collect()
}

async fn session(server: &Arc<Server>) -> Ended {
    let (mut sink, mut source, nonce) = match dial(&server.config).await {
        Ok(d) => d,
        Err(e) => return e,
    };
    let keys = match sign_in(&server.config, &mut sink, &mut source, nonce).await {
        Ok(k) => k,
        Err(e) => return e,
    };
    server.set_keys(keys);
    tracing::info!(id = %server.config.id, hub = ?server.config.hub, "signed in on the Hub");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    server.set_link(Some(tx));
    let timing = server.config.timing;
    let mut health = tokio::time::interval(timing.health_every);
    let mut usage = tokio::time::interval(timing.usage_every);
    let mut last_total = server.meter.total();
    let mut last_at = Instant::now();
    loop {
        tokio::select! {
            m = source.next() => {
                let text = match m {
                    Some(Ok(Message::Text(t))) => t,
                    Some(Ok(Message::Close(frame))) => return Ended::Retry(format!("closed: {}", frame.map(|f| f.reason.to_string()).unwrap_or_default())),
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => return Ended::Retry(e.to_string()),
                    None => return Ended::Retry("closed".into()),
                };
                let Ok(v) = serde_json::from_str::<Value>(crate::traffic::got(text.as_str())) else { continue };
                if let Some(end) = on_message(server, &v) {
                    return end;
                }
            }
            Some(text) = rx.recv() => {
                if sink.send(Message::Text(crate::traffic::sent(text).into())).await.is_err() {
                    return Ended::Retry("the link closed".into());
                }
            }
            _ = health.tick() => {
                let (conns, channels) = server.counts();
                let total = server.meter.total();
                let secs = last_at.elapsed().as_secs_f64().max(0.001);
                let rate = ((total - last_total) as f64 / secs) as u64;
                last_total = total;
                last_at = Instant::now();
                let message = json!({
                    "t": "health", "conns": conns, "channels": channels, "bytes_in_per_s": rate, "bytes_out_per_s": rate,
                    "load": server.load(), "version": VERSION,
                });
                if send(&mut sink, &message).await.is_err() {
                    return Ended::Retry("the link closed".into());
                }
            }
            _ = usage.tick() => {
                for report in server.meter.pending() {
                    if sink.send(Message::Text(crate::traffic::sent(serde_json::to_string(&report).unwrap_or_default()).into())).await.is_err() {
                        return Ended::Retry("the link closed".into());
                    }
                }
            }
            () = server.shutdown.cancelled() => return Ended::Fatal("shutting down".into()),
        }
    }
}

/// One message from the Hub; `Some` ends the link.
fn on_message(server: &Server, v: &Value) -> Option<Ended> {
    match v["t"].as_str() {
        Some("keys") => server.set_keys(keys_of(&v["ticket_keys"])),
        Some("revoke") => {
            if let Some(account) = v["account"].as_str() {
                let at_ms = v["at_ms"].as_u64().unwrap_or_else(now_ms);
                // Unscoped (no `role`, or null): every ticket of the id, as
                // before `revoke-role/1`. A role this relay does not know has
                // no tickets here: nothing to revoke.
                match v["role"].as_str() {
                    None => server.revoke(account, v["id"].as_str(), at_ms),
                    Some("device") => server.revoke_role(account, v["id"].as_str(), Some(TicketRole::Device), at_ms),
                    Some("agent") => server.revoke_role(account, v["id"].as_str(), Some(TicketRole::Agent), at_ms),
                    Some(role) => tracing::warn!(%account, role, "a revocation for a ticket role this relay does not know: ignored"),
                }
            }
        }
        Some("throttle") => {
            if let Some(account) = v["account"].as_str() {
                server.throttle(account, v["bytes_per_sec"].as_u64());
            }
        }
        Some("throttles") => {
            let table: std::collections::HashMap<String, u64> = v["accounts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| Some((a["account"].as_str()?.to_string(), a["bytes_per_sec"].as_u64()?)))
                .collect();
            server.throttle_snapshot(&table);
        }
        Some("replayed") => server.mark_synced(),
        Some("drain") => {
            tracing::info!("the Hub asked this relay to drain");
            server.goaway(1_000, "drain");
        }
        Some("usage_ok") => {
            if let (Some(boot), Some(seq)) = (v["boot"].as_str(), v["seq"].as_u64()) {
                server.meter.ack(boot, seq);
            }
        }
        Some("error") => {
            let code = v["code"].as_str().unwrap_or_default();
            tracing::warn!(code, message = %v["message"], "the Hub reported an error");
            if code == "retired" {
                return Some(Ended::Fatal("the Hub retired this relay".into()));
            }
        }
        _ => {}
    }
    None
}

/// `droppen-relay deregister`: sign in and retire this relay at the Hub.
pub async fn deregister(config: &Config, reason: &str) -> Result<(), String> {
    let (mut sink, mut source, nonce) = dial(config).await.map_err(describe)?;
    sign_in(config, &mut sink, &mut source, nonce).await.map_err(describe)?;
    send(&mut sink, &json!({"t": "deregister", "reason": reason})).await.map_err(describe)?;
    loop {
        let v = next_json(&mut source).await.map_err(describe)?;
        if v["t"] == "deregistered" {
            return Ok(());
        }
    }
}

/// `droppen-relay rotate-key`: sign in under the current key and have the
/// Hub pin `new` instead (both keys sign the challenge).
pub async fn rotate(config: &Config, new: &ed25519_dalek::SigningKey) -> Result<(), String> {
    let (mut sink, mut source, nonce) = dial(config).await.map_err(describe)?;
    sign_in(config, &mut sink, &mut source, nonce).await.map_err(describe)?;
    let time_ms = now_ms();
    let hub = config.hub.clone().unwrap_or_default();
    send(
        &mut sink,
        &json!({"t": "rotate", "new_key": hex(&new.verifying_key().to_bytes()), "time_ms": time_ms, "sig_new": login_sig(new, &config.id, &hub, nonce, time_ms)}),
    )
    .await
    .map_err(describe)?;
    loop {
        let v = next_json(&mut source).await.map_err(describe)?;
        match v["t"].as_str() {
            Some("rotated") => return Ok(()),
            Some("error") => return Err(format!("the Hub refused: {}", v["message"])),
            _ => {}
        }
    }
}

fn describe(e: Ended) -> String {
    match e {
        Ended::Retry(s) | Ended::Fatal(s) => s,
    }
}
