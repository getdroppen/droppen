//! The local gateway, `GET /api/agg/ws` (local-aggregator §6.3, S1-7): the
//! other devices' copies, for this computer's own pages — the local app, a
//! phone on the LAN — which hold no key and never reach the hub.
//!
//! One WebSocket per page, JSON text frames tagged `t`:
//!
//! | from | frame |
//! |---|---|
//! | here | `dir {machines: [{id, name, state, sinceMs, copyAsOfMs, os, agentVersion}]}` — on connecting, then the whole list again on every change |
//! | page | `sub {s, machine, name: {kind: index\|status} \| {kind: session, source, id}, from?, fromEpoch?}`, `unsub {s}`, `ping` |
//! | here | `copy {s, reset?, bodies, headSeq, epoch, headTimeMs}`; `missing {s}`; `copy_error {s, code, message}` (`no_device`, `bad_name`, `too_large`, `gone`, `overrun`, `limit`, `busy`) |
//! | here | `refused {p, code: unsupported}` for `open` (live pipes come with S2-2); `bye {code, message}` before closing when the page may no longer see other devices; `pong` |
//!
//! Everything comes from memory ([`super::live`]); nothing here reads the
//! disk or writes anywhere: read-only, in every sense.
//!
//! **Who**: [`crate::api::cloud::remote_viewer`] — the rule `GET /api/cloud`'s
//! `copy` follows (security review of S1-5, F8): the local account that
//! signed this device in, and those it allowed; and the login behind the
//! socket still signed in — before the upgrade, again once upgraded, then
//! every [`RECHECK`] and whenever `cloud.json` changes; a page that loses the
//! right is told (`bye`) and closed.
//!
//! **Bounded**: [`MAX_SUBS`] subscriptions, asked for at most [`SUB_BURST`]
//! at once and [`SUB_PER_SECOND`] after (`busy`); an outbox of [`OUTBOX`]
//! frames and [`OUTBOX_BYTES`] (a subscription that does not fit is
//! `overrun`); a frame that takes over [`SEND_WITHIN`] to go out ends the
//! connection, and a shutdown does not wait for one. A device bridged from Droppen
//! (`cloud:*`) never gets here (the bridge refuses `/api/agg/*`, and so does
//! the rule).
//!
//! Nothing it logs carries content: the local account, device ids, kinds,
//! counts.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use gotcha::axum::extract::ws::{CloseFrame, Message, WebSocket};
use gotcha::axum::response::{IntoResponse, Response};
use gotcha::{State, WebSocketUpgrade};
use serde_json::{json, Value};

use super::live::{error_frame, DirHost, Key, LiveCopies, Outbox, Subscription};
use super::store::Kind;
use crate::api::{ApiRouter, AppState};
use crate::auth::MaybeIdentity;

/// How often a connection looks again at whether its page may still see other devices.
pub const RECHECK: Duration = Duration::from_secs(2);
/// Subscriptions one connection may hold.
pub const MAX_SUBS: usize = 512;
/// Frames waiting to go out on one connection before a subscription is overrun.
pub const OUTBOX: usize = 256;
/// Bytes waiting to go out on one connection before a subscription is overrun.
pub const OUTBOX_BYTES: usize = 32 * 1024 * 1024;
/// How long one frame may take to go out before the page is let go of (a page
/// that stops reading must not hold up the rechecks).
pub const SEND_WITHIN: Duration = Duration::from_secs(10);
/// Subscriptions a page may ask for in a burst.
pub const SUB_BURST: f64 = 256.0;
/// Subscriptions a page may ask for each second, after a burst.
pub const SUB_PER_SECOND: f64 = 32.0;
/// The largest frame a page may send.
const MAX_INCOMING: usize = 64 * 1024;

/// Register the gateway's route.
pub fn routes(router: ApiRouter) -> ApiRouter {
    router.get("/api/agg/ws", agg_ws)
}

