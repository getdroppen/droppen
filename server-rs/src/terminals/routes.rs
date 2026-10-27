//! The terminal REST surface and the websocket the browser's xterm attaches to.
//!
//! The four documented routes are the REST surface. The socket is on the same
//! router but has no `#[api]` ([`terminal_ws`] says why), so it is not in the
//! spec.
//!
//! The wire protocol, as `web/src/features/terminals/xterm-pane.tsx` reads
//! and writes it:
//!
//! * **server -> client**: text frames of raw pty output, nothing wrapped around
//!   them (`ws.onmessage` does `if (typeof e.data === "string") term.write(e.data)`,
//!   `xterm-pane.tsx`). The first frame after a successful attach is the
//!   replay buffer, sent whether or not there is anything in it; later frames
//!   are the 16 ms-coalesced chunks. An unknown id gets
//!   `"\r\n[terminal not found]\r\n"` and the socket closes.
//!   Notes are the only thing the server writes into that stream itself, and
//!   they are plain text like everything around them: the not-found one above,
//!   plus `[terminal unreachable: reconnecting]` and `[terminal reconnected]`
//!   for a terminal that exists but whose link to its shim is down.
//! * **client -> server**: JSON text frames, `{"type":"input","data":"…"}` and
//!   `{"type":"resize","cols":N,"rows":N}`. Anything else — malformed JSON, an
//!   unknown `type`, a wrongly typed field — is dropped in silence.

use futures_util::{SinkExt, StreamExt};
use gotcha::axum::extract::ws::{Message, WebSocket};
use gotcha::axum::response::Response;
use gotcha::{api, Json, State, WebSocketUpgrade};
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::api::extract::{ApiJson, ApiPath};
use crate::api::{send_within, ApiRouter, AppState, SendResult, WsSender as Sender};
use crate::openapi_ext::ExtraResponses;

use super::{Chunk, TerminalCreate, TerminalError, TerminalInfo, TerminalList, TerminalRename, SLOW_NOTE};

/// What a client that asked for a terminal that is gone sees instead of output.
const NOT_FOUND_NOTE: &str = "\r\n[terminal not found]\r\n";
/// What a client attached to a terminal the server cannot currently reach sees.
///
/// The terminal is not gone — the shell is alive and the replay above this note
/// is real — but the link to its shim is down, so what is on screen is frozen at
/// whenever it last worked and nothing typed will be acted on until it is back.
const UNREACHABLE_NOTE: &str = "\r\n[terminal unreachable: reconnecting]\r\n";
/// What that same client sees when the link returns, so the note above stops
/// being displayed at about the moment it stops being true.
const RELINKED_NOTE: &str = "\r\n[terminal reconnected]\r\n";

/// Every terminal whose process is still running.
#[api(id = "list_terminals", group = "terminals", summary = "List terminals")]
pub async fn list_terminals(State(state): State<AppState>) -> Json<TerminalList> {
    Json(TerminalList {
        terminals: state.terminals.list(),
    })
}

/// Start a terminal running the user's login shell.
#[api(id = "create_terminal", group = "terminals", summary = "Create a terminal")]
pub async fn create_terminal(State(state): State<AppState>, ApiJson(body): ApiJson<TerminalCreate>) -> Result<Json<TerminalInfo>, ApiError> {
    let cwd = match body.cwd.as_deref() {
        // An empty string means "no cwd".
        Some(cwd) if !cwd.is_empty() => Some(state.terminals.resolve_safe(cwd).map_err(bad_request)?),
        _ => None,
    };
    let info = state.terminals.create_terminal(cwd, body.title.0).await.map_err(create_error)?;
    Ok(Json(info))
}

/// Give a terminal a new display name.
#[api(id = "rename_terminal", group = "terminals", summary = "Rename a terminal")]
pub async fn rename_terminal(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiJson(body): ApiJson<TerminalRename>,
) -> Result<Json<TerminalInfo>, ApiError> {
    state
        .terminals
        .rename_terminal(&id, &body.title)
        .await
        .map(Json)
        .ok_or_else(|| ApiError::not_found("terminal not found"))
}

