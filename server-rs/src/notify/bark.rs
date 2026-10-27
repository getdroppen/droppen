//! The one channel that exists: Bark, and the shape a future second one would
//! have to fit.
//!
//! Bark is a generic webhook wearing an iOS app: a nudge is a `POST` of a small
//! JSON document to a URL whose last path segment *is* the credential. That is
//! the whole protocol, which is why [`send`] is short and why nothing above it
//! knows the word "Bark" — [`super::Nudge`] is channel-agnostic, and adding a
//! second channel means another `send` beside this one rather than a rewrite of
//! anything that calls it.
//!
//! What is deliberately *not* general is the endpoint: it comes from the stored
//! configuration and never from a caller. A route that took a URL and posted to
//! it would be an authenticated SSRF jump — and this one is reachable from
//! inside an agent's session, which is the last place to hand out a
//! "fetch whatever I say" primitive.

use std::time::Duration;

use serde_json::json;

use super::Nudge;

/// How long Bark is given to accept a notification.
///
/// Shorter than [`crate::push`]'s fifteen seconds because of who is waiting: a
/// push goes out on a background task nobody is blocked on, and this one has an
/// agent's `rh-notify` sitting on the answer. Ten seconds is well past a
/// healthy round trip to `api.day.app` and well short of a turn stalling on it.
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

/// The public Bark relay, and what a bare device key is resolved against.
const DEFAULT_SERVER: &str = "https://api.day.app";

/// The env opt-in that lets plain-HTTP loopback endpoints through.
///
/// The twin of [`crate::push`]'s, and separate from it on purpose: turning on
/// insecure *push* endpoints to run a test should not also turn on insecure
/// notification endpoints, and a deployment that wants one and not the other
/// should be able to say so. Off in production, where a plain-HTTP endpoint
/// could only be a mistake or an attempt to make this server post to something
/// on its own host; on only for `1`, `true`, `yes` or `on` (any case).
pub const ALLOW_INSECURE_ENDPOINT_ENV: &str = "DROPPEN_NOTIFY_ALLOW_INSECURE_ENDPOINT";

/// One shared client, for [`crate::push`]'s reason: building one means building
/// a rustls configuration, which costs far more than the request it carries.
///
/// Redirects are refused rather than followed. A Bark server has no reason to
/// 3xx, and following one is the only way an endpoint that passed
/// [`endpoint_is_acceptable`]'s first-hop check could still reach
/// `http://127.0.0.1:<port>/…` — that is, the loopback-only services on this
/// host, including this server's own API.
static CLIENT: std::sync::LazyLock<Option<reqwest::Client>> = std::sync::LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent(concat!("remote-hand/", env!("CARGO_PKG_VERSION")))
        .timeout(SEND_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok()
});

/// Whether plain-HTTP loopback endpoints are permitted in this process.
fn insecure_endpoints_allowed() -> bool {
    crate::env::flag(ALLOW_INSECURE_ENDPOINT_ENV)
}

