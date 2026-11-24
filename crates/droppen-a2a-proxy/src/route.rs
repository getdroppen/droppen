//! The proxy's routes (`docs/a2a.md` §7.4): `/a2a/<device>/agents/<entry>/…`,
//! where `<device>` is a device id or one of the proxy's aliases, carried to
//! that device's `/a2a/agents/<entry>/…` as they are written (the query too).
//!
//! What comes after the entry says which A2A binding answers (§2), and so in
//! which form the proxy writes an error of its own: `/jsonrpc` (and
//! `/jsonrpc/`) JSON-RPC, `/rest/…` HTTP+JSON, `/.well-known/agent-card.json`
//! the Agent Card, anything else HTTP+JSON's form. A path the proxy would not
//! know how the device reads — a dot segment, `//`, a backslash, a byte that
//! is not printable, the same once percent-decoded — is no route.

/// Which binding a route is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Binding {
    JsonRpc,
    Rest,
    Card,
    /// Anything else under the entry: carried, its errors in HTTP+JSON's form.
    Other,
}

/// A request's route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// The device as the path names it (an id or an alias).
    pub device: String,
    pub entry: String,
    /// What follows the entry, from its `/` on (empty for none).
    pub rest: String,
    pub query: Option<String>,
    pub binding: Binding,
}

/// The path of the Agent Card under an entry.
pub const CARD: &str = "/.well-known/agent-card.json";

impl Route {
    /// `path` (and `query`) as a route, if it is one.
    pub fn parse(path: &str, query: Option<&str>) -> Option<Route> {
        let tail = path.strip_prefix("/a2a/")?;
        let (device, tail) = tail.split_once('/')?;
        let tail = tail.strip_prefix("agents/")?;
        let (entry, rest) = match tail.split_once('/') {
            Some((entry, rest)) => (entry, format!("/{rest}")),
            None => (tail, String::new()),
        };
        if !segment_ok(device) || !entry_ok(entry) || !clean(path) || query.is_some_and(|q| !q.bytes().all(|b| b.is_ascii_graphic())) {
            return None;
        }
        let binding = match rest.as_str() {
            CARD => Binding::Card,
            "/jsonrpc" | "/jsonrpc/" => Binding::JsonRpc,
            r if r == "/rest" || r.starts_with("/rest/") => Binding::Rest,
            _ => Binding::Other,
        };
        Some(Route {
            device: device.into(),
            entry: entry.into(),
            rest,
            query: query.map(String::from),
            binding,
        })
    }

    /// Where it goes on the device: `/a2a/agents/<entry>…[?query]`.
    pub fn upstream_path(&self) -> String {
        let mut path = format!("/a2a/agents/{}{}", self.entry, self.rest);
        if let Some(query) = &self.query {
            path.push('?');
            path.push_str(query);
        }
        path
    }
}

/// A device id or an alias in a path: 1–64 of letters, digits, `-`, `_`, `.`
/// and `~` (no percent-encoding), and not `.` or `..`.
pub fn segment_ok(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s != "." && s != ".." && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.~".contains(&b))
}

/// An entry id: `^[a-z0-9][a-z0-9-]{0,39}$` (§4).
pub fn entry_ok(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=40).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// No dot segment, `//` or backslash, raw or percent-decoded, and nothing
/// that is not printable.
fn clean(path: &str) -> bool {
    let decoded = percent_decoded(path);
    path.bytes().all(|b| b.is_ascii_graphic())
        && [path, decoded.as_str()]
            .iter()
            .all(|p| !p.split('/').any(|seg| seg == "." || seg == "..") && !p.contains("//") && !p.contains('\\'))
        && decoded.matches('/').count() == path.matches('/').count()
}

fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(b) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_route_names_a_device_an_entry_and_a_binding() {
        let r = Route::parse("/a2a/host-1/agents/main/jsonrpc", None).unwrap();
        assert_eq!((r.device.as_str(), r.entry.as_str(), r.binding), ("host-1", "main", Binding::JsonRpc));
        assert_eq!(r.upstream_path(), "/a2a/agents/main/jsonrpc");
        assert_eq!(Route::parse("/a2a/host-1/agents/main/jsonrpc/", None).unwrap().binding, Binding::JsonRpc);
        let r = Route::parse("/a2a/mac/agents/main/rest/tasks/t-1:cancel", Some("x=1")).unwrap();
        assert_eq!(r.binding, Binding::Rest);
        assert_eq!(r.upstream_path(), "/a2a/agents/main/rest/tasks/t-1:cancel?x=1");
        let r = Route::parse("/a2a/mac/agents/main/.well-known/agent-card.json", None).unwrap();
        assert_eq!(r.binding, Binding::Card);
        assert_eq!(Route::parse("/a2a/mac/agents/main", None).unwrap().binding, Binding::Other);
        for bad in [
            "/a2a/agents/main/jsonrpc",
            "/a2a/host/main/jsonrpc",
            "/a2a/host/agents/Main/jsonrpc",
            "/a2a/../agents/main/jsonrpc",
            "/a2a/host/agents/main/rest/../../api/sessions",
            "/a2a/host/agents/main/rest/%2e%2e/x",
            "/a2a/host/agents/main/rest//x",
            "/a2a/host/agents/main/rest/a%2fb",
            "/a2a/ho%73t/agents/main/jsonrpc",
            "/a2a/host/agents/main/rest/a\\b",
            "/api/sessions",
        ] {
            assert!(Route::parse(bad, None).is_none(), "{bad}");
        }
        assert!(Route::parse("/a2a/host/agents/main/jsonrpc", Some("a b")).is_none());
    }
}
