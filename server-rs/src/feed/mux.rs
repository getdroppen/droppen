//! Session feeds carried on the global socket, and compression for that socket.
//!
//! A browser reaches this server through a tunnel, and every websocket it opens
//! is a connection of its own: a TCP and a TLS handshake to the edge, a ticket
//! round-trip and an upgrade, before the first byte of a transcript. The web
//! used to open one per session it showed, so on a weak link every switch
//! between sessions started with seconds of handshaking. This module lets the
//! one socket a page already holds — `/api/events`, the session list — carry
//! any number of session feeds as well.
//!
//! # Negotiation
//!
//! A client opts in with `/api/events?v=2`, and is then sent
//! `{"type":"hello","features":[…],"compress":…}` before anything else. A
//! server too old to know any of this sends `settings` first, which is how the
//! client tells the two apart and falls back to a socket per session. The
//! per-session route (`/api/sessions/{source}/{id}/ws`) is untouched, so a page
//! loaded before an upgrade keeps working against the new server.
//!
//! `&z=1` asks for compression: every frame of [`COMPRESS_MIN`] bytes or more
//! goes out as a *binary* message holding the frame's JSON as raw deflate
//! (RFC 1951, `DecompressionStream("deflate-raw")` in a browser); smaller ones
//! stay text. Nothing else compresses a websocket on its way to the browser —
//! tungstenite has no permessage-deflate, and the tunnel passes frames through
//! as they are — and a session's opening page is a few hundred kilobytes of
//! JSON that deflates to a quarter of that.
//!
//! # Subscriptions
//!
//! Up the socket, JSON text:
//!
//! * `{"type":"sub","feeds":[{"sub":7,"source":"claude","id":"…","since":120,"fp":"…","limit":100}]}`
//!   — attach to a session's feed. `sub` is the client's own number for the
//!   subscription, and every frame of it comes back carrying it as its first
//!   field: `{"sub":7,"type":"snapshot",…}`. `since`/`fp` are the resume marker
//!   exactly as on the per-session route ([`super::ResumeFrom`]); `limit` caps
//!   the opening snapshot ([`super::attach_session_feed_with`]). Numbers only
//!   ever increase on one socket and are **never reused**: one that is not
//!   above every number the socket has already been sent is refused, because
//!   frames of the subscription it once named may still be on their way.
//! * `{"type":"unsub","subs":[7]}` — detach.
//! * `{"type":"resync","sub":7,"since":…,"fp":…}` — the per-session socket's
//!   `resync`, addressed to one subscription.
//! * `{"type":"ping"}` — answered with `{"type":"pong"}`. A browser cannot see
//!   the server's own pings, so this is how a page finds out that a socket
//!   which still looks open has quietly died under a network change.
//!
//! Every frame a session's feed would send on its own socket is sent here
//! tagged, including `{"sub":7,"type":"closed"}` when the feed ends or the
//! session is not there. A request refused outright is answered the same way
//! with a `reason`: `invalid` (an entry this module cannot read, whose `sub`
//! it can), `reused` (see above), or `limit` ([`MAX_SUBSCRIPTIONS`]) — so that
//! no client waits on a subscription the server never started. Each subscription is a task that pushes into one
//! bounded queue the socket drains, so a slow client holds its tasks back,
//! their broadcast receivers lag, and the feed's own catch-up (a delta or a
//! reset from the latest read) is what it gets when it drains — the same
//! backpressure a per-session socket had.

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

use gotcha::axum::extract::ws::Message;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::{attach_session_feed_with, FeedRegistry, ResumeFrom, SESSION_PAGE_SIZE};
use crate::drive::Drive;
use crate::sessions::Source;

/// Frames smaller than this go out as text: below a kilobyte deflate saves a
/// few hundred bytes at best, and every binary frame costs the page an
/// asynchronous decode.
pub const COMPRESS_MIN: usize = 1024;
/// How many session feeds one socket may hold. A page shows a handful and
/// keeps a few more warm; this only bounds a client that never unsubscribes.
pub const MAX_SUBSCRIPTIONS: usize = 32;
/// The smallest opening page a client may ask for.
const MIN_LIMIT: usize = 20;
/// Frames queued for the socket across all of its subscriptions.
const OUT_CAPACITY: usize = 256;
/// `resync` requests queued for one subscription.
const CONTROL_CAPACITY: usize = 8;