/// Kill a terminal's process.
#[api(id = "kill_terminal", group = "terminals", summary = "Kill a terminal")]
pub async fn kill_terminal(State(state): State<AppState>, ApiPath(id): ApiPath<String>) -> Result<Json<SendResult>, ApiError> {
    if !state.terminals.kill_terminal(&id) {
        return Err(ApiError::not_found("terminal not found"));
    }
    Ok(Json(SendResult { accepted: true }))
}

/// The attach socket. Deliberately **not** annotated with `#[api]`, for the same
/// reason [`crate::api::session_feed_ws`] is not: `WebSocketUpgrade` is not a
/// `ParameterProvider`.
///
/// A socket opened by a login closes with 4401 once that login no longer
/// stands ([`crate::auth::Auth::watch_login`]): a shell is the last thing to leave
/// open for a login that was signed out.
pub async fn terminal_ws(
    State(state): State<AppState>,
    who: crate::auth::MaybeIdentity,
    ApiPath(id): ApiPath<String>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let login = who.login();
    upgrade.on_upgrade(move |socket| run_terminal_socket(socket, state, id, login))
}

/// One control frame from the browser.
///
/// `#[serde(untagged)]`-style leniency is deliberate: an unknown `type`, a
/// missing field or a wrongly typed one must be ignored rather than closing the
/// socket.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum Control {
    #[serde(rename = "input")]
    Input { data: String },
    #[serde(rename = "resize")]
    Resize { cols: f64, rows: f64 },
}

async fn run_terminal_socket(socket: WebSocket, state: AppState, id: String, login: Option<(String, String)>) {
    let (mut sender, mut receiver) = socket.split();
    let ended = state.drive.auth.clone().watch_login(login);
    tokio::pin!(ended);

    let Some(mut attachment) = state.terminals.attach(&id) else {
        let _ = send_within(&mut sender, Message::Text(NOT_FOUND_NOTE.into()), state.send_limit, &state.cancel).await;
        close_gracefully(sender, receiver, CLOSE_GRACE, &state.cancel).await;
        return;
    };
    // Unconditional, empty replay and all. The client arms a screen reset on
    // re-attach and disarms it on the first frame it receives
    // (`web/src/features/terminals/xterm-pane.tsx`), so without a first frame a
    // terminal with nothing buffered would leave that reset armed indefinitely —
    // until some keystroke minutes later produced the first frame and wiped the
    // screen the user was looking at. One frame on every attach makes the reset
    // fire when the attach happens, which is the only moment it means anything.
    if !send_within(&mut sender, Message::Text(attachment.replay.clone().into()), state.send_limit, &state.cancel).await {
        return;
    }
    // A terminal whose link is down still lists, still attaches and still hands
    // out a full replay, so the pane it paints looks exactly like a working one.
    // A screen frozen at the last adoption, with the server saying nothing about
    // why, is what the user sees. It is said here, after the replay, so the user
    // gets the frozen screen *and* the reason for it.
    //
    // Subscribed before the health is read, not after: a link that came back in
    // between would otherwise leave this client holding a note about a problem
    // that is already over, with no change left to wake it.
    let mut link_changes = attachment.link_changes();
    let mut told_unreachable = !attachment.is_linked();
    if told_unreachable && !send_within(&mut sender, Message::Text(UNREACHABLE_NOTE.into()), state.send_limit, &state.cancel).await {
        return;
    }

    // Output stops when the terminal finishes, but the socket does not: closing
    // would send the client into its reconnect loop; left open, `[process
    // exited]` stays the last thing on screen.
    let mut live = true;
    let mut heartbeat = crate::api::Heartbeat::default();
    loop {
        tokio::select! {
            () = &mut ended => {
                let _ = send_within(&mut sender, Message::Close(Some(crate::auth::signed_out_frame())), state.send_limit, &state.cancel).await;
                break;
            }
            chunk = attachment.next_chunk(), if live => match chunk {
                Some(Chunk::Data(chunk)) => {
                    if !send_within(&mut sender, Message::Text(chunk.as_ref().into()), state.send_limit, &state.cancel).await {
                        break;
                    }
                }
                // This client stopped draining its queue. Say why and go: the
                // pty reader must not be held up by one browser tab, and the
                // fan-out cannot drop chunks without corrupting the stream.
                Some(Chunk::TooSlow) => {
                    let _ = send_within(&mut sender, Message::Text(SLOW_NOTE.into()), state.send_limit, &state.cancel).await;
                    break;
                }
                None => live = false,
            },
            incoming = receiver.next() => {
                heartbeat.saw_frame();
                let unreachable = match incoming {
                    Some(Ok(Message::Text(text))) => handle_control(&attachment, &text),
                    Some(Ok(Message::Binary(bytes))) => {
                        // A binary frame is accepted too, as long as it is UTF-8.
                        std::str::from_utf8(&bytes).is_ok_and(|text| handle_control(&attachment, text))
                    }
                    Some(Ok(_)) => false,
                    None | Some(Err(_)) => break,
                };
                // One note per attachment for as long as the link stays down,
                // not one per keystroke: someone typing into a terminal that
                // cannot hear them is owed an explanation, not thirty of them.
                if unreachable && !told_unreachable {
                    told_unreachable = true;
                    if !send_within(&mut sender, Message::Text(UNREACHABLE_NOTE.into()), state.send_limit, &state.cancel).await {
                        break;
                    }
                }
            }
            // Only ever polled for a client that has been told the terminal is
            // unreachable; a healthy attach never selects on this at all.
            Ok(()) = link_changes.changed(), if told_unreachable => {
                // A change is not necessarily good news: `relink` marks the link
                // down on the way into its backoff too.
                if attachment.is_linked() {
                    told_unreachable = false;
                    if !send_within(&mut sender, Message::Text(RELINKED_NOTE.into()), state.send_limit, &state.cancel).await {
                        break;
                    }
                }
            }
            alive = heartbeat.tick() => {
                if !alive || !send_within(&mut sender, Message::Ping(Default::default()), state.send_limit, &state.cancel).await {
                    break;
                }
            }
            _ = state.cancel.cancelled() => break,
        }
    }
    // Nothing more is sent to this client, so it stops collecting output now
    // rather than for as long as the goodbye takes.
    drop(attachment);
    close_gracefully(sender, receiver, CLOSE_GRACE, &state.cancel).await;
}

