//! Opt-in capture of this relay's control link to the Hub (`/v1/relay`), for
//! the contract check in `docs/hub-api/` (`node docs/hub-api/check.mjs
//! --traffic <file>`).
//!
//! Off unless `DROPPEN_HUB_API_TRAFFIC=<path>` is set. Then every JSON message on
//! the link is appended to that file as one line, `{"src": "relay", "kind":
//! "ws", "socket": "/v1/relay", "dir": "in"|"out", "body"}`. What is written
//! already crosses the wire to the Hub (a registration token among it), so
//! the file is created `0600`; delete it after.

use std::io::Write;
use std::sync::{Mutex, OnceLock};

use serde::Serialize;
use serde_json::Value;

fn file() -> Option<&'static Mutex<std::fs::File>> {
    static FILE: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();
    FILE.get_or_init(|| {
        let path = crate::config::env("DROPPEN_HUB_API_TRAFFIC")?;
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        options.open(path).ok().map(Mutex::new)
    })
    .as_ref()
}

/// One line of the capture, in the shape `check.mjs --traffic` reads.
#[derive(Serialize)]
struct Line<'a> {
    src: &'static str,
    kind: &'static str,
    socket: &'static str,
    dir: &'a str,
    body: Value,
}

fn record(dir: &str, text: &str) {
    let Some(file) = file() else { return };
    let Ok(body) = serde_json::from_str::<Value>(text) else { return };
    let line = Line {
        src: "relay",
        kind: "ws",
        socket: "/v1/relay",
        dir,
        body,
    };
    let Ok(mut line) = serde_json::to_string(&line) else { return };
    line.push('\n');
    if let Ok(mut file) = file.lock() {
        let _ = file.write_all(line.as_bytes());
    }
}

/// A message to the Hub, as sent.
pub fn sent(text: String) -> String {
    record("out", &text);
    text
}

/// A message from the Hub, as received.
pub fn got(text: &str) -> &str {
    record("in", text);
    text
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    /// The only test in this crate that touches the capture: the file is
    /// opened once per process, from the variable set here.
    #[test]
    fn a_captured_line_is_the_message_and_its_direction() {
        let path = std::env::temp_dir().join(format!("rh-relay-traffic-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);
        std::env::set_var("DROPPEN_HUB_API_TRAFFIC", &path);
        assert_eq!(super::sent(r#"{"t":"health","conns":1}"#.into()), r#"{"t":"health","conns":1}"#);
        assert_eq!(super::got(r#"{"t":"keys","ticket_keys":[]}"#), r#"{"t":"keys","ticket_keys":[]}"#);
        super::sent("not json".into());
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let lines: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(
            lines,
            [
                json!({"src": "relay", "kind": "ws", "socket": "/v1/relay", "dir": "out", "body": {"t": "health", "conns": 1}}),
                json!({"src": "relay", "kind": "ws", "socket": "/v1/relay", "dir": "in", "body": {"t": "keys", "ticket_keys": []}}),
            ]
        );
    }
}
