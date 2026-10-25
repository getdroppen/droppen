//! Which address a browser opens for the hub: the sign-in popup
//! (`<hubWeb>/link?v=1`) and the browser hand-off (`<hubWeb>/link#droppen-link=…`,
//! [`super::link_handoff`]).
//!
//! This computer signs in to its hub at `wss://<hub>` (api.droppen.app); the page
//! a person signs in on may be on another host of the same hub (the app at
//! my.droppen.app). The hub says which in `GET /.well-known/droppen-hub`,
//! `{origin, app, site}` (docs/hub-api/http.md). Its `app` is taken when:
//!
//! - it is a plain origin (scheme, host, port; nothing more), on the hub's own
//!   scheme (`https` for a `wss://` hub; `http` only for a loopback `ws://`
//!   one, a test hub);
//! - its host is the hub's host, a subdomain of it, or a name on the hub
//!   host's parent domain (my.droppen.app for api.droppen.app; a hub can keep
//!   the API on `api.` and its app on another host of the same domain): a hub
//!   can name its own app host, never somebody else's;
//! - the document's `origin`, when it gives one, is this hub as dialled.
//!
//! Anything else — no such document (a hub from before it answers with the
//! app's shell), an answer that is not one, a redirect, the hub out of reach —
//! leaves today's address: the hub's own origin as `https://`
//! ([`super::cloud::web_origin`]). The agent's own connection never changes:
//! it stays `wss://<hub>`.
//!
//! An answer is kept for [`ANSWER_TTL`], a failure for [`FAILURE_TTL`], per
//! hub. `GET /api/cloud` waits for a first look at most [`STATUS_WAIT`] (the
//! page opens the popup from it, in the click) and never for a refresh;
//! `POST /api/cloud/link` waits for a stale or missing answer as long as the
//! lookup may take ([`LOOKUP_TIMEOUT`]), and otherwise uses what it had.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::future::{FutureExt, Shared};
use serde_json::Value;

use super::cloud::web_origin;

/// Where a hub says which hosts it serves.
pub const DISCOVERY_PATH: &str = "/.well-known/droppen-hub";
/// How long one lookup may take.
pub const LOOKUP_TIMEOUT: Duration = Duration::from_secs(3);
/// How long `GET /api/cloud` waits for a hub's first answer.
pub const STATUS_WAIT: Duration = Duration::from_secs(1);
/// How long an answer (a document, or a hub that has none) is kept.
pub const ANSWER_TTL: Duration = Duration::from_secs(10 * 60);
/// How long a failure (the network, a timeout) is kept.
pub const FAILURE_TTL: Duration = Duration::from_secs(30);
/// The longest document read.
const MAX_DOCUMENT: usize = 16 * 1024;

/// What one lookup found: the app origin the hub named, when this computer
/// takes it; and whether the hub answered at all (a hub without the document
/// answers too).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Found {
    app: Option<String>,
    answered: bool,
}

const NOTHING: Found = Found {
    app: None,
    answered: false,
};

type Lookup = Shared<Pin<Box<dyn Future<Output = Found> + Send>>>;

struct Entry {
    found: Found,
    at: Instant,
}

#[derive(Default)]
struct Inner {
    entries: Mutex<HashMap<String, Entry>>,
    running: Mutex<HashMap<String, Lookup>>,
}

/// The hubs' app origins, as their discovery documents said them. One per
/// server, in [`super::cloud::Cloud`].
pub struct HubWebs {
    inner: Arc<Inner>,
    /// Off in unit tests unless a test turns it on: they never reach the network.
    discover: AtomicBool,
}

impl Default for HubWebs {
    fn default() -> Self {
        Self {
            inner: Arc::default(),
            discover: AtomicBool::new(!cfg!(test)),
        }
    }
}

impl HubWebs {
    /// Tests: look hubs up (or not).
    #[cfg(test)]
    pub fn set_discover(&self, on: bool) {
        self.discover.store(on, Ordering::SeqCst);
    }

