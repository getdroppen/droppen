//! Opt-in capture of the agent's control messages to the hub and to relays,
//! for the contract check in `docs/hub-api/` (`node docs/hub-api/check.mjs
//! --traffic <file>`).
//!
//! Off unless `DROPPEN_HUB_API_TRAFFIC=<path>` is set. Then every JSON control
//! message on the hub's `/v1/agent` and on a relay's `/v1/connect` is
//! appended to that file as one line, `{"src": "agent", "kind": "ws",
//! "socket", "dir": "in"|"out", "body"}` — and, through `rh-client`'s tap
//! ([`install_client_tap`]), every JSON request and answer of its device
//! role over HTTP (S1-4) as `{"src": "agent", "kind": "http", "dir",
//! "method", "path", "query"?, "status"?, "body"}` and its `/v1/client`
//! control messages, a device session's token redacted. Binary frames never are. What is
//! written is what already crosses the wire to the hub or a relay (tickets
//! and signatures among it), so the file is created `0600`; delete it after.

use std::io::Write;
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

/// The hub's agent socket.
pub const AGENT: &str = rh_proto::frame::AGENT_PATH;
/// A relay's socket.
pub const CONNECT: &str = "/v1/connect";

fn file() -> Option<&'static Mutex<std::fs::File>> {
    static FILE: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();
    FILE.get_or_init(|| {
        let path = crate::env::var("DROPPEN_HUB_API_TRAFFIC")?;
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        match options.open(&path) {
            Ok(file) => Some(Mutex::new(file)),
            Err(error) => {
                tracing::warn!(%error, "DROPPEN_HUB_API_TRAFFIC: cannot open the capture file");
                None
            }
        }
    })
    .as_ref()
}

/// Append one capture record (`rh_client::tap::line`'s) as a line, under the
/// file's lock.
fn write(record: Value) {
    let Some(file) = file() else { return };
    let mut line = record.to_string();
    line.push('\n');
    if let Ok(mut file) = file.lock() {
        let _ = file.write_all(line.as_bytes());
    }
}

fn record(socket: &str, dir: &str, body: &Value) {
    write(rh_client::tap::line(
        "agent",
        rh_client::tap::Event::Ws {
            socket,
            dir,
            body,
        },
    ));
}

fn record_text(socket: &str, dir: &str, text: &str) {
    if file().is_some() {
        if let Ok(body) = serde_json::from_str::<Value>(text) {
            record(socket, dir, &body);
        }
    }
}

/// A control message to the hub, as sent.
pub fn agent_out(text: String) -> String {
    record_text(AGENT, "out", &text);
    text
}

/// A control message from the hub, as received.
pub fn agent_in(text: &str) -> &str {
    record_text(AGENT, "in", text);
    text
}

/// A control message to the hub, before it is sent.
pub fn agent_value(value: &Value) {
    if file().is_some() {
        record(AGENT, "out", value);
    }
}

/// A control message to a relay, as sent.
pub fn relay_out(text: String) -> String {
    record_text(CONNECT, "out", &text);
    text
}

/// A control message from a relay, as received.
pub fn relay_in(text: &str) -> &str {
    record_text(CONNECT, "in", text);
    text
}

/// A control message to a relay, before it is sent.
pub fn relay_value(value: Value) -> Value {
    if file().is_some() {
        record(CONNECT, "out", &value);
    }
    value
}

fn http(dir: &str, method: &str, url: &str, status: Option<u16>, body: &Value) {
    write(rh_client::tap::line(
        "agent",
        rh_client::tap::Event::Http {
            dir,
            method,
            url,
            status,
            body,
        },
    ));
}

/// A copy of `value` with what is not the capture's to keep replaced,
/// wherever it is in the body: a device session's bearer token (the hub's
/// answer that mints one, `token`, and the `/v1/client` login that presents
/// one, `session`), a known-client token (`known_client`, `kc1.…` in `GET
/// /v1/me`), and the account's e-mail. The capture is for checking shapes;
/// those are credentials and personal data (security reviews of S1-4, N4,
/// and S1-5, F9).
pub fn redacted(value: &Value) -> Value {
    redact(value, false)
}

/// [`redacted`]; `in_email`: inside an `email` object (`GET /v1/me`'s
/// `{address, verified, …}`), whose `address` is the e-mail.
fn redact(value: &Value, in_email: bool) -> Value {
    const SECRET: [&str; 4] = ["token", "session", "known_client", "email"];
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| {
                    let secret = SECRET.contains(&k.as_str()) || (in_email && k == "address");
                    let v = if secret && v.is_string() {
                        Value::String("[redacted]".into())
                    } else {
                        redact(v, k == "email")
                    };
                    (k.clone(), v)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(|v| redact(v, false)).collect()),
        other => other.clone(),
    }
}

/// What `rh-client` sends and gets for this computer's device role (S1-4,
/// S1-5), captured as the agent's own: `"src": "agent"`, tokens redacted.
struct DeviceRole;

impl rh_client::tap::Tap for DeviceRole {
    fn record(&self, event: rh_client::tap::Event<'_>) {
        if file().is_none() {
            return;
        }
        match event {
            rh_client::tap::Event::Http {
                dir,
                method,
                url,
                status,
                body,
            } => http(dir, method, url, status, &redacted(body)),
            rh_client::tap::Event::Ws { socket, dir, body } => record(socket, dir, &redacted(body)),
        }
    }
}

/// Route `rh-client`'s capture here (once per process; again is a no-op).
/// Called before this computer's device role first reaches the hub, so a
/// device session never lands in the capture file under `rh-client`'s own
/// default.
pub fn install_client_tap() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        rh_client::tap::set(Box::new(DeviceRole));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_device_session_token_never_reaches_the_traffic_capture() {
        let answer = json!({"account": "a-1", "device": "m-1", "token": "secret-bearer", "expires_ms": 5});
        let kept = redacted(&answer);
        assert_eq!(kept["token"], "[redacted]");
        assert_eq!(kept["expires_ms"], 5);
        assert!(!kept.to_string().contains("secret-bearer"));
        let login = json!({"t": "login", "role": "device", "session": "secret-bearer", "sig": "00"});
        let kept = redacted(&login);
        assert_eq!(kept["session"], "[redacted]");
        assert_eq!(kept["sig"], "00");
        assert!(!kept.to_string().contains("secret-bearer"));
        assert_eq!(redacted(&json!({"mail": []})), json!({"mail": []}));
        // F9: `GET /v1/me`'s known-client token and the account's e-mail, wherever they are.
        let me = json!({"account": "a-1", "email": {"address": "someone@example.com", "verified": true}, "known_client": "kc1.secret", "devices": [{"id": "d-1", "known_client": "kc1.other", "email": "other@example.com"}], "relay": {"address": "kept"}});
        let kept = redacted(&me);
        let text = kept.to_string();
        assert!(!text.contains("someone@example.com") && !text.contains("kc1."), "{text}");
        assert_eq!(kept["account"], "a-1");
        assert_eq!(kept["devices"][0]["id"], "d-1");
        assert_eq!(kept["email"]["verified"], true, "the shape stays");
        assert_eq!(kept["relay"]["address"], "kept", "an address that is not an e-mail stays");
    }
}
