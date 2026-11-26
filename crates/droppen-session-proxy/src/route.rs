//! What the proxy's paths are: the session → device registry behind
//! `/sessions/{id}/…`, the `/api/…` a `/rpc` path must be, and the session id
//! a straight-through path names.
//!
//! A session id is carried as the CLI wrote it and never parsed, so the
//! registry is the only thing that says whether an id is one the proxy
//! started; an id it does not hold is `404 unknown_session`, never sent to a
//! device that may not have it.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

/// The most session → device pairs kept.
pub const CAP: usize = 4096;

/// The sessions this proxy started, and the device each was started on: a
/// bounded LRU, so a long-lived proxy forgets its oldest sessions rather than
/// growing without end. (A forgotten session still answers; it just has no
/// device to go to until `/sessions` names it again.)
pub struct Sessions {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    map: HashMap<String, String>,
    order: VecDeque<String>,
}

impl Default for Sessions {
    fn default() -> Self {
        Self::new()
    }
}

impl Sessions {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Record that `id` lives on `device`, moving it to the fresh end.
    pub fn bind(&self, id: impl Into<String>, device: impl Into<String>) {
        let (id, device) = (id.into(), device.into());
        let mut inner = self.inner.lock().expect("sessions");
        inner.order.retain(|seen| seen != &id);
        inner.order.push_back(id.clone());
        inner.map.insert(id.clone(), device);
        while inner.order.len() > CAP {
            if let Some(old) = inner.order.pop_front() {
                inner.map.remove(&old);
            }
        }
    }

    /// The device `id` was started on, if it is still remembered (and now the
    /// freshest).
    pub fn device_of(&self, id: &str) -> Option<String> {
        let mut inner = self.inner.lock().expect("sessions");
        let device = inner.map.get(id).cloned()?;
        inner.order.retain(|seen| seen != id);
        inner.order.push_back(id.to_string());
        Some(device)
    }

    pub fn len(&self) -> usize {
        self.inner.lock().expect("sessions").map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The largest `/rpc` path taken.
pub const MAX_PATH: usize = 4096;

/// Whether `path` is one a `/rpc` may carry: `/api/…`, ASCII, no dot segment,
/// `//` or backslash — raw or percent-decoded — and no fragment.
pub fn clean_api_path(path: &str) -> bool {
    if path.len() > MAX_PATH || !path.bytes().all(|b| b.is_ascii_graphic()) || path.contains('#') {
        return false;
    }
    let route = path.split('?').next().unwrap_or_default();
    if !route.starts_with("/api/") || route.starts_with("//") {
        return false;
    }
    let decoded = percent_decoded(route);
    if !decoded.starts_with("/api/") || decoded.matches('/').count() != route.matches('/').count() {
        return false;
    }
    [route, decoded.as_str()]
        .iter()
        .all(|p| !p.contains("//") && !p.contains('\\') && !p.split('/').any(|seg| seg == "." || seg == ".."))
}

/// The session id `path` names, for `/api/sessions/{source}/{id}[/…]`: `None`
/// for `/api/sessions` itself and `/api/sessions/new`.
pub fn session_id_of(path: &str) -> Option<&str> {
    let route = path.split(['?', '#']).next().unwrap_or_default();
    let rest = route.strip_prefix("/api/sessions/")?;
    let (_source, tail) = rest.split_once('/')?;
    let id = tail.split('/').next().unwrap_or_default();
    (!id.is_empty()).then_some(id)
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
    fn a_session_remembers_its_device_until_it_is_forgotten() {
        let sessions = Sessions::new();
        assert!(sessions.is_empty());
        assert_eq!(sessions.device_of("s-1"), None, "unknown");
        sessions.bind("s-1", "host-1");
        assert_eq!(sessions.device_of("s-1").as_deref(), Some("host-1"));
        sessions.bind("s-1", "host-2");
        assert_eq!(sessions.device_of("s-1").as_deref(), Some("host-2"), "rebound");
        assert_eq!(sessions.len(), 1);
        let sessions = Sessions::new();
        for n in 0..CAP + 5 {
            sessions.bind(format!("s-{n}"), "host-1");
        }
        assert_eq!(sessions.len(), CAP, "capped");
        assert_eq!(sessions.device_of("s-0"), None, "the oldest went");
        assert_eq!(sessions.device_of(&format!("s-{}", CAP + 4)).as_deref(), Some("host-1"));
    }

    #[test]
    fn a_session_path_names_its_id() {
        assert_eq!(session_id_of("/api/sessions/claude/abc"), Some("abc"));
        assert_eq!(session_id_of("/api/sessions/claude/abc/message"), Some("abc"));
        assert_eq!(session_id_of("/api/sessions/claude/abc?x=1"), Some("abc"));
        assert_eq!(session_id_of("/api/sessions/new"), None);
        assert_eq!(session_id_of("/api/sessions"), None);
        assert_eq!(session_id_of("/api/sessions/claude/"), None);
        assert_eq!(session_id_of("/api/events"), None);
    }

    #[test]
    fn a_rpc_path_must_be_a_clean_api_path() {
        for good in [
            "/api/",
            "/api/sessions",
            "/api/sessions/new",
            "/api/sessions/claude/abc/message",
            "/api/events?v=2",
            "/api/sessions/claude/a%20b",
            "/api/sessions/claude/abc/questions/q-1/answer",
        ] {
            assert!(clean_api_path(good), "{good}");
        }
        for bad in [
            "/a2a/agents/main/jsonrpc",
            "/api",
            "//api/sessions",
            "/api//sessions",
            "/api/../api/sessions",
            "/api/sessions/../../etc/passwd",
            "/api/sessions/%2e%2e/x",
            "/api/sessions/a%2fb",
            "/api/sessions/claude/a b",
            "/api/sessions/claude/a\\b",
            "/api/sessions#frag",
            &format!("/api/{}", "a".repeat(MAX_PATH)),
        ] {
            assert!(!clean_api_path(bad), "{bad}");
        }
    }
}
