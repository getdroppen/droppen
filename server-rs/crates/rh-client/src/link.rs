//! A signed-in connection to the hub's `/v1/client` (or a relay's
//! `/v1/connect`, which speaks the same frames): control messages, and
//! frames routed by `ch` (`docs/hub-api/websocket.md`).
//!
//! A stream subscription's own controls (`stream_synced`, `stream_refused`,
//! each with its `ch`) are routed to the subscription, as a
//! [`Frame::Unknown`] of type [`CONTROL_FRAME`]; every other control message
//! is in [`Link::next_control`]. The link pings every 20 s.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ed25519_dalek::SigningKey;
use futures_util::{SinkExt, StreamExt};
use rh_proto::chain::Attestation;
use rh_proto::frame::{Frame, CLIENT_PATH};
use rh_proto::sig::{HubLogin, LoginRole};
use rh_proto::wire::client::DeviceToHub;
use rh_proto::wire::connect::PeerToRelay;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::{err, hex, key32, now_ms, tap, HubUrl, Result};

/// A relay's device socket.
pub const CONNECT_PATH: &str = "/v1/connect";

/// The frame type a [`Link`] delivers a stream subscription's text controls
/// (`stream_synced`, `stream_refused`) as, on the subscription's own route.
pub const CONTROL_FRAME: u8 = 0xfe;

/// How long the far end has to say anything during sign-in.
const STEP: Duration = Duration::from_secs(10);
/// How long opening the socket (TCP, TLS, the upgrade) may take.
const DIAL: Duration = Duration::from_secs(20);

/// Open the socket at `url`, within [`DIAL`].
async fn dial(url: &str) -> Result<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>> {
    match tokio::time::timeout(DIAL, tokio_tungstenite::connect_async(url)).await {
        Ok(Ok((ws, _))) => Ok(ws),
        Ok(Err(e)) => Err(format!("connect: {e}")),
        Err(_) => Err(format!("connect: no answer within {DIAL:?}")),
    }
}

/// The next JSON control message on `stream` (`socket`: its path, for the
/// tap), waiting at most 10 s.
pub async fn next_json<S>(stream: &mut S, socket: &str) -> Result<Value>
where
    S: futures_util::Stream<Item = std::result::Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        match tokio::time::timeout(STEP, stream.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => return serde_json::from_str(tap::got(socket, t.as_str())).map_err(err("json")),
            Ok(Some(Ok(Message::Close(c)))) => return Err(format!("closed: {c:?}")),
            Ok(Some(Ok(_))) => continue,
            Ok(Some(Err(e))) => return Err(e.to_string()),
            Ok(None) => return Err("closed".into()),
            Err(_) => return Err("timed out waiting for the hub".into()),
        }
    }
}

/// Why a sign-in on `/v1/client` did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectError {
    /// The hub answered the login with `error {code, …}`: `denied` (renew
    /// the device session), `removed`, `host_devices_off`, …
    Refused { code: String, answer: String },
    /// No answer, or not one that says (the network, a close, a timeout).
    Failed(String),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::Refused { answer, .. } => write!(f, "sign-in refused: {answer}"),
            ConnectError::Failed(why) => f.write_str(why),
        }
    }
}

/// Sign `device` of `account` in on the hub's `/v1/client` with its key and
/// a device session.
pub async fn connect(hub: &HubUrl, account: &str, device: &str, sign: &SigningKey, session: &str) -> Result<Link> {
    connect_device(hub, account, device, sign, session).await.map_err(|e| e.to_string())
}

/// [`connect`], telling a refusal apart.
pub async fn connect_device(hub: &HubUrl, account: &str, device: &str, sign: &SigningKey, session: &str) -> std::result::Result<Link, ConnectError> {
    let failed = ConnectError::Failed;
    let url = format!("{}{CLIENT_PATH}", hub.ws);
    let ws = dial(&url).await.map_err(failed)?;
    let (mut sink, mut stream) = ws.split();
    let challenge = next_json(&mut stream, CLIENT_PATH).await.map_err(failed)?;
    let nonce: [u8; 32] = key32(challenge["nonce"].as_str().ok_or_else(|| failed("no challenge".into()))?).map_err(failed)?;
    let time_ms = now_ms();
    let sig = HubLogin {
        role: LoginRole::Device,
        account: account.to_string(),
        id: device.to_string(),
        hub: hub.origin.clone(),
        nonce,
        time_ms,
    }
    .sign(sign)
    .map_err(|e| failed(format!("sign-in: {e}")))?;
    let login = DeviceToHub::Login {
        role: "device".into(),
        account: account.into(),
        id: device.into(),
        time_ms,
        sig: hex(&sig),
        session: session.into(),
    };
    let login = serde_json::to_string(&login).map_err(|e| failed(format!("sign-in: {e}")))?;
    sink.send(Message::Text(tap::sent(CLIENT_PATH, login).into()))
        .await
        .map_err(|e| failed(format!("send: {e}")))?;
    let welcome = next_json(&mut stream, CLIENT_PATH).await.map_err(failed)?;
    if welcome["t"] != "welcome" {
        return Err(match welcome["code"].as_str() {
            Some(code) if welcome["t"] == "error" => ConnectError::Refused {
                code: code.to_string(),
                answer: welcome.to_string(),
            },
            _ => failed(format!("sign-in refused: {welcome}")),
        });
    }
    Ok(Link::spawn(sink, stream, CLIENT_PATH))
}

