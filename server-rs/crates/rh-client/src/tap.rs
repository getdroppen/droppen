//! Opt-in capture of the hub-API traffic this crate exchanges, for the
//! contract check in `docs/hub-api/` (`node docs/hub-api/check.mjs --traffic
//! <file>`).
//!
//! Whoever uses the crate may install a [`Tap`] of its own ([`set`]) — the
//! local install does, so its records say `"src": "agent"` and a device
//! session's token never reaches the file. Without one, and only when
//! `DROPPEN_HUB_API_TRAFFIC=<path>` is set, every JSON body sent to or received
//! from the hub's HTTP routes and every JSON control message on a socket is
//! appended to that file as one line: `{"src": "client", "kind":
//! "http"|"ws", "socket"?, "dir": "in"|"out", "method"?, "path"?, "query"?,
//! "status"?, "body"}` (0600: it holds test tokens; delete it after). Binary
//! frames and binary bodies never are.

use std::io::Write;
use std::sync::{Mutex, OnceLock};

use serde::Serialize;
use serde_json::Value;

/// One thing that crossed the wire.
#[derive(Debug, Clone, Copy)]
pub enum Event<'a> {
    /// A JSON request body (`status: None`) or answer.
    Http {
        dir: &'a str,
        method: &'a str,
        url: &'a str,
        status: Option<u16>,
        body: &'a Value,
    },
    /// A JSON control message on `socket` (`/v1/client`, `/v1/connect`, …).
    Ws { socket: &'a str, dir: &'a str, body: &'a Value },
}

/// Where the capture goes.
pub trait Tap: Send + Sync {
    fn record(&self, event: Event<'_>);
}

static TAP: OnceLock<Box<dyn Tap>> = OnceLock::new();

/// Install `tap` for this process (the first one wins): `false` when one was
/// already there.
pub fn set(tap: Box<dyn Tap>) -> bool {
    TAP.set(tap).is_ok()
}

/// The capture file: `DROPPEN_HUB_API_TRAFFIC`, trimmed, an empty value
/// unset, as every Droppen binary reads its variables. The devkit's own
/// capture reads its path here too.
pub fn traffic_path() -> Option<std::path::PathBuf> {
    traffic_path_in(|name| std::env::var(name).ok())
}

/// [`traffic_path`] through `read`.
fn traffic_path_in(read: impl Fn(&str) -> Option<String>) -> Option<std::path::PathBuf> {
    read("DROPPEN_HUB_API_TRAFFIC").map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).map(Into::into)
}

/// The default: [`traffic_path`]'s file, or nothing.
fn file() -> Option<&'static Mutex<std::fs::File>> {
    static FILE: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();
    FILE.get_or_init(|| {
        let path = traffic_path()?;
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        options.open(path).ok().map(Mutex::new)
    })
    .as_ref()
}

/// The capture line of an HTTP body: `query` and `status` only when there
/// are some.
#[derive(Serialize)]
struct HttpLine<'a> {
    src: &'a str,
    kind: &'static str,
    dir: &'a str,
    method: &'a str,
    path: &'a str,
    body: &'a Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    query: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u16>,
}

/// The capture line of a control message.
#[derive(Serialize)]
struct WsLine<'a> {
    src: &'a str,
    kind: &'static str,
    socket: &'a str,
    dir: &'a str,
    body: &'a Value,
}

/// The line the default writes for `event` (also what a caller's own tap may write).
pub fn line(src: &str, event: Event<'_>) -> Value {
    let line = match event {
        Event::Http {
            dir,
            method,
            url,
            status,
            body,
        } => {
            let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
            let target = after_scheme.find('/').map_or("/", |i| &after_scheme[i..]);
            let (path, query) = target.split_once('?').map_or((target, None), |(p, q)| (p, Some(q)));
            serde_json::to_value(HttpLine {
                src,
                kind: "http",
                dir,
                method,
                path,
                body,
                query,
                status,
            })
        }
        Event::Ws { socket, dir, body } => serde_json::to_value(WsLine {
            src,
            kind: "ws",
            socket,
            dir,
            body,
        }),
    };
    line.expect("a capture line encodes")
}

fn emit(event: Event<'_>) {
    if let Some(tap) = TAP.get() {
        tap.record(event);
        return;
    }
    record("client", event);
}