/// What the user typed, turned into the URL a nudge is POSTed to.
///
/// Three shapes are accepted, because all three are what people actually have
/// in their clipboard when they open the Bark app:
///
/// * the full endpoint, `https://api.day.app/<key>`, which the app's own "copy"
///   button produces;
/// * the same against a self-hosted bark-server on any host or port;
/// * the bare device key, which is what the app *displays*.
///
/// A trailing slash is dropped either way: `…/<key>/` and `…/<key>` are the same
/// endpoint to bark-server, and keeping the slash would make the two configure
/// as different strings and log as different origins.
///
/// `None` for anything that is neither a URL nor a plausible key, so the
/// settings page can refuse it while the user is still looking at the field
/// rather than at a notification that never arrives.
pub fn normalize_endpoint(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed.contains("://") {
        // A device key is bark-server's own id format: url-safe, and long enough
        // that a typo'd hostname cannot be mistaken for one.
        let is_key = trimmed.len() >= 8 && trimmed.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        return is_key.then(|| format!("{DEFAULT_SERVER}/{trimmed}"));
    }
    let url = reqwest::Url::parse(trimmed).ok()?;
    // A bare origin is a bark-server with no device key, which would answer
    // every nudge with a 404 nobody would think to look for.
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() || url.path().trim_matches('/').is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

/// Whether an endpoint is somewhere this server is willing to send to.
///
/// Bark's own relay is HTTPS and every documented self-hosted deployment is
/// behind one, so production requires it. Plain HTTP is accepted only for
/// loopback and only under [`ALLOW_INSECURE_ENDPOINT_ENV`] — the opt-in the send
/// path is exercised through, where there is nothing on the wire to protect.
/// A *first-hop* check, which is why the shared `CLIENT` refuses to follow
/// redirects.
pub fn endpoint_is_acceptable(endpoint: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(endpoint) else {
        return false;
    };
    match url.scheme() {
        "https" => true,
        "http" if insecure_endpoints_allowed() => {
            matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1" | "[::1]"))
        }
        _ => false,
    }
}

/// An endpoint's scheme, host and port, and never its path.
///
/// The rule [`crate::push`]'s own `endpoint_origin` follows, applied to the one
/// credential this
/// module holds: the last path segment *is* the device key, so the origin is the
/// most that may appear in a log line — or, here, in an API response. It is
/// enough to tell `api.day.app` from a self-hosted server, and none of the
/// secret.
pub fn endpoint_origin(endpoint: &str) -> String {
    if endpoint.is_empty() {
        return String::new();
    }
    match reqwest::Url::parse(endpoint) {
        Ok(url) => match (url.host_str(), url.port()) {
            (Some(host), Some(port)) => format!("{}://{host}:{port}", url.scheme()),
            (Some(host), None) => format!("{}://{host}", url.scheme()),
            (None, _) => format!("{}://<no-host>", url.scheme()),
        },
        Err(_) => "<unparseable>".to_string(),
    }
}

/// An endpoint a person can recognise but not use.
///
/// [`endpoint_origin`] is the rule for logs, where nobody is trying to identify
/// anything; a settings page has a second job. A user with a phone and a tablet
/// both running Bark, or one who has rotated their key, needs to be able to see
/// *which* endpoint is armed without being shown a value that would let the page
/// — or anything that scrapes it — send to it. Four trailing characters of a
/// twenty-odd character key identify it to a human and reduce the search space
/// by nothing that matters.
///
/// A key too short to have four characters left over is hidden completely: the
/// point is to jog a memory, not to leak a short secret.
pub fn endpoint_mask(endpoint: &str) -> String {
    if endpoint.is_empty() {
        return String::new();
    }
    let origin = endpoint_origin(endpoint);
    let Some(key) = endpoint.rsplit('/').next().filter(|key| !key.is_empty()) else {
        return origin;
    };
    let tail: String = key.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    if key.chars().count() <= 8 {
        return format!("{origin}/{}", "\u{2022}".repeat(8));
    }
    format!("{origin}/{}{tail}", "\u{2022}".repeat(4))
}

/// The JSON body of one nudge.
///
/// Split from [`send`] so the wire shape can be asserted without a socket, which
/// matters more here than usual: every field is optional to Bark and a
/// misspelled key is silently ignored by it, so a typo would show up as a
/// notification that is merely *plainer* than intended and nobody would notice.
///
/// Absent fields are omitted rather than sent as `null`: bark-server parses the
/// document into a struct of strings and a `null` where it expects one is a
/// parse error for the whole request, not an empty value.
pub fn payload(nudge: &Nudge) -> serde_json::Value {
    let mut body = json!({ "title": nudge.title, "body": nudge.body });
    let map = body.as_object_mut().expect("a json object");
    if let Some(subtitle) = nudge.subtitle.as_deref().filter(|value| !value.is_empty()) {
        map.insert("subtitle".into(), json!(subtitle));
    }
    if let Some(group) = nudge.group.as_deref().filter(|value| !value.is_empty()) {
        map.insert("group".into(), json!(group));
    }
    if let Some(url) = nudge.url.as_deref().filter(|value| !value.is_empty()) {
        map.insert("url".into(), json!(url));
    }
    map.insert("level".into(), json!(nudge.level.as_str()));
    // The notification is worth keeping: the point of this feature is that the
    // user is somewhere else when it arrives, so the history is where they will
    // read it from.
    map.insert("isArchive".into(), json!("1"));
    body
}

/// Deliver one nudge, or say why it did not go.
///
/// The error is returned rather than logged-and-swallowed because there is
/// always somebody to tell: the settings page's test button, or the agent's own
/// `rh-notify`, which must not report success for a notification the user will
/// never see.
pub async fn send(endpoint: &str, nudge: &Nudge) -> Result<(), String> {
    if !endpoint_is_acceptable(endpoint) {
        return Err("the configured endpoint is not an https URL".to_string());
    }
    let client = CLIENT.as_ref().ok_or_else(|| "no HTTP client".to_string())?;
    let response = client
        .post(endpoint)
        .json(&payload(nudge))
        .send()
        .await
        .map_err(|error| format!("{}: {error}", endpoint_origin(endpoint)))?;
    let status = response.status();
    if !status.is_success() {
        // Bark says why in the body, and the body is the only part of this worth
        // repeating: the URL holds the device key.
        let detail = response.text().await.unwrap_or_default();
        let detail = detail.trim();
        let detail: String = detail.chars().take(200).collect();
        return Err(if detail.is_empty() {
            format!("{} answered http {status}", endpoint_origin(endpoint))
        } else {
            format!("{} answered http {status}: {detail}", endpoint_origin(endpoint))
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::Level;
    use std::sync::Mutex;

    /// Serializes the tests that read or mutate the process-global opt-in.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn every_shape_a_person_has_in_their_clipboard_normalises() {
        assert_eq!(
            normalize_endpoint("https://api.day.app/AbCd1234"),
            Some("https://api.day.app/AbCd1234".into())
        );
        assert_eq!(
            normalize_endpoint("  https://api.day.app/AbCd1234/  "),
            Some("https://api.day.app/AbCd1234".into()),
            "the app's copy button and a hand-typed slash are one endpoint"
        );
        assert_eq!(
            normalize_endpoint("AbCd1234"),
            Some("https://api.day.app/AbCd1234".into()),
            "the app displays the key, not the URL"
        );
        assert_eq!(
            normalize_endpoint("https://bark.example.com:8080/AbCd1234"),
            Some("https://bark.example.com:8080/AbCd1234".into()),
            "a self-hosted bark-server is the same protocol"
        );

        assert_eq!(normalize_endpoint(""), None);
        assert_eq!(normalize_endpoint("   "), None);
        assert_eq!(normalize_endpoint("short"), None, "too short to be a device key");
        assert_eq!(normalize_endpoint("api.day.app/AbCd1234"), None, "a host is not a key");
        assert_eq!(normalize_endpoint("https://api.day.app"), None, "an origin with no key notifies nobody");
        assert_eq!(normalize_endpoint("https://api.day.app/"), None);
        assert_eq!(normalize_endpoint("ftp://api.day.app/AbCd1234"), None);
    }

    #[test]
    fn only_the_origin_of_an_endpoint_is_ever_repeatable() {
        assert_eq!(endpoint_origin("https://api.day.app/AbCd1234"), "https://api.day.app");
        assert_eq!(endpoint_origin("http://127.0.0.1:9331/AbCd1234"), "http://127.0.0.1:9331");
        assert_eq!(endpoint_origin(""), "");
        assert_eq!(endpoint_origin("not a url"), "<unparseable>");
        assert!(
            !endpoint_origin("https://api.day.app/AbCd1234").contains("AbCd1234"),
            "the last path segment is the credential"
        );
    }

    #[test]
    fn the_mask_identifies_an_endpoint_without_handing_it_over() {
        let endpoint = "https://api.day.app/FAKEBARKDEVICEKEY0Wc31";
        let mask = endpoint_mask(endpoint);
        assert_eq!(mask, "https://api.day.app/\u{2022}\u{2022}\u{2022}\u{2022}Wc31");
        assert!(!mask.contains("FAKEBARKDEVICEKEY0"), "the usable part of the key is gone");

        assert_eq!(endpoint_mask(""), "");
        assert_eq!(
            endpoint_mask("https://api.day.app/short12"),
            "https://api.day.app/\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}",
            "a key with too little to spare is hidden outright"
        );
        assert_eq!(
            endpoint_mask("http://127.0.0.1:9331/FAKEBARKDEVICEKEY0Wc31"),
            "http://127.0.0.1:9331/\u{2022}\u{2022}\u{2022}\u{2022}Wc31"
        );
    }

    #[test]
    fn plain_http_is_refused_unless_it_is_loopback_and_opted_in() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        assert!(endpoint_is_acceptable("https://api.day.app/AbCd1234"));
        assert!(!endpoint_is_acceptable("ftp://api.day.app/AbCd1234"));
        assert!(!endpoint_is_acceptable("nonsense"));
        // The opt-in is a process-wide environment variable, so this asserts the
        // rule it implies rather than flipping it: with the opt-in off, no http
        // endpoint is acceptable; with it on, only loopback ones are.
        let allowed = insecure_endpoints_allowed();
        assert_eq!(endpoint_is_acceptable("http://127.0.0.1:9331/AbCd1234"), allowed);
        assert!(
            !endpoint_is_acceptable("http://bark.example.com/AbCd1234"),
            "a remote plain-http endpoint is never acceptable"
        );
    }

    #[test]
    fn a_switch_set_to_off_is_off() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        for value in ["0", "false", "", "no", "off"] {
            std::env::set_var(ALLOW_INSECURE_ENDPOINT_ENV, value);
            assert!(!insecure_endpoints_allowed(), "{value:?}");
            assert!(!endpoint_is_acceptable("http://127.0.0.1:9331/AbCd1234"), "{value:?}");
        }
        std::env::remove_var(ALLOW_INSECURE_ENDPOINT_ENV);
        assert!(!insecure_endpoints_allowed(), "unset");
    }

    #[test]
    fn the_payload_carries_what_bark_understands_and_omits_the_rest() {
        let bare = payload(&Nudge {
            title: "Blocked on a decision".into(),
            body: "Postgres or SQLite?".into(),
            subtitle: None,
            group: None,
            url: None,
            level: Level::Active,
        });
        assert_eq!(bare["title"], "Blocked on a decision");
        assert_eq!(bare["body"], "Postgres or SQLite?");
        assert_eq!(bare["level"], "active");
        assert_eq!(bare["isArchive"], "1");
        let map = bare.as_object().expect("object");
        assert!(!map.contains_key("subtitle"), "an absent field is absent, never null");
        assert!(!map.contains_key("url"));
        assert!(!map.contains_key("group"));

        let full = payload(&Nudge {
            title: "Done".into(),
            body: "All 41 tests pass.".into(),
            subtitle: Some("remote-hand · bark notifications".into()),
            group: Some("remote-hand".into()),
            url: Some("https://rh.example/sessions/claude/abc".into()),
            level: Level::TimeSensitive,
        });
        assert_eq!(full["subtitle"], "remote-hand · bark notifications");
        assert_eq!(full["group"], "remote-hand");
        assert_eq!(full["url"], "https://rh.example/sessions/claude/abc");
        assert_eq!(full["level"], "timeSensitive");

        // An empty string is the shape a client sends for "I have nothing here",
        // and Bark renders it as a blank line rather than as nothing.
        let empty = payload(&Nudge {
            title: "Done".into(),
            body: "…".into(),
            subtitle: Some(String::new()),
            group: Some(String::new()),
            url: Some(String::new()),
            level: Level::Passive,
        });
        let map = empty.as_object().expect("object");
        assert!(!map.contains_key("subtitle"));
        assert!(!map.contains_key("group"));
        assert!(!map.contains_key("url"));
    }
}