/// Sign `device` of `account` in on a relay (`origin`, e.g.
/// `wss://sin-1.relay.example`) with a ticket from the hub
/// (`POST /v1/relays/tickets`): a link channels open on like the hub's. The
/// relay's `welcome` and `presence` come first.
pub async fn connect_relay(origin: &str, account: &str, device: &str, sign: &SigningKey, ticket: &str) -> Result<Link> {
    let url = format!("{origin}{CONNECT_PATH}");
    let ws = dial(&url).await?;
    let (mut sink, mut stream) = ws.split();
    let challenge = next_json(&mut stream, CONNECT_PATH).await?;
    let nonce: [u8; 32] = key32(challenge["nonce"].as_str().ok_or("no challenge")?)?;
    let time_ms = now_ms();
    let sig = HubLogin {
        role: LoginRole::RelayDevice,
        account: account.to_string(),
        id: device.to_string(),
        hub: origin.to_string(),
        nonce,
        time_ms,
    }
    .sign(sign)
    .map_err(err("sign-in"))?;
    let login = PeerToRelay::Login {
        ticket: ticket.into(),
        time_ms,
        sig: hex(&sig),
    };
    let login = serde_json::to_string(&login).map_err(err("sign-in"))?;
    sink.send(Message::Text(tap::sent(CONNECT_PATH, login).into()))
        .await
        .map_err(err("send"))?;
    let welcome = next_json(&mut stream, CONNECT_PATH).await?;
    if welcome["t"] != "welcome" {
        return Err(format!("the relay refused the sign-in: {welcome}"));
    }
    Ok(Link::spawn(sink, stream, CONNECT_PATH))
}

type Routes = Arc<Mutex<HashMap<u32, mpsc::UnboundedSender<Frame>>>>;

/// A signed-in `/v1/client` connection: control messages, and frames routed
/// to channels by `ch`.
pub struct Link {
    out: mpsc::UnboundedSender<Frame>,
    text: mpsc::UnboundedSender<String>,
    routes: Routes,
    next_ch: AtomicU32,
    control: tokio::sync::Mutex<mpsc::UnboundedReceiver<Value>>,
    closed: tokio::sync::watch::Receiver<bool>,
    /// When (ms) the far end last sent anything (a `PONG` included).
    heard: Arc<AtomicU64>,
    /// Dropped with the link: its reader stops, and the socket goes, even if
    /// the far end never answers the close.
    _reader_stop: tokio::sync::watch::Sender<()>,
}

