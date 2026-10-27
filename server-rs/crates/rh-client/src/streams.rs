//! Reading a machine's encrypted streams from the hub, as a device does:
//! subscribe, start from the manifest (its pages fetched over HTTP and checked
//! against the signed list), follow the records, and release nothing a
//! machine-signed head does not cover — and folding what a stream holds as
//! the web does (`web/src/lib/hub/streams.ts`).

use std::time::Duration;

use rh_proto::frame::Frame;
use rh_proto::record::StreamId;
use rh_proto::stream::MAX_PAGE_LEN;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::follow::{Follower, Step};
use crate::link::Link;
pub use crate::link::CONTROL_FRAME;
use crate::{err, hex, HubUrl, Result};

/// Where a stream's snapshot pages come from: `GET
/// /v1/streams/{stream}/pages/{hash}` at `hub`, under a device session.
#[derive(Clone, Copy)]
pub struct Pages<'a> {
    pub http: &'a reqwest::Client,
    pub hub: &'a HubUrl,
    pub token: &'a str,
}

impl Pages<'_> {
    /// One page, as stored (sealed; check it against the manifest). Never
    /// more than [`MAX_PAGE_LEN`] bytes, whatever the hub sends: a larger
    /// answer is refused as it streams in, not read whole first.
    pub async fn fetch(&self, stream: &StreamId, hash: &[u8; 32]) -> Result<Vec<u8>> {
        let url = format!("{}/v1/streams/{}/pages/{}", self.hub.http, hex(&stream.0), hex(hash));
        let res = self.http.get(&url).bearer_auth(self.token).send().await.map_err(err("request"))?;
        if !res.status().is_success() {
            return Err(format!("GET page: {}", res.status()));
        }
        read_capped(res, MAX_PAGE_LEN).await
    }
}

