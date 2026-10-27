//! Opt-in capture of the hub-API traffic the headless device and the echo
//! agent exchange, for the contract check in `docs/hub-api/` (`node
//! docs/hub-api/check.mjs --traffic <file>`).
//!
//! Off unless `DROPPEN_HUB_API_TRAFFIC=<path>` is set. Then every JSON body this
//! crate sends to or gets from the hub's HTTP routes itself, and every JSON
//! control message on the echo agent's `/v1/agent` and `/v1/connect`, is
//! appended to that file as one line (what goes through `rh-client` — its
//! HTTP helpers, `/v1/client`, a device's `/v1/connect` — `rh_client::tap`
//! writes to the same file, as `"src": "client"`): `{"src": "devkit", "kind": "http"|"ws",
//! "socket"?, "dir": "in"|"out", "method"?, "path"?, "status"?, "body"}`.
//! Binary frames and binary bodies never are. What is written already
//! crosses the wire to the hub or a relay (test passwords, tokens and tickets
//! among it), so the file is created `0600`; delete it after.

use serde_json::Value;

fn ws(socket: &str, dir: &str, text: &str) {
    if let Ok(body) = serde_json::from_str::<Value>(text) {
        rh_client::tap::record(
            "devkit",
            rh_client::tap::Event::Ws {
                socket,
                dir,
                body: &body,
            },
        );
    }
}

/// A control message sent on `socket` (`/v1/client`, `/v1/agent`, `/v1/connect`).
pub fn sent(socket: &str, text: String) -> String {
    ws(socket, "out", &text);
    text
}

/// A control message received on `socket`.
pub fn got<'a>(socket: &str, text: &'a str) -> &'a str {
    ws(socket, "in", text);
    text
}

/// A JSON request body, before it is sent to `url`.
pub fn http_out(method: &str, url: &str, body: &Value) {
    rh_client::tap::record(
        "devkit",
        rh_client::tap::Event::Http {
            dir: "out",
            method,
            url,
            status: None,
            body,
        },
    );
}

/// A response from `url` (`body`: its JSON, or `null` when it had none).
pub fn http_in(method: &str, url: &str, status: u16, body: &Value) {
    rh_client::tap::record(
        "devkit",
        rh_client::tap::Event::Http {
            dir: "in",
            method,
            url,
            status: Some(status),
            body,
        },
    );
}