impl Link {
    /// Drive a signed-in socket (`socket`: its path, for the tap): one task
    /// writes (pinging every 20 s), one reads and routes.
    pub fn spawn<Si, St>(mut sink: Si, mut stream: St, socket: &'static str) -> Link
    where
        Si: futures_util::Sink<Message> + Unpin + Send + 'static,
        St: futures_util::Stream<Item = std::result::Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin + Send + 'static,
    {
        let (out, mut out_rx) = mpsc::unbounded_channel::<Frame>();
        let (text, mut text_rx) = mpsc::unbounded_channel::<String>();
        let (control_tx, control_rx) = mpsc::unbounded_channel();
        let routes: Routes = Arc::default();
        let (closed_tx, closed) = tokio::sync::watch::channel(false);
        let heard = Arc::new(AtomicU64::new(now_ms()));
        tokio::spawn(async move {
            let mut ping = tokio::time::interval(Duration::from_secs(20));
            let mut nonce = 0u64;
            loop {
                let message = tokio::select! {
                    f = out_rx.recv() => match f {
                        Some(f) => match f.encode() { Ok(b) => Message::Binary(b.into()), Err(_) => continue },
                        None => break,
                    },
                    t = text_rx.recv() => match t { Some(t) => Message::Text(tap::sent(socket, t).into()), None => break },
                    _ = ping.tick() => {
                        nonce += 1;
                        Message::Binary(Frame::Ping { nonce }.encode().expect("ping").into())
                    }
                };
                if sink.send(message).await.is_err() {
                    break;
                }
            }
            let _ = sink.close().await;
        });
        let (reader_stop, mut stop) = tokio::sync::watch::channel(());
        {
            let routes = routes.clone();
            let heard = heard.clone();
            tokio::spawn(async move {
                loop {
                    let message = tokio::select! {
                        m = stream.next() => match m {
                            Some(Ok(m)) => m,
                            _ => break,
                        },
                        // The link was dropped.
                        _ = stop.changed() => break,
                    };
                    heard.store(now_ms(), Ordering::Relaxed);
                    match message {
                        Message::Text(t) => {
                            if let Ok(v) = serde_json::from_str::<Value>(tap::got(socket, t.as_str())) {
                                // A stream subscription's own controls go to its route.
                                let for_ch = v["t"]
                                    .as_str()
                                    .is_some_and(|t| t.starts_with("stream_"))
                                    .then(|| v["ch"].as_u64())
                                    .flatten();
                                if let Some(ch) = for_ch.and_then(|c| u32::try_from(c).ok()) {
                                    let route = routes.lock().expect("routes").get(&ch).cloned();
                                    if let Some(route) = route {
                                        let _ = route.send(Frame::Unknown {
                                            ty: CONTROL_FRAME,
                                            ch,
                                            payload: v.to_string().into_bytes(),
                                        });
                                        continue;
                                    }
                                }
                                let _ = control_tx.send(v);
                            }
                        }
                        Message::Binary(b) => {
                            let Ok(frame) = Frame::decode(&b) else { continue };
                            let ch = match &frame {
                                Frame::Data { ch, .. } | Frame::Close { ch, .. } | Frame::Credit { ch, .. } => *ch,
                                Frame::Recs { ch, .. }
                                | Frame::Head { ch, .. }
                                | Frame::Gone { ch, .. }
                                | Frame::Checkpoint { ch, .. }
                                | Frame::Ephemeral { ch, .. } => *ch,
                                _ => continue,
                            };
                            let route = routes.lock().expect("routes").get(&ch).cloned();
                            if let Some(route) = route {
                                let close = matches!(frame, Frame::Close { .. });
                                let _ = route.send(frame);
                                if close {
                                    routes.lock().expect("routes").remove(&ch);
                                }
                            }
                        }
                        Message::Close(_) => break,
                        _ => {}
                    }
                }
                routes.lock().expect("routes").clear();
                let _ = closed_tx.send(true);
            });
        }
        Link {
            out,
            text,
            routes,
            next_ch: AtomicU32::new(1),
            control: tokio::sync::Mutex::new(control_rx),
            closed,
            heard,
            _reader_stop: reader_stop,
        }
    }

    /// How long (ms) since the far end last sent anything. The link pings
    /// every 20 s and the hub answers each: much longer than that is a dead
    /// connection the socket has not noticed yet.
    pub fn silent_ms(&self) -> u64 {
        now_ms().saturating_sub(self.heard.load(Ordering::Relaxed))
    }

    /// A fresh channel number on this connection, and the frames that arrive on it.
    pub fn reserve(&self) -> (u32, mpsc::UnboundedReceiver<Frame>) {
        let ch = self.next_ch.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::unbounded_channel();
        self.routes.lock().expect("routes").insert(ch, tx);
        (ch, rx)
    }

    /// Send a frame.
    pub fn send(&self, frame: Frame) -> Result<()> {
        self.out.send(frame).map_err(|_| "the link is closed".to_string())
    }

    /// Stop routing `ch` (a stream subscription that ended).
    pub fn release(&self, ch: u32) {
        self.routes.lock().expect("routes").remove(&ch);
    }

    /// Where frames to send go (a channel driver's outbound half).
    pub fn outbound(&self) -> mpsc::UnboundedSender<Frame> {
        self.out.clone()
    }

    /// Hand the hub this device's attestation (relayed to the account's agents).
    pub fn attest(&self, attestation: &Attestation) -> Result<()> {
        self.send(Frame::Attest {
            ch: 0,
            attestation: attestation.encode(),
        })
    }

    /// Send a control message.
    pub fn send_control(&self, message: &DeviceToHub) -> Result<()> {
        let text = serde_json::to_string(message).map_err(err("control message"))?;
        self.text.send(text).map_err(|_| "the link is closed".to_string())
    }

    /// The next control message, or `None` after `timeout`.
    pub async fn next_control(&self, timeout: Duration) -> Option<Value> {
        tokio::time::timeout(timeout, self.control.lock().await.recv()).await.ok().flatten()
    }

    /// Wait for a control message matching `pred`, dropping the others.
    pub async fn wait_control(&self, timeout: Duration, pred: impl Fn(&Value) -> bool) -> Option<Value> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            let v = self.next_control(left).await?;
            if pred(&v) {
                return Some(v);
            }
        }
    }

    /// Whether the far end closed this connection.
    pub fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    /// Wait until the hub closed this connection (at once if it already has).
    pub async fn closed(&self) {
        let mut rx = self.closed.clone();
        let _ = rx.wait_for(|closed| *closed).await;
    }
}