/// An answer's body, refused once it passes `cap` bytes.
pub(crate) async fn read_capped(mut res: reqwest::Response, cap: usize) -> Result<Vec<u8>> {
    let too_large = || format!("a page larger than {cap} bytes");
    if res.content_length().is_some_and(|n| n > cap as u64) {
        return Err(too_large());
    }
    let mut out = Vec::new();
    while let Some(chunk) = res.chunk().await.map_err(err("body"))? {
        if out.len() + chunk.len() > cap {
            return Err(too_large());
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// A stream as read: every body in order (the manifest's pages, then the
/// records after it), each with its format.
#[derive(Debug, Clone, Default)]
pub struct StreamCopy {
    pub exists: bool,
    pub bodies: Vec<(u16, Vec<u8>)>,
    /// Where the manifest stood, if one was read.
    pub manifest_upto: Option<u64>,
    pub confirmed: u64,
    /// The newest machine-signed head's time.
    pub freshest_ms: Option<u64>,
    /// Every sealed byte string read (record ciphertexts, encoded pages), for
    /// checking that the hub's database holds none of them.
    pub sealed: Vec<Vec<u8>>,
}

/// Read `stream` once, whole, verified: `machine_key` is the writer's
/// Ed25519 key, `keys` every machine key epoch the reader holds.
pub async fn read_stream(
    link: &Link,
    pages: Pages<'_>,
    machine_key: [u8; 32],
    keys: &[(u32, [u8; 32])],
    stream: StreamId,
    timeout: Duration,
) -> Result<StreamCopy> {
    let (ch, mut rx) = link.reserve();
    link.send(Frame::Sub { ch, stream, from_seq: 0 })?;
    let result = follow(&mut rx, pages, machine_key, keys, stream, timeout).await;
    let _ = link.send_control(&json!({"t": "stream_unsub", "ch": ch}));
    link.release(ch);
    result
}

/// Drive one subscription to its end over a [`Follower`] — the reader the
/// local install follows with — so a read is checked by the same rules as
/// production: only what a machine-signed head covers is released, a
/// checkpoint or head behind what the copy already holds is refused, and a
/// `GONE` before where the reader stands is not taken.
async fn follow(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Frame>,
    pages: Pages<'_>,
    machine_key: [u8; 32],
    keys: &[(u32, [u8; 32])],
    stream: StreamId,
    timeout: Duration,
) -> Result<StreamCopy> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut follower = Follower::new(stream, machine_key, keys);
    let mut copy = StreamCopy::default();
    let mut received = 0u64;
    let mut synced = false;
    loop {
        if synced && received == follower.from_seq() {
            copy.confirmed = follower.from_seq();
            copy.freshest_ms = follower.freshest_ms();
            return Ok(copy);
        }
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        let frame = match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(frame)) => frame,
            Ok(None) => return Err("the link closed".into()),
            Err(_) => {
                return Err(format!(
                    "the stream did not settle in time (synced {synced}, received {received}, confirmed {})",
                    follower.from_seq()
                ))
            }
        };
        // Every sealed byte string read, before the frame is folded: what
        // the hub's database must hold none of.
        if let Frame::Recs { records, .. } = &frame {
            for record in records {
                copy.sealed.push(record.ct.clone());
                received = record.seq + 1;
            }
        }
        let gone = matches!(frame, Frame::Gone { .. });
        match follower.on_frame(frame).map_err(|e| e.to_string())? {
            Step::Nothing => {
                if gone {
                    // Everything starts again; the next bodies released
                    // replace what was folded.
                    copy.bodies.clear();
                    received = 0;
                }
            }
            Step::Missing => {
                copy.exists = false;
                return Ok(copy);
            }
            Step::Synced => {
                copy.exists = true;
                synced = true;
            }
            Step::Checkpoint(manifest) => {
                let mut fetched = Vec::with_capacity(manifest.pages.len());
                for hash in &manifest.pages {
                    let data = pages.fetch(&stream, hash).await?;
                    copy.sealed.push(data.clone());
                    fetched.push(data);
                }
                match follower.checkpoint(&manifest, &fetched).map_err(|e| e.to_string())? {
                    Step::Released { bodies, .. } => {
                        copy.bodies = bodies;
                        copy.manifest_upto = Some(manifest.upto_seq());
                        received = manifest.upto_seq();
                    }
                    _ => return Err("a checkpoint that released nothing".into()),
                }
            }
            Step::Released { reset, bodies } => {
                if reset {
                    copy.bodies.clear();
                }
                copy.bodies.extend(bodies);
            }
        }
    }
}

/// A machine's session list as its index stream folds (`web/src/lib/hub/streams.ts`'s
/// `indexFold`): rows by `source/id`, and the time the agent last listed them.
/// `{"asOf"}` alone — the agent's daily write that keeps a quiet machine's
/// index on the hub — moves only the time.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexCopy {
    pub rows: std::collections::BTreeMap<String, Value>,
    pub as_of: Option<u64>,
}

/// A session's transcript as its stream folds (`feedFold`): items
/// `[start, end)`, and — when the agent trimmed the copy to the account's
/// retention window (relay-and-plans §3.4) — `base`, where the copy starts
/// with the earlier entries left on the machine, and `before`, the time
/// (RFC 3339) of the newest of them.
///
/// `fp` and `prefix_fp` are the last body's marks (the agent's fingerprint of
/// the last item, and of all but the last three), as `feedFold` keeps them:
/// a copy rebuilt from its [`Snapshot`] carries them, so a reader that
/// resumes windows by them (the web's offline transport) can. A copy stored
/// before they were kept reads them as absent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeedCopy {
    pub started: bool,
    pub start: usize,
    pub end: usize,
    pub items: Vec<Value>,
    pub base: Option<usize>,
    pub before: Option<String>,
    #[serde(default)]
    pub fp: Option<String>,
    #[serde(default)]
    pub prefix_fp: Option<String>,
}

/// The bodies that fold, from nothing, into a copy — written as a JSON array
/// — so a reader with the same fold (the web's `indexFold`, `feedFold`) holds
/// the very same state without the stream: what the local gateway hands a
/// page that subscribes to a copy this computer already holds
/// (local-aggregator §6.3, S1-7). Borrowed, never cloned: a session's items
/// may be megabytes.
pub trait Snapshot {
    fn snapshot_bodies(&self) -> String;
}