/// What a client asked of the global socket in its URL.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MuxOptions {
    /// `v=2`: send `hello`, and take subscriptions.
    pub enabled: bool,
    /// `z=1`: compress large frames. Only honoured together with `enabled`, so
    /// that a client that cannot read `hello` is never sent a binary frame.
    pub compress: bool,
}

impl MuxOptions {
    /// Read the two query parameters, leniently: anything unrecognised is off.
    pub fn from_query(v: Option<&str>, z: Option<&str>) -> Self {
        let enabled = v.is_some_and(|v| v.trim().parse::<u32>().is_ok_and(|v| v >= 2));
        Self {
            enabled,
            compress: enabled && z.is_some_and(|z| z.trim() == "1"),
        }
    }
}

/// `{"type":"hello","features":["sub","ping"],"compress":"deflate-raw"|null}`.
pub fn hello_frame(options: MuxOptions) -> String {
    serde_json::json!({
        "type": "hello",
        "features": ["sub", "ping"],
        "compress": options.compress.then_some("deflate-raw"),
    })
    .to_string()
}

/// One subscription a client asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct SubRequest {
    /// The client's number for it, echoed on every frame.
    pub sub: u64,
    /// Which CLI the session belongs to.
    pub source: Source,
    /// The session.
    pub id: String,
    /// What the client already holds, if anything.
    pub resume: Option<ResumeFrom>,
    /// The most items an opening snapshot may carry.
    pub limit: usize,
}

/// A message up the socket this module understands.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientMessage {
    /// Attach to these feeds.
    Sub {
        /// The entries that could be read.
        feeds: Vec<SubRequest>,
        /// The `sub` numbers of the entries that could not.
        rejected: Vec<u64>,
    },
    /// Detach from these subscriptions.
    Unsub(Vec<u64>),
    /// A `resync` for one subscription.
    Resync {
        /// Which subscription.
        sub: u64,
        /// The text as sent: the feed reads `since` and `fp` off it itself.
        text: String,
    },
    /// Are you there?
    Ping,
}

fn as_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        other => Some(other.to_string()),
    }
}

fn sub_request(value: &Value) -> Option<SubRequest> {
    let sub = value.get("sub")?.as_u64()?;
    let source: Source = serde_json::from_value(value.get("source")?.clone()).ok()?;
    let id = value.get("id")?.as_str()?;
    // A session id is a uuid or a codex thread id; anything long or odd is not
    // one, and is refused here rather than handed to a path join.
    if id.is_empty() || id.len() > 128 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return None;
    }
    let since = value.get("since").and_then(as_text);
    let resume = ResumeFrom::parse(since.as_deref(), value.get("fp").and_then(Value::as_str));
    let limit = value
        .get("limit")
        .and_then(Value::as_u64)
        .map_or(SESSION_PAGE_SIZE, |limit| (limit as usize).clamp(MIN_LIMIT, SESSION_PAGE_SIZE));
    Some(SubRequest {
        sub,
        source,
        id: id.to_string(),
        resume,
        limit,
    })
}

/// Read a message from the client, or `None` for anything this module does
/// not handle. A malformed entry in a `sub` is refused on its own — by number,
/// when it has one — and the rest of the batch still stands.
pub fn parse_client_message(text: &str) -> Option<ClientMessage> {
    let message: Value = serde_json::from_str(text).ok()?;
    match message.get("type")?.as_str()? {
        "sub" => {
            let mut feeds = Vec::new();
            let mut rejected = Vec::new();
            for entry in message.get("feeds")?.as_array()? {
                match sub_request(entry) {
                    Some(request) => feeds.push(request),
                    None => rejected.extend(entry.get("sub").and_then(Value::as_u64)),
                }
            }
            Some(ClientMessage::Sub { feeds, rejected })
        }
        "unsub" => Some(ClientMessage::Unsub(
            message.get("subs")?.as_array()?.iter().filter_map(Value::as_u64).collect(),
        )),
        "resync" => Some(ClientMessage::Resync {
            sub: message.get("sub")?.as_u64()?,
            text: text.to_string(),
        }),
        "ping" => Some(ClientMessage::Ping),
        _ => None,
    }
}

/// `frame` with `"sub":N` as its first field.
///
/// Every feed frame is a JSON object, so this is a splice rather than a parse
/// and re-serialisation of what may be a few hundred kilobytes.
pub fn tag(sub: u64, frame: &str) -> String {
    match frame.strip_prefix('{') {
        Some("}") => format!("{{\"sub\":{sub}}}"),
        Some(rest) => format!("{{\"sub\":{sub},{rest}"),
        None => frame.to_string(),
    }
}

