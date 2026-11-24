//! One end-to-end channel from this device to a host, through a relay
//! (`docs/cross-machine-architecture.md` §5 "E2E 通道"): `OPEN` a fresh `ch`
//! to the host, the Noise IK first message whose payload is this device's
//! chain view and never a request ([`handshake::start`]), the agent's reply
//! read and its chain entries merged into a copy of the view
//! ([`handshake::finish`]), and the channel run on its own task
//! ([`droppen_proto::driver::Channel`]).
//!
//! One try is [`attempt`]; the retries around it (sync and try again when the
//! agent asks for entries or is stale, another relay when one does not carry
//! the host) are [`crate::device`]'s. What the reply taught the view is
//! handed back whatever the outcome, for the caller to keep.
//!
//! A [`HostChannel`] remembers why it ended ([`HostChannel::ended`]): the
//! `CLOSE` it got (from the relay or the agent), or the relay link going
//! away. Its streams end with the same code as the channel, which on their
//! own could not be told from the bridge's (both use 4403), so a stream that
//! closes asks the channel first ([`crate::rpc`]).

use std::sync::Arc;
use std::time::Duration;

use droppen_proto::chain::{ChainClock, ChainError, ChainState, Entry};
use droppen_proto::channel::Session;
use droppen_proto::driver::{Channel, DriverError, Stream};
use droppen_proto::frame::Frame;
use droppen_proto::handshake::{self, HandshakeError, ReplyStatus};
use droppen_proto::mux::{Role, StreamClass};
use droppen_proto::noise::RekeyPolicy;
use tokio::sync::{mpsc, watch};

use crate::failure::Failure;
use crate::identity::DeviceIdentity;
use crate::now_ms;
use crate::relays::RelayConn;

/// A channel to a host, open or ended.
pub struct HostChannel {
    pub host: String,
    /// The relay it runs through.
    pub relay: String,
    /// The host's agent and the features its reply listed.
    pub server: serde_json::Value,
    channel: Channel,
    ended: watch::Receiver<Option<Failure>>,
    /// The relay connection, kept up while the channel lives.
    _conn: Arc<RelayConn>,
}

impl Drop for HostChannel {
    fn drop(&mut self) {
        if !self.channel.is_closed() {
            self.channel.close(1000, "done");
        }
    }
}

impl HostChannel {
    /// Still up: new requests may go on it.
    pub fn is_open(&self) -> bool {
        !self.channel.is_closed() && self.ended.borrow().is_none()
    }

    /// Why it ended, once it did.
    pub fn ended(&self) -> Option<Failure> {
        self.ended.borrow().clone()
    }

    /// Wait until it ended; why.
    pub async fn wait_ended(&self) -> Failure {
        let mut rx = self.ended.clone();
        let end = rx.wait_for(|e| e.is_some()).await.ok().and_then(|end| end.clone());
        end.unwrap_or_else(|| Failure::Offline("the channel ended".into()))
    }

    /// A sub-stream of `class` opened with `meta` (a request head).
    pub(crate) async fn open_stream(&self, class: StreamClass, meta: &[u8]) -> Result<Stream, Failure> {
        match self.channel.open(class, meta).await {
            Ok(stream) => Ok(stream),
            Err(DriverError::ChannelClosed) => Err(self.ended().unwrap_or_else(|| Failure::Offline("the channel is closed".into()))),
            Err(e) => Err(Failure::Other(format!("a stream: {e}"))),
        }
    }

    /// End it (its streams with it).
    pub fn close(&self, code: u16, reason: &str) {
        self.channel.close(code, reason);
    }
}

/// How one try went.
pub enum Attempt {
    Open(HostChannel),
    /// The agent is behind this device's view: it asked for entries.
    NeedEntries,
    /// Nobody vouched for the agent's view recently.
    Stale,
    /// Anything else, sorted ([`Failure`]): a relay that does not carry the
    /// host, a refusal, a reply that does not open.
    Failed(Failure),
}

/// What a try needs of the device.
pub struct Hello<'a> {
    pub identity: &'a DeviceIdentity,
    /// The device's verified view, and its clock (a copy: the reply's entries
    /// are merged into it, and handed back for the view to take).
    pub chain: ChainState,
    pub clock: ChainClock,
    /// What the hello says this device speaks.
    pub client: &'a [u8],
    /// How long the agent has to answer.
    pub reply_timeout: Duration,
}

