//! Running an established channel on tokio (feature `runtime`): the one piece
//! of async glue the agent and a headless device share.
//!
//! A [`Channel`] owns a [`Session`] in a task of its own. The connection that
//! carries the channel feeds it the `DATA`/`CREDIT`/`CLOSE` frames for its `ch`
//! and takes the frames it produces; the application opens and accepts
//! [`Stream`]s on it. Three kinds of flow control meet here, each bounding a
//! different queue:
//!
//! - **hub credit** ([`crate::flow`]): the task stops polling the multiplexer
//!   while the hub window could not take a whole frame, and hands credit back
//!   as soon as it has read what arrived;
//! - **stream windows** ([`crate::mux`]): a stream's received data is
//!   acknowledged only when the application takes it from [`Stream::recv`], so
//!   a slow consumer slows its sender and nobody else;
//! - **send budgets**: [`Stream::send`] waits while too much of what it queued
//!   has not left yet, so a producer cannot buffer without bound behind a peer
//!   that grants no credit.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot, watch, Semaphore};

use crate::channel::{PathEvent, PathStats, Room, Session, SessionError, Wire};
use crate::flow::{RecvWindow, SendWindow};
use crate::frame::{Frame, MAX_PAYLOAD_LEN};
use crate::mux::{MuxError, MuxEvent, StreamClass, MAX_CHUNK};
use crate::noise::Transport;
use crate::path::DownReason;

/// What one stream may have queued and not yet sent.
const SEND_BUDGET: usize = 512 * 1024;
/// What may be handed to a direct path's owner and not yet be on the wire.
pub const DIRECT_BUDGET: usize = 1024 * 1024;
/// How often a driver whose direct budget is spent looks for it to drain.
const DRAIN_POLL_US: u64 = 5_000;

fn now_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

/// A direct path's messages as the driver sees them, once the handshake on
/// it is done ([`crate::path`]): the owner (the WebRTC peer) writes what
/// arrives on `tx` to the DataChannel, subtracting each message's length from
/// `queued` once it is on the wire, and feeds `rx` with what the DataChannel
/// brings. The owner closing `rx`'s sender says the DataChannel is gone; the
/// driver dropping `tx` says it stopped using the path (it failed, timed out,
/// the peer gave it up, or the channel ended) and the peer connection should
/// be torn down.
pub struct DirectPipe {
    pub tx: mpsc::UnboundedSender<Vec<u8>>,
    pub rx: mpsc::UnboundedReceiver<Vec<u8>>,
    pub queued: Arc<AtomicUsize>,
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum DriverError {
    #[error("the channel is closed")]
    ChannelClosed,
    #[error("the stream is closed")]
    StreamClosed,
    #[error(transparent)]
    Mux(#[from] MuxError),
}

/// What a stream's reader sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    Data(Vec<u8>),
    /// The stream ended with this code (the peer's, or the channel's end).
    Closed(u16),
}

enum Cmd {
    Open {
        class: StreamClass,
        meta: Vec<u8>,
        reply: oneshot::Sender<Result<Stream, DriverError>>,
    },
    Send {
        sid: u32,
        data: Vec<u8>,
    },
    Close {
        sid: u32,
        code: u16,
    },
    Consumed {
        sid: u32,
        n: usize,
    },
    End {
        code: u16,
        reason: String,
    },
    AttachDirect {
        transport: Box<Transport>,
        pipe: DirectPipe,
    },
    DetachDirect {
        reason: DownReason,
    },
}

/// A stream the peer opened.
pub struct Incoming {
    pub class: StreamClass,
    pub meta: Vec<u8>,
    pub stream: Stream,
}

/// A handle on a running channel. Dropping it does not close the channel;
/// the channel ends when its inbound frames end or it fails.
#[derive(Clone)]
pub struct Channel {
    cmd: mpsc::UnboundedSender<Cmd>,
    stats: watch::Receiver<PathStats>,
}

/// One sub-stream: read with [`Stream::recv`], write with [`Stream::send`]
/// (or split the writing half off with [`Stream::writer`]).
pub struct Stream {
    pub sid: u32,
    rx: mpsc::UnboundedReceiver<StreamEvent>,
    writer: StreamWriter,
}

/// The writing half of a stream; cheap to clone.
#[derive(Clone)]
pub struct StreamWriter {
    sid: u32,
    cmd: mpsc::UnboundedSender<Cmd>,
    budget: Arc<Semaphore>,
}

impl StreamWriter {
    /// Queue `data`, waiting while this stream has too much still unsent.
    pub async fn send(&self, data: &[u8]) -> Result<(), DriverError> {
        for chunk in data.chunks(MAX_CHUNK) {
            let permit = self
                .budget
                .acquire_many(chunk.len() as u32)
                .await
                .map_err(|_| DriverError::StreamClosed)?;
            permit.forget();
            self.cmd
                .send(Cmd::Send {
                    sid: self.sid,
                    data: chunk.to_vec(),
                })
                .map_err(|_| DriverError::ChannelClosed)?;
        }
        Ok(())
    }