#[derive(Serialize)]
struct IndexBody<'a> {
    reset: bool,
    upserts: Vec<&'a Value>,
    removed: &'static [&'static str],
    #[serde(rename = "asOf", skip_serializing_if = "Option::is_none")]
    as_of: Option<u64>,
}

impl Snapshot for IndexCopy {
    /// One body: every row an upsert, from an emptied list.
    fn snapshot_bodies(&self) -> String {
        let body = IndexBody {
            reset: true,
            upserts: self.rows.values().collect(),
            removed: &[],
            as_of: self.as_of,
        };
        serde_json::to_string(&[body]).unwrap_or_else(|_| "[]".into())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FeedBody<'a> {
    from_index: usize,
    items: &'a [Value],
    end_index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    base: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    before: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fp: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prefix_fp: Option<&'a str>,
}

impl Snapshot for FeedCopy {
    /// Nothing for a copy that never started; else — after a window-start
    /// page where the copy was trimmed — one replace-from body holding every
    /// item, with the last body's marks.
    fn snapshot_bodies(&self) -> String {
        if !self.started {
            return "[]".into();
        }
        let marks = (self.fp.as_deref(), self.prefix_fp.as_deref());
        let mut bodies = Vec::with_capacity(2);
        if let Some(base) = self.base {
            bodies.push(FeedBody {
                from_index: self.start,
                items: &[],
                end_index: self.start,
                base: Some(base),
                before: self.before.as_deref(),
                fp: marks.0,
                prefix_fp: marks.1,
            });
        }
        bodies.push(FeedBody {
            from_index: self.start,
            items: &self.items,
            end_index: self.end,
            base: None,
            before: None,
            fp: marks.0,
            prefix_fp: marks.1,
        });
        serde_json::to_string(&bodies).unwrap_or_else(|_| "[]".into())
    }
}

impl IndexCopy {
    /// Fold one index body in.
    pub fn apply(&mut self, body: &Value) {
        if body["reset"] == true {
            self.rows.clear();
        }
        for key in body["removed"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            self.rows.remove(key);
        }
        for row in body["upserts"].as_array().into_iter().flatten() {
            if let (Some(source), Some(id)) = (row["source"].as_str(), row["id"].as_str()) {
                self.rows.insert(format!("{source}/{id}"), row.clone());
            }
        }
        if let Some(t) = body["asOf"].as_u64() {
            self.as_of = Some(t);
        }
    }