/// `{"sub":N,"type":"closed","reason":…}`: a subscription refused outright.
pub fn refused(sub: u64, reason: &str) -> String {
    serde_json::json!({ "sub": sub, "type": "closed", "reason": reason }).to_string()
}

/// `bytes` as raw deflate.
pub fn deflate_raw(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::DeflateEncoder::new(Vec::with_capacity(bytes.len() / 3), flate2::Compression::default());
    // Writing into a `Vec` cannot fail.
    let _ = encoder.write_all(bytes);
    encoder.finish().unwrap_or_default()
}

/// A frame as the socket sends it: compressed into a binary message when the
/// client asked for that and the frame is worth it, text otherwise.
pub fn encode(frame: String, compress: bool) -> Message {
    if compress && frame.len() >= COMPRESS_MIN {
        Message::Binary(deflate_raw(frame.as_bytes()).into())
    } else {
        Message::Text(frame.into())
    }
}

struct Sub {
    task: JoinHandle<()>,
    control: mpsc::Sender<String>,
}

/// One socket's session subscriptions. Dropping it detaches every one.
pub struct Subscriptions {
    registry: Arc<FeedRegistry>,
    drive: Arc<Drive>,
    out: mpsc::Sender<String>,
    subs: HashMap<u64, Sub>,
    /// The highest `sub` number this socket has been sent.
    highest: Option<u64>,
}

impl Drop for Subscriptions {
    fn drop(&mut self) {
        for sub in self.subs.values() {
            sub.task.abort();
        }
    }
}

impl Subscriptions {
    /// No subscriptions yet, and the queue every later frame of theirs arrives on.
    pub fn new(registry: Arc<FeedRegistry>, drive: Arc<Drive>) -> (Self, mpsc::Receiver<String>) {
        let (out, rx) = mpsc::channel(OUT_CAPACITY);
        (
            Self {
                registry,
                drive,
                out,
                subs: HashMap::new(),
                highest: None,
            },
            rx,
        )
    }

    /// How many subscriptions are live.
    pub fn len(&self) -> usize {
        self.subs.len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.subs.is_empty()
    }