/// The gateway's socket. Deliberately not annotated with `#[api]` (a
/// websocket carries no OpenAPI operation; see `feed::global::events_ws`).
/// Refused before the upgrade unless the caller may see other devices; the
/// changes to `cloud.json` are listened to from before that check, so one
/// landing in between is not missed.
pub async fn agg_ws(State(state): State<AppState>, who: MaybeIdentity, upgrade: WebSocketUpgrade) -> Response {
    let changes = state.cloud.changes();
    let user = match crate::api::cloud::remote_viewer(&state, &who).await {
        Ok(user) => user,
        Err(refusal) => return refusal.into_response(),
    };
    upgrade
        .max_message_size(MAX_INCOMING)
        .on_upgrade(move |socket| run(socket, state, user, who, changes))
}

/// What a page asks for.
#[derive(Debug, PartialEq, Eq)]
enum Ask {
    Sub {
        s: u32,
        key: Key,
        /// Where the page's copy stands: `(epoch, seq)`.
        from: Option<(u32, u64)>,
    },
    Unsub {
        s: u32,
    },
    Open {
        p: Value,
    },
    Ping,
    /// A `sub` that cannot be one: `copy_error` with this code.
    Bad {
        s: u32,
        code: &'static str,
    },
    Ignore,
}

/// A session's name part: non-empty, short, printable, no `/` (it is joined
/// as `source/id`).
fn name_part(v: &Value, max: usize) -> Option<&str> {
    let text = v.as_str()?;
    let fine = !text.is_empty() && text.len() <= max && !text.contains('/') && !text.chars().any(char::is_control);
    fine.then_some(text)
}

fn parse(text: &str) -> Ask {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return Ask::Ignore;
    };
    let s = v["s"].as_u64().and_then(|s| u32::try_from(s).ok());
    match v["t"].as_str() {
        Some("ping") => Ask::Ping,
        Some("open") => Ask::Open { p: v["p"].clone() },
        Some("unsub") => s.map_or(Ask::Ignore, |s| Ask::Unsub { s }),
        Some("sub") => {
            let Some(s) = s else { return Ask::Ignore };
            let Some(machine) = v["machine"].as_str().filter(|m| !m.is_empty() && m.len() <= 64) else {
                return Ask::Bad { s, code: "no_device" };
            };
            let name = &v["name"];
            let (kind, name) = match name["kind"].as_str() {
                Some("index") => (Kind::Index, String::new()),
                Some("status") => (Kind::Status, String::new()),
                Some("session") => match (name_part(&name["source"], 32), name_part(&name["id"], 200)) {
                    (Some(source), Some(id)) => (Kind::Session, format!("{source}/{id}")),
                    _ => return Ask::Bad { s, code: "bad_name" },
                },
                _ => return Ask::Bad { s, code: "bad_name" },
            };
            let from = match (v["fromEpoch"].as_u64().and_then(|e| u32::try_from(e).ok()), v["from"].as_u64()) {
                (Some(epoch), Some(seq)) => Some((epoch, seq)),
                _ => None,
            };
            Ask::Sub {
                s,
                key: Key::new(machine, kind, &name),
                from,
            }
        }
        _ => Ask::Ignore,
    }
}

fn dir_frame(dir: &[DirHost]) -> String {
    json!({"t": "dir", "machines": dir}).to_string()
}

fn bad_message(code: &str) -> &'static str {
    match code {
        "no_device" => "that device is not one this device shows",
        "bad_name" => "that is not a copy a device keeps",
        "limit" => "too many copies followed on one connection",
        "busy" => "too many subscriptions at once: ask again in a moment",
        "overrun" => "the page fell behind: subscribe again",
        _ => "that cannot be followed",
    }
}

/// A token bucket for a page's subscriptions.
struct Bucket {
    tokens: f64,
    at: tokio::time::Instant,
}

impl Bucket {
    fn new() -> Self {
        Self {
            tokens: SUB_BURST,
            at: tokio::time::Instant::now(),
        }
    }

