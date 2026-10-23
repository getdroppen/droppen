//! Signing this device in to Droppen from the app (`docs/local-aggregator.md`
//! §4.9, the browser hand-off), the app's half.
//!
//! The web app in the window has no popups — a new window it asks for goes to
//! the default browser — so its Settings → Droppen opens the sign-in attempt's
//! droppen.app address, `<hub>/link#droppen-link=<payload>`, as a new window.
//! Two things are the app's to do with it:
//!
//! * **Hand it to the browser whole** ([`browser_url`]): every other address
//!   loses its fragment on the way out (on the app's own pages it can be the
//!   login, [`web::for_browser`]), but this one *is* its fragment — the
//!   attempt's public keys and state, which droppen.app reads there and a
//!   server never sees. Only an address of exactly that shape keeps it.
//! * **Come to the front once the device is in the account**
//!   ([`SignInWatch`]): the person is in the browser, and the app hears
//!   nothing from droppen.app; it asks its own server how the device stands
//!   (`GET /api/cloud`, [`cloud_state`]) every few seconds, for as long as an
//!   attempt and its enrollment can last, and brings its window forward when
//!   the device has joined.
//!
//! Neither the address nor the login token is ever logged.

use std::time::{Duration, Instant};

use serde::Deserialize;
use url::Url;

use crate::web;

/// The path of droppen.app's sign-in page.
pub const LINK_PATH: &str = "/link";
/// The fragment key of the hand-off (`server-rs/src/api/link_handoff.rs`).
pub const HANDOFF_FRAGMENT: &str = "droppen-link";

/// How often the app asks its server.
pub const POLL: Duration = Duration::from_secs(2);
/// How long it keeps asking: the attempt (15 minutes) and the enrollment
/// code it ends in (10 minutes) can run back to back, at most.
pub const WATCH_FOR: Duration = Duration::from_secs(26 * 60);

/// Whether `url` is a sign-in hand-off: `http(s)://<hub>/link#droppen-link=…`.
pub fn is_sign_in(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.path().trim_end_matches('/') == LINK_PATH
        && url.query().is_none()
        && url.fragment().is_some_and(|f| f.starts_with(&format!("{HANDOFF_FRAGMENT}=")))
}

/// `url` as a new window the web app asked for goes to the browser: whole for
/// a sign-in hand-off to another origin than the app's own server, without its
/// fragment otherwise ([`web::for_browser`]).
pub fn browser_url(url: &Url, port: Option<u16>) -> Url {
    let own = port.is_some_and(|port| web::is_server(url, port));
    if is_sign_in(url) && !own {
        url.clone()
    } else {
        web::for_browser(url)
    }
}

/// What to do after one look at the device's standing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Keep asking.
    Wait,
    /// The device joined: bring the window to the front, and stop.
    Focus,
    /// Stop asking (the time is up).
    Stop,
}

/// Following one sign-in from the app: when the device's standing turns
/// `linked` from anything else, the window comes forward, once.
#[derive(Debug, Clone)]
pub struct SignInWatch {
    until: Instant,
    /// The standing seen first: a device that was linked already is not
    /// "just joined".
    first: Option<String>,
}

impl SignInWatch {
    /// A watch from `now`.
    pub fn new(now: Instant) -> Self {
        Self {
            until: now + WATCH_FOR,
            first: None,
        }
    }

    /// Another sign-in opened meanwhile: keep watching for as long again.
    pub fn extend(&mut self, now: Instant) {
        self.until = now + WATCH_FOR;
    }

    /// Whether the time is up.
    pub fn over(&self, now: Instant) -> bool {
        now >= self.until
    }

    /// One look: the standing `GET /api/cloud` said (`None`: it did not answer).
    pub fn observe(&mut self, state: Option<&str>, now: Instant) -> Step {
        if self.over(now) {
            return Step::Stop;
        }
        let Some(state) = state else { return Step::Wait };
        let first = self.first.get_or_insert_with(|| state.to_string());
        if state == "linked" && first != "linked" {
            return Step::Focus;
        }
        Step::Wait
    }
}

#[derive(Deserialize)]
struct CloudState {
    state: String,
}

