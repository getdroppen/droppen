//! The terminal REST surface and the websocket the browser's xterm attaches to.
//!
//! Ports the former TypeScript server's `app.ts` (the four documented routes) and
//! its `index.ts` (the socket, which the TypeScript mounts outside its OpenAPI
//! app and therefore never documents — so neither does this).
//!
//! The wire protocol, as `web/src/features/terminals/xterm-pane.tsx:46-65` reads
//! and writes it:
//!
//! * **server -> client**: text frames of raw pty output, nothing wrapped around
//!   them (`ws.onmessage` does `if (typeof e.data === "string") term.write(e.data)`,
//!   `xterm-pane.tsx:53`). The first frame after a successful attach is the
//!   replay buffer, sent whether or not there is anything in it; later frames
//!   are the 16 ms-coalesced chunks. An unknown id gets
//!   `"\r\n[terminal not found]\r\n"` and the socket closes (`index.ts:29-33`).
//!   Notes are the only thing the server writes into that stream itself, and
//!   they are plain text like everything around them: the not-found one above,
//!   plus — with no TypeScript counterpart, because the TypeScript owned its
//!   ptys in-process and could not lose one this way — `[terminal unreachable:
//!   reconnecting]` and `[terminal reconnected]` for a terminal that exists but
//!   whose link to its shim is down.
//! * **client -> server**: JSON text frames, `{"type":"input","data":"…"}` and
//!   `{"type":"resize","cols":N,"rows":N}`. Anything else — malformed JSON, an
//!   unknown `type`, a wrongly typed field — is dropped in silence
//!   (`index.ts:36-48`).

use futures_util::{SinkExt, StreamExt};
use gotcha::axum::extract::ws::{Message, WebSocket};
use gotcha::axum::response::Response;
use gotcha::{api, Json, State, WebSocketUpgrade};
use serde::Deserialize;

use crate::api::error::ApiError;
use crate::api::extract::{ApiJson, ApiPath};
use crate::api::{ApiRouter, AppState, SendResult};
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
        // An empty string is falsy in the TypeScript, so it means "no cwd".
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
/// `ParameterProvider`, and the TypeScript leaves this route out of the spec too.
///
/// A socket opened by a login closes with 4401 once that login no longer
/// stands ([`crate::auth::login_ended`]): a shell is the last thing to leave
/// open for a login that was signed out.
pub async fn terminal_ws(
    State(state): State<AppState>,
    who: crate::auth::MaybeIdentity,
    ApiPath(id): ApiPath<String>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let login = crate::auth::login_of(&who);
    upgrade.on_upgrade(move |socket| run_terminal_socket(socket, state, id, login))
}

/// One control frame from the browser.
///
/// `#[serde(untagged)]`-style leniency is deliberate: an unknown `type`, a
/// missing field or a wrongly typed one must be ignored rather than closing the
/// socket, which is what the TypeScript's `try { JSON.parse } catch {}` plus its
/// `typeof` guards add up to.
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
    let ended = crate::auth::login_ended(state.drive.auth.clone(), login);
    tokio::pin!(ended);

    let Some(mut attachment) = state.terminals.attach(&id) else {
        let _ = sender.send(Message::Text(NOT_FOUND_NOTE.into())).await;
        let _ = sender.close().await;
        return;
    };
    // Unconditional, empty replay and all. The client arms a screen reset on
    // re-attach and disarms it on the first frame it receives
    // (`web/src/features/terminals/xterm-pane.tsx`), so a terminal with nothing
    // buffered used to leave that reset armed indefinitely — until some
    // keystroke minutes later produced the first frame and wiped the screen the
    // user was looking at. One frame on every attach makes the reset fire when
    // the attach happens, which is the only moment it means anything.
    if sender.send(Message::Text(attachment.replay.clone().into())).await.is_err() {
        return;
    }
    // A terminal whose link is down still lists, still attaches and still hands
    // out a full replay, so the pane it paints looks exactly like a working one.
    // That is what "I can see the terminal but I cannot type into it" was: a
    // screen frozen at the last adoption, with the server saying nothing about
    // why. It is said here, after the replay, so the user gets the frozen screen
    // *and* the reason for it.
    //
    // Subscribed before the health is read, not after: a link that came back in
    // between would otherwise leave this client holding a note about a problem
    // that is already over, with no change left to wake it.
    let mut link_changes = attachment.link_changes();
    let mut told_unreachable = !attachment.is_linked();
    if told_unreachable && sender.send(Message::Text(UNREACHABLE_NOTE.into())).await.is_err() {
        return;
    }

    // Output stops when the terminal finishes, but the socket does not: the
    // TypeScript leaves it open so the client keeps `[process exited]` on screen
    // until it navigates away, and closing here would paint `[disconnected]`
    // over it (`xterm-pane.tsx:56`).
    let mut live = true;
    let mut heartbeat = crate::api::Heartbeat::default();
    loop {
        tokio::select! {
            () = &mut ended => {
                let _ = sender.send(Message::Close(Some(crate::auth::signed_out_frame()))).await;
                break;
            }
            chunk = attachment.next_chunk(), if live => match chunk {
                Some(Chunk::Data(chunk)) => {
                    if sender.send(Message::Text(chunk.as_ref().into())).await.is_err() {
                        break;
                    }
                }
                // This client stopped draining its queue. Say why and go: the
                // pty reader must not be held up by one browser tab, and the
                // fan-out cannot drop chunks without corrupting the stream.
                Some(Chunk::TooSlow) => {
                    let _ = sender.send(Message::Text(SLOW_NOTE.into())).await;
                    break;
                }
                None => live = false,
            },
            incoming = receiver.next() => {
                heartbeat.saw_frame();
                let unreachable = match incoming {
                    Some(Ok(Message::Text(text))) => handle_control(&attachment, &text),
                    Some(Ok(Message::Binary(bytes))) => {
                        // node's `String(evt.data)` accepts a binary frame too.
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
                    if sender.send(Message::Text(UNREACHABLE_NOTE.into())).await.is_err() {
                        break;
                    }
                }
            }
            // Only ever polled for a client that has been told the terminal is
            // unreachable — a healthy attach never selects on this at all, and so
            // sends exactly what it sent before any of this existed.
            Ok(()) = link_changes.changed(), if told_unreachable => {
                // A change is not necessarily good news: `relink` marks the link
                // down on the way into its backoff too.
                if attachment.is_linked() {
                    told_unreachable = false;
                    if sender.send(Message::Text(RELINKED_NOTE.into())).await.is_err() {
                        break;
                    }
                }
            }
            alive = heartbeat.tick() => {
                if !alive || sender.send(Message::Ping(Default::default())).await.is_err() {
                    break;
                }
            }
            _ = state.cancel.cancelled() => break,
        }
    }
    let _ = sender.close().await;
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
            // The TypeScript only checks `typeof === "number"` and then leans on
            // the `0 < n < 1000` guard in `attachTerminal`; a non-integral or
            // out-of-range value simply never reaches `pty.resize`.
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