    /// The web origin a browser opens for `hub` (`wss://…` as
    /// [`crate::agent::hub::origin`] writes it), for the local page's status:
    /// a kept answer at once (refreshed in the background once stale), a
    /// first one waited for at most [`STATUS_WAIT`].
    pub async fn for_status(&self, hub: &str) -> String {
        let wait = if self.kept(hub).is_some() { Duration::ZERO } else { STATUS_WAIT };
        self.resolve(hub, wait).await
    }

    /// The same for a sign-in attempt: a fresh answer, waited for; a stale
    /// one when the hub does not answer in time.
    pub async fn for_sign_in(&self, hub: &str) -> String {
        self.resolve(hub, LOOKUP_TIMEOUT + Duration::from_millis(500)).await
    }

    /// The kept answer for `hub`, and whether it is still fresh.
    fn kept(&self, hub: &str) -> Option<(Found, bool)> {
        let entries = self.inner.entries.lock().expect("hub webs");
        entries.get(hub).map(|e| {
            let ttl = if e.found.answered { ANSWER_TTL } else { FAILURE_TTL };
            (e.found.clone(), e.at.elapsed() < ttl)
        })
    }

    async fn resolve(&self, hub: &str, wait: Duration) -> String {
        let fallback = web_origin(hub);
        if !self.discover.load(Ordering::SeqCst) {
            return fallback;
        }
        let kept = self.kept(hub);
        if let Some((found, true)) = &kept {
            return found.app.clone().unwrap_or(fallback);
        }
        let lookup = self.lookup(hub);
        let found = if wait.is_zero() {
            None
        } else {
            tokio::time::timeout(wait, lookup).await.ok()
        };
        found
            .or(kept.map(|(found, _)| found))
            .and_then(|found| found.app)
            .unwrap_or(fallback)
    }

    /// The lookup of `hub` running now, or a new one: it runs to its end in a
    /// task of its own, whoever stops waiting for it, and keeps what it found.
    fn lookup(&self, hub: &str) -> Lookup {
        let mut running = self.inner.running.lock().expect("hub webs");
        if let Some(lookup) = running.get(hub) {
            return lookup.clone();
        }
        let inner = self.inner.clone();
        let target = hub.to_string();
        let task = tokio::spawn(async move {
            let found = fetch(&target).await;
            inner.entries.lock().expect("hub webs").insert(
                target.clone(),
                Entry {
                    found: found.clone(),
                    at: Instant::now(),
                },
            );
            inner.running.lock().expect("hub webs").remove(&target);
            found
        });
        let lookup: Lookup = async move { task.await.unwrap_or(NOTHING) }.boxed().shared();
        running.insert(hub.to_string(), lookup.clone());
        lookup
    }
}

/// Ask `hub` (`wss://…`) for its discovery document.
async fn fetch(hub: &str) -> Found {
    // No proxy, as the agent's websocket to the same hub takes none; no
    // redirects: the document is the hub's own, at its own address.
    let Ok(http) = reqwest::Client::builder()
        .timeout(LOOKUP_TIMEOUT)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
    else {
        return NOTHING;
    };
    let url = format!("{}{DISCOVERY_PATH}", web_origin(hub));
    let answer = async {
        let mut response = http.get(&url).header("accept", "application/json").send().await.ok()?;
        let status = response.status();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.ok()? {
            body.extend_from_slice(&chunk);
            if body.len() > MAX_DOCUMENT {
                return Some((status, None));
            }
        }
        Some((status, Some(body)))
    };
    match tokio::time::timeout(LOOKUP_TIMEOUT, answer).await {
        Ok(Some((status, body))) => {
            let doc = body
                .filter(|_| status.is_success())
                .and_then(|body| serde_json::from_slice::<Value>(&body).ok());
            let app = doc.as_ref().and_then(|doc| app_origin(doc, hub));
            match (&app, doc.is_some()) {
                (Some(app), _) => tracing::info!(%hub, %app, "the hub's sign-in page is at its app origin"),
                (None, true) => tracing::warn!(%hub, "the hub's discovery document names no app origin this device takes; signing in on the hub's own origin"),
                (None, false) => tracing::debug!(%hub, %status, "the hub has no discovery document; signing in on its own origin"),
            }
            Found { app, answered: true }
        }
        _ => {
            tracing::debug!(%hub, "the hub's discovery document did not come; signing in on its own origin for now");
            NOTHING
        }
    }
}