/// How long a closing socket waits for the client to answer its close.
///
/// Long enough for a client that is still reading to get through a full
/// queue (`LISTENER_QUEUE_CAP`) and the kernel's buffers on a loaded machine:
/// with 10 s, a test client reading at a measured pace on two busy CPUs ran
/// out of it once in seven runs.
const CLOSE_GRACE: std::time::Duration = std::time::Duration::from_secs(30);

/// Send a close frame, and keep reading until the client answers it.
///
/// Dropping the socket right after the close frame could lose the end of the
/// stream. If the client sent anything that had not been read yet (a pong, a
/// keystroke), or sends something afterwards, the kernel answers with a reset
/// and discards whatever it had not transmitted. For a client cut off for
/// being too slow, that is most of its backlog and the note saying why. The
/// client answers the close only after reading everything sent before it, so
/// waiting for that answer means waiting until the stream has arrived. The
/// wait is bounded, because a client that has stopped reading may never
/// answer, and a server on its way down does not wait at all.
///
/// Sending the close frame is inside both bounds too: a client that has
/// stopped reading leaves no room for it, and would otherwise hold the task
/// right here. Biased toward the goodbye, so on shutdown a close frame that
/// can go at once still goes.
async fn close_gracefully(
    mut sender: Sender,
    mut receiver: futures_util::stream::SplitStream<WebSocket>,
    grace: std::time::Duration,
    shutdown: &tokio_util::sync::CancellationToken,
) {
    let goodbye = async {
        let _ = sender.close().await;
        while let Some(Ok(message)) = receiver.next().await {
            if matches!(message, Message::Close(_)) {
                break;
            }
        }
    };
    tokio::select! {
        biased;
        _ = tokio::time::timeout(grace, goodbye) => {}
        () = shutdown.cancelled() => {}
    }
}

/// Apply one control frame, and say whether it went nowhere because the link to
/// the terminal's shim is down.
///
/// Only input can answer `true`. A resize that is dropped costs the user nothing
/// they would notice, and noting it would fire on the size every pane sends the
/// instant it attaches — turning one honest note into two.
fn handle_control(attachment: &super::Attachment, text: &str) -> bool {
    match serde_json::from_str::<Control>(text) {
        Ok(Control::Input { data }) => !attachment.write(&data),
        Ok(Control::Resize { cols, rows }) => {
            // Refused before the cast; `as u16` saturates, so
            // [`Attachment::resize`]'s `0 < n < 1000` guard would refuse these
            // too. A fractional size is truncated.
            if cols.is_finite() && rows.is_finite() && (0.0..1000.0).contains(&cols) && (0.0..1000.0).contains(&rows) {
                attachment.resize(cols as u16, rows as u16);
            }
            false
        }
        Err(_) => false,
    }
}

