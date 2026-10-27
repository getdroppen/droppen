//! The agent's side of the hub's `/v1/agent` socket: dialling out, the
//! challenge, and the signed answers to it (`rh_proto::sig::HubLogin`). The
//! messages are in `docs/hub-api/websocket.md` (`/v1/agent`).

use std::time::Duration;

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use rh_proto::frame::Frame;
use rh_proto::identity::MachineIdentity;
use rh_proto::sig::{HubLogin, LoginRole};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// The agent's socket to its hub.
pub type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
/// Its sending half.
pub type Sink = SplitSink<Socket, Message>;
/// Its receiving half.
pub type Source = SplitStream<Socket>;

/// How long a peer has to say anything during sign-in.
pub(super) const STEP: Duration = Duration::from_secs(15);
/// The heartbeat on the hub's and a relay's connection
/// (`docs/cross-machine-architecture.md` §5).
pub(super) const HEARTBEAT: Duration = Duration::from_secs(20);
/// Silence from the hub or a relay after which the connection is presumed dead.
pub(super) const SILENCE: Duration = Duration::from_secs(60);
/// The first wait before reconnecting.
pub(super) const BACKOFF_MIN: Duration = Duration::from_millis(500);
/// The longest wait between reconnects.
pub(super) const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// A connection that lasted this long resets the backoff.
pub(super) const STABLE: Duration = Duration::from_secs(60);

/// `wss://host[:port]` — what sign-in signatures name — from whatever URL the
/// user typed (`https://`, `wss://…/v1/agent`, …). Plain `ws://`/`http://`
/// only to loopback, for local testing.
pub fn origin(url: &str) -> Result<String, String> {
    let (scheme, rest) = url
        .trim()
        .split_once("://")
        .ok_or_else(|| format!("{url:?}: a hub URL looks like wss://hub.example"))?;
    let secure = match scheme.to_ascii_lowercase().as_str() {
        "wss" | "https" => true,
        "ws" | "http" => false,
        _ => return Err(format!("{url:?}: a hub is reached over wss:// (or https://)")),
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default().to_ascii_lowercase();
    if authority.is_empty() || authority.contains('@') {
        return Err(format!("{url:?}: no host"));
    }
    let host = authority.rsplit_once(':').map_or(authority.as_str(), |(h, _)| h);
    if !secure && !matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
        return Err(format!("{url:?}: plain ws:// is only for a hub on this device; use wss://"));
    }
    let authority = match (secure, authority.rsplit_once(':')) {
        (true, Some((h, "443"))) | (false, Some((h, "80"))) => h.to_string(),
        _ => authority,
    };
    Ok(format!("{}://{authority}", if secure { "wss" } else { "ws" }))
}

/// Milliseconds since the Unix epoch ([`super::now_ms`], one clock for the agent).
pub use super::now_ms;

/// Lower-case hex, as every binary field in the hub's control messages.
pub fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

/// Connect to `origin`'s agent socket (Nagle off: terminals) and read the challenge nonce.
pub async fn dial(origin: &str) -> Result<(Sink, Source, [u8; 32]), String> {
    let url = format!("{origin}{}", rh_proto::frame::AGENT_PATH);
    let (socket, _) = tokio::time::timeout(STEP, tokio_tungstenite::connect_async_with_config(url.as_str(), None, true))
        .await
        .map_err(|_| format!("{url}: timed out"))?
        .map_err(|e| format!("{url}: {e}"))?;
    let (sink, mut source) = socket.split();
    let challenge = next_json(&mut source).await?;
    if challenge["t"] != "challenge" {
        return Err(format!("the hub did not send a challenge: {challenge}"));
    }
    let nonce = challenge["nonce"].as_str().and_then(unhex32).ok_or("the hub's challenge has no nonce")?;
    Ok((sink, source, nonce))
}

/// 32 bytes from 64 hex characters.
pub fn unhex32(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// The signature over the hub's `nonce` as machine `machine` of `account`.
pub fn login_signature(
    identity: &MachineIdentity,
    account: &str,
    machine: &str,
    origin: &str,
    nonce: [u8; 32],
    time_ms: u64,
) -> Result<[u8; 64], String> {
    HubLogin {
        role: LoginRole::Agent,
        account: account.into(),
        id: machine.into(),
        hub: origin.into(),
        nonce,
        time_ms,
    }
    .sign(identity.signing_key())
    .map_err(|e| e.to_string())
}

/// Why a sign-in failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginError {
    /// The hub refused this machine as removed from the account (`error`
    /// `removed`, then close 4403). Only availability rests on believing it:
    /// a hub that lies can refuse the machine anyway.
    Removed(String),
    /// The hub answered the sign-in with any other refusal (`error` with its
    /// `code`: `denied` for a key it does not take, `rotated` for one an
    /// identity renewal retired, …).
    Refused {
        /// The `error`'s code.
        code: String,
        /// The hub's words.
        message: String,
    },
    /// Anything else: the network, a hub that is down or says nothing.
    Other(String),
}