/// The app origin `doc` (a hub's discovery document) names, when a browser
/// may be sent there for `hub` (`wss://…`/`ws://…` as
/// [`crate::agent::hub::origin`] writes it) — by the rules in the module
/// docs; `None` otherwise.
pub fn app_origin(doc: &Value, hub: &str) -> Option<String> {
    if let Some(origin) = doc.get("origin").filter(|o| !o.is_null()) {
        let said = crate::agent::hub::origin(origin.as_str()?).ok()?;
        if said != hub {
            return None;
        }
    }
    let (hub_secure, hub_authority) = split(hub, &["wss", "ws"])?;
    let (secure, authority) = split(doc.get("app")?.as_str()?, &["https", "http"])?;
    if secure != hub_secure {
        return None;
    }
    let host = host_of(&authority);
    let hub_host = host_of(&hub_authority);
    let ip = |h: &str| h.starts_with('[') || h.parse::<std::net::Ipv4Addr>().is_ok();
    let under = |apex: &str| host.ends_with(&format!(".{apex}"));
    let own = host == hub_host
        || (!ip(hub_host)
            && (under(hub_host) || parent_domain(hub_host).is_some_and(|parent| host == parent || under(parent))));
    if !own || host.is_empty() {
        return None;
    }
    Some(format!("{}://{authority}", if secure { "https" } else { "http" }))
}

/// `scheme://authority` with nothing after it, scheme one of `schemes`
/// (`[secure, plain]`): `(secure, authority)` lower-cased, the scheme's
/// default port dropped. A plain one only on loopback.
fn split(text: &str, schemes: &[&str; 2]) -> Option<(bool, String)> {
    let text = text.trim();
    if text.len() > 255 {
        return None;
    }
    let (scheme, rest) = text.split_once("://")?;
    let secure = match scheme.to_ascii_lowercase() {
        s if s == schemes[0] => true,
        s if s == schemes[1] => false,
        _ => return None,
    };
    // A host name as written on the wire (an IDN as its `xn--` form), an
    // IPv6 literal in brackets, a port: nothing a browser would read otherwise.
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | ':' | '[' | ']');
    if rest.is_empty() || !rest.chars().all(allowed) {
        return None;
    }
    let mut authority = rest.to_ascii_lowercase();
    let default_port = if secure { ":443" } else { ":80" };
    if let Some(stripped) = authority.strip_suffix(default_port) {
        authority = stripped.to_string();
    }
    let host = host_of(&authority);
    if host.is_empty() || host.ends_with('.') || host.starts_with('.') || host.contains("..") {
        return None;
    }
    if let Some(port) = authority[host.len()..].strip_prefix(':') {
        if port.is_empty() || port.len() > 5 || !port.chars().all(|c| c.is_ascii_digit()) || port.parse::<u16>().is_err() {
            return None;
        }
    } else if authority.len() != host.len() {
        return None;
    }
    let loopback = matches!(host, "127.0.0.1" | "localhost" | "[::1]") || host.ends_with(".localhost");
    if !secure && !loopback {
        return None;
    }
    Some((secure, authority))
}

/// The host of `host[:port]` (an IPv6 literal keeps its brackets).
fn host_of(authority: &str) -> &str {
    if authority.starts_with('[') {
        return authority.find(']').map_or(authority, |end| &authority[..=end]);
    }
    authority.rsplit_once(':').map_or(authority, |(host, _)| host)
}