fn bad_request(error: TerminalError) -> ApiError {
    ApiError::bad_request(error.to_string())
}

/// A refused create. Everything is the client's request being wrong — except a
/// full registry, which is the server saying "later".
fn create_error(error: TerminalError) -> ApiError {
    match error {
        TerminalError::TooMany => ApiError::new(gotcha::axum::http::StatusCode::TOO_MANY_REQUESTS, error.to_string()),
        other => bad_request(other),
    }
}

/// Everything gotcha cannot derive about these operations.
pub fn describe(reg: &mut ExtraResponses) {
    reg.describe("GET", "/api/terminals", 200, "Live terminals")
        .describe("POST", "/api/terminals", 200, "Terminal created; attach via /api/terminals/{id}/ws")
        .error("POST", "/api/terminals", 400, "Invalid working directory")
        .error("POST", "/api/terminals", 429, "Too many terminals")
        .describe("PATCH", "/api/terminals/{id}", 200, "Terminal renamed")
        .error("PATCH", "/api/terminals/{id}", 404, "Terminal not found")
        .describe("DELETE", "/api/terminals/{id}", 200, "Terminal killed")
        .error("DELETE", "/api/terminals/{id}", 404, "Terminal not found");
}

/// Register every terminal route.
pub fn routes(router: ApiRouter) -> ApiRouter {
    router
        .get("/api/terminals", list_terminals)
        .post("/api/terminals", create_terminal)
        .patch("/api/terminals/{id}", rename_terminal)
        .delete("/api/terminals/{id}", kill_terminal)
        .get("/api/terminals/{id}/ws", terminal_ws)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use gotcha::axum::extract::ws::WebSocketUpgrade as Upgrade;
    use tokio::sync::oneshot;
    use tokio_util::sync::CancellationToken;

    /// A client that stops reading altogether cannot hold its socket task: a
    /// send it never makes room for gives up at its limit, and a server
    /// shutting down does not wait for the close frame to get through.
    ///
    /// Real sockets on both ends, and a client that never reads a byte, so the
    /// kernel's buffers fill and the server's sends meet a shut window.
    #[tokio::test]
    async fn a_client_that_never_reads_cannot_hold_its_socket_task() {
        let shutdown = CancellationToken::new();
        let (gave_up_tx, gave_up) = oneshot::channel::<()>();
        let (closed_tx, closed) = oneshot::channel::<()>();
        let slot = std::sync::Arc::new(std::sync::Mutex::new(Some((gave_up_tx, closed_tx))));
        let app = gotcha::axum::Router::new().route(
            "/ws",
            gotcha::axum::routing::get({
                let shutdown = shutdown.clone();
                move |upgrade: Upgrade| {
                    let (gave_up_tx, closed_tx) = slot.lock().expect("slot").take().expect("one client");
                    let shutdown = shutdown.clone();
                    async move {
                        upgrade.on_upgrade(move |socket| async move {
                            let (mut sender, receiver) = socket.split();
                            let chunk = "x".repeat(64 * 1024);
                            let limit = Duration::from_millis(500);
                            while send_within(&mut sender, Message::Text(chunk.clone().into()), limit, &shutdown).await {}
                            let _ = gave_up_tx.send(());
                            close_gracefully(sender, receiver, CLOSE_GRACE, &shutdown).await;
                            let _ = closed_tx.send(());
                        })
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move { gotcha::axum::serve(listener, app).await });

        // Connected, and then never read from again.
        let (client, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await.expect("connect");

        tokio::time::timeout(Duration::from_secs(30), gave_up)
            .await
            .expect("a send nobody makes room for gives up at its limit")
            .expect("the socket task reported");
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(5), closed)
            .await
            .expect("a shutdown does not wait for a close frame that cannot get through")
            .expect("the socket task reported");
        drop(client);
    }
}