/// One try at a channel to `host` over `conn`: the outcome, and the entries
/// the agent's reply taught the view (whatever the outcome).
pub async fn attempt(conn: Arc<RelayConn>, host: &str, hello: Hello<'_>) -> (Attempt, Vec<Entry>) {
    let Hello {
        identity,
        mut chain,
        mut clock,
        client,
        reply_timeout,
    } = hello;
    let link = conn.link();
    let (initiator, first) = match handshake::start(&chain, identity.dh(), &identity.device, host, now_ms(), client) {
        Ok(started) => started,
        Err(HandshakeError::Unauthorized) => return (Attempt::Failed(Failure::Chain(format!("the chain names no host {host}"))), Vec::new()),
        Err(e) => return (Attempt::Failed(Failure::Other(format!("handshake: {e}"))), Vec::new()),
    };
    let (ch, mut inbound) = link.reserve();
    let sent = link
        .send(Frame::Open { ch, peer: host.into() })
        .and_then(|()| link.send(Frame::Data { ch, payload: first }));
    if sent.is_err() {
        link.release(ch);
        return (Attempt::Failed(Failure::from_link_close(link.close_frame().get())), Vec::new());
    }
    let reply = match tokio::time::timeout(reply_timeout, inbound.recv()).await {
        Ok(Some(Frame::Data { payload, .. })) => payload,
        Ok(Some(Frame::Close { code, reason, .. })) => return (Attempt::Failed(Failure::from_channel_close(code, &reason, false)), Vec::new()),
        Ok(Some(_)) => {
            link.release(ch);
            let _ = link.send(Frame::Close {
                ch,
                code: droppen_proto::bridge::CLOSE_BAD_REQUEST,
                reason: "not a handshake reply".into(),
            });
            return (
                Attempt::Failed(Failure::Other("the answer to the hello was not a reply".into())),
                Vec::new(),
            );
        }
        Ok(None) => return (Attempt::Failed(Failure::from_link_close(link.close_frame().get())), Vec::new()),
        Err(_) => {
            link.release(ch);
            let _ = link.send(Frame::Close {
                ch,
                code: 1000,
                reason: "no answer to the hello".into(),
            });
            return (
                Attempt::Failed(Failure::Offline(format!("the device did not answer within {reply_timeout:?}"))),
                Vec::new(),
            );
        }
    };
    let before = chain.seq();
    let result = handshake::finish(initiator, &mut chain, &mut clock, &reply, RekeyPolicy::default(), now_ms());
    let learnt = chain.entries_after(Some(before)).to_vec();
    let (transport, body) = match result {
        Ok(opened) => opened,
        Err(error) => {
            // A refusal's `CLOSE` follows its reply: nothing more for this `ch`.
            link.release(ch);
            let outcome = match error {
                HandshakeError::Refused {
                    status: ReplyStatus::NeedEntries,
                    ..
                } => Attempt::NeedEntries,
                HandshakeError::Refused {
                    status: ReplyStatus::Stale, ..
                } => Attempt::Stale,
                HandshakeError::Chain(e @ (ChainError::Fork { .. } | ChainError::Rollback { .. })) => {
                    Attempt::Failed(Failure::Chain(format!("the device's view of the chain conflicts with this one's: {e}")))
                }
                other => {
                    let _ = link.send(Frame::Close {
                        ch,
                        code: droppen_proto::bridge::CLOSE_BAD_REQUEST,
                        reason: "handshake failed".into(),
                    });
                    Attempt::Failed(Failure::Other(format!("handshake: {other}")))
                }
            };
            return (outcome, learnt);
        }
    };
    let server: serde_json::Value = serde_json::from_slice(&body.server).unwrap_or(serde_json::Value::Null);
    let session = Session::new(ch, transport, Role::Initiator);
    let (tx, rx) = mpsc::unbounded_channel();
    let (channel, _incoming) = Channel::spawn(session, rx, link.outbound());
    let (end_tx, ended) = watch::channel(None);
    tokio::spawn(forward(conn.clone(), ch, inbound, tx, end_tx));
    let opened = HostChannel {
        host: host.into(),
        relay: conn.relay.clone(),
        server,
        channel,
        ended,
        _conn: conn,
    };
    (Attempt::Open(opened), learnt)
}

/// Carry the relay link's frames for `ch` to the channel, noting why it ends
/// before the channel hears: the `CLOSE` it got, or the link that went away.
async fn forward(
    conn: Arc<RelayConn>,
    ch: u32,
    mut inbound: mpsc::UnboundedReceiver<Frame>,
    tx: mpsc::UnboundedSender<Frame>,
    ended: watch::Sender<Option<Failure>>,
) {
    let note = |failure: Failure| {
        ended.send_if_modified(|end| {
            if end.is_some() {
                return false;
            }
            *end = Some(failure);
            true
        });
    };
    loop {
        tokio::select! {
            frame = inbound.recv() => match frame {
                Some(frame) => {
                    if let Frame::Close { code, reason, .. } = &frame {
                        note(Failure::from_channel_close(*code, reason, true));
                    }
                    if tx.send(frame).is_err() {
                        break;
                    }
                }
                None => {
                    note(Failure::from_link_close(conn.link().close_frame().get()));
                    break;
                }
            },
            () = tx.closed() => break,
        }
    }
    // The channel's task is done (or will be once it reads the end).
    note(Failure::Offline("the channel ended".into()));
    conn.link().release(ch);
}