    /// The stream a session's copy is under (relay-and-plans §5): named under
    /// the epoch its entry gives (`se`), or — an entry without one — the
    /// first epoch held. `None`: not in the index, or no key for that epoch.
    pub fn session_stream(&self, keys: &[(u32, [u8; 32])], source: &str, id: &str) -> Option<StreamId> {
        let row = self.rows.get(&format!("{source}/{id}"))?;
        let epoch = match row.get("se") {
            Some(se) => u32::try_from(se.as_u64()?).ok()?,
            None => keys.iter().map(|(e, _)| *e).min()?,
        };
        let mk = keys.iter().find(|(e, _)| *e == epoch)?.1;
        rh_proto::record::stream_id(&mk, source, id).ok()
    }
}

impl FeedCopy {
    /// Fold one session body in: replace-from, the first body (or a
    /// window-start page) saying where the copy starts.
    pub fn apply(&mut self, body: &Value) -> Result<()> {
        let (Some(from), Some(end), Some(items)) = (
            body["fromIndex"].as_u64().map(|v| v as usize),
            body["endIndex"].as_u64().map(|v| v as usize),
            body["items"].as_array(),
        ) else {
            return Err("a session body has fromIndex, items and endIndex".into());
        };
        if end != from + items.len() {
            return Err("a session body's items do not run from fromIndex to endIndex".into());
        }
        // The body's marks, as `feedFold` takes them: a non-empty string, else none.
        let mark = |name: &str| body[name].as_str().filter(|s| !s.is_empty()).map(str::to_string);
        let (fp, prefix_fp) = (mark("fp"), mark("prefixFp"));
        if let (Some(base), true) = (body["base"].as_u64(), items.is_empty()) {
            // A window-start page: the copy starts here.
            *self = FeedCopy {
                started: true,
                start: from,
                end: from,
                items: Vec::new(),
                base: Some(base as usize),
                before: body["before"].as_str().map(str::to_string),
                fp,
                prefix_fp,
            };
            return Ok(());
        }
        if self.started && from > self.start && from > self.end {
            return Err(format!("a session body starts at {from}, past the {} items held", self.end));
        }
        self.fp = fp;
        self.prefix_fp = prefix_fp;
        if self.base.is_some_and(|b| from < b) {
            self.base = None;
            self.before = None;
        }
        if !self.started || from <= self.start {
            self.started = true;
            self.start = from;
            self.end = end;
            self.items = items.clone();
            return Ok(());
        }
        self.items.truncate(from - self.start);
        self.items.extend(items.iter().cloned());
        self.end = end;
        Ok(())
    }
}

/// The body of format `fmt`, parsed: a body of another format, or one that
/// is not JSON, is an error (the stream is not what its name says).
pub fn json_body(fmt: u16, body: &(u16, Vec<u8>)) -> Result<Value> {
    if body.0 != fmt {
        return Err(format!("a body of format {} on a stream of format {fmt}", body.0));
    }
    serde_json::from_slice(&body.1).map_err(|e| format!("a body that is not JSON: {e}"))
}

/// A machine's index as a device reading epoch names finds it: under the
/// newest epoch held, then older ones (a machine that has not moved its
/// streams yet — off since a rotation). The epoch it was under (`None`:
/// none has one), and the copy.
pub async fn read_index(
    link: &Link,
    pages: Pages<'_>,
    machine_key: [u8; 32],
    keys: &[(u32, [u8; 32])],
    timeout: Duration,
) -> Result<(Option<u32>, StreamCopy)> {
    let mut newest_first = keys.to_vec();
    newest_first.sort_by_key(|k| std::cmp::Reverse(k.0));
    let mut last = StreamCopy::default();
    for (epoch, mk) in &newest_first {
        let copy = read_stream(link, pages, machine_key, keys, rh_proto::stream::index_stream(mk), timeout).await?;
        if copy.exists {
            return Ok((Some(*epoch), copy));
        }
        last = copy;
    }
    Ok((None, last))
}

impl StreamCopy {
    /// The bodies of `fmt`, parsed.
    fn json_bodies(&self, fmt: u16) -> Result<Vec<Value>> {
        self.bodies.iter().map(|b| json_body(fmt, b)).collect()
    }

    /// Fold an index stream.
    pub fn index(&self) -> Result<IndexCopy> {
        let mut copy = IndexCopy::default();
        for body in self.json_bodies(rh_proto::record::FMT_INDEX_V1)? {
            copy.apply(&body);
        }
        Ok(copy)
    }