    fn take(&mut self) -> bool {
        let now = tokio::time::Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.at).as_secs_f64() * SUB_PER_SECOND).min(SUB_BURST);
        self.at = now;
        if self.tokens < 1.0 {
            return false;
        }
        self.tokens -= 1.0;
        true
    }
}

type Sink = futures_util::stream::SplitSink<WebSocket, Message>;

/// Send one frame, within [`SEND_WITHIN`] and not past a shutdown
/// ([`crate::api::send_within`]): `false` when it could not go.
async fn send(sink: &mut Sink, message: Message, shutdown: &tokio_util::sync::CancellationToken) -> bool {
    crate::api::send_within(sink, message, SEND_WITHIN, shutdown).await
}

/// Serve one page until it leaves, the server stops, or it may no longer see
/// other devices — checked again at once, every [`RECHECK`], and on every
/// change of `cloud.json`, with the login (`who`) that opened it.
async fn run(socket: WebSocket, state: AppState, user: String, who: MaybeIdentity, mut changes: tokio::sync::watch::Receiver<u64>) {
    let live: Arc<LiveCopies> = state.cloud.control.live().clone();
    let (mut sink, mut stream) = socket.split();
    let (out, mut queued) = Outbox::new(OUTBOX, OUTBOX_BYTES);
    let mut dir = live.dir();
    let mut subs: HashMap<u32, Subscription> = HashMap::new();
    let mut bucket = Bucket::new();
    let mut recheck = tokio::time::interval_at(tokio::time::Instant::now() + RECHECK, RECHECK);
    recheck.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut heartbeat = crate::api::Heartbeat::default();
    tracing::info!(%user, "gateway: a page connected");
    let mut bye: Option<(u16, String, String)> = None;
    // A change between the check before the upgrade and here is not missed.
    if let Err(refusal) = crate::api::cloud::remote_viewer(&state, &who).await {
        bye = Some(closing(&refusal));
    } else {
        // Out of the directory's borrow before anything else (sending it may wait).
        let first = {
            let now = dir.borrow_and_update().clone();
            dir_frame(&now)
        };
        if send(&mut sink, Message::Text(first.into()), &state.cancel).await {
            loop {
                tokio::select! {
                    Some(text) = queued.recv() => {
                        out.taken(text.len());
                        if !send(&mut sink, Message::Text(text.into()), &state.cancel).await {
                            break;
                        }
                    }
                    () = out.overrun() => {
                        // Ended by the registry (their device left, too large): let go of them here too.
                        for s in out.take_ended() {
                            subs.remove(&s);
                        }
                        let mut gone = false;
                        for s in out.take_overrun() {
                            subs.remove(&s);
                            if !send(&mut sink, Message::Text(error_frame(s, "overrun", bad_message("overrun")).into()), &state.cancel).await {
                                gone = true;
                                break;
                            }
                        }
                        if gone {
                            break;
                        }
                    }
                    changed = dir.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        // The list is taken out of the watch before any subscription is
                        // let go of: letting go takes the registry's lock, which the
                        // registry holds while it sends a new list (a deadlock otherwise).
                        let now = dir.borrow_and_update().clone();
                        subs.retain(|_, sub| now.iter().any(|h| h.id == sub.key().host));
                        if !send(&mut sink, Message::Text(dir_frame(&now).into()), &state.cancel).await {
                            break;
                        }
                    }
                    _ = recheck.tick() => {
                        if let Err(refusal) = crate::api::cloud::remote_viewer(&state, &who).await {
                            bye = Some(closing(&refusal));
                            break;
                        }
                    }
                    changed = changes.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        if let Err(refusal) = crate::api::cloud::remote_viewer(&state, &who).await {
                            bye = Some(closing(&refusal));
                            break;
                        }
                    }
                    incoming = stream.next() => {
                        heartbeat.saw_frame();
                        let text = match incoming {
                            None | Some(Ok(Message::Close(_))) | Some(Err(_)) => break,
                            Some(Ok(Message::Text(text))) => text,
                            Some(Ok(_)) => continue,
                        };
                        if let Some(reply) = handle(parse(text.as_str()), &live, &out, &mut subs, &mut bucket) {
                            if !send(&mut sink, Message::Text(reply.into()), &state.cancel).await {
                                break;
                            }
                        }
                    }
                    alive = heartbeat.tick() => {
                        if !alive || !send(&mut sink, Message::Ping(Default::default()), &state.cancel).await {
                            break;
                        }
                    }
                    () = state.cancel.cancelled() => break,
                }
            }
        }
    }
    let held = subs.len();
    drop(subs);
    if let Some((code, error, message)) = bye {
        tracing::info!(%user, %error, "gateway: the page may no longer see other devices; closing it");
        let _ = send(
            &mut sink,
            Message::Text(json!({"t": "bye", "code": error, "message": message}).to_string().into()),
            &state.cancel,
        )
        .await;
        let _ = send(&mut sink, Message::Close(Some(CloseFrame { code, reason: error.into() })), &state.cancel).await;
    }
    crate::api::close_within(&mut sink, SEND_WITHIN, &state.cancel).await;
    tracing::info!(%user, held, "gateway: a page left");
}

