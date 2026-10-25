//! The seam between the app and the web app it shows.
//!
//! The window shows the server's own embedded web app at
//! `http://127.0.0.1:<port>/` — the same build a browser gets — and the app
//! talks to it through exactly two things, both in the URL and the document,
//! with no IPC bridge (so the web app needs no change and the server origin is
//! given no access to the app):
//!
//! * **In**: the app's login rides in the URL *fragment* of the navigation
//!   that opens the web app in a window ([`entry_url`]), with a nonce made for
//!   this launch; the init script (`init.js`, built with the same nonce)
//!   stores the token under the key the web app reads it from — only when the
//!   nonce matches — and strips the fragment before any page script runs. A
//!   fragment is never sent to a server, and a link somebody else made cannot
//!   plant a login.
//! * **Out**: the web app already puts its unread count in the document title,
//!   `(3) Droppen` ([`unread_from_title`]); the app copies it to the menu-bar
//!   item and the Dock badge.
//!
//! Plus two rules: the web app's login and setup pages are never shown — a
//! navigation to `/login` or `/setup`, in any of the app's windows, is
//! cancelled and answered with a fresh login ([`login_redirect`]), handed to
//! every window ([`Handoff`]) — and the window itself never leaves the app: a
//! page outside it opens in the default browser ([`leaves_the_app`]), while a
//! frame inside a page may load what it likes ([`route`]).
//!
//! The setup page is the web app's answer to a dead token on a server whose
//! only account is the app's own: `droppen-app` is app-internal, so the
//! server reports its instance as still in its setup state
//! (`setupRequired`), and the web app heads for `/setup` rather than
//! `/login`. In the window that is the same thing — the app's login has
//! lapsed — and gets the same answer.
//!
//! A future `TauriShell` (`docs/local-aggregator.md` §6.2) replaces this with a
//! real bridge; until then this is the whole contract.

use url::Url;

/// The fragment key carrying the token.
pub const TOKEN_FRAGMENT: &str = "droppen-desktop-token";

/// The fragment key carrying this launch's nonce.
pub const NONCE_FRAGMENT: &str = "droppen-desktop-nonce";

/// The fragment key saying macOS runs the app from a translocated copy.
pub const MOVED_FRAGMENT: &str = "droppen-desktop-moved";

/// The web app's login route (`web/src/lib/auth.ts`, `LOGIN_PATH`).
pub const LOGIN_PATH: &str = "/login";

/// The web app's setup route (`web/src/lib/auth.ts`, `SETUP_PATH`): where it
/// goes instead of [`LOGIN_PATH`] while the instance has no user account,
/// which on the app's instance is always (its own account is app-internal).
pub const SETUP_PATH: &str = "/setup";

/// The query key that makes every load the app asks for a new document.
pub const LOAD_QUERY: &str = "droppen-desktop-load";

static LOADS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// What the app tells the web app when it opens it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entry {
    /// This launch's nonce: the init script keeps a token only when it matches.
    pub nonce: String,
    /// The app runs from an App Translocation copy: say to move it.
    pub translocated: bool,
}

/// A fresh nonce for a launch: 16 random bytes, hex.
pub fn new_nonce() -> String {
    let mut bytes = [0u8; 16];
    // A failure here leaves zeros, which the init script still requires to
    // match exactly; it only weakens the defence, it does not open anything.
    let _ = getrandom::fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The URL that opens the web app at `next`, logged in with `token`.
///
/// `next` is cleaned by [`safe_next`]. The token is percent-encoded although a
/// server token is URL-safe already, so nothing about its alphabet is assumed.
///
/// Every URL this returns carries a fresh `droppen-desktop-load=<n>` in its
/// query (the init script removes it with the fragment). Without it, opening
/// `/sessions#…` while the page is already on `/sessions` is only a fragment
/// change inside the same document: the init script never runs, the token is
/// never stored and the web app goes straight back to its login page.
pub fn entry_url(port: u16, next: &str, token: &str, entry: &Entry) -> String {
    let mut fragment = format!("{NONCE_FRAGMENT}={}&{TOKEN_FRAGMENT}={}", encode(&entry.nonce), encode(token));
    if entry.translocated {
        fragment.push_str(&format!("&{MOVED_FRAGMENT}=no"));
    }
    // Never a fragment of its own, which would end up in front of ours.
    let next = entry_path(next);
    let separator = if next.contains('?') { '&' } else { '?' };
    let load = LOADS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("http://127.0.0.1:{port}{next}{separator}{LOAD_QUERY}={load}#{fragment}")
}

/// A same-origin path to go to, or `/` — the web app's own rule
/// (`web/src/lib/auth.ts`, `safeNext`): it must start with one `/`, and `//` or
/// `/\` (a protocol-relative URL to somewhere else) is refused.
pub fn safe_next(next: &str) -> String {
    let cleaned: String = next.chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).collect();
    let mut chars = cleaned.chars();
    match (chars.next(), chars.next()) {
        (Some('/'), Some('/' | '\\')) => "/".to_string(),
        (Some('/'), _) => cleaned,
        _ => "/".to_string(),
    }
}