    /// Close the stream once what was sent before has gone.
    pub fn close(&self, code: u16) {
        let _ = self.cmd.send(Cmd::Close { sid: self.sid, code });
    }
}

impl Stream {
    pub fn writer(&self) -> StreamWriter {
        self.writer.clone()
    }

    pub async fn send(&self, data: &[u8]) -> Result<(), DriverError> {
        self.writer.send(data).await
    }

    pub fn close(&self, code: u16) {
        self.writer.close(code)
    }

    /// The next thing that happened on the stream; `None` once it is gone.
    /// Taking data is what lets the peer send more.
    pub async fn recv(&mut self) -> Option<StreamEvent> {
        let event = self.rx.recv().await;
        if let Some(StreamEvent::Data(data)) = &event {
            let _ = self.writer.cmd.send(Cmd::Consumed {
                sid: self.sid,
                n: data.len(),
            });
        }
        event
    }
}

struct Slot {
    tx: mpsc::UnboundedSender<StreamEvent>,
    budget: Arc<Semaphore>,
}

impl Channel {
    /// Run `session` (its handshake done). `inbound` is every frame the hub
    /// sends on this channel's `ch`; `outbound` takes the frames to send. The
    /// receiver yields the streams the peer opens.
    pub fn spawn(
        session: Session,
        inbound: mpsc::UnboundedReceiver<Frame>,
        outbound: mpsc::UnboundedSender<Frame>,
    ) -> (Channel, mpsc::UnboundedReceiver<Incoming>) {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (accept_tx, accept_rx) = mpsc::unbounded_channel();
        let (stats_tx, stats) = watch::channel(session.stats());
        let task = Task {
            session,
            cmd_tx: cmd_tx.clone(),
            streams: HashMap::new(),
            send_window: SendWindow::new(),
            recv_window: RecvWindow::new(),
            outbound,
            accept: accept_tx,
            held: None,
            direct: None,
            stats: stats_tx,
        };
        tokio::spawn(task.run(inbound, cmd_rx));
        (Channel { cmd: cmd_tx, stats }, accept_rx)
    }

    /// Carry the channel over a direct path whose handshake has completed
    /// (`transport`): new frames go that way until it fails, when what it did
    /// not deliver goes again over the hub. Ignored (and `pipe` dropped) on a
    /// channel without paths.
    pub fn attach_direct(&self, transport: Transport, pipe: DirectPipe) -> Result<(), DriverError> {
        self.cmd
            .send(Cmd::AttachDirect {
                transport: Box::new(transport),
                pipe,
            })
            .map_err(|_| DriverError::ChannelClosed)
    }

    /// Stop using the direct path (and tell the peer).
    pub fn detach_direct(&self, reason: DownReason) {
        let _ = self.cmd.send(Cmd::DetachDirect { reason });
    }

    /// The paths' state, updated as it changes.
    pub fn path_stats(&self) -> watch::Receiver<PathStats> {
        self.stats.clone()
    }

    /// Open a stream of `class` with `meta` (a [`crate::bridge::RequestHead`]
    /// for the bridge).
    pub async fn open(&self, class: StreamClass, meta: &[u8]) -> Result<Stream, DriverError> {
        let (reply, rx) = oneshot::channel();
        self.cmd
            .send(Cmd::Open {
                class,
                meta: meta.to_vec(),
                reply,
            })
            .map_err(|_| DriverError::ChannelClosed)?;
        rx.await.map_err(|_| DriverError::ChannelClosed)?
    }