impl std::fmt::Display for LoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoginError::Removed(message) => write!(f, "the hub refused the sign-in: this device was removed ({message})"),
            LoginError::Refused { message, .. } => write!(f, "the hub refused the sign-in: {message:?}"),
            LoginError::Other(message) => f.write_str(message),
        }
    }
}

impl From<String> for LoginError {
    fn from(message: String) -> Self {
        LoginError::Other(message)
    }
}

/// What a refusal of the sign-in says: [`LoginError::Removed`] for the hub's
/// `removed`, [`LoginError::Refused`] with its code for any other `error`,
/// else the hub's words.
pub fn refusal(answer: &Value) -> LoginError {
    let message = answer["message"].as_str().unwrap_or_default().to_string();
    if answer["t"] == "error" && answer["code"] == "removed" {
        LoginError::Removed(message)
    } else if answer["t"] == "error" {
        LoginError::Refused {
            code: answer["code"].as_str().unwrap_or_default().to_string(),
            message,
        }
    } else {
        LoginError::Other(format!("the hub refused the sign-in: {}", answer["message"]))
    }
}

/// Whether the hub's `welcome` offers `feature` (a hub from before features
/// offers none).
pub fn offers(welcome: &Value, feature: &str) -> bool {
    welcome["features"].as_array().is_some_and(|f| f.iter().any(|v| v == feature))
}

/// Sign in as an enrolled machine; `Ok` once the hub said `welcome`.
pub async fn login(
    sink: &mut Sink,
    source: &mut Source,
    identity: &MachineIdentity,
    account: &str,
    machine: &str,
    origin: &str,
    nonce: [u8; 32],
) -> Result<Value, LoginError> {
    let time_ms = now_ms();
    let sig = login_signature(identity, account, machine, origin, nonce, time_ms)?;
    send_json(
        sink,
        &json!({"t": "login", "role": "agent", "account": account, "id": machine, "time_ms": time_ms, "sig": hex(&sig)}),
    )
    .await?;
    let welcome = next_json(source).await?;
    if welcome["t"] != "welcome" {
        return Err(refusal(&welcome));
    }
    Ok(welcome)
}

/// Send one control message.
pub async fn send_json(sink: &mut Sink, value: &Value) -> Result<(), String> {
    super::traffic::agent_value(value);
    sink.send(Message::Text(value.to_string().into())).await.map_err(|e| e.to_string())
}

/// Whose socket a JSON control message is read from: the hub's `/v1/agent`
/// and a relay's `/v1/connect` differ in what may arrive before sign-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Peer {
    /// Nothing binary comes before sign-in; a close names its reason.
    Hub,
    /// Messages that are not text are skipped; a close names its code, which
    /// [`super::relays`] reads a revocation from.
    Relay,
}

impl Peer {
    /// How a peer is named in errors.
    fn name(self) -> &'static str {
        match self {
            Peer::Hub => "the hub",
            Peer::Relay => "the relay",
        }
    }

    /// The traffic capture's transform for this peer's control text.
    fn transform(self, text: &str) -> &str {
        match self {
            Peer::Hub => super::traffic::agent_in(text),
            Peer::Relay => super::traffic::relay_in(text),
        }
    }
}

/// The next text message, as JSON (binary messages before it are an error:
/// nothing binary comes before sign-in).
pub async fn next_json(source: &mut Source) -> Result<Value, String> {
    read_json(source, Peer::Hub).await
}

/// The next text control message, as JSON, from `peer`'s socket (up to
/// [`STEP`]). The hub's rule — nothing binary before sign-in — and a relay's
/// leniency are [`Peer`]'s; so are each one's close and silence errors.
pub(super) async fn read_json<S>(source: &mut S, peer: Peer) -> Result<Value, String>
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match tokio::time::timeout(STEP, source.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => {
                return serde_json::from_str(peer.transform(t.as_str())).map_err(|e| e.to_string())
            }
            Ok(Some(Ok(Message::Close(frame)))) => {
                return Err(match peer {
                    Peer::Hub => format!(
                        "the hub closed the connection: {}",
                        frame.map(|f| f.reason.to_string()).unwrap_or_default()
                    ),
                    Peer::Relay => format!(
                        "the relay closed the connection: {}",
                        frame.map(|f| format!("{} {}", u16::from(f.code), f.reason)).unwrap_or_default()
                    ),
                })
            }
            // The hub: nothing binary (or unknown) comes before sign-in.
            Ok(Some(Ok(Message::Ping(_) | Message::Pong(_)))) if peer == Peer::Hub => continue,
            Ok(Some(Ok(_))) if peer == Peer::Hub => return Err("unexpected binary message before sign-in".into()),
            // A relay: anything that is not text is skipped.
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(e))) => return Err(e.to_string()),
            Ok(None) => return Err(format!("{} closed the connection", peer.name())),
            Err(_) => return Err(format!("{} did not answer in time", peer.name())),
        }
    }
}