    /// Act on a message; what to send straight back, if anything — a `pong`,
    /// or the refusals of a `sub`. Sent by the socket itself rather than queued
    /// behind the subscriptions' frames, so a refusal is never lost to a full
    /// queue.
    pub fn handle(&mut self, message: ClientMessage) -> Vec<String> {
        match message {
            ClientMessage::Sub { feeds, rejected } => {
                let mut replies: Vec<String> = rejected.into_iter().map(|sub| refused(sub, "invalid")).collect();
                for request in feeds {
                    replies.extend(self.subscribe(request));
                }
                replies
            }
            ClientMessage::Unsub(subs) => {
                for sub in subs {
                    if let Some(sub) = self.subs.remove(&sub) {
                        sub.task.abort();
                    }
                }
                Vec::new()
            }
            ClientMessage::Resync { sub, text } => {
                if let Some(sub) = self.subs.get(&sub) {
                    // Full means one is already queued, and its answer covers this.
                    let _ = sub.control.try_send(text);
                }
                Vec::new()
            }
            ClientMessage::Ping => vec![r#"{"type":"pong"}"#.to_string()],
        }
    }

    /// Start one subscription, or say why not.
    fn subscribe(&mut self, request: SubRequest) -> Option<String> {
        // Never a number this socket has seen: an aborted subscription's frames
        // may still be in the queue, and they would arrive under the new one.
        if self.highest.is_some_and(|highest| request.sub <= highest) {
            return Some(refused(request.sub, "reused"));
        }
        self.highest = Some(request.sub);
        // Finished tasks (a session that went away) still hold a slot until
        // the client unsubscribes; reclaim those before refusing anybody.
        self.subs.retain(|_, sub| !sub.task.is_finished());
        if self.subs.len() >= MAX_SUBSCRIPTIONS {
            return Some(refused(request.sub, "limit"));
        }
        let (control, control_rx) = mpsc::channel(CONTROL_CAPACITY);
        let sub = request.sub;
        let task = tokio::spawn(pump(self.registry.clone(), self.drive.clone(), request, self.out.clone(), control_rx));
        self.subs.insert(sub, Sub { task, control });
        None
    }
}

/// One subscription: attach, send the opening frames, then follow the feed.
async fn pump(registry: Arc<FeedRegistry>, drive: Arc<Drive>, request: SubRequest, out: mpsc::Sender<String>, mut control: mpsc::Receiver<String>) {
    let sub = request.sub;
    let closed = || tag(sub, r#"{"type":"closed"}"#);
    let Some(attached) = attach_session_feed_with(&registry, &drive, request.source, &request.id, request.resume.as_ref(), request.limit).await
    else {
        let _ = out.send(closed()).await;
        return;
    };
    let mut attachment = attached.attachment;
    for frame in attached.opening {
        if out.send(tag(sub, &frame)).await.is_err() {
            return;
        }
    }
    loop {
        tokio::select! {
            frame = attachment.next_frame() => match frame {
                Some(frame) => {
                    if out.send(tag(sub, &frame)).await.is_err() {
                        return;
                    }
                }
                None => {
                    let _ = out.send(closed()).await;
                    return;
                }
            },
            text = control.recv() => match text {
                Some(text) => {
                    if let Some(frame) = attachment.client_frame(&text) {
                        if out.send(tag(sub, &frame)).await.is_err() {
                            return;
                        }
                    }
                }
                None => return,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_need_v2_and_compression_needs_both() {
        assert_eq!(MuxOptions::from_query(None, None), MuxOptions::default());
        assert_eq!(
            MuxOptions::from_query(Some("1"), Some("1")),
            MuxOptions::default(),
            "v1 is the old protocol"
        );
        assert_eq!(
            MuxOptions::from_query(Some("2"), None),
            MuxOptions {
                enabled: true,
                compress: false
            }
        );
        assert_eq!(
            MuxOptions::from_query(Some("2"), Some("1")),
            MuxOptions {
                enabled: true,
                compress: true
            }
        );
        assert!(
            !MuxOptions::from_query(None, Some("1")).compress,
            "a client that does not read hello is never sent a binary frame"
        );
        assert!(!MuxOptions::from_query(Some("x"), Some("1")).enabled);
    }

    #[test]
    fn hello_says_whether_frames_will_be_compressed() {
        let on: Value = serde_json::from_str(&hello_frame(MuxOptions::from_query(Some("2"), Some("1")))).unwrap();
        assert_eq!(on["type"], "hello");
        assert_eq!(on["compress"], "deflate-raw");
        assert!(on["features"].as_array().unwrap().iter().any(|f| f == "sub"));
        let off: Value = serde_json::from_str(&hello_frame(MuxOptions::from_query(Some("2"), None))).unwrap();
        assert_eq!(off["compress"], Value::Null);
    }

    #[test]
    fn a_sub_batch_keeps_its_good_entries_and_clamps_the_limit() {
        let text = r#"{"type":"sub","feeds":[
            {"sub":1,"source":"claude","id":"0f3c-aa","since":120,"fp":"AAAAAAAAAAAAAAAAAAAAAA","limit":5},
            {"sub":2,"source":"codex","id":"t_1"},
            {"sub":3,"source":"nope","id":"x"},
            {"sub":4,"source":"claude","id":"../etc/passwd"},
            {"source":"claude","id":"no-sub"}
        ]}"#;
        let Some(ClientMessage::Sub { feeds, rejected }) = parse_client_message(text) else {
            panic!("a sub")
        };
        assert_eq!(feeds.len(), 2, "{feeds:?}");
        assert_eq!(rejected, vec![3, 4], "refused by number; an entry with no number cannot be answered");
        assert_eq!(feeds[0].sub, 1);
        assert_eq!(feeds[0].source, Source::Claude);
        assert_eq!(feeds[0].limit, MIN_LIMIT, "a tiny limit is raised to the floor");
        assert_eq!(
            feeds[0].resume,
            ResumeFrom::parse(Some("120"), Some("AAAAAAAAAAAAAAAAAAAAAA")),
            "the marker reads like the per-session route's query"
        );
        assert_eq!(feeds[1].source, Source::Codex);
        assert_eq!(feeds[1].limit, SESSION_PAGE_SIZE, "no limit is a full page");
        assert_eq!(feeds[1].resume, None);
    }

    #[test]
    fn the_other_messages_parse_and_strangers_do_not() {
        assert_eq!(
            parse_client_message(r#"{"type":"unsub","subs":[1,2]}"#),
            Some(ClientMessage::Unsub(vec![1, 2]))
        );
        assert_eq!(parse_client_message(r#"{"type":"ping"}"#), Some(ClientMessage::Ping));
        let resync = r#"{"type":"resync","sub":3,"since":10,"fp":"x"}"#;
        assert_eq!(
            parse_client_message(resync),
            Some(ClientMessage::Resync {
                sub: 3,
                text: resync.to_string()
            })
        );
        assert_eq!(parse_client_message(r#"{"type":"resync"}"#), None, "a resync must name its subscription");
        assert_eq!(parse_client_message(r#"{"type":"what"}"#), None);
        assert_eq!(parse_client_message("not json"), None);
    }

    #[test]
    fn a_tag_is_the_first_field_and_the_rest_is_untouched() {
        assert_eq!(tag(7, r#"{"type":"closed"}"#), r#"{"sub":7,"type":"closed"}"#);
        assert_eq!(tag(7, "{}"), r#"{"sub":7}"#);
        let frame = r#"{"type":"delta","items":[{"text":"{\"sub\":1}"}]}"#;
        let tagged: Value = serde_json::from_str(&tag(12, frame)).unwrap();
        assert_eq!(tagged["sub"], 12);
        assert_eq!(tagged["items"][0]["text"], r#"{"sub":1}"#);
    }

    #[test]
    fn large_frames_are_deflated_and_small_ones_stay_text() {
        use std::io::Read;
        let small = r#"{"type":"state"}"#.to_string();
        assert!(matches!(encode(small.clone(), true), Message::Text(text) if text.as_str() == small));
        let large = serde_json::json!({ "type": "snapshot", "items": vec!["the same words again"; 400] }).to_string();
        assert!(large.len() >= COMPRESS_MIN);
        assert!(matches!(encode(large.clone(), false), Message::Text(_)), "uncompressed unless asked");
        let Message::Binary(bytes) = encode(large.clone(), true) else {
            panic!("binary")
        };
        assert!(bytes.len() * 4 < large.len(), "{} -> {}", large.len(), bytes.len());
        let mut back = String::new();
        flate2::read::DeflateDecoder::new(&bytes[..]).read_to_string(&mut back).unwrap();
        assert_eq!(back, large, "raw deflate, as DecompressionStream(\"deflate-raw\") reads it");
    }

    // ---------------------------------------------------------- end to end

    use super::super::tests::{claude_turns, test_drive};
    use std::time::Duration;

    const ID: &str = "55555555-5555-4555-8555-555555555555";

    /// A drive with one ~1000-item claude session in its managed root.
    fn session(dir: &std::path::Path) -> (Arc<Drive>, std::path::PathBuf) {
        let (paths, drive) = test_drive(dir);
        let project = paths.claude_managed_projects().join("-w-mux");
        std::fs::create_dir_all(&project).expect("project");
        let file = project.join(format!("{ID}.jsonl"));
        std::fs::write(&file, claude_turns(ID, 0, 250)).expect("transcript");
        (drive, file)
    }

    fn sub(sub: u64, extra: Value) -> ClientMessage {
        let mut entry = serde_json::json!({ "sub": sub, "source": "claude", "id": ID });
        if let (Value::Object(entry), Value::Object(extra)) = (&mut entry, extra) {
            entry.extend(extra);
        }
        parse_client_message(&serde_json::json!({ "type": "sub", "feeds": [entry] }).to_string()).expect("a sub")
    }

    async fn next(rx: &mut mpsc::Receiver<String>) -> Value {
        let frame = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("a frame in time")
            .expect("open");
        serde_json::from_str(&frame).expect("json")
    }

    async fn settle(registry: &FeedRegistry) {
        for _ in 0..100 {
            if registry.is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn a_subscription_opens_with_a_short_tagged_snapshot_and_resumes_with_a_delta() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (drive, _) = session(dir.path());
        let registry = FeedRegistry::new();
        let (mut subs, mut rx) = Subscriptions::new(registry.clone(), drive);

        assert!(subs.handle(sub(1, serde_json::json!({ "limit": 60 }))).is_empty());
        let snapshot = next(&mut rx).await;
        assert_eq!(snapshot["sub"], 1);
        assert_eq!(snapshot["type"], "snapshot");
        assert_eq!(snapshot["endIndex"], 1000);
        assert_eq!(snapshot["items"].as_array().map(Vec::len), Some(60), "the page the client asked for");
        assert_eq!(snapshot["startIndex"], 940);
        assert_eq!(snapshot["hasMoreBefore"], true);
        assert!(snapshot["startFp"].is_string(), "a short page is still provable for history");
        let state = next(&mut rx).await;
        assert_eq!((state["sub"].clone(), state["type"].clone()), (Value::from(1), Value::from("state")));

        // A second subscription to the same session, resuming: the overlap only.
        subs.handle(sub(2, serde_json::json!({ "since": 1000, "fp": snapshot["prefixFp"] })));
        let delta = next(&mut rx).await;
        assert_eq!(delta["sub"], 2);
        assert_eq!(delta["type"], "delta");
        assert_eq!(delta["fromIndex"], 1000 - super::super::RESUME_OVERLAP);
        assert_eq!(registry.len(), 1, "two subscriptions, one watcher");
        assert_eq!(subs.len(), 2);
    }

    #[tokio::test]
    async fn unsubscribing_and_dropping_detach_every_feed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (drive, _) = session(dir.path());
        let registry = FeedRegistry::new();
        let (mut subs, mut rx) = Subscriptions::new(registry.clone(), drive);
        subs.handle(sub(1, Value::Null));
        subs.handle(sub(2, Value::Null));
        next(&mut rx).await;
        assert_eq!(registry.len(), 1);

        subs.handle(ClientMessage::Unsub(vec![1]));
        assert_eq!(subs.len(), 1);
        drop(subs);
        settle(&registry).await;
        assert!(registry.is_empty(), "no watcher outlives the socket's subscriptions");
    }

    #[tokio::test]
    async fn a_missing_session_is_a_tagged_closed_and_a_ping_is_a_pong() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (drive, _) = session(dir.path());
        let (mut subs, mut rx) = Subscriptions::new(FeedRegistry::new(), drive);
        let missing = parse_client_message(r#"{"type":"sub","feeds":[{"sub":9,"source":"claude","id":"00000000-0000-4000-8000-000000000000"}]}"#)
            .expect("a sub");
        subs.handle(missing);
        let closed = next(&mut rx).await;
        assert_eq!(closed, serde_json::json!({ "sub": 9, "type": "closed" }));
        assert_eq!(subs.handle(ClientMessage::Ping), vec![r#"{"type":"pong"}"#.to_string()]);
    }

    #[tokio::test]
    async fn a_resync_reaches_its_own_subscription_and_live_changes_arrive_tagged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (drive, file) = session(dir.path());
        let (mut subs, mut rx) = Subscriptions::new(FeedRegistry::new(), drive);
        subs.handle(sub(4, Value::Null));
        let snapshot = next(&mut rx).await;
        assert_eq!(snapshot["type"], "snapshot");
        next(&mut rx).await; // state

        let resync = serde_json::json!({ "type": "resync", "sub": 4, "since": 1000, "fp": snapshot["prefixFp"] }).to_string();
        subs.handle(parse_client_message(&resync).expect("a resync"));
        let answer = next(&mut rx).await;
        assert_eq!((answer["sub"].clone(), answer["type"].clone()), (Value::from(4), Value::from("delta")));

        let mut handle = std::fs::OpenOptions::new().append(true).open(&file).expect("open");
        std::io::Write::write_all(&mut handle, claude_turns(ID, 250, 1).as_bytes()).expect("append");
        drop(handle);
        loop {
            let frame = next(&mut rx).await;
            assert_eq!(frame["sub"], 4, "{frame}");
            if frame["type"] == "delta" {
                assert_eq!(frame["endIndex"], 1004, "the new turn, live");
                break;
            }
        }
    }

    /// The queue the socket drains is bounded, so a client that stops reading
    /// holds its subscriptions back rather than growing memory; when it reads
    /// again each feed catches it up from its latest read.
    ///
    /// Each step waits for the state it needs, not for a guess at how long
    /// that takes: on a starved runner the 32 attaches (each parses the
    /// transcript) outlasted every append and the whole wait after them, so
    /// nothing was ever queued, and with a little less load the subscriptions
    /// opened on the newest turn and nothing was held back at all.
    #[tokio::test]
    async fn a_client_that_stops_reading_is_caught_up_rather_than_buffered_without_bound() {
        // Long enough for a runner that has a fraction of a CPU; a step that
        // never comes still fails, in minutes rather than hanging.
        const PATIENCE: Duration = Duration::from_secs(120);
        let dir = tempfile::tempdir().expect("tempdir");
        let (drive, file) = session(dir.path());
        let (mut subs, mut rx) = Subscriptions::new(FeedRegistry::new(), drive);
        for n in 1..=MAX_SUBSCRIPTIONS as u64 {
            assert!(subs.handle(sub(n, Value::Null)).is_empty());
        }
        // Every subscription is open before the client stops reading: what
        // is held back is the session moving on, not the attaches.
        let mut open = std::collections::HashSet::new();
        while open.len() < MAX_SUBSCRIPTIONS {
            let frame = tokio::time::timeout(PATIENCE, rx.recv())
                .await
                .unwrap_or_else(|_| panic!("{} of {MAX_SUBSCRIPTIONS} subscriptions opened", open.len()))
                .expect("open");
            let frame: Value = serde_json::from_str(&frame).expect("json");
            if frame["type"] == "snapshot" {
                open.insert(frame["sub"].as_u64().expect("tagged"));
            }
        }
        // Nothing is read from here on while the session moves on, turn after
        // turn: until the queue is full, and then a few turns more.
        let mut turns = 250;
        let append = |turn: usize| {
            let mut handle = std::fs::OpenOptions::new().append(true).open(&file).expect("open");
            std::io::Write::write_all(&mut handle, claude_turns(ID, turn, 1).as_bytes()).expect("append");
        };
        let deadline = tokio::time::Instant::now() + PATIENCE;
        while rx.len() < OUT_CAPACITY {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the queue never filled: {} of {OUT_CAPACITY} after {} turns",
                rx.len(),
                turns - 250
            );
            append(turns);
            turns += 1;
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        for _ in 0..4 {
            append(turns);
            turns += 1;
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        assert_eq!(rx.len(), OUT_CAPACITY, "held back at the queue's bound, and no further");
        // Once the client reads again, the held-back subscriptions catch up to
        // the newest turn rather than replaying every one they missed.
        let newest = (turns * 4) as u64;
        let mut caught_up = std::collections::HashSet::new();
        while caught_up.len() < MAX_SUBSCRIPTIONS {
            let frame = tokio::time::timeout(PATIENCE, rx.recv())
                .await
                .unwrap_or_else(|_| panic!("{} of {MAX_SUBSCRIPTIONS} caught up to item {newest}", caught_up.len()))
                .expect("open");
            let frame: Value = serde_json::from_str(&frame).expect("json");
            if frame["endIndex"] == newest {
                caught_up.insert(frame["sub"].as_u64().expect("tagged"));
            }
        }
    }

    #[tokio::test]
    async fn a_request_the_server_will_not_start_is_answered_rather_than_dropped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (drive, _) = session(dir.path());
        let (mut subs, _rx) = Subscriptions::new(FeedRegistry::new(), drive);
        let refusal = |text: &str| -> Value { serde_json::from_str(text).expect("json") };

        let batch = parse_client_message(
            r#"{"type":"sub","feeds":[{"sub":5,"source":"claude","id":"../x"},{"sub":6,"source":"claude","id":"55555555-5555-4555-8555-555555555555"}]}"#,
        )
        .expect("a sub");
        let replies = subs.handle(batch);
        assert_eq!(replies.len(), 1, "{replies:?}");
        assert_eq!(
            refusal(&replies[0]),
            serde_json::json!({ "sub": 5, "type": "closed", "reason": "invalid" })
        );
        assert_eq!(subs.len(), 1, "the good entry of the batch still stands");

        // A number at or below the highest this socket has seen is never a
        // new subscription: frames of the one it named may still be queued.
        for again in [6, 3] {
            let replies = subs.handle(sub(again, Value::Null));
            assert_eq!(
                refusal(&replies[0]),
                serde_json::json!({ "sub": again, "type": "closed", "reason": "reused" })
            );
        }
        assert_eq!(subs.len(), 1);

        for n in 7..(7 + MAX_SUBSCRIPTIONS as u64 - 1) {
            assert!(subs.handle(sub(n, Value::Null)).is_empty());
        }
        let over = 7 + MAX_SUBSCRIPTIONS as u64;
        let replies = subs.handle(sub(over, Value::Null));
        assert_eq!(
            refusal(&replies[0]),
            serde_json::json!({ "sub": over, "type": "closed", "reason": "limit" })
        );
    }
}