/// The device's standing with Droppen (`GET /api/cloud`'s `state`: `none`,
/// `enrolling`, `linked`, …), asked with the window's login. Loopback, no
/// proxy, no redirects, like every call the app makes ([`crate::server`]).
pub fn cloud_state(port: u16, token: &str, timeout: Duration) -> Option<String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .max_redirects(0)
        .proxy(None)
        .user_agent(crate::server::USER_AGENT)
        .build()
        .into();
    let mut response = agent
        .get(format!("{}/api/cloud", crate::server::base_url(port)))
        .header("Authorization", &format!("Bearer {token}"))
        .call()
        .ok()?;
    if response.status() != 200 {
        return None;
    }
    response.body_mut().read_json::<CloudState>().ok().map(|c| c.state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).expect("url")
    }

    #[test]
    fn only_a_sign_in_hand_off_keeps_its_fragment_on_the_way_to_the_browser() {
        let sign_in = url("https://droppen.app/link#droppen-link=eyJ2IjoxfQ");
        assert!(is_sign_in(&sign_in));
        assert_eq!(browser_url(&sign_in, Some(51_234)), sign_in, "whole");
        // A local hub, as in testing, on another port than the app's server.
        let local = url("http://127.0.0.1:9000/link#droppen-link=abc");
        assert_eq!(browser_url(&local, Some(51_234)).fragment(), Some("droppen-link=abc"));
        for other in [
            "https://droppen.app/#droppen-link=abc",
            "https://droppen.app/link?x=1#droppen-link=abc",
            "https://droppen.app/link#droppen-desktop-token=secret",
            "https://droppen.app/elsewhere#droppen-link=abc",
            "https://droppen.app/link#x=1&droppen-link=abc",
            "mailto:someone@example.com#droppen-link=abc",
        ] {
            let other = url(other);
            assert!(!is_sign_in(&other), "{other}");
            assert_eq!(browser_url(&other, Some(51_234)).fragment(), None, "{other}");
        }
        // Never the app's own server, whatever its path: its fragment can be the login.
        let own = url("http://127.0.0.1:51234/link#droppen-link=abc");
        assert_eq!(browser_url(&own, Some(51_234)).fragment(), None);
    }

    /// The hub may name its app on another host (`/.well-known/droppen-hub`,
    /// `server-rs/src/api/hub_web.rs`): the hand-off is then there, and goes
    /// to the browser whole all the same — any host, only the shape counts.
    #[test]
    fn a_sign_in_on_the_hub_s_app_host_goes_to_the_browser_whole() {
        for at in [
            "https://my.droppen.app/link#droppen-link=eyJ2IjoxfQ",
            "http://my.localhost:9000/link#droppen-link=abc",
        ] {
            let sign_in = url(at);
            assert!(is_sign_in(&sign_in), "{at}");
            assert_eq!(browser_url(&sign_in, Some(51_234)), sign_in, "{at}: whole");
        }
        let other = url("https://my.droppen.app/sessions#droppen-link=abc");
        assert_eq!(browser_url(&other, Some(51_234)).fragment(), None);
    }

    #[test]
    fn the_window_comes_forward_once_when_the_device_joins() {
        let start = Instant::now();
        let mut watch = SignInWatch::new(start);
        assert_eq!(watch.observe(None, start), Step::Wait, "the server did not answer");
        assert_eq!(watch.observe(Some("none"), start), Step::Wait);
        assert_eq!(watch.observe(Some("enrolling"), start + POLL), Step::Wait);
        assert_eq!(watch.observe(Some("linked"), start + POLL * 2), Step::Focus);

        // Linked already when the watch began (another sign-in attempt from a linked device): not news.
        let mut linked = SignInWatch::new(start);
        assert_eq!(linked.observe(Some("linked"), start), Step::Wait);
        assert_eq!(linked.observe(Some("linked"), start + POLL), Step::Wait);

        // A removed device that joins again does come forward.
        let mut removed = SignInWatch::new(start);
        assert_eq!(removed.observe(Some("removed"), start), Step::Wait);
        assert_eq!(removed.observe(Some("linked"), start + POLL), Step::Focus);

        // Time is up: stop; another sign-in opened meanwhile extends it.
        let mut late = SignInWatch::new(start);
        assert_eq!(late.observe(Some("none"), start + WATCH_FOR), Step::Stop);
        late.extend(start + WATCH_FOR);
        assert_eq!(late.observe(Some("none"), start + WATCH_FOR + POLL), Step::Wait);
    }
}