/// Exponential backoff with full jitter: each wait is drawn from
/// `0..=current` and the ceiling doubles, up to `max`. A connection that
/// lasted at least `stable` starts over.
pub(super) struct Backoff {
    min: Duration,
    max: Duration,
    stable: Duration,
    current: Duration,
}

impl Backoff {
    pub(super) fn new(min: Duration, max: Duration, stable: Duration) -> Self {
        Self {
            min,
            max,
            stable,
            current: min,
        }
    }

    /// The wait before the next try, after one that lasted `lasted`.
    pub(super) fn after(&mut self, lasted: Duration) -> Duration {
        if lasted > self.stable {
            self.current = self.min;
        }
        let wait = rand::thread_rng().gen_range(Duration::ZERO..=self.current);
        self.current = (self.current * 2).min(self.max);
        wait
    }
}

/// The one writer a socket has: frames from every channel, control text, and
/// a heartbeat ping — the hub's connection and a relay's alike, the text
/// transform being the only difference.
pub(super) fn spawn_writer(
    mut sink: Sink,
    mut frames: mpsc::UnboundedReceiver<Frame>,
    mut text: mpsc::UnboundedReceiver<String>,
    heartbeat: Duration,
    wrap: fn(String) -> String,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut beat = tokio::time::interval(heartbeat);
        let mut nonce = 0u64;
        loop {
            let message = tokio::select! {
                f = frames.recv() => match f {
                    Some(f) => match f.encode() { Ok(b) => Message::Binary(b.into()), Err(_) => continue },
                    None => break,
                },
                t = text.recv() => match t { Some(t) => Message::Text(wrap(t).into()), None => break },
                _ = beat.tick() => {
                    nonce += 1;
                    Message::Binary(Frame::Ping { nonce }.encode().expect("a ping encodes").into())
                }
            };
            if sink.send(message).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_as_removed_is_told_apart() {
        let removed = json!({"t": "error", "code": "removed", "message": "this device was removed from the account"});
        assert!(matches!(refusal(&removed), LoginError::Removed(_)));
        let denied = json!({"t": "error", "code": "denied", "message": "sign-in refused"});
        assert!(matches!(refusal(&denied), LoginError::Refused { code, message } if code == "denied" && message == "sign-in refused"));
        assert!(refusal(&denied).to_string().contains("sign-in refused"));
        let rotated = json!({"t": "error", "code": "rotated", "message": "this device's identity key was renewed"});
        assert!(matches!(refusal(&rotated), LoginError::Refused { code, .. } if code == "rotated"));
        // Any other answer than an `error` (a hub message that is no refusal at all).
        assert!(matches!(refusal(&json!({"t": "welcome"})), LoginError::Other(_)));
    }

    #[test]
    fn a_welcome_offers_what_its_features_name() {
        assert!(offers(&json!({"t": "welcome", "features": ["rotate/1"]}), "rotate/1"));
        assert!(!offers(&json!({"t": "welcome", "features": []}), "rotate/1"));
        assert!(!offers(&json!({"t": "welcome"}), "rotate/1"), "a hub from before features");
        assert!(!offers(&json!({"t": "welcome", "features": "rotate/1"}), "rotate/1"));
    }

    #[test]
    fn hub_urls_become_origins_and_plain_ws_is_loopback_only() {
        assert_eq!(origin("wss://Hub.Example/v1/agent").unwrap(), "wss://hub.example");
        assert_eq!(origin("https://hub.example:443").unwrap(), "wss://hub.example");
        assert_eq!(origin("ws://127.0.0.1:9000").unwrap(), "ws://127.0.0.1:9000");
        assert_eq!(origin("http://localhost:9000/").unwrap(), "ws://localhost:9000");
        assert!(origin("ws://hub.example").is_err());
        assert!(origin("ftp://x").is_err());
        assert!(origin("wss://").is_err());
    }
}
