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

use serde_json::{json, Value};

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

/// The capture file: `DROPPEN_HUB_API_TRAFFIC`, else its name before the
/// rename to Droppen, `RH_HUB_API_TRAFFIC`. The rule is the one every
/// Droppen binary reads a renamed variable by: trimmed, an empty value
/// unset, and the old name — or both names set to different values, when
/// the new one wins — said once on stderr, by name. The devkit's own capture
/// reads its path here too.
pub fn traffic_path() -> Option<std::path::PathBuf> {
    static SAID: std::sync::Once = std::sync::Once::new();
    let (path, line) = traffic_path_in(|name| std::env::var(name).ok());
    if let Some(line) = line {
        SAID.call_once(|| eprintln!("warning: {line}"));
    }
    path
}

/// [`traffic_path`] through `read`, and the line to say, if any.
fn traffic_path_in(read: impl Fn(&str) -> Option<String>) -> (Option<std::path::PathBuf>, Option<String>) {
    const NEW: &str = "DROPPEN_HUB_API_TRAFFIC";
    const OLD: &str = "RH_HUB_API_TRAFFIC";
    let get = |name: &str| read(name).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    match (get(NEW), get(OLD)) {
        (None, Some(was)) => (
            Some(was.into()),
            Some(format!("{OLD} is deprecated: rename it {NEW} (the old name is still read, for now)")),
        ),
        (Some(path), Some(was)) if path != was => (
            Some(path.into()),
            Some(format!("{NEW} and the deprecated {OLD} are both set, to different values: {NEW} is used; remove {OLD}")),
        ),
        (path, _) => (path.map(Into::into), None),
    }
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

/// The line the default writes for `event` (also what a caller's own tap may write).
pub fn line(src: &str, event: Event<'_>) -> Value {
    match event {
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
            let mut record = json!({"src": src, "kind": "http", "dir": dir, "method": method, "path": path, "body": body});
            if let Some(query) = query {
                record["query"] = json!(query);
            }
            if let Some(status) = status {
                record["status"] = json!(status);
            }
            record
        }
        Event::Ws { socket, dir, body } => json!({"src": src, "kind": "ws", "socket": socket, "dir": dir, "body": body}),
    }
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

    /// The line's exact bytes: one record, keys sorted (no feature turns
    /// insertion order on), the URL split into path and query. Every writer
    /// of the file — this crate's own, the devkit's, the agent's — builds it
    /// here, so the contract check reads one shape.
    #[test]
    fn a_capture_line_has_the_documented_shape() {
        let body = json!({"t": "welcome", "b": 1});
        assert_eq!(
            line("devkit", Event::Ws { socket: "/v1/client", dir: "out", body: &body }).to_string(),
            r#"{"body":{"b":1,"t":"welcome"},"dir":"out","kind":"ws","socket":"/v1/client","src":"devkit"}"#
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
            r#"{"body":{"b":1,"t":"welcome"},"dir":"in","kind":"http","method":"GET","path":"/v1/me","query":"x=1","src":"agent","status":200}"#
        );
    }
}

#[cfg(test)]
mod traffic_path_tests {
    use super::traffic_path_in;

    fn path_in(vars: &[(&str, &str)]) -> (Option<String>, Option<String>) {
        let vars: std::collections::HashMap<&str, &str> = vars.iter().copied().collect();
        let (path, line) = traffic_path_in(|name| vars.get(name).map(|v| v.to_string()));
        (path.map(|p| p.display().to_string()), line)
    }

    #[test]
    fn the_capture_path_is_read_by_the_rename_rule() {
        assert_eq!(path_in(&[("DROPPEN_HUB_API_TRAFFIC", " /tmp/x.jsonl ")]), (Some("/tmp/x.jsonl".into()), None));
        let (path, line) = path_in(&[("RH_HUB_API_TRAFFIC", "/tmp/old.jsonl")]);
        assert_eq!(path.as_deref(), Some("/tmp/old.jsonl"));
        assert!(line.is_some_and(|l| l.starts_with("RH_HUB_API_TRAFFIC is deprecated") && !l.contains("/tmp")));
        let (path, line) = path_in(&[("DROPPEN_HUB_API_TRAFFIC", "/tmp/new"), ("RH_HUB_API_TRAFFIC", "/tmp/old")]);
        assert_eq!(path.as_deref(), Some("/tmp/new"));
        assert!(line.is_some_and(|l| l.contains("DROPPEN_HUB_API_TRAFFIC is used")));
        assert_eq!(path_in(&[("DROPPEN_HUB_API_TRAFFIC", "/tmp/x"), ("RH_HUB_API_TRAFFIC", "/tmp/x")]), (Some("/tmp/x".into()), None));
        assert_eq!(path_in(&[("DROPPEN_HUB_API_TRAFFIC", ""), ("RH_HUB_API_TRAFFIC", " ")]), (None, None));
    }
}