/// The parent domain of a hub host that is itself one label under it
/// (`api.droppen.app` → `droppen.app`): where a hub keeps its app while its
/// API answers on a separate host. `None` when that would leave fewer than two
/// labels (`droppen.app`, `localhost`), so a bare name never becomes an apex.
///
/// A two-letter TLD is where registries put names one label deeper (`co.uk`,
/// `com.au`): a hub at `evil.co.uk` is then the registrable domain itself, and
/// its parent (`co.uk`) is a public suffix that must never become an apex. Only
/// a longer TLD's parent is taken as a domain of the hub's (`droppen.app`).
fn parent_domain(hub_host: &str) -> Option<&str> {
    let (_label, parent) = hub_host.split_once('.')?;
    let (_sld, tld) = parent.rsplit_once('.')?;
    (tld.len() > 2).then_some(parent)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_app_origin_is_the_hub_s_own_host_or_a_subdomain_of_it() {
        let hub = "wss://droppen.app";
        let app = |doc: Value| app_origin(&doc, hub);
        assert_eq!(app(json!({"origin": hub, "app": "https://my.droppen.app", "site": "https://www.droppen.app"})).as_deref(), Some("https://my.droppen.app"));
        assert_eq!(app(json!({"app": "https://droppen.app"})).as_deref(), Some("https://droppen.app"), "the hub's own host, no origin said");
        assert_eq!(app(json!({"app": "HTTPS://My.Droppen.App:443"})).as_deref(), Some("https://my.droppen.app"), "as an origin is written");
        assert_eq!(app(json!({"app": "https://a.b.droppen.app:8443"})).as_deref(), Some("https://a.b.droppen.app:8443"));
        for refused in [
            json!({"app": "https://evil.example"}),
            json!({"app": "https://evildroppen.app"}),
            json!({"app": "https://droppen.app.evil.example"}),
            json!({"app": "http://my.droppen.app"}),
            json!({"app": "https://my.droppen.app/"}),
            json!({"app": "https://my.droppen.app/link"}),
            json!({"app": "https://my.droppen.app?x"}),
            json!({"app": "https://my.droppen.app#x"}),
            json!({"app": "https://user@my.droppen.app"}),
            json!({"app": "https://my.droppen.app."}),
            json!({"app": "https://my.dröppen.app"}),
            json!({"app": "https://evil。droppen.app"}),
            json!({"app": "https://my.droppen.app:x"}),
            json!({"app": "https://my.droppen.app:99999"}),
            json!({"app": "https://.droppen.app"}),
            json!({"app": "https://my..droppen.app"}),
            json!({"app": "wss://my.droppen.app"}),
            json!({"app": "https://"}),
            json!({"app": ""}),
            json!({"app": 7}),
            json!({"app": null}),
            json!({}),
            json!({"origin": "wss://other.example", "app": "https://my.droppen.app"}),
            json!({"origin": 1, "app": "https://my.droppen.app"}),
        ] {
            assert_eq!(app(refused.clone()), None, "{refused}");
        }
        // Another hub's document — the user typed my.droppen.app as the hub:
        // its own origin as before (that host serves the app too).
        assert_eq!(app_origin(&json!({"origin": hub, "app": "https://my.droppen.app"}), "wss://my.droppen.app"), None);
    }

    #[test]
    fn an_api_host_may_name_the_app_on_another_host_of_the_same_domain() {
        let hub = "wss://api.droppen.app";
        let app = |doc: Value| app_origin(&doc, hub);
        assert_eq!(
            app(json!({"origin": hub, "app": "https://my.droppen.app", "site": "https://www.droppen.app"})).as_deref(),
            Some("https://my.droppen.app")
        );
        // The apex itself, and the hub's own host, are still the hub's to name.
        assert_eq!(app(json!({"origin": hub, "app": "https://droppen.app"})).as_deref(), Some("https://droppen.app"));
        assert_eq!(app(json!({"app": "https://api.droppen.app"})).as_deref(), Some("https://api.droppen.app"));
        // A sibling under *another* domain is not.
        for refused in [
            json!({"app": "https://my.other.example"}),
            json!({"app": "https://evildroppen.app"}),
            json!({"app": "https://api.droppen.app.evil.example"}),
            // The document must still be for the hub as dialled: the apex's
            // document is another hub, during the compatibility window too.
            json!({"origin": "wss://droppen.app", "app": "https://my.droppen.app"}),
            json!({"origin": "wss://other.example", "app": "https://my.droppen.app"}),
        ] {
            assert_eq!(app(refused.clone()), None, "{refused}");
        }
        // A two-letter TLD's registrable domain is itself one label under the
        // registry's suffix: `evil.co.uk` must not name `victim.co.uk`, and
        // neither may a host one label under `evil.co.uk` name a host under
        // another registrable domain of the same suffix.
        assert_eq!(app_origin(&json!({"app": "https://victim.co.uk"}), "wss://evil.co.uk"), None);
        assert_eq!(app_origin(&json!({"app": "https://my.victim.co.uk"}), "wss://api.evil.co.uk"), None);
        // A deeper name still works (`app` on a sibling of its parent).
        assert_eq!(
            app_origin(&json!({"app": "https://my.droppen.example"}), "wss://api.droppen.example").as_deref(),
            Some("https://my.droppen.example")
        );
        // A hub with the app under its own host is unchanged.
        assert_eq!(
            app_origin(&json!({"app": "https://my.droppen.app"}), "wss://my.droppen.app").as_deref(),
            Some("https://my.droppen.app")
        );
        assert_eq!(app_origin(&json!({"app": "https://evil.example"}), "wss://droppen.app"), None);
    }

    #[test]
    fn a_loopback_test_hub_may_name_a_plain_http_app() {
        let hub = "ws://localhost:9000";
        assert_eq!(app_origin(&json!({"origin": "ws://localhost:9000", "app": "http://my.localhost:9000"}), hub).as_deref(), Some("http://my.localhost:9000"));
        assert_eq!(app_origin(&json!({"app": "https://my.localhost:9000"}), hub), None, "the hub's scheme");
        // An address is nobody's parent domain.
        assert_eq!(app_origin(&json!({"app": "http://my.127.0.0.1:9000"}), "ws://127.0.0.1:9000"), None);
        assert_eq!(app_origin(&json!({"app": "http://127.0.0.1:9001"}), "ws://127.0.0.1:9000").as_deref(), Some("http://127.0.0.1:9001"));
        // Plain http off loopback, never.
        assert_eq!(app_origin(&json!({"app": "http://my.example.com"}), "wss://example.com"), None);
    }

    /// A hub on a loopback port, `ws://localhost:<port>`: `answer(hub)` is
    /// what `/.well-known/droppen-hub` gets (status, location, JSON), and
    /// every other path the app's shell, as a hub from before the document
    /// answers. How often the document was asked for.
    pub(crate) async fn hub_answering(answer: impl Fn(&str) -> (u16, Option<&'static str>, String)) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        use gotcha::axum::http::{StatusCode, Uri};
        use gotcha::axum::response::IntoResponse;
        use gotcha::axum::{routing::get, Router};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let hub = format!("ws://localhost:{}", listener.local_addr().unwrap().port());
        let (status, location, body) = answer(&hub);
        let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = asked.clone();
        let router = Router::new().fallback(get(move |uri: Uri| {
            let (counter, body) = (counter.clone(), body.clone());
            async move {
                if uri.path() != DISCOVERY_PATH {
                    return (StatusCode::OK, [("content-type", "text/html")], "<!doctype html><title>shell</title>").into_response();
                }
                counter.fetch_add(1, Ordering::SeqCst);
                let mut response = (StatusCode::from_u16(status).unwrap(), [("content-type", "application/json")], body).into_response();
                if let Some(location) = location {
                    response.headers_mut().insert("location", location.parse().unwrap());
                }
                response
            }
        }));
        tokio::spawn(async move { gotcha::axum::serve(listener, router).await });
        (hub, asked)
    }

    fn on() -> HubWebs {
        let webs = HubWebs::default();
        webs.set_discover(true);
        webs
    }

    pub(crate) fn port(hub: &str) -> &str {
        hub.rsplit_once(':').unwrap().1
    }

    #[tokio::test]
    async fn a_hub_s_document_moves_the_sign_in_to_its_app_and_is_kept() {
        let (hub, asked) = hub_answering(|hub| {
            let doc = json!({"origin": hub, "app": format!("http://my.localhost:{}", port(hub)), "site": format!("http://www.localhost:{}", port(hub))});
            (200, None, doc.to_string())
        })
        .await;
        let webs = on();
        let app = format!("http://my.localhost:{}", port(&hub));
        assert_eq!(webs.for_sign_in(&hub).await, app);
        assert_eq!(webs.for_status(&hub).await, app);
        assert_eq!(webs.for_sign_in(&hub).await, app);
        assert_eq!(asked.load(Ordering::SeqCst), 1, "kept, not asked again");
        // Another server's state asks for itself.
        assert_eq!(on().for_status(&hub).await, app);
        assert_eq!(asked.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn without_a_document_it_takes_the_sign_in_is_on_the_hub_s_own_origin() {
        let shell = |_: &str| (200, None, "<!doctype html><title>shell</title>".to_string());
        let missing = |_: &str| (404, None, "{\"error\":\"not_found\"}".to_string());
        let elsewhere = |hub: &str| (302, Some("https://evil.example/.well-known/droppen-hub"), json!({"origin": hub, "app": "http://my.localhost:1"}).to_string());
        let foreign = |hub: &str| (200, None, json!({"origin": hub, "app": "https://evil.example"}).to_string());
        let other_hub = |hub: &str| (200, None, json!({"origin": "wss://other.example", "app": format!("http://my.localhost:{}", port(hub))}).to_string());
        let huge = |hub: &str| (200, None, json!({"origin": hub, "app": format!("http://my.localhost:{}", port(hub)), "pad": "x".repeat(MAX_DOCUMENT)}).to_string());
        let answers: [(&str, &dyn Fn(&str) -> (u16, Option<&'static str>, String)); 6] =
            [("the shell", &shell), ("404", &missing), ("a redirect", &elsewhere), ("a third party", &foreign), ("another hub's", &other_hub), ("too long", &huge)];
        for (what, answer) in answers {
            let (hub, asked) = hub_answering(answer).await;
            let webs = on();
            assert_eq!(webs.for_sign_in(&hub).await, web_origin(&hub), "{what}");
            assert_eq!(webs.for_status(&hub).await, web_origin(&hub), "{what}");
            assert_eq!(asked.load(Ordering::SeqCst), 1, "{what}: an answer is kept too");
        }
    }

    #[tokio::test]
    async fn an_unreachable_hub_keeps_its_own_origin_and_unit_tests_ask_nobody() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let dead = format!("ws://127.0.0.1:{}", listener.local_addr().unwrap().port());
        drop(listener);
        let started = Instant::now();
        assert_eq!(on().for_sign_in(&dead).await, web_origin(&dead));
        assert!(started.elapsed() < LOOKUP_TIMEOUT, "refused at once");

        let (hub, asked) = hub_answering(|hub| (200, None, json!({"origin": hub, "app": format!("http://my.localhost:{}", port(hub))}).to_string())).await;
        assert_eq!(HubWebs::default().for_sign_in(&hub).await, web_origin(&hub));
        assert_eq!(asked.load(Ordering::SeqCst), 0, "off unless a test turns it on");
    }

    /// A hub that takes the connection and never answers: the page's status
    /// waits a second at most, once, and a sign-in no longer than the lookup.
    #[tokio::test]
    async fn a_silent_hub_holds_nothing_up_for_long() {
        let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let hub = format!("ws://{}", silent.local_addr().unwrap());
        let webs = on();
        let started = Instant::now();
        assert_eq!(webs.for_status(&hub).await, web_origin(&hub));
        assert!(started.elapsed() < STATUS_WAIT + Duration::from_millis(500), "{:?}", started.elapsed());
        let started = Instant::now();
        assert_eq!(webs.for_sign_in(&hub).await, web_origin(&hub));
        assert!(started.elapsed() < LOOKUP_TIMEOUT + Duration::from_secs(1), "{:?}", started.elapsed());
        drop(silent);
    }
}