/// Where the web app's own pages never are — the first segment of a path: the
/// server's API, its embedded apps' reverse proxy, its API documents, its
/// static assets. A navigation there can be answered with a redirect off the
/// server, and a redirect keeps the URL's fragment — so the login is never
/// handed to one.
const NOT_PAGES: [&str; 6] = ["api", "embed", "openapi.json", "redoc", "scalar", "assets"];

/// How many layers of percent-encoding [`entry_path`] looks through before it
/// gives up on a path (and opens `/` instead).
const DECODINGS: usize = 4;

/// Where the app may open the web app with a login in the fragment: a path of
/// the web app's own pages, with its query, and no fragment — or `/`.
///
/// `next` is cleaned by [`safe_next`], then resolved against the server's
/// origin as the window resolves it (`Url::join`: dot segments, `%2e` ones
/// included, and `\` read as `/`), and the result is both what is checked and
/// what is returned — so `/x/../embed/1/` cannot pass as `/x/…` and then be
/// loaded as `/embed/1/`. The check reads the path the way any server could:
/// through every layer of percent-encoding (`/%65mbed/`), with dot segments,
/// backslashes and empty segments (`//embed/`) resolved again after each, and
/// with the prefixes matched whatever their case.
pub fn entry_path(next: &str) -> String {
    // The port is the server's in the end; the path resolves the same on any.
    let base = Url::parse("http://127.0.0.1/").expect("a valid base URL");
    let Ok(url) = base.join(&safe_next(next)) else {
        return "/".to_string();
    };
    if url.origin() != base.origin() || !is_page(url.path()) {
        return "/".to_string();
    }
    match url.query() {
        Some(query) => format!("{}?{query}", url.path()),
        None => url.path().to_string(),
    }
}

/// Whether a resolved `path` is none of [`NOT_PAGES`], at any layer of its
/// percent-encoding; one still encoded after [`DECODINGS`] layers is not a page
/// anybody links to.
fn is_page(path: &str) -> bool {
    let mut path = path.to_string();
    for _ in 0..DECODINGS {
        let first = first_segment(&path);
        if NOT_PAGES.iter().any(|not_a_page| first.eq_ignore_ascii_case(not_a_page)) {
            return false;
        }
        let decoded = percent_encoding::percent_decode_str(&path).decode_utf8_lossy().into_owned();
        if decoded == path {
            return true;
        }
        path = decoded;
    }
    false
}

/// The first segment of `path` once `.`, `..` and empty segments are resolved,
/// with `\` read as `/`.
fn first_segment(path: &str) -> &str {
    let mut segments = Vec::new();
    for segment in path.split(['/', '\\']) {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    segments.first().copied().unwrap_or_default()
}

/// `url` as it may be handed to the browser: without its fragment, which on
/// this window's URLs can be the app's login.
pub fn for_browser(url: &Url) -> Url {
    let mut url = url.clone();
    url.set_fragment(None);
    url
}

/// An app page's URL as the window may go back to it: no fragment (the login
/// rides there) and no load marker.
pub fn for_return(url: &Url) -> Url {
    let mut url = url.clone();
    url.set_fragment(None);
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| key != LOAD_QUERY)
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    if kept.is_empty() {
        url.set_query(None);
    } else {
        url.query_pairs_mut().clear().extend_pairs(kept);
    }
    url
}

/// Whether a download from `url` is the app's own: from its server, or a
/// `blob:` the server's pages made. Anything else goes to the browser, which
/// has the user's download settings and marks what it saves.
pub fn download_allowed(url: &Url, port: Option<u16>) -> bool {
    let Some(port) = port else { return false };
    match url.scheme() {
        "http" => is_server(url, port),
        "blob" => Url::parse(url.path()).is_ok_and(|inner| is_server(&inner, port)),
        _ => false,
    }
}