/// Append one record built by [`line`] to the default capture file, as a
/// line: for a caller that writes the same file but is not this crate's
/// traffic (the headless device says `"src": "devkit"`). Nothing when no
/// capture file is configured.
pub fn record(src: &str, event: Event<'_>) {
    let Some(file) = file() else { return };
    let mut text = line(src, event).to_string();
    text.push('\n');
    if let Ok(mut file) = file.lock() {
        let _ = file.write_all(text.as_bytes());
    }
}

fn active() -> bool {
    TAP.get().is_some() || file().is_some()
}

/// A JSON request body, before it is sent to `url`.
pub fn http_out(method: &str, url: &str, body: &Value) {
    if active() {
        emit(Event::Http {
            dir: "out",
            method,
            url,
            status: None,
            body,
        });
    }
}

/// An answer from `url` (`body`: its JSON, or `null` when it had none).
pub fn http_in(method: &str, url: &str, status: u16, body: &Value) {
    if active() {
        emit(Event::Http {
            dir: "in",
            method,
            url,
            status: Some(status),
            body,
        });
    }
}

/// A control message sent on `socket`, as sent.
pub fn sent(socket: &str, text: String) -> String {
    if active() {
        if let Ok(body) = serde_json::from_str::<Value>(&text) {
            emit(Event::Ws {
                socket,
                dir: "out",
                body: &body,
            });
        }
    }
    text
}

/// A control message received on `socket`.
pub fn got<'a>(socket: &str, text: &'a str) -> &'a str {
    if active() {
        if let Ok(body) = serde_json::from_str::<Value>(text) {
            emit(Event::Ws {
                socket,
                dir: "in",
                body: &body,
            });
        }
    }
    text
}

#[cfg(test)]
mod line_tests {
    use super::{line, Event};
    use serde_json::json;

    /// The line's exact bytes: one record, keys in written order (serde_json's
    /// `preserve_order`), the URL split into path and query. Every writer
    /// of the file — this crate's own, the devkit's, the agent's — builds it
    /// here, so the contract check reads one shape.
    #[test]
    fn a_capture_line_has_the_documented_shape() {
        let body = json!({"t": "welcome", "b": 1});
        assert_eq!(
            line("devkit", Event::Ws { socket: "/v1/client", dir: "out", body: &body }).to_string(),
            r#"{"src":"devkit","kind":"ws","socket":"/v1/client","dir":"out","body":{"t":"welcome","b":1}}"#
        );
        assert_eq!(
            line(
                "agent",
                Event::Http {
                    dir: "in",
                    method: "GET",
                    url: "https://hub.example/v1/me?x=1",
                    status: Some(200),
                    body: &body,
                }
            )
            .to_string(),
            r#"{"src":"agent","kind":"http","dir":"in","method":"GET","path":"/v1/me","body":{"t":"welcome","b":1},"query":"x=1","status":200}"#
        );
    }
}

#[cfg(test)]
mod absent_keys_tests {
    use super::{line, Event};
    use serde_json::json;

    /// A request has no status, and a URL without a query has no query: the
    /// keys are left out, never null.
    #[test]
    fn a_request_line_leaves_out_status_and_query() {
        let body = json!({"account": "a-1"});
        let event = Event::Http {
            dir: "out",
            method: "POST",
            url: "https://hub.example/v1/session/challenge",
            status: None,
            body: &body,
        };
        assert_eq!(
            line("agent", event),
            json!({"src": "agent", "kind": "http", "dir": "out", "method": "POST", "path": "/v1/session/challenge", "body": {"account": "a-1"}})
        );
    }
}

#[cfg(test)]
mod traffic_path_tests {
    use super::traffic_path_in;

    fn path_in(vars: &[(&str, &str)]) -> Option<String> {
        let vars: std::collections::HashMap<&str, &str> = vars.iter().copied().collect();
        traffic_path_in(|name| vars.get(name).map(|v| v.to_string())).map(|p| p.display().to_string())
    }

    #[test]
    fn the_capture_path_is_read_by_its_droppen_name_only() {
        assert_eq!(path_in(&[("DROPPEN_HUB_API_TRAFFIC", " /tmp/x.jsonl ")]), Some("/tmp/x.jsonl".into()));
        assert_eq!(path_in(&[("DROPPEN_HUB_API_TRAFFIC", " ")]), None, "empty is unset");
        let asked = std::cell::RefCell::new(Vec::new());
        traffic_path_in(|name| {
            asked.borrow_mut().push(name.to_string());
            None
        });
        assert_eq!(asked.into_inner(), ["DROPPEN_HUB_API_TRAFFIC"], "no other name, the one from before the rename included");
    }
}