    /// Fold a session stream ([`FeedCopy::apply`]).
    pub fn feed(&self) -> Result<FeedCopy> {
        let mut copy = FeedCopy::default();
        for body in self.json_bodies(rh_proto::record::FMT_FEED_V1)? {
            copy.apply(&body)?;
        }
        Ok(copy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A one-shot HTTP server answering `head` then `body` (raw), its URL.
    async fn serve_once(head: String, body: Vec<u8>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = socket.read(&mut buf).await;
            let _ = socket.write_all(head.as_bytes()).await;
            let _ = socket.write_all(&body).await;
            let _ = socket.shutdown().await;
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn a_page_larger_than_any_page_is_refused_as_it_streams_in() {
        let http = reqwest::Client::new();
        let get = |url: String| {
            let http = http.clone();
            async move {
                let hub = HubUrl::parse(&url).unwrap();
                Pages {
                    http: &http,
                    hub: &hub,
                    token: "t",
                }
                .fetch(&StreamId([1; 32]), &[2; 32])
                .await
            }
        };
        // Within the cap: read.
        let ok = serve_once(
            "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\n".into(),
            b"hello".to_vec(),
        )
        .await;
        assert_eq!(get(ok).await.unwrap(), b"hello");
        // A length over the cap: refused before the body.
        let said = serve_once(
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", MAX_PAGE_LEN + 1),
            Vec::new(),
        )
        .await;
        assert!(get(said).await.unwrap_err().contains("larger than"));
        // No length, a body that does not stop: refused once past the cap.
        let mut chunked = Vec::new();
        for _ in 0..(MAX_PAGE_LEN / 65_536 + 2) {
            chunked.extend_from_slice(format!("{:x}\r\n", 65_536).as_bytes());
            chunked.extend(std::iter::repeat_n(b'x', 65_536));
            chunked.extend_from_slice(b"\r\n");
        }
        chunked.extend_from_slice(b"0\r\n\r\n");
        let endless = serve_once(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".into(),
            chunked,
        )
        .await;
        assert!(get(endless).await.unwrap_err().contains("larger than"));
    }
    use rh_proto::record::{FMT_FEED_V1, FMT_INDEX_V1};

    fn copy(fmt: u16, bodies: &[Value]) -> StreamCopy {
        StreamCopy {
            exists: true,
            bodies: bodies.iter().map(|b| (fmt, serde_json::to_vec(b).unwrap())).collect(),
            ..StreamCopy::default()
        }
    }

    fn items(from: usize, to: usize) -> Vec<Value> {
        (from..to).map(|i| json!({"kind": "user", "text": format!("item {i}")})).collect()
    }

    #[test]
    fn the_index_fold_takes_a_lone_as_of_for_the_time_only() {
        let c = copy(
            FMT_INDEX_V1,
            &[
                json!({"reset": true, "upserts": [{"source": "claude", "id": "a"}, {"source": "claude", "id": "b"}], "removed": [], "asOf": 1}),
                json!({"upserts": [], "removed": ["claude/a"], "asOf": 2}),
                json!({"asOf": 86_400_002u64}),
            ],
        );
        let index = c.index().unwrap();
        assert_eq!(index.rows.keys().collect::<Vec<_>>(), vec!["claude/b"]);
        assert_eq!(index.as_of, Some(86_400_002));
        assert!(copy(FMT_FEED_V1, &[json!({})]).index().is_err(), "another stream's bodies");
    }

    #[test]
    fn a_session_stream_is_named_under_its_entrys_epoch_or_else_the_first() {
        let keys = [(0, [1u8; 32]), (3, [2u8; 32])];
        let index = copy(
            FMT_INDEX_V1,
            &[json!({"upserts": [
                {"source": "claude", "id": "old"},
                {"source": "claude", "id": "moved", "se": 3},
                {"source": "claude", "id": "lost", "se": 7},
            ], "removed": []})],
        )
        .index()
        .unwrap();
        let id = |mk: &[u8; 32], name: &str| rh_proto::record::stream_id(mk, "claude", name).unwrap();
        assert_eq!(index.session_stream(&keys, "claude", "old"), Some(id(&[1; 32], "old")));
        assert_eq!(index.session_stream(&keys, "claude", "moved"), Some(id(&[2; 32], "moved")));
        assert_eq!(index.session_stream(&keys, "claude", "lost"), None, "no key for epoch 7");
        assert_eq!(index.session_stream(&keys, "claude", "absent"), None);
        assert_ne!(id(&[1; 32], "moved"), id(&[2; 32], "moved"), "the old key does not name the moved stream");
    }

    #[test]
    fn the_session_fold_starts_a_trimmed_copy_at_its_window_page() {
        let c = copy(
            FMT_FEED_V1,
            &[
                json!({"fromIndex": 14, "items": [], "endIndex": 14, "fp": "f", "prefixFp": "p", "base": 14, "before": "2026-10-01T08:00:00.000Z"}),
                json!({"fromIndex": 14, "items": items(14, 21), "endIndex": 21}),
                json!({"fromIndex": 21, "items": items(21, 23), "endIndex": 23}),
                json!({"fromIndex": 22, "items": items(22, 24), "endIndex": 24}),
            ],
        );
        let feed = c.feed().unwrap();
        assert_eq!((feed.start, feed.end, feed.items.len()), (14, 24, 10));
        assert_eq!((feed.base, feed.before.as_deref()), (Some(14), Some("2026-10-01T08:00:00.000Z")));
        assert_eq!(feed.items[0]["text"], "item 14");
        // An untrimmed copy has no base, whatever it starts at.
        let plain = copy(FMT_FEED_V1, &[json!({"fromIndex": 0, "items": items(0, 3), "endIndex": 3})])
            .feed()
            .unwrap();
        assert_eq!((plain.start, plain.end, plain.base), (0, 3, None));
        assert!(copy(
            FMT_FEED_V1,
            &[
                json!({"fromIndex": 0, "items": items(0, 1), "endIndex": 1}),
                json!({"fromIndex": 5, "items": items(5, 6), "endIndex": 6})
            ]
        )
        .feed()
        .is_err());
    }

    /// S1-7: a copy's snapshot folds, from nothing, into the same copy — marks,
    /// window and all — so the local gateway can hand a page what it holds
    /// without the stream.
    #[test]
    fn a_snapshot_folds_back_into_the_copy_it_was_taken_of() {
        let feed = copy(
            FMT_FEED_V1,
            &[
                json!({"fromIndex": 14, "items": [], "endIndex": 14, "fp": "f0", "prefixFp": "p0", "base": 14, "before": "2026-10-01T08:00:00.000Z"}),
                json!({"fromIndex": 14, "items": items(14, 21), "endIndex": 21, "fp": "f1", "prefixFp": "p1"}),
                json!({"fromIndex": 20, "items": items(20, 24), "endIndex": 24, "fp": "f2", "prefixFp": "p2"}),
            ],
        )
        .feed()
        .unwrap();
        assert_eq!((feed.fp.as_deref(), feed.prefix_fp.as_deref()), (Some("f2"), Some("p2")));
        let bodies: Vec<Value> = serde_json::from_str(&feed.snapshot_bodies()).unwrap();
        assert_eq!(bodies.len(), 2, "the window page, then every item");
        assert_eq!(copy(FMT_FEED_V1, &bodies).feed().unwrap(), feed);
        // Untrimmed, and never started.
        let plain = copy(FMT_FEED_V1, &[json!({"fromIndex": 0, "items": items(0, 3), "endIndex": 3, "fp": "x"})])
            .feed()
            .unwrap();
        let bodies: Vec<Value> = serde_json::from_str(&plain.snapshot_bodies()).unwrap();
        assert_eq!(bodies.len(), 1);
        assert_eq!(copy(FMT_FEED_V1, &bodies).feed().unwrap(), plain);
        assert_eq!(FeedCopy::default().snapshot_bodies(), "[]");
        // A body that does not follow leaves the marks as they were.
        let mut held = plain.clone();
        assert!(held
            .apply(&json!({"fromIndex": 9, "items": items(9, 10), "endIndex": 10, "fp": "y"}))
            .is_err());
        assert_eq!(held, plain);
        // The index: every row, the time.
        let index = copy(
            FMT_INDEX_V1,
            &[
                json!({"reset": true, "upserts": [{"source": "claude", "id": "a", "asking": true}, {"source": "codex", "id": "b"}], "removed": [], "asOf": 7}),
                json!({"upserts": [{"source": "claude", "id": "c"}], "removed": ["codex/b"]}),
            ],
        )
        .index()
        .unwrap();
        let bodies: Vec<Value> = serde_json::from_str(&index.snapshot_bodies()).unwrap();
        assert_eq!(bodies[0]["reset"], true);
        assert_eq!(copy(FMT_INDEX_V1, &bodies).index().unwrap(), index);
        // Folded on top of anything, the reset starts it over.
        let mut over = copy(FMT_INDEX_V1, &[json!({"upserts": [{"source": "claude", "id": "z"}]})])
            .index()
            .unwrap();
        over.apply(&bodies[0]);
        assert_eq!(over, index);
    }
}