/// Whether `url` is the app's own server on `port`.
pub fn is_server(url: &Url, port: u16) -> bool {
    url.scheme() == "http" && url.host_str() == Some("127.0.0.1") && url.port() == Some(port)
}

/// If `url` is the server's login or setup page, where the web app wanted to
/// go after logging in (its `next`, cleaned; `/` when it says none), else
/// `None`.
pub fn login_redirect(url: &Url, port: u16) -> Option<String> {
    if !is_server(url, port) || !is_sign_in(url.path()) {
        return None;
    }
    Some(login_next(url))
}

/// Whether `path` is one of the web app's ways in: its login or setup page.
fn is_sign_in(path: &str) -> bool {
    path == LOGIN_PATH || path == SETUP_PATH
}

/// Where the login (or setup) page at `url` would go after logging in: its
/// `next`, cleaned.
fn login_next(url: &Url) -> String {
    let next = url.query_pairs().find(|(key, _)| key == "next").map(|(_, value)| value.into_owned());
    safe_next(next.as_deref().unwrap_or("/"))
}

/// A fresh login, for every one of the app's windows ([`Handoff::url_for`]).
///
/// The windows share one login — one `localStorage`, one token — and a new
/// login revokes the one it replaces. So it goes to all of them: a window left
/// out would be left on a dead login, and when it asked for one in turn, that
/// would revoke this one under the others. Its `Debug` never shows the token.
#[derive(Clone)]
pub struct Handoff {
    port: u16,
    token: String,
    entry: Entry,
    asking: String,
    next: String,
}

impl std::fmt::Debug for Handoff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handoff")
            .field("port", &self.port)
            .field("asking", &self.asking)
            .field("next", &self.next)
            .finish_non_exhaustive()
    }
}

impl Handoff {
    /// The login `token` on `port`, asked for by the window labelled `asking`,
    /// which wants to go to `next`.
    pub fn new(port: u16, token: &str, entry: Entry, asking: &str, next: &str) -> Self {
        Self {
            port,
            token: token.to_string(),
            entry,
            asking: asking.to_string(),
            next: next.to_string(),
        }
    }

    /// The window that asked for this login.
    pub fn asking(&self) -> &str {
        &self.asking
    }

    /// Where the window labelled `label` (the main one if `main`), showing
    /// `shown`, loads this login ([`entry_url`]), or `None` to leave it be:
    ///
    /// * the window that asked goes to its `next`;
    /// * any other window showing a page of the app's server — on whatever port
    ///   it had, as a window shows the old one until a server that moved is
    ///   handed to it — goes back to that page (its path and query; a login
    ///   page read as where it would go);
    /// * the main window goes to `/` when it shows anything else (the start-up
    ///   page): it must end up logged in;
    /// * any other window showing anything else is left be — a new window on
    ///   `about:blank` is the web app's to fill.
    pub fn url_for(&self, label: &str, main: bool, shown: Option<&Url>) -> Option<Url> {
        let path = if label == self.asking {
            self.next.clone()
        } else if let Some(page) = shown.and_then(app_page) {
            page
        } else if main {
            "/".to_string()
        } else {
            return None;
        };
        Url::parse(&entry_url(self.port, &path, &self.token, &self.entry)).ok()
    }
}

/// The path and query of `url` as a window shows it, when it is a page of the
/// app's server on any port; a login or setup page is read as where it would
/// go.
fn app_page(url: &Url) -> Option<String> {
    if url.scheme() != "http" || url.host_str() != Some("127.0.0.1") {
        return None;
    }
    if is_sign_in(url.path()) {
        return Some(login_next(url));
    }
    let page = for_return(url);
    Some(match page.query() {
        Some(query) => format!("{}?{query}", page.path()),
        None => page.path().to_string(),
    })
}

/// What to do with a navigation, in any frame of an app window.
///
/// WebKit asks about subframes as well as the page, and does not say which it
/// is asking about, so this answer has to be right for both: a page's iframe
/// may load whatever the page puts in it. Keeping the *window* inside the app
/// is [`leaves_the_app`]'s job, on the page loads only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// Let it load.
    Here,
    /// Do not load it — it is the web app's login or setup page — log in
    /// again and go to this path instead.
    Relogin(String),
    /// Do not load it; hand it to the system (a `mailto:` link).
    System,
    /// Do not load it at all, and say nothing.
    Refuse,
    /// Do not load it: a page asks the app to restart into its downloaded
    /// update (`init.js`, `restartToUpdate`), with this token — which must be
    /// the one the updater handed that page (never the login nonce), or it is
    /// ignored.
    RestartToUpdate(String),
    /// Do not load it: a page asks the app to check for updates now (`init.js`,
    /// `checkForUpdates`), with the same token.
    CheckForUpdates(String),
}