/// The close code and the words for a refusal that ends a connection.
fn closing(refusal: &crate::api::error::ApiError) -> (u16, String, String) {
    let body = refusal.body();
    let error = body["error"].as_str().unwrap_or("forbidden").to_string();
    let message = body["message"].as_str().unwrap_or_default().to_string();
    let code = match refusal.status().as_u16() {
        409 => 4409,
        401 => 4401,
        _ => 4403,
    };
    (code, error, message)
}

/// Act on what a page asked: the frame to answer with at once, if any (a
/// copy itself, and `missing`, go through the outbox, in order).
fn handle(ask: Ask, live: &Arc<LiveCopies>, out: &Arc<Outbox>, subs: &mut HashMap<u32, Subscription>, bucket: &mut Bucket) -> Option<String> {
    match ask {
        Ask::Ping => Some(r#"{"t":"pong"}"#.into()),
        Ask::Ignore => None,
        Ask::Open { p } => Some(
            json!({"t": "refused", "p": p, "code": "unsupported", "message": "this device shows other devices' copies only, read-only; using them live comes later"})
                .to_string(),
        ),
        Ask::Unsub { s } => {
            subs.remove(&s);
            None
        }
        Ask::Bad { s, code } => {
            subs.remove(&s);
            Some(error_frame(s, code, bad_message(code)))
        }
        Ask::Sub { s, key, from } => {
            // The same `s` again replaces what it was.
            subs.remove(&s);
            if !bucket.take() {
                return Some(error_frame(s, "busy", bad_message("busy")));
            }
            if subs.len() >= MAX_SUBS {
                return Some(error_frame(s, "limit", bad_message("limit")));
            }
            if !live.has_host(&key.host) {
                return Some(error_frame(s, "no_device", bad_message("no_device")));
            }
            // A session its device's list does not name: no interest (nothing to follow).
            let listed = if key.kind == Kind::Session { live.index_names(&key.host, &key.name) } else { None };
            tracing::debug!(host = %key.host, kind = key.kind.as_str(), "gateway: a page follows a copy");
            match live.subscribe(key, s, out, from, listed != Some(false)) {
                Ok(sub) => {
                    subs.insert(s, sub);
                    None
                }
                Err(ended) => Some(error_frame(s, ended.code(), ended_message(ended))),
            }
        }
    }
}

fn ended_message(ended: super::live::Ended) -> &'static str {
    match ended {
        super::live::Ended::Gone => "that device is not one this device shows",
        super::live::Ended::TooLarge => "this session's copy is too large to keep on this device",
    }
}

#[cfg(test)]
mod tests {
    use futures_util::SinkExt;