    pub fn is_closed(&self) -> bool {
        self.cmd.is_closed()
    }

    /// End the channel: tell the hub (and so the peer), and close every stream.
    pub fn close(&self, code: u16, reason: &str) {
        let _ = self.cmd.send(Cmd::End { code, reason: reason.into() });
    }
}

struct Task {
    session: Session,
    cmd_tx: mpsc::UnboundedSender<Cmd>,
    streams: HashMap<u32, Slot>,
    send_window: SendWindow,
    recv_window: RecvWindow,
    outbound: mpsc::UnboundedSender<Frame>,
    accept: mpsc::UnboundedSender<Incoming>,
    /// A sealed frame waiting for hub credit.
    held: Option<Frame>,
    direct: Option<DirectPipe>,
    stats: watch::Sender<PathStats>,
}

/// The next message from the direct path, if one is attached (else never).
async fn recv_direct(direct: &mut Option<DirectPipe>) -> Option<Vec<u8>> {
    match direct {
        Some(pipe) => pipe.rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Until the session's next timer, if it has one (else never).
async fn until(deadline_us: Option<u64>) {
    match deadline_us {
        Some(at) => tokio::time::sleep(Duration::from_micros(at.saturating_sub(now_us()))).await,
        None => std::future::pending().await,
    }
}

/// Why the task stopped.
enum End {
    Peer(u16),
    Local(u16),
    Failed,
    Gone,
}

impl Task {
    fn slot(&mut self, sid: u32) -> Stream {
        let (tx, rx) = mpsc::unbounded_channel();
        let budget = Arc::new(Semaphore::new(SEND_BUDGET));
        self.streams.insert(sid, Slot { tx, budget: budget.clone() });
        Stream {
            sid,
            rx,
            writer: StreamWriter {
                sid,
                cmd: self.cmd_tx.clone(),
                budget,
            },
        }
    }

    async fn run(mut self, mut inbound: mpsc::UnboundedReceiver<Frame>, mut cmds: mpsc::UnboundedReceiver<Cmd>) {
        let end = loop {
            let room = self.room();
            let mut deadline = self.session.next_deadline_us(room);
            // Nothing says when the direct path's owner drains what it was
            // handed: while its budget is spent, look again shortly.
            if self.direct.is_some() && !room.direct {
                let soon = now_us() + DRAIN_POLL_US;
                deadline = Some(deadline.map_or(soon, |d| d.min(soon)));
            }
            tokio::select! {
                frame = inbound.recv() => match frame {
                    None => break End::Gone,
                    Some(Frame::Data { payload, .. }) => {
                        if let Some(bytes) = self.recv_window.received(payload.len()) {
                            let _ = self.outbound.send(Frame::Credit { ch: self.session.ch(), bytes });
                        }
                        match self.session.receive_relay(&payload, now_us()) {
                            Ok(events) => events.into_iter().for_each(|e| self.on_event(e)),
                            Err(_) => break End::Failed,
                        }
                    }
                    Some(Frame::Credit { bytes, .. }) => {
                        if self.send_window.grant(bytes).is_err() {
                            break End::Peer(crate::bridge::CLOSE_BAD_REQUEST);
                        }
                    }
                    Some(Frame::Close { code, .. }) => break End::Peer(code),
                    Some(_) => {}
                },
                Some(cmd) = cmds.recv() => {
                    if let Cmd::End { code, reason } = cmd {
                        let _ = self.outbound.send(Frame::Close { ch: self.session.ch(), code, reason });
                        break End::Local(code);
                    }
                    self.on_cmd(cmd);
                }
                message = recv_direct(&mut self.direct) => match message {
                    Some(message) => match self.session.receive_direct(&message, now_us()) {
                        Ok(events) => events.into_iter().for_each(|e| self.on_event(e)),
                        Err(_) => break End::Failed,
                    },
                    // The DataChannel is gone: fall back to the hub.
                    None => self.session.detach_direct(DownReason::Failed, true),
                },
                () = until(deadline) => self.session.tick(now_us()),
            }
            self.paths_changed();
            self.sweep();
            if self.pump().is_err() {
                break End::Failed;
            }
            if self.outbound.is_closed() {
                break End::Gone;
            }
        };
        // Whatever ended the channel ends its direct path with it: dropping
        // the pipe tells its owner to tear the peer connection down.
        self.direct = None;
        let code = match end {
            End::Peer(code) | End::Local(code) => code,
            End::Failed | End::Gone => crate::bridge::CLOSE_UPSTREAM,
        };
        if matches!(end, End::Failed) {
            let _ = self.outbound.send(Frame::Close {
                ch: self.session.ch(),
                code: crate::bridge::CLOSE_BAD_REQUEST,
                reason: "channel error".into(),
            });
        }
        for (_, slot) in self.streams.drain() {
            let _ = slot.tx.send(StreamEvent::Closed(code));
            slot.budget.close();
        }
    }

    /// After anything that may have moved the paths: let go of a direct pipe
    /// the session stopped using, and publish the paths' state.
    fn paths_changed(&mut self) {
        if !self.session.has_paths() {
            return;
        }
        if self.direct.is_some() && !self.session.direct_active() {
            self.direct = None;
        }
        let mut moved = false;
        let mut downs = Vec::new();
        for event in self.session.take_events() {
            match event {
                PathEvent::DirectUp => moved = true,
                PathEvent::DirectDown { reason, remote, why } => {
                    moved = true;
                    downs.push((reason, remote, why));
                }
                PathEvent::Rtt { .. } => {}
            }
        }
        let stats = self.session.stats();
        for (reason, remote, why) in downs {
            // What a direct path's owner logs ("the owner let the peer go")
            // does not say why; this does.
            tracing::info!(
                ch = self.session.ch(),
                ?reason,
                remote,
                why,
                direct_rtt_ms = ?stats.direct_rtt_ms,
                resent = stats.lanes.resent,
                "the channel let its direct path go"
            );
        }
        if moved || *self.stats.borrow() != stats {
            self.stats.send_replace(stats);
        }
    }

    fn on_event(&mut self, event: MuxEvent) {
        match event {
            MuxEvent::Opened { sid, class, meta } => {
                let stream = self.slot(sid);
                if self.accept.send(Incoming { class, meta, stream }).is_err() {
                    self.streams.remove(&sid);
                    let _ = self.session.mux().close(sid, crate::bridge::CLOSE_FORBIDDEN);
                }
            }
            MuxEvent::Data { sid, data } => {
                if let Some(slot) = self.streams.get(&sid) {
                    if slot.tx.send(StreamEvent::Data(data)).is_err() {
                        // Nobody reads it: close it rather than stall the peer.
                        self.streams.remove(&sid);
                        let _ = self.session.mux().close(sid, crate::bridge::CLOSE_DONE);
                    }
                } else {
                    // Data for a stream this end already let go: keep the window moving.
                    self.session.mux().consumed(sid, data.len());
                }
            }
            MuxEvent::Closed { sid, code } => {
                if let Some(slot) = self.streams.remove(&sid) {
                    let _ = slot.tx.send(StreamEvent::Closed(code));
                    slot.budget.close();
                }
            }
            MuxEvent::Pong { .. } => {}
        }
    }

    fn on_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Open { class, meta, reply } => {
                let result = self.session.mux().open(class, &meta).map(|sid| self.slot(sid));
                let _ = reply.send(result.map_err(DriverError::from));
            }
            Cmd::Send { sid, data } => {
                if self.session.mux().send(sid, &data).is_err() {
                    if let Some(slot) = self.streams.get(&sid) {
                        slot.budget.add_permits(data.len());
                    }
                }
            }
            Cmd::Close { sid, code } => {
                let _ = self.session.mux().close(sid, code);
            }
            Cmd::Consumed { sid, n } => self.session.mux().consumed(sid, n),
            Cmd::AttachDirect { transport, pipe } => {
                if self.session.attach_direct(*transport, now_us()).is_ok() {
                    self.direct = Some(pipe);
                }
            }
            Cmd::DetachDirect { reason } => self.session.detach_direct(reason, true),
            Cmd::End { .. } => unreachable!("handled by run"),
        }
    }

    /// Streams this end closed are forgotten by the multiplexer when the peer
    /// answers; tell their readers.
    fn sweep(&mut self) {
        let gone: Vec<u32> = self.streams.keys().copied().filter(|sid| !self.session.mux().is_open(*sid)).collect();
        for sid in gone {
            if let Some(slot) = self.streams.remove(&sid) {
                let _ = slot.tx.send(StreamEvent::Closed(crate::bridge::CLOSE_DONE));
                slot.budget.close();
            }
        }
    }

    /// Which paths can take a sealed message now. Only seal another hub
    /// frame when any frame would fit the window, so nothing sealed ever
    /// waits long.
    fn room(&self) -> Room {
        Room {
            relay: self.held.is_none() && self.send_window.can_send(MAX_PAYLOAD_LEN),
            direct: self.direct.as_ref().is_some_and(|d| d.queued.load(Ordering::Relaxed) < DIRECT_BUDGET),
        }
    }

    /// Send what the hub's credit and the direct path's budget allow.
    fn pump(&mut self) -> Result<(), SessionError> {
        loop {
            if let Some(frame) = self.held.take() {
                let len = match &frame {
                    Frame::Data { payload, .. } => payload.len(),
                    _ => 0,
                };
                if !self.send_window.can_send(len) {
                    self.held = Some(frame);
                } else {
                    self.send_window.consume(len).expect("checked");
                    if self.outbound.send(frame).is_err() {
                        return Ok(());
                    }
                }
            }
            let room = self.room();
            if !room.relay && !room.direct {
                return Ok(());
            }
            let Some((wire, data)) = self.session.poll(now_us(), room)? else {
                return Ok(());
            };
            if let Some((sid, len)) = data {
                if let Some(slot) = self.streams.get(&sid) {
                    slot.budget.add_permits(len);
                }
            }
            match wire {
                Wire::Relay(frame) => self.held = Some(frame),
                Wire::Direct(message) => {
                    let pipe = self.direct.as_ref().expect("room only with a pipe");
                    pipe.queued.fetch_add(message.len(), Ordering::Relaxed);
                    if pipe.tx.send(message).is_err() {
                        // Its owner let go: what it did not deliver goes over the hub.
                        self.session.detach_direct(DownReason::Failed, true);
                        self.paths_changed();
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::tests::{fixture, h, sample_chain};
    use crate::chain::ChainState;
    use crate::mux::Role;
    use crate::noise::{Binding, Initiator, RekeyPolicy, Responder};

    fn pair() -> (Session, Session) {
        let f = fixture();
        let chain = ChainState::verify(&sample_chain(&f), None, &mut h()).unwrap();
        let b = Binding {
            account: chain.account().into(),
            device: "dev-0".into(),
            machine: "m-1".into(),
        };
        let mut i = Initiator::new(&f.devices[0].1, &f.machine.1.public(), &b).unwrap();
        let mut r = Responder::new(&f.machine.1, &b).unwrap();
        r.read_hello(&i.write_hello(0, b"").unwrap()).unwrap();
        let (m2, rt) = r.write_reply(b"", RekeyPolicy::default(), 0).unwrap();
        let (it, _) = i.read_reply(&m2, RekeyPolicy::default(), 0).unwrap();
        (Session::new(1, it, Role::Initiator), Session::new(1, rt, Role::Responder))
    }

    /// Two channels wired back to back, like a hub that forwards everything.
    fn wire() -> (Channel, mpsc::UnboundedReceiver<Incoming>, Channel, mpsc::UnboundedReceiver<Incoming>) {
        let (device, agent) = pair();
        let (d_out, mut d_out_rx) = mpsc::unbounded_channel();
        let (a_out, mut a_out_rx) = mpsc::unbounded_channel();
        let (d_in, d_in_rx) = mpsc::unbounded_channel();
        let (a_in, a_in_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(f) = d_out_rx.recv().await {
                if a_in.send(f).is_err() {
                    break;
                }
            }
        });
        tokio::spawn(async move {
            while let Some(f) = a_out_rx.recv().await {
                if d_in.send(f).is_err() {
                    break;
                }
            }
        });
        let (dc, d_acc) = Channel::spawn(device, d_in_rx, d_out);
        let (ac, a_acc) = Channel::spawn(agent, a_in_rx, a_out);
        (dc, d_acc, ac, a_acc)
    }

    #[tokio::test]
    async fn streams_flow_both_ways_through_running_channels() {
        let (device, _d_acc, _agent, mut a_acc) = wire();
        let mut s = device.open(StreamClass::Rpc, b"GET /api/sessions").await.unwrap();
        // More than a stream window and more than the hub window, to exercise both.
        let big = vec![7u8; 3 * 1024 * 1024];
        let writer = s.writer();
        let send = tokio::spawn(async move { writer.send(&big).await });
        let Incoming { class, meta, mut stream } = a_acc.recv().await.unwrap();
        assert_eq!((class, meta.as_slice()), (StreamClass::Rpc, &b"GET /api/sessions"[..]));
        let mut got = 0;
        while got < 3 * 1024 * 1024 {
            match stream.recv().await.unwrap() {
                StreamEvent::Data(d) => got += d.len(),
                other => panic!("{other:?}"),
            }
        }
        send.await.unwrap().unwrap();
        stream.send(b"pong").await.unwrap();
        stream.close(0);
        assert_eq!(s.recv().await, Some(StreamEvent::Data(b"pong".to_vec())));
        assert_eq!(s.recv().await, Some(StreamEvent::Closed(0)));
    }

    #[tokio::test]
    async fn a_channel_that_loses_its_connection_closes_its_streams() {
        let (device, _agent) = pair();
        let (out, _out_rx) = mpsc::unbounded_channel();
        let (d_in, d_in_rx) = mpsc::unbounded_channel::<Frame>();
        let (channel, _acc) = Channel::spawn(device, d_in_rx, out);
        let mut s = channel.open(StreamClass::Term, b"t").await.unwrap();
        drop(d_in);
        assert_eq!(s.recv().await, Some(StreamEvent::Closed(crate::bridge::CLOSE_UPSTREAM)));
    }

    #[tokio::test]
    async fn a_stream_this_end_closes_ends_for_its_reader_too() {
        let (device, _d_acc, _agent, mut a_acc) = wire();
        let mut s = device.open(StreamClass::Mux, b"/api/events").await.unwrap();
        s.send(b"sub").await.unwrap();
        let mut theirs = a_acc.recv().await.unwrap().stream;
        assert_eq!(theirs.recv().await, Some(StreamEvent::Data(b"sub".to_vec())));
        s.close(0);
        assert_eq!(theirs.recv().await, Some(StreamEvent::Closed(0)));
        assert_eq!(s.recv().await, Some(StreamEvent::Closed(0)), "the answer to our close");
        // Ending the channel ends everything on it.
        let mut other = device.open(StreamClass::Term, b"t").await.unwrap();
        device.close(1000, "bye");
        assert!(matches!(other.recv().await, Some(StreamEvent::Closed(1000))));
    }

    fn transports() -> (Transport, Transport) {
        let f = fixture();
        let chain = ChainState::verify(&sample_chain(&f), None, &mut h()).unwrap();
        let b = Binding {
            account: chain.account().into(),
            device: "dev-0".into(),
            machine: "m-1".into(),
        };
        let mut i = Initiator::new(&f.devices[0].1, &f.machine.1.public(), &b).unwrap();
        let mut r = Responder::new(&f.machine.1, &b).unwrap();
        r.read_hello(&i.write_hello(0, b"").unwrap()).unwrap();
        let (m2, rt) = r.write_reply(b"", RekeyPolicy::default(), 0).unwrap();
        let (it, _) = i.read_reply(&m2, RekeyPolicy::default(), 0).unwrap();
        (it, rt)
    }

    /// Two channels with paths, wired back to back through a "hub" that adds
    /// `relay_delay` to every frame (in order).
    fn wire_paths(relay_delay: Duration) -> (Channel, mpsc::UnboundedReceiver<Incoming>, Channel, mpsc::UnboundedReceiver<Incoming>) {
        let (it, rt) = transports();
        let now = now_us();
        let device = Session::with_paths(1, it, Role::Initiator, crate::channel::PathOptions { probe_relay: true }, now);
        let agent = Session::with_paths(1, rt, Role::Responder, crate::channel::PathOptions::default(), now);
        let (d_out, d_out_rx) = mpsc::unbounded_channel();
        let (a_out, a_out_rx) = mpsc::unbounded_channel();
        let (d_in, d_in_rx) = mpsc::unbounded_channel();
        let (a_in, a_in_rx) = mpsc::unbounded_channel();
        for (mut from, to) in [(d_out_rx, a_in), (a_out_rx, d_in)] {
            tokio::spawn(async move {
                while let Some(f) = from.recv().await {
                    tokio::time::sleep(relay_delay).await;
                    if to.send(f).is_err() {
                        break;
                    }
                }
            });
        }
        let (dc, d_acc) = Channel::spawn(device, d_in_rx, d_out);
        let (ac, a_acc) = Channel::spawn(agent, a_in_rx, a_out);
        (dc, d_acc, ac, a_acc)
    }

    /// A DataChannel between the two drivers: `blackhole` drops everything
    /// silently (a network that stopped carrying UDP); either owner ending
    /// closes it for the other (a peer connection torn down). The receivers
    /// say when each driver let its pipe go.
    struct FakeDc {
        blackhole: Arc<std::sync::atomic::AtomicBool>,
        let_go: [oneshot::Receiver<()>; 2],
    }

    fn fake_dc() -> (DirectPipe, DirectPipe, FakeDc) {
        let blackhole = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (a_tx, a_tx_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (b_tx, b_tx_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let (a_rx_tx, a_rx) = mpsc::unbounded_channel();
        let (b_rx_tx, b_rx) = mpsc::unbounded_channel();
        let a_queued = Arc::new(AtomicUsize::new(0));
        let b_queued = Arc::new(AtomicUsize::new(0));
        let mut let_go = Vec::new();
        for (mut from, to, queued) in [(a_tx_rx, b_rx_tx, a_queued.clone()), (b_tx_rx, a_rx_tx, b_queued.clone())] {
            let (gone_tx, gone_rx) = oneshot::channel();
            let_go.push(gone_rx);
            let blackhole = blackhole.clone();
            tokio::spawn(async move {
                while let Some(m) = from.recv().await {
                    queued.fetch_sub(m.len(), Ordering::Relaxed);
                    if !blackhole.load(Ordering::Relaxed) && to.send(m).is_err() {
                        break;
                    }
                }
                // The driver dropped its sender: tear the "peer connection" down.
                let _ = gone_tx.send(());
            });
        }
        let [g0, g1]: [oneshot::Receiver<()>; 2] = let_go.try_into().unwrap();
        (
            DirectPipe {
                tx: a_tx,
                rx: a_rx,
                queued: a_queued,
            },
            DirectPipe {
                tx: b_tx,
                rx: b_rx,
                queued: b_queued,
            },
            FakeDc { blackhole, let_go: [g0, g1] },
        )
    }

    async fn wait_for(rx: &mut watch::Receiver<PathStats>, what: impl Fn(&PathStats) -> bool) -> PathStats {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if what(&rx.borrow()) {
                    return *rx.borrow();
                }
                rx.changed().await.unwrap();
            }
        })
        .await
        .expect("the paths got there")
    }

    async fn type_keys(typer: &StreamWriter, keys: std::ops::Range<u8>) {
        for k in keys {
            typer.send(&[k]).await.unwrap();
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }

    /// An echoing terminal on the agent's side; the device's reader collects `n` bytes.
    fn echo_terminal(mut a_acc: mpsc::UnboundedReceiver<Incoming>, mut term: Stream, n: usize) -> tokio::task::JoinHandle<Vec<u8>> {
        tokio::spawn(async move {
            let Incoming { mut stream, .. } = a_acc.recv().await.unwrap();
            while let Some(StreamEvent::Data(d)) = stream.recv().await {
                stream.send(&d).await.unwrap();
            }
        });
        tokio::spawn(async move {
            let mut echoed = Vec::new();
            while echoed.len() < n {
                match term.recv().await {
                    Some(StreamEvent::Data(d)) => echoed.extend(d),
                    other => panic!("{other:?} after {} bytes", echoed.len()),
                }
            }
            echoed
        })
    }

    /// Typing into a terminal while the channel moves to a direct path, and
    /// while that path dies without a word mid-typing: every keystroke is
    /// echoed exactly once, in order.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn typing_survives_a_switch_to_direct_and_its_silent_death() {
        let (device, _d_acc, agent, a_acc) = wire_paths(Duration::from_millis(5));
        let term = device.open(StreamClass::Term, b"t-1").await.unwrap();
        let typer = term.writer();
        let reader = echo_terminal(a_acc, term, 120);
        let mut d_stats = device.path_stats();
        let mut a_stats = agent.path_stats();
        type_keys(&typer, 0..30).await;

        let (d_pipe, a_pipe, dc) = fake_dc();
        let (dt, at) = transports();
        agent.attach_direct(at, a_pipe).unwrap();
        device.attach_direct(dt, d_pipe).unwrap();
        wait_for(&mut d_stats, |s| s.direct).await;
        type_keys(&typer, 30..60).await;
        let up = wait_for(&mut d_stats, |s| s.direct_rtt_ms.is_some()).await;
        assert!(up.direct_rtt_ms.unwrap() < 5.0, "the direct path is the fast one: {up:?}");

        // The network stops carrying the direct path; nobody says a word.
        dc.blackhole.store(true, Ordering::Relaxed);
        type_keys(&typer, 60..120).await;
        let echoed = tokio::time::timeout(Duration::from_secs(20), reader).await.unwrap().unwrap();
        assert_eq!(echoed, (0..120u8).collect::<Vec<_>>(), "every key, once, in order");
        let d = wait_for(&mut d_stats, |s| !s.direct).await;
        let a = wait_for(&mut a_stats, |s| !s.direct).await;
        assert_eq!((d.ups, d.downs), (1, 1));
        assert!(d.lanes.resent + a.lanes.resent > 0, "something was resent over the hub: {d:?} {a:?}");
        // Both let their pipes go, which tears the peer connection down.
        let [g0, g1] = dc.let_go;
        tokio::time::timeout(Duration::from_secs(5), async {
            g0.await.unwrap();
            g1.await.unwrap();
        })
        .await
        .unwrap();
    }

    /// A DataChannel that closes (the peer connection failed): the fallback is
    /// at once, not after a timeout, and nothing is lost.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_closed_data_channel_falls_back_at_once() {
        let (device, _d_acc, agent, a_acc) = wire_paths(Duration::from_millis(2));
        let term = device.open(StreamClass::Term, b"t-1").await.unwrap();
        let typer = term.writer();
        let reader = echo_terminal(a_acc, term, 60);
        let mut d_stats = device.path_stats();
        let (d_pipe, a_pipe, _dc) = fake_dc();
        let (dt, at) = transports();
        agent.attach_direct(at, a_pipe).unwrap();
        device.attach_direct(dt, d_pipe).unwrap();
        wait_for(&mut d_stats, |s| s.direct).await;
        type_keys(&typer, 0..30).await;
        let started = std::time::Instant::now();
        agent.detach_direct(DownReason::Failed);
        wait_for(&mut d_stats, |s| !s.direct).await;
        assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
        type_keys(&typer, 30..60).await;
        let echoed = tokio::time::timeout(Duration::from_secs(10), reader).await.unwrap().unwrap();
        assert_eq!(echoed, (0..60u8).collect::<Vec<_>>());
    }

    /// The agent closes a channel (its device was removed from the chain,
    /// `agent::link::drop_removed`): the channel's direct path goes with it —
    /// the agent's driver lets its pipe go at once, and the device's side,
    /// seeing its DataChannel close, stops using it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn closing_a_channel_closes_its_direct_path() {
        let (device, _d_acc, agent, _a_acc) = wire_paths(Duration::from_millis(1));
        let mut d_stats = device.path_stats();
        let (d_pipe, a_pipe, dc) = fake_dc();
        let (dt, at) = transports();
        agent.attach_direct(at, a_pipe).unwrap();
        device.attach_direct(dt, d_pipe).unwrap();
        wait_for(&mut d_stats, |s| s.direct).await;
        agent.close(4403, "this device was removed from the account");
        let [device_side, agent_side] = dc.let_go;
        tokio::time::timeout(Duration::from_secs(2), agent_side)
            .await
            .expect("the agent let its direct pipe go")
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), device_side)
            .await
            .expect("and so did the device")
            .unwrap();
        assert!(device.is_closed() || !d_stats.borrow().direct);
    }
}