/// The scheme of what the app's own pages ask the app (`init.js`): never loaded.
pub const APP_SCHEME: &str = "droppen-desktop";

/// Decide a navigation. `port` is the server's, or `None` before it is up.
pub fn route(url: &Url, port: Option<u16>) -> Route {
    match url.scheme() {
        // The bundled start-up page, and what pages make on the fly.
        "tauri" | "about" | "blob" | "data" => Route::Here,
        "http" | "https" => match port.and_then(|port| login_redirect(url, port)) {
            Some(next) => Route::Relogin(next),
            None => Route::Here,
        },
        "mailto" => Route::System,
        APP_SCHEME if url.host_str() == Some("update") && url.path() == "/restart" => Route::RestartToUpdate(token(url)),
        APP_SCHEME if url.host_str() == Some("update") && url.path() == "/check" => Route::CheckForUpdates(token(url)),
        _ => Route::Refuse,
    }
}

/// The `token` of a `droppen-desktop://` request (`init.js`), or nothing.
fn token(url: &Url) -> String {
    url.query_pairs()
        .find(|(name, _)| name == "token")
        .map(|(_, token)| token.into_owned())
        .unwrap_or_default()
}

/// Whether a *page* load (never a frame) at `url` would take the window out of
/// the app: the web, another local server, anything but this server and the
/// bundled start-up page. Such a page opens in the default browser instead.
pub fn leaves_the_app(url: &Url, port: Option<u16>) -> bool {
    match url.scheme() {
        "http" | "https" => !port.is_some_and(|port| is_server(url, port)),
        _ => false,
    }
}

/// Whether the app runs from an App Translocation copy (a quarantined app
/// opened where it was downloaded): its path is random and read-only, so a
/// login item pointing at it breaks, and it should be moved to Applications.
pub fn translocated(exe: &std::path::Path) -> bool {
    exe.to_string_lossy().contains("/AppTranslocation/")
}

/// The unread count the web app put in front of `title`: `(3) Droppen` is
/// `Some("3")`, `(99+) …` is `Some("99+")`, anything else `None`
/// (`web/src/features/sessions/session-attention.ts`, `tabTitle`).
pub fn unread_from_title(title: &str) -> Option<String> {
    let rest = title.strip_prefix('(')?;
    let close = rest.find(") ")?;
    let count = &rest[..close];
    let digits = count.strip_suffix('+').unwrap_or(count);
    (!digits.is_empty() && digits.len() <= 4 && digits.bytes().all(|b| b.is_ascii_digit()) && digits != "0").then(|| count.to_string())
}

/// The number for the Dock badge: the count, `99+` read as 99.
pub fn badge_number(unread: &str) -> Option<i64> {
    unread.trim_end_matches('+').parse().ok().filter(|n| *n > 0)
}

fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).expect("url")
    }

    #[test]
    fn the_entry_url_carries_the_token_only_in_the_fragment() {
        let entry = Entry {
            nonce: "n0nce".into(),
            ..Entry::default()
        };
        let parsed = url(&entry_url(51_234, "/sessions", "abc_DEF-123", &entry));
        assert_eq!(parsed.host_str(), Some("127.0.0.1"));
        assert_eq!(parsed.port(), Some(51_234));
        assert_eq!(parsed.path(), "/sessions");
        assert_eq!(parsed.fragment(), Some("droppen-desktop-nonce=n0nce&droppen-desktop-token=abc_DEF-123"));
        let query: Vec<(String, String)> = parsed.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
        assert_eq!(query.len(), 1, "nothing but the load marker is sent to the server: {query:?}");
        assert_eq!(query[0].0, LOAD_QUERY);
        assert!(!parsed.query().unwrap_or_default().contains("abc_DEF"));

        let flagged = Entry {
            nonce: "n".into(),
            translocated: true,
        };
        let odd = url(&entry_url(51_234, "https://evil.example/", "t+k/n=", &flagged));
        assert_eq!(odd.path(), "/");
        assert_eq!(
            odd.fragment(),
            Some("droppen-desktop-nonce=n&droppen-desktop-token=t%2Bk%2Fn%3D&droppen-desktop-moved=no")
        );
    }

    #[test]
    fn every_entry_url_is_a_new_document() {
        let entry = Entry::default();
        let one = url(&entry_url(51_234, "/sessions", "t", &entry));
        let two = url(&entry_url(51_234, "/sessions", "t", &entry));
        assert_ne!(
            one.query(),
            two.query(),
            "the same path twice must not be a same-document fragment change"
        );
        let with_query = url(&entry_url(51_234, "/files?path=%2Ftmp#top", "t", &entry));
        assert_eq!(with_query.path(), "/files");
        let keys: Vec<String> = with_query.query_pairs().map(|(k, _)| k.into_owned()).collect();
        assert_eq!(keys, vec!["path".to_string(), LOAD_QUERY.to_string()]);
        assert!(with_query
            .fragment()
            .is_some_and(|fragment| fragment.ends_with("droppen-desktop-token=t")));
    }

    #[test]
    fn the_login_is_never_handed_to_a_path_that_is_not_a_page() {
        assert_eq!(entry_path("/sessions/claude/x?y=1"), "/sessions/claude/x?y=1");
        assert_eq!(entry_path("/p/tasks/"), "/p/tasks/");
        for not_a_page in [
            "/embed/app-1/",
            "/embed",
            "/api/auth/status",
            "/openapi.json",
            "/redoc",
            "/scalar/x",
            "/assets/a.js",
        ] {
            assert_eq!(entry_path(not_a_page), "/", "{not_a_page}");
        }
        assert_eq!(entry_path("/embedded-notes"), "/embedded-notes", "only the prefix as a whole segment");
        assert_eq!(entry_path("//evil.example"), "/");
        let entry = Entry::default();
        let parsed = url(&entry_url(51_234, "/embed/app-1/", "t", &entry));
        assert_eq!(parsed.path(), "/");
    }

    #[test]
    fn the_path_is_checked_as_the_window_will_load_it() {
        // Dot segments, plain and percent-encoded, that WebKit resolves away.
        for sneaky in [
            "/x/../embed/app-1/",
            "/x/./../embed/app-1/",
            "/sessions/a/b/../../../api/auth/status",
            "/../embed/app-1/",
            "/x/%2e%2e/embed/app-1/",
            "/x/%2E%2E/embed/app-1/",
            "/x/.%2e/embed/app-1/",
            "/%2e%2e/embed/app-1/",
            "/%2e/embed/app-1/",
            "/x/..%2fembed/app-1/",
            "/x/..%5Cembed/app-1/",
        ] {
            assert_eq!(entry_path(sneaky), "/", "{sneaky}");
        }
        // Percent-encoded prefixes, one layer or more, and any case.
        for sneaky in [
            "/%65mbed/app-1/",
            "/%65%6D%62%65%64/app-1/",
            "/%61pi/auth/status",
            "/openapi%2Ejson",
            "/%2565mbed/app-1/",
            "/%252565mbed/app-1/",
            "/%2fembed/app-1/",
            "/EMBED/app-1/",
            "/Api/auth/status",
        ] {
            assert_eq!(entry_path(sneaky), "/", "{sneaky}");
        }
        // Too many layers to be anything anybody links to.
        assert_eq!(entry_path("/%2525252565mbed/"), "/");
        // Backslashes, which an http URL reads as slashes.
        for sneaky in ["/x\\..\\embed\\app-1\\", "/x/..\\embed/app-1/", "/.\\embed/app-1/"] {
            assert_eq!(entry_path(sneaky), "/", "{sneaky:?}");
        }
        assert_eq!(entry_path("/\\evil.example"), "/");
        // Double slashes: empty segments in front do not hide the prefix, and
        // an empty segment elsewhere leaves a page a page.
        assert_eq!(entry_path("/.//embed/app-1/"), "/");
        assert_eq!(entry_path("/x/..//embed/app-1/"), "/");
        assert_eq!(entry_path("/x//embed/app-1/"), "/x//embed/app-1/");
        // The query and the fragment are not the path.
        assert_eq!(entry_path("/sessions?next=/embed/app-1/"), "/sessions?next=/embed/app-1/");
        assert_eq!(entry_path("/sessions#/embed/app-1/"), "/sessions");
        assert_eq!(entry_path("/embed/app-1/?x=1"), "/");
        assert_eq!(entry_path("/embed#x"), "/");
        assert_eq!(entry_path("/x/../embed?y=1#z"), "/");
        // A page comes back resolved, with its query and without its fragment:
        // what was checked is what is loaded.
        assert_eq!(entry_path("/sessions/../settings?tab=a#frag"), "/settings?tab=a");
        assert_eq!(entry_path("/a/./b/../c"), "/a/c");
        assert_eq!(entry_path("/p/%2e%2e/files?path=%2Ftmp"), "/files?path=%2Ftmp");
        assert_eq!(entry_path("/sessions/%E4%BD%A0"), "/sessions/%E4%BD%A0");
        assert_eq!(entry_path("/notes/%zz"), "/notes/%zz", "a stray % is left as it is");
        assert_eq!(entry_path("/x/../embedded-notes"), "/embedded-notes");

        let entry = Entry::default();
        let parsed = url(&entry_url(51_234, "/x/../embed/app-1/?y=1#z", "t", &entry));
        assert_eq!(parsed.path(), "/");
        let keys: Vec<String> = parsed.query_pairs().map(|(k, _)| k.into_owned()).collect();
        assert_eq!(keys, vec![LOAD_QUERY.to_string()]);
        let parsed = url(&entry_url(51_234, "/sessions/x/../y#top", "t", &entry));
        assert_eq!(parsed.path(), "/sessions/y");
        assert!(parsed.fragment().is_some_and(|fragment| fragment.starts_with(NONCE_FRAGMENT)));
    }

    #[test]
    fn what_leaves_the_window_never_carries_the_fragment() {
        let with_login = url("https://example.com/path?x=1#droppen-desktop-nonce=n&droppen-desktop-token=t");
        assert_eq!(for_browser(&with_login).as_str(), "https://example.com/path?x=1");
        let app = url("http://127.0.0.1:51234/sessions?droppen-desktop-load=4&q=a#droppen-desktop-token=t");
        assert_eq!(for_return(&app).as_str(), "http://127.0.0.1:51234/sessions?q=a");
        let bare = url("http://127.0.0.1:51234/?droppen-desktop-load=4#x");
        assert_eq!(for_return(&bare).as_str(), "http://127.0.0.1:51234/");
    }

    #[test]
    fn only_the_apps_own_downloads_are_taken() {
        let port = Some(51_234);
        assert!(download_allowed(&url("http://127.0.0.1:51234/api/fs/raw?path=x"), port));
        assert!(download_allowed(&url("blob:http://127.0.0.1:51234/0b5e-4c"), port));
        assert!(!download_allowed(&url("https://example.com/setup.pkg"), port));
        assert!(!download_allowed(&url("http://127.0.0.1:8700/x"), port));
        assert!(!download_allowed(&url("blob:https://example.com/0b5e-4c"), port));
        assert!(!download_allowed(&url("data:application/octet-stream;base64,AAAA"), port));
        assert!(!download_allowed(&url("http://127.0.0.1:51234/x"), None));
    }

    #[test]
    fn nonces_are_fresh_hex() {
        let one = new_nonce();
        assert_eq!(one.len(), 32);
        assert!(one.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(one, new_nonce());
    }

    #[test]
    fn next_is_cleaned_like_the_web_app_cleans_it() {
        assert_eq!(safe_next("/sessions/claude/x?y=1"), "/sessions/claude/x?y=1");
        assert_eq!(safe_next(""), "/");
        assert_eq!(safe_next("sessions"), "/");
        assert_eq!(safe_next("//evil.example"), "/");
        assert_eq!(safe_next("/\\evil.example"), "/");
        assert_eq!(safe_next("/\t/evil.example"), "/");
        assert_eq!(safe_next("https://evil.example"), "/");
    }

    #[test]
    fn the_login_page_is_turned_into_a_relogin() {
        let port = 51_234;
        assert_eq!(
            route(&url("http://127.0.0.1:51234/login?next=%2Fsettings"), Some(port)),
            Route::Relogin("/settings".into())
        );
        assert_eq!(route(&url("http://127.0.0.1:51234/login"), Some(port)), Route::Relogin("/".into()));
        assert_eq!(
            route(&url("http://127.0.0.1:51234/login?next=%2F%2Fevil.example"), Some(port)),
            Route::Relogin("/".into())
        );
        assert_eq!(route(&url("http://127.0.0.1:51234/sessions"), Some(port)), Route::Here);
        assert_eq!(route(&url("http://127.0.0.1:51234/loginx"), Some(port)), Route::Here);
    }

    #[test]
    fn the_setup_page_is_turned_into_a_relogin_too() {
        // The app's own account is app-internal, so its instance is always in
        // its setup state: a dead token sends the web app to `/setup`, which in
        // the window means the app's login lapsed.
        let port = 51_234;
        assert_eq!(route(&url("http://127.0.0.1:51234/setup"), Some(port)), Route::Relogin("/".into()));
        assert_eq!(
            route(&url("http://127.0.0.1:51234/setup?next=%2Fsettings"), Some(port)),
            Route::Relogin("/settings".into())
        );
        assert_eq!(
            route(&url("http://127.0.0.1:51234/setup?next=%2F%5Cevil.example"), Some(port)),
            Route::Relogin("/".into())
        );
        assert_eq!(route(&url("http://127.0.0.1:51234/setupx"), Some(port)), Route::Here);
        assert_eq!(route(&url("http://127.0.0.1:51234/settings/setup"), Some(port)), Route::Here);
        // Another port's setup page is not the app's server.
        assert_eq!(route(&url("http://127.0.0.1:8700/setup"), Some(port)), Route::Here);
    }

    #[test]
    fn a_fresh_login_reaches_every_window_and_the_asking_one_goes_where_it_wanted() {
        let entry = Entry {
            nonce: "n".into(),
            ..Entry::default()
        };
        let login = Handoff::new(51_234, "tok", entry, "view-2", "/sessions/claude/x?y=1");
        assert_eq!(login.asking(), "view-2");
        let debug = format!("{login:?}");
        assert!(!debug.contains("tok"), "the token never shows: {debug}");
        let token_of = |url: &Url| url.fragment().is_some_and(|fragment| fragment.ends_with("droppen-desktop-token=tok"));
        let page = |url: &Url| {
            let page = for_return(url);
            match page.query() {
                Some(query) => format!("{}?{query}", page.path()),
                None => page.path().to_string(),
            }
        };

        // A second window asked (its /login was cancelled): it goes to its next,
        // whatever it showed.
        let asking = login
            .url_for("view-2", false, Some(&url("http://127.0.0.1:51234/sessions/codex/y")))
            .expect("the asking window");
        assert!(is_server(&asking, 51_234) && token_of(&asking));
        assert_eq!(page(&asking), "/sessions/claude/x?y=1");

        // The main window gets the login too, back on the page it shows (as the
        // web app routed to it, with its query, without load marker or fragment).
        let main = login
            .url_for(
                "main",
                true,
                Some(&url(
                    "http://127.0.0.1:51234/settings?tab=a&droppen-desktop-load=3#droppen-desktop-token=old",
                )),
            )
            .expect("the main window");
        assert!(is_server(&main, 51_234) && token_of(&main));
        assert_eq!(page(&main), "/settings?tab=a");
        // On the start-up page (or nothing yet), the main window goes home.
        let start = login.url_for("main", true, Some(&url("tauri://localhost/index.html"))).expect("main");
        assert_eq!(page(&start), "/");
        assert_eq!(page(&login.url_for("main", true, None).expect("main")), "/");

        // A third window on an app page is handed the login where it is — on the
        // new port if the server moved — and one showing its login page goes
        // where that would go.
        let third = login
            .url_for("view-3", false, Some(&url("http://127.0.0.1:50000/p/tasks/")))
            .expect("a window on an app page");
        assert!(is_server(&third, 51_234) && token_of(&third));
        assert_eq!(page(&third), "/p/tasks/");
        let on_login = login
            .url_for("view-4", false, Some(&url("http://127.0.0.1:51234/login?next=%2Fsettings")))
            .expect("a window on the login page");
        assert_eq!(page(&on_login), "/settings");
        // Whatever a window shows, the login still never lands outside a page.
        let sneaky = login
            .url_for("view-5", false, Some(&url("http://127.0.0.1:51234/x/%2e%2e/embed/app-1/")))
            .expect("a window");
        assert_eq!(page(&sneaky), "/");

        // A new window still on about:blank, or anything not an app page, is
        // left to the web app.
        assert_eq!(login.url_for("view-6", false, Some(&url("about:blank"))), None);
        assert_eq!(login.url_for("view-6", false, Some(&url("https://example.com/"))), None);
        assert_eq!(login.url_for("view-6", false, Some(&url("http://localhost:51234/"))), None);
        assert_eq!(login.url_for("view-6", false, None), None);

        // The main window asking (a start, a restart, its own login page) goes
        // to its next as before; the others go back to their pages.
        let own = Handoff::new(51_234, "tok", Entry::default(), "main", "/");
        let main = own.url_for("main", true, Some(&url("http://127.0.0.1:51234/settings"))).expect("main");
        assert_eq!(page(&main), "/");
        let other = own
            .url_for("view-2", false, Some(&url("http://127.0.0.1:51234/sessions/codex/y")))
            .expect("the other window");
        assert_eq!(page(&other), "/sessions/codex/y");
        assert_ne!(main.query(), other.query(), "each window gets a new document");
    }

    #[test]
    fn frames_may_load_the_web_but_odd_schemes_are_refused() {
        let port = Some(51_234);
        assert_eq!(route(&url("tauri://localhost/index.html"), port), Route::Here);
        assert_eq!(route(&url("about:blank"), port), Route::Here);
        // An iframe a page shows (a video, a widget): loaded in place, never
        // pushed to the browser.
        assert_eq!(route(&url("https://www.example.com/embed"), port), Route::Here);
        assert_eq!(route(&url("http://127.0.0.1:8700/"), port), Route::Here);
        assert_eq!(route(&url("mailto:someone@example.com"), port), Route::System);
        assert_eq!(route(&url("file:///etc/passwd"), port), Route::Refuse);
        assert_eq!(route(&url("javascript:alert(1)"), port), Route::Refuse);
        // Before the server is up nothing is a login page.
        assert_eq!(route(&url("http://127.0.0.1:51234/login"), None), Route::Here);
    }

    #[test]
    fn the_app_scheme_only_carries_the_restart_into_an_update() {
        let port = Some(51_234);
        assert_eq!(
            route(&url("droppen-desktop://update/restart?token=abc123"), port),
            Route::RestartToUpdate("abc123".into())
        );
        assert_eq!(
            route(&url("droppen-desktop://update/check?token=abc123"), port),
            Route::CheckForUpdates("abc123".into())
        );
        assert_eq!(
            route(&url("droppen-desktop://update/check"), port),
            Route::CheckForUpdates(String::new())
        );
        // The login nonce's name carries nothing here.
        assert_eq!(
            route(&url("droppen-desktop://update/restart?nonce=abc123"), port),
            Route::RestartToUpdate(String::new())
        );
        assert_eq!(
            route(&url("droppen-desktop://update/restart"), port),
            Route::RestartToUpdate(String::new())
        );
        assert_eq!(route(&url("droppen-desktop://update/install?token=abc123"), port), Route::Refuse);
        assert_eq!(route(&url("droppen-desktop://quit"), port), Route::Refuse);
    }

    #[test]
    fn a_page_outside_the_app_leaves_it() {
        let port = Some(51_234);
        assert!(!leaves_the_app(&url("http://127.0.0.1:51234/sessions"), port));
        assert!(!leaves_the_app(&url("tauri://localhost/index.html"), port));
        assert!(!leaves_the_app(&url("about:blank"), port));
        assert!(leaves_the_app(&url("https://droppen.app/link"), port));
        assert!(leaves_the_app(&url("https://my.droppen.app/link"), port));
        assert!(leaves_the_app(&url("http://127.0.0.1:8700/"), port));
        assert!(leaves_the_app(&url("http://localhost:51234/"), port));
        assert!(leaves_the_app(&url("http://127.0.0.1:51234/"), None));
    }

    #[test]
    fn app_translocation_is_recognised_by_its_path() {
        assert!(translocated(std::path::Path::new(
            "/private/var/folders/xy/T/AppTranslocation/4F1A/d/Droppen.app/Contents/MacOS/Droppen"
        )));
        assert!(!translocated(std::path::Path::new("/Applications/Droppen.app/Contents/MacOS/Droppen")));
    }

    #[test]
    fn the_unread_count_is_read_from_the_title() {
        assert_eq!(unread_from_title("(3) Droppen"), Some("3".into()));
        assert_eq!(unread_from_title("(99+) Droppen · fix the build"), Some("99+".into()));
        assert_eq!(unread_from_title("Droppen"), None);
        assert_eq!(unread_from_title("(draft) notes"), None);
        assert_eq!(unread_from_title("(0) Droppen"), None);
        assert_eq!(unread_from_title("() Droppen"), None);
        assert_eq!(unread_from_title("(3)Droppen"), None);
        assert_eq!(badge_number("3"), Some(3));
        assert_eq!(badge_number("99+"), Some(99));
        assert_eq!(badge_number("x"), None);
    }
}