    use super::*;

    #[test]
    fn a_page_asks_in_a_few_shapes_and_nothing_else() {
        let sub = parse(r#"{"t":"sub","s":3,"machine":"m-2","name":{"kind":"session","source":"claude","id":"abc"},"from":9,"fromEpoch":2}"#);
        assert_eq!(
            sub,
            Ask::Sub {
                s: 3,
                key: Key::new("m-2", Kind::Session, "claude/abc"),
                from: Some((2, 9))
            }
        );
        // A seq without its epoch says nothing (a page's copy is a snapshot again).
        assert!(matches!(
            parse(r#"{"t":"sub","s":3,"machine":"m-2","name":{"kind":"index"},"from":9}"#),
            Ask::Sub { from: None, .. }
        ));
        assert_eq!(
            parse(r#"{"t":"sub","s":1,"machine":"m-2","name":{"kind":"index"}}"#),
            Ask::Sub {
                s: 1,
                key: Key::new("m-2", Kind::Index, ""),
                from: None
            }
        );
        assert_eq!(
            parse(r#"{"t":"sub","s":1,"machine":"m-2","name":{"kind":"keys"}}"#),
            Ask::Bad { s: 1, code: "bad_name" }
        );
        assert_eq!(
            parse(r#"{"t":"sub","s":1,"machine":"m-2","name":{"kind":"session","source":"claude","id":"../x/y"}}"#),
            Ask::Bad { s: 1, code: "bad_name" }
        );
        assert_eq!(
            parse(r#"{"t":"sub","s":1,"name":{"kind":"index"}}"#),
            Ask::Bad { s: 1, code: "no_device" }
        );
        assert_eq!(parse(r#"{"t":"sub","s":-1,"machine":"m-2","name":{"kind":"index"}}"#), Ask::Ignore);
        assert_eq!(parse(r#"{"t":"unsub","s":4}"#), Ask::Unsub { s: 4 });
        assert_eq!(parse(r#"{"t":"ping"}"#), Ask::Ping);
        assert!(matches!(parse(r#"{"t":"open","p":2,"machine":"m-2","kind":"rpc"}"#), Ask::Open { .. }));
        assert_eq!(parse("not json"), Ask::Ignore);
        assert_eq!(parse(r#"{"t":"write","s":1}"#), Ask::Ignore);
    }

    #[tokio::test]
    async fn a_page_follows_only_devices_shown_within_its_limit_and_cannot_open_anything() {
        let live = LiveCopies::new();
        live.set_hosts(&["m-2".into()]);
        let (out, _rx) = Outbox::new(16, OUTBOX_BYTES);
        let mut subs = HashMap::new();
        let mut bucket = Bucket::new();
        let reply = handle(
            Ask::Sub {
                s: 1,
                key: Key::new("m-9", Kind::Index, ""),
                from: None,
            },
            &live,
            &out,
            &mut subs,
            &mut bucket,
        )
        .unwrap();
        assert!(reply.contains(r#""code":"no_device""#));
        assert!(handle(
            Ask::Sub {
                s: 1,
                key: Key::new("m-2", Kind::Index, ""),
                from: None
            },
            &live,
            &out,
            &mut subs,
            &mut bucket
        )
        .is_none());
        assert_eq!(subs.len(), 1);
        for s in 2..=MAX_SUBS as u32 {
            // (Asked for slowly enough: see the next test.)
            handle(
                Ask::Sub {
                    s,
                    key: Key::new("m-2", Kind::Status, ""),
                    from: None,
                },
                &live,
                &out,
                &mut subs,
                &mut Bucket::new(),
            );
        }
        assert_eq!(subs.len(), MAX_SUBS);
        let over = handle(
            Ask::Sub {
                s: 9999,
                key: Key::new("m-2", Kind::Status, ""),
                from: None,
            },
            &live,
            &out,
            &mut subs,
            &mut bucket,
        )
        .unwrap();
        assert!(over.contains(r#""code":"limit""#));
        handle(Ask::Unsub { s: 1 }, &live, &out, &mut subs, &mut bucket);
        assert_eq!(live.subscribers(&Key::new("m-2", Kind::Index, "")), 0);
        let open = handle(Ask::Open { p: json!(7) }, &live, &out, &mut subs, &mut bucket).unwrap();
        assert!(open.contains(r#""code":"unsupported""#) && open.contains(r#""p":7"#));
    }

    /// Review of S1-7: a page in a loop of `sub` is slowed down (`busy`), not
    /// served as fast as it asks.
    #[tokio::test(start_paused = true)]
    async fn a_page_asking_too_fast_is_told_to_wait() {
        let live = LiveCopies::new();
        live.set_hosts(&["m-2".into()]);
        let (out, _rx) = Outbox::new(16, OUTBOX_BYTES);
        let mut subs = HashMap::new();
        let mut bucket = Bucket::new();
        let ask = |bucket: &mut Bucket, subs: &mut HashMap<u32, Subscription>| {
            handle(
                Ask::Sub {
                    s: 1,
                    key: Key::new("m-2", Kind::Status, ""),
                    from: None,
                },
                &live,
                &out,
                subs,
                bucket,
            )
        };
        for _ in 0..SUB_BURST as usize {
            assert_eq!(ask(&mut bucket, &mut subs), None);
        }
        let busy = ask(&mut bucket, &mut subs).expect("refused");
        assert!(busy.contains(r#""code":"busy""#), "{busy}");
        assert!(subs.is_empty(), "the refused one replaced nothing it kept");
        tokio::time::advance(Duration::from_secs(1)).await;
        for _ in 0..SUB_PER_SECOND as usize {
            assert_eq!(ask(&mut bucket, &mut subs), None);
        }
        assert!(ask(&mut bucket, &mut subs).expect("refused").contains("busy"));
    }

    /// Review of S1-7: a session its device's index does not name is
    /// `missing` at once, through the outbox (after what went before), and
    /// is not followed for the page.
    #[tokio::test]
    async fn a_session_the_index_does_not_name_is_missing_and_not_followed() {
        let live = LiveCopies::new();
        live.set_hosts(&["m-2".into()]);
        let mut index = rh_client::streams::IndexCopy::default();
        index.rows.insert("claude/q".into(), json!({"source": "claude", "id": "q"}));
        let slot: super::super::live::Slot = Arc::new(std::sync::Mutex::new(super::super::live::Published::new(
            super::super::copies::Fold::Index(index),
            1,
            1,
        )));
        live.publish(&Key::new("m-2", Kind::Index, ""), 0, &slot);
        let (out, mut rx) = Outbox::new(16, OUTBOX_BYTES);
        let mut subs = HashMap::new();
        let mut bucket = Bucket::new();
        let sub = |s: u32, id: &str| Ask::Sub {
            s,
            key: Key::new("m-2", Kind::Session, &format!("claude/{id}")),
            from: None,
        };
        assert_eq!(handle(sub(1, "nope"), &live, &out, &mut subs, &mut bucket), None);
        assert_eq!(rx.try_recv().unwrap(), r#"{"t":"missing","s":1}"#);
        assert!(live.opened().is_empty(), "nothing is followed for it");
        assert_eq!(handle(sub(2, "q"), &live, &out, &mut subs, &mut bucket), None);
        assert_eq!(live.opened(), vec![("m-2".to_string(), "claude/q".to_string())]);
    }

    /// The gateway end to end, in process: refused with no local account,
    /// to a local account that may not see other devices, and closed (`bye`)
    /// the moment the owner takes that back; to the owner, the directory and
    /// the copies as they change.
    #[tokio::test]
    async fn the_gateway_serves_only_the_local_accounts_that_may_see_other_devices() {
        use tokio_tungstenite::tungstenite::{self, Message as WsMessage};
        type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
        async fn next(ws: &mut Ws) -> Value {
            match tokio::time::timeout(Duration::from_secs(5), ws.next()).await.expect("a frame in time") {
                Some(Ok(WsMessage::Text(t))) => serde_json::from_str::<Value>(t.as_str()).unwrap(),
                Some(Ok(WsMessage::Close(frame))) => json!({"closed": frame.map(|f| u16::from(f.code))}),
                other => panic!("{other:?}"),
            }
        }
        let home = tempfile::tempdir().unwrap();
        let (state, addr) = crate::api::test_support::served(home.path()).await;
        let connect = |ticket: Option<String>| async move {
            let url = match ticket {
                Some(t) => format!("ws://{addr}/api/agg/ws?ticket={t}"),
                None => format!("ws://{addr}/api/agg/ws"),
            };
            tokio_tungstenite::connect_async(url).await.map(|(ws, _)| ws)
        };
        let status_of = |r: Result<Ws, tungstenite::Error>| match r {
            Err(tungstenite::Error::Http(res)) => res.status().as_u16(),
            Err(other) => panic!("not refused over HTTP: {other}"),
            Ok(_) => panic!("let in"),
        };
        // No local account and no session: the setup state, and nobody gets
        // in for connecting from this device.
        assert_eq!(status_of(connect(None).await), 401);
        let auth = state.drive.auth.clone();
        for user in ["alice", "bob"] {
            auth.users.add(user, "correct horse battery staple").await.unwrap();
        }
        // A login and the identity it gives, and a `ws` ticket minted by it (as `POST /api/auth/ticket` does).
        let login = |user: &'static str| {
            let auth = auth.clone();
            async move {
                let digest = crate::auth::sessions::digest_of(&auth.sessions.issue(user, None).await.unwrap().token);
                let who = MaybeIdentity(Some(crate::auth::AuthIdentity {
                    username: user.into(),
                    token_sha256: Some(digest.clone()),
                }));
                (digest, who)
            }
        };
        let ticket_for = |user: &str, digest: &str| Some(auth.tickets.issue(user, digest, crate::auth::tickets::TicketScope::Ws).unwrap().0);
        let (alice_login, alice) = login("alice").await;
        let (bob_login, _) = login("bob").await;
        let ticket = |user: &str| ticket_for(user, if user == "alice" { &alice_login } else { &bob_login });
        // alice signed this device in: she sees the other devices, and says who else may.
        let created: std::collections::BTreeMap<String, String> = auth.users.list().await.into_iter().map(|u| (u.name, u.created_at)).collect();
        std::fs::write(
            crate::api::cloud::file_path(&state.paths),
            serde_json::json!({"owner": "alice", "aggregate": true, "linkedAt": 1, "accountsSince": {"alice": created["alice"]}}).to_string(),
        )
        .unwrap();
        // A ticket whose login is not one (nothing live to check it against): refused.
        let unbound = Some(
            auth.tickets
                .issue("alice", "no-such-login", crate::auth::tickets::TicketScope::Ws)
                .unwrap()
                .0,
        );
        assert_eq!(status_of(connect(unbound).await), 401);
        // Without a ticket, with auth on: nobody.
        assert_eq!(status_of(connect(None).await), 401);
        let mut a = connect(ticket("alice")).await.expect("alice is let in");
        let dir = next(&mut a).await;
        assert_eq!((dir["t"].as_str(), dir["machines"].as_array().map(Vec::len)), (Some("dir"), Some(0)));
        // A device appears, with its index: the directory, then the copy.
        let live = state.cloud.control.live().clone();
        live.set_hosts(&["m-2".into()]);
        let dir = next(&mut a).await;
        assert_eq!(dir["machines"][0]["id"], "m-2");
        let mut index = rh_client::streams::IndexCopy::default();
        index
            .rows
            .insert("claude/q".into(), json!({"source": "claude", "id": "q", "asking": true}));
        let slot: super::super::live::Slot = std::sync::Arc::new(std::sync::Mutex::new(super::super::live::Published::new(
            super::super::copies::Fold::Index(index),
            4,
            1_700_000_000_000,
        )));
        live.publish(&Key::new("m-2", Kind::Index, ""), 0, &slot);
        let dir = next(&mut a).await;
        assert_eq!(dir["machines"][0]["copyAsOfMs"], 1_700_000_000_000u64);
        a.send(WsMessage::Text(r#"{"t":"sub","s":1,"machine":"m-2","name":{"kind":"index"}}"#.into()))
            .await
            .unwrap();
        let copy = next(&mut a).await;
        assert_eq!((copy["t"].as_str(), copy["reset"].as_bool()), (Some("copy"), Some(true)));
        assert_eq!(copy["bodies"][0]["upserts"][0]["asking"], true);
        a.send(WsMessage::Text(
            r#"{"t":"open","p":1,"machine":"m-2","kind":"rpc","path":"/api/sessions"}"#.into(),
        ))
        .await
        .unwrap();
        assert_eq!(next(&mut a).await["code"], "unsupported");
        // bob may not see other devices.
        assert_eq!(status_of(connect(ticket("bob")).await), 403);
        // alice lets him, then takes it back: he is told, and closed.
        let set = |remote: bool| {
            crate::api::cloud::put_access(
                State(state.clone()),
                alice.clone(),
                crate::api::extract::ApiJson(crate::api::cloud::AccessRequest { user: "bob".into(), remote }),
            )
        };
        let _ = set(true).await.unwrap();
        let mut b = connect(ticket("bob")).await.expect("bob is let in now");
        assert_eq!(next(&mut b).await["machines"][0]["id"], "m-2");
        let t0 = std::time::Instant::now();
        let _ = set(false).await.unwrap();
        let bye = next(&mut b).await;
        assert_eq!((bye["t"].as_str(), bye["code"].as_str()), (Some("bye"), Some("no_remote_access")));
        assert_eq!(next(&mut b).await["closed"], 4403);
        assert!(t0.elapsed() < Duration::from_secs(1), "closed {:?} after", t0.elapsed());
        // alice stays.
        a.send(WsMessage::Text(r#"{"t":"ping"}"#.into())).await.unwrap();
        assert_eq!(next(&mut a).await["t"], "pong");
        // Review of S1-7: a page opened with a login that is then signed out
        // is closed with it.
        let (digest, _) = login("alice").await;
        let mut signed_in = connect(ticket_for("alice", &digest)).await.expect("alice's login is let in");
        assert_eq!(next(&mut signed_in).await["t"], "dir");
        assert!(auth.sessions.revoke(&digest).await);
        let bye = next(&mut signed_in).await;
        assert_eq!((bye["t"].as_str(), bye["code"].as_str()), (Some("bye"), Some("unauthorized")));
        assert_eq!(next(&mut signed_in).await["closed"], 4401);
        // The page opened by a login still signed in stays.
        a.send(WsMessage::Text(r#"{"t":"ping"}"#.into())).await.unwrap();
        assert_eq!(next(&mut a).await["t"], "pong");
        // No owner on file (a device signed in with the CLI): nobody sees the
        // other devices, not even who may manage the link, until one claims it.
        std::fs::write(
            crate::api::cloud::file_path(&state.paths),
            serde_json::json!({"aggregate": true, "linkedAt": 1}).to_string(),
        )
        .unwrap();
        // (Written behind the server's back: the next recheck sees it.)
        let bye = next(&mut a).await;
        assert_eq!((bye["t"].as_str(), bye["code"].as_str()), (Some("bye"), Some("no_remote_access")));
        assert_eq!(status_of(connect(ticket("alice")).await), 403);
        // A file that does not read: nobody either.
        std::fs::write(crate::api::cloud::file_path(&state.paths), "{not json").unwrap();
        assert_eq!(status_of(connect(ticket("alice")).await), 403);
    }
}
