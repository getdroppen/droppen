//! The device side of the Droppen hub API, in Rust (`docs/local-aggregator.md`
//! §8, S1-5): what a device does at a hub — sign in with its own key, keep a
//! `/v1/client` link, and read a machine's encrypted streams — shared by the
//! headless test device (`rh-devkit`) and the local install, which controls
//! the account's other devices with its one identity (S1-4) and follows
//! their copies (S1-5).
//!
//! It speaks only the documented hub API (`docs/hub-api/`) and the protocol
//! in [`rh_proto`]; it never links the hub. Nothing here decides what a
//! device may do: the chain does, in the caller.
//!
//! - [`http`]: JSON requests to the hub's HTTP routes, with the hub's error
//!   shape kept ([`http::HttpError`]).
//! - [`session`]: a device session, minted with the device's own key.
//! - [`link`]: the `/v1/client` (or a relay's `/v1/connect`) connection —
//!   control messages, and frames routed by `ch`.
//! - [`streams`]: reading one stream whole, verified, and folding what it holds.
//! - [`follow`]: following one stream for good, from a cursor (sans-IO).
//! - [`tap`]: the opt-in capture of hub-API traffic for the contract check.

pub mod follow;
pub mod http;
pub mod link;
pub mod session;
pub mod streams;
pub mod tap;

/// What most calls here return: an error in words.
pub type Result<T> = std::result::Result<T, String>;

/// `context: error`, for `map_err`.
pub fn err<E: std::fmt::Display>(context: &'static str) -> impl FnOnce(E) -> String {
    move |e| format!("{context}: {e}")
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    rh_proto::clock::now_ms()
}

/// Lower-case hex.
pub fn hex(data: &[u8]) -> String {
    ::hex::encode(data)
}

/// Bytes from hex.
pub fn unhex(text: &str) -> Result<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return Err("odd-length hex".into());
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2).ok_or("not hex")?, 16).map_err(|e| e.to_string()))
        .collect()
}

/// 32 bytes from 64 hex digits.
pub fn key32(text: &str) -> Result<[u8; 32]> {
    unhex(text)?.try_into().map_err(|_| "not 32 bytes".to_string())
}

/// Where a hub is: its HTTP base, its websocket base and its origin (what
/// sign-in signatures bind).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubUrl {
    pub http: String,
    pub ws: String,
    pub origin: String,
}

impl HubUrl {
    /// From `http(s)://` or `ws(s)://host[:port]`, any path ignored.
    pub fn parse(url: &str) -> Result<Self> {
        let (scheme, rest) = url.trim().split_once("://").ok_or("a hub url has a scheme")?;
        let secure = match scheme {
            "https" | "wss" => true,
            "http" | "ws" => false,
            other => return Err(format!("unsupported scheme {other}")),
        };
        let authority = rest.split('/').next().unwrap_or_default().to_ascii_lowercase();
        let (http, ws) = if secure { ("https", "wss") } else { ("http", "ws") };
        Ok(Self {
            http: format!("{http}://{authority}"),
            ws: format!("{ws}://{authority}"),
            origin: format!("{ws}://{authority}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hub_url_names_its_http_and_websocket_bases_and_its_origin() {
        let h = HubUrl::parse("https://Droppen.App/link?x=1").unwrap();
        assert_eq!(
            (h.http.as_str(), h.ws.as_str(), h.origin.as_str()),
            ("https://droppen.app", "wss://droppen.app", "wss://droppen.app")
        );
        let l = HubUrl::parse("ws://127.0.0.1:9000").unwrap();
        assert_eq!((l.http.as_str(), l.origin.as_str()), ("http://127.0.0.1:9000", "ws://127.0.0.1:9000"));
        assert!(HubUrl::parse("ftp://x").is_err());
        assert_eq!(unhex(&hex(&[0, 1, 0xfe])).unwrap(), vec![0, 1, 0xfe]);
        assert!(unhex("abc").is_err());
        assert!(key32("00").is_err());
    }
}
