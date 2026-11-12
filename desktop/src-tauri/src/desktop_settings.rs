//! Settings → Desktop app: what the app's own window may ask of it, and the
//! Start at Login state it shows back.
//!
//! The page is served by the server, not by the app, and the app has no IPC to
//! it (`droppen_desktop_core::web`). The smallest channel only the app's own
//! pages can use is therefore a navigation to a fixed
//! `droppen-desktop://settings/…` URL: `init.js` (part 9) keeps this launch's
//! random token in a closure and offers four functions that navigate with it
//! (`openInBrowser`, `setStartAtLogin`, `openLoginItems`,
//! `refreshDesktopSettings`). The window cancels the navigation
//! (`web::route`), and this module acts only when both hold:
//!
//! * the window that asked is showing the app's own server
//!   (`web::is_server` of its current URL), so a page from another origin in
//!   the window — a link the user opened there — cannot use the channel even
//!   if it somehow carried the token; and
//! * the token is this launch's ([`Channel`]). The page has it only inside
//!   the init script's closure: it is not a property of
//!   `window.__droppenDesktop`, and it is not the login nonce.
//!
//! The state comes back the other way, as the update status does
//! (`updater.rs`): a small script the app evaluates into a server window after
//! every page load and after every change, guarded again by the page's origin,
//! calling `__droppenDesktop.settings(token, state)`. It carries no login
//! token, only whether Start at Login is on, whether the switch can change it
//! (not from an App Translocation copy), and whether macOS needs the user in
//! Login Items.
//!
//! Everything here runs on the main thread: it starts from a navigation
//! question, and the tray menu it updates ([`crate::tray::set_autostart`]) is
//! main-thread only.

use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime, Url, WebviewWindow};

use droppen_desktop_core::login_item::Note;
use droppen_desktop_core::web;

use crate::shell::Shell;
use crate::tray;

/// The settings channel's per-launch token. Managed state, set up before the
/// first window.
pub struct Channel {
    token: String,
}

/// Set up the channel: a fresh token, this launch's.
pub fn install<R: Runtime>(app: &AppHandle<R>) {
    app.manage(Channel { token: web::new_nonce() });
}

/// The token the init script embeds. A fresh one nothing will match if the
/// channel is not set up, rather than no token at all.
pub fn token<R: Runtime>(app: &AppHandle<R>) -> String {
    app.try_state::<Channel>()
        .map(|channel| channel.token.clone())
        .unwrap_or_else(web::new_nonce)
}

/// What Settings → Desktop app shows of Start at Login. No login token, and no
/// port: the page already knows the server it came from.
#[derive(Serialize)]
struct State {
    checked: bool,
    enabled: bool,
    note: Option<&'static str>,
}

fn state<R: Runtime>(app: &AppHandle<R>) -> Option<State> {
    let shown = tray::login_shown(app)?;
    Some(State {
        checked: shown.checked,
        enabled: shown.enabled,
        note: shown.note.map(|note| match note {
            Note::NeedsApproval => "needsApproval",
            Note::TurnOnFailed => "turnOnFailed",
        }),
    })
}

/// The call that hands `state` to `init.js`'s bridge: it runs only in a page
/// of the server's origin, and carries the settings token, never the login's.
fn script<R: Runtime>(app: &AppHandle<R>, port: u16) -> Option<String> {
    let state = serde_json::to_string(&state(app)?).ok()?;
    let token = serde_json::to_string(&token(app)).ok()?;
    let origin = serde_json::to_string(&format!("http://127.0.0.1:{port}")).ok()?;
    Some(format!(
        "(function(){{if(location.origin!=={origin})return;var d=window.__droppenDesktop;if(d)d.settings({token},{state});}})()"
    ))
}

/// Tell every window showing the app's server the state, now.
pub fn push<R: Runtime>(app: &AppHandle<R>) {
    let Some(port) = app.try_state::<Arc<Shell>>().and_then(|shell| shell.port()) else {
        return;
    };
    let Some(script) = script(app, port) else { return };
    for window in app.webview_windows().into_values() {
        if window.url().is_ok_and(|url| web::is_server(&url, port)) {
            let _ = window.eval(&script);
        }
    }
}

/// A page of the app's server finished loading: hand it the state.
pub fn page_loaded<R: Runtime>(window: &WebviewWindow<R>, port: u16) {
    if let Some(script) = script(window.app_handle(), port) {
        let _ = window.eval(&script);
    }
}

/// Whether a request may act: the window shows the app's own server, and the
/// token is this launch's. A page from any other origin is refused even if it
/// somehow carried the token.
fn is_ours(current: Option<&Url>, port: Option<u16>, token: &str, mine: &str) -> bool {
    current
        .and_then(|url| port.map(|port| web::is_server(url, port)))
        .unwrap_or(false)
        && !token.is_empty()
        && token == mine
}

/// [`is_ours`] against the window labelled `label` and the managed token.
fn from_app_page<R: Runtime>(app: &AppHandle<R>, label: &str, token: &str) -> bool {
    let Some(channel) = app.try_state::<Channel>() else { return false };
    let port = app.try_state::<Arc<Shell>>().and_then(|shell| shell.port());
    let current = app.get_webview_window(label).and_then(|window| window.url().ok());
    is_ours(current.as_ref(), port, token, &channel.token)
}

fn refuse(what: &str) {
    // Never the token itself: whether it was wrong or absent, the answer is no.
    log::warn!("settings: a {what} request without this launch's token, or outside the app's page, was ignored");
}

/// Settings → Desktop app: *Open in Browser*, the tray item's own action.
pub fn open_in_browser<R: Runtime>(app: &AppHandle<R>, label: &str, token: &str) {
    if !from_app_page(app, label, token) {
        return refuse("open in browser");
    }
    tray::open_in_browser_from(app, "the app's settings");
}

/// Settings → Desktop app: turn Start at Login on or off. The tray's menu is
/// updated with it, and every window showing the page is told the result.
pub fn set_start_at_login<R: Runtime>(app: &AppHandle<R>, label: &str, token: &str, on: bool) {
    if !from_app_page(app, label, token) {
        return refuse("start at login");
    }
    tray::set_autostart(app, on);
    push(app);
}

/// Settings → Desktop app: ask for macOS's Login Items page.
pub fn open_login_items<R: Runtime>(app: &AppHandle<R>, label: &str, token: &str) {
    if !from_app_page(app, label, token) {
        return refuse("login items");
    }
    crate::login_item::open_login_items();
}

/// Settings → Desktop app: read Start at Login again (the platform may have
/// changed it while the page was open) and tell the pages.
pub fn refresh<R: Runtime>(app: &AppHandle<R>, label: &str, token: &str) {
    if !from_app_page(app, label, token) {
        return refuse("refresh");
    }
    tray::reread_login_item(app);
    push(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).expect("url")
    }

    #[test]
    fn only_the_apps_own_server_page_with_this_launchs_token_may_ask() {
        let port = Some(51_234);
        let server = url("http://127.0.0.1:51234/settings?section=desktop%20app");
        let elsewhere = url("https://example.com/settings?section=desktop%20app");
        let another_port = url("http://127.0.0.1:8700/settings");
        assert!(is_ours(Some(&server), port, "tok", "tok"));
        assert!(!is_ours(Some(&elsewhere), port, "tok", "tok"), "another origin");
        assert!(!is_ours(Some(&another_port), port, "tok", "tok"), "another local port");
        assert!(!is_ours(Some(&server), port, "wrong", "tok"), "another token");
        assert!(!is_ours(Some(&server), port, "", "tok"), "no token");
        assert!(!is_ours(Some(&server), None, "tok", "tok"), "no server port yet");
        assert!(!is_ours(None, port, "tok", "tok"), "no page");
    }
}
