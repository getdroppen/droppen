//! Web Push, end to end: a real socket, a real subscription, a real decrypt.
//!
//! The unit tests in `src/push` pin the cryptography against RFC 8291's own
//! vector and the store against its own file. What only this level can show is
//! that the two are wired to each other and to the thing that triggers them:
//! that a CLI run finishing produces exactly one `POST`, to the endpoint the
//! browser registered, carrying a body the browser's key can open, with the
//! payload the service worker expects inside it.
//!
//! So the fake endpoint here is not a mock. It is an HTTP server that keeps
//! every request it receives, and the assertions run the captured bytes back
//! through P-256 and AES-128-GCM with the subscription's private key. If the
//! server encrypted against the wrong key, framed the header wrong, or sent the
//! right bytes to the wrong URL, none of that survives.

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use p256::elliptic_curve::sec1::ToEncodedPoint;
use remote_hand_server::db::scope;
use remote_hand_server::push::ece;
use serde_json::{json, Value};
use support::{common, plan_dying_cli, Probe};

/// One session per test: the notification throttle is process-global (it is
/// keyed by session, which is the point), so two tests sharing an id would
/// throttle each other.
const DELIVERED_ID: &str = "aaaaaaaa-0001-4000-8000-aaaaaaaaaaaa";
const GONE_ID: &str = "aaaaaaaa-0002-4000-8000-aaaaaaaaaaaa";
const SEEN_ID: &str = "aaaaaaaa-0003-4000-8000-aaaaaaaaaaaa";
const FAILED_ID: &str = "aaaaaaaa-0004-4000-8000-aaaaaaaaaaaa";
const REDIRECT_ID: &str = "aaaaaaaa-0005-4000-8000-aaaaaaaaaaaa";

/// The send-path tests all talk to a loopback listener over plain http, which
/// `endpoint_is_acceptable` only allows when this env opt-in is set: production
/// requires https. It only ever loosens and never flaps, so setting it from
/// several parallel tests in this binary is safe.
fn allow_loopback_endpoints() {
    std::env::set_var("DROPPEN_PUSH_ALLOW_INSECURE_ENDPOINT", "1");
}

/* ── a push service that keeps what it is sent ───────────────────────── */

/// One captured request.
struct Captured {
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Captured {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Clone)]
struct Received(Arc<Mutex<Vec<Captured>>>);

/// A push service on loopback, answering every `POST` with a fixed status.
struct FakeEndpoint {
    url: String,
    received: Received,
}

impl FakeEndpoint {
    async fn start(status: u16) -> FakeEndpoint {
        use axum::extract::State;
        use axum::http::{HeaderMap, StatusCode};

        async fn receive(State((received, status)): State<(Received, u16)>, headers: HeaderMap, body: axum::body::Bytes) -> StatusCode {
            received.0.lock().expect("captured poisoned").push(Captured {
                headers: headers
                    .iter()
                    .map(|(name, value)| (name.as_str().to_string(), String::from_utf8_lossy(value.as_bytes()).into_owned()))
                    .collect(),
                body: body.to_vec(),
            });
            StatusCode::from_u16(status).expect("a status")
        }

        let received = Received(Arc::new(Mutex::new(Vec::new())));
        let app = axum::Router::new()
            .route("/push", axum::routing::post(receive))
            .with_state((received.clone(), status));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        FakeEndpoint {
            url: format!("http://127.0.0.1:{}/push", addr.port()),
            received,
        }
    }

    /// A push service that records what it is sent and then answers every `POST`
    /// with a `307` to `location`. Used to prove the send path does not follow a
    /// redirect: it reaches this first hop but never the place it points at.
    async fn start_redirect(location: String) -> FakeEndpoint {
        use axum::extract::State;
        use axum::http::HeaderMap;
        use axum::response::Redirect;

        async fn receive(State((received, location)): State<(Received, String)>, headers: HeaderMap, body: axum::body::Bytes) -> Redirect {
            received.0.lock().expect("captured poisoned").push(Captured {
                headers: headers
                    .iter()
                    .map(|(name, value)| (name.as_str().to_string(), String::from_utf8_lossy(value.as_bytes()).into_owned()))
                    .collect(),
                body: body.to_vec(),
            });
            // 307 preserves the method and body, which is exactly what a
            // redirect-following client would re-`POST` to the canary.
            Redirect::temporary(&location)
        }

        let received = Received(Arc::new(Mutex::new(Vec::new())));
        let app = axum::Router::new()
            .route("/push", axum::routing::post(receive))
            .with_state((received.clone(), location));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        FakeEndpoint {
            url: format!("http://127.0.0.1:{}/push", addr.port()),
            received,
        }
    }

    fn count(&self) -> usize {
        self.received.0.lock().expect("captured poisoned").len()
    }

    /// Wait until at least `n` requests have arrived, or fail the test.
    async fn wait_for(&self, n: usize) {
        common::wait_for(&format!("{n} push request(s)"), Duration::from_secs(10), || self.count() >= n).await;
    }

    /// Give anything still in flight a chance to land, for asserting an absence.
    async fn settle(&self) {
        tokio::time::sleep(Duration::from_millis(600)).await;
    }

    fn take(&self) -> Vec<Captured> {
        std::mem::take(&mut *self.received.0.lock().expect("captured poisoned"))
    }
}

/* ── a subscription whose private half the test keeps ────────────────── */

/// The browser side of a subscription: the keys go to the server, the private
/// scalar stays here so the captured payload can actually be opened.
struct Browser {
    private: p256::SecretKey,
    auth: [u8; ece::AUTH_SECRET_LEN],
}

impl Browser {
    fn new() -> Browser {
        Browser {
            private: p256::SecretKey::random(&mut rand::rngs::OsRng),
            auth: [0x5a; ece::AUTH_SECRET_LEN],
        }
    }

    fn subscription(&self, endpoint: &str) -> Value {
        json!({
            "endpoint": endpoint,
            "keys": {
                "p256dh": ece::b64url(self.private.public_key().to_encoded_point(false).as_bytes()),
                "auth": ece::b64url(&self.auth),
            },
        })
    }

    /// What the service worker would read out of a captured request.
    fn open(&self, captured: &Captured) -> Value {
        let plaintext = ece::decrypt(&self.private, &self.auth, &captured.body).expect("the payload decrypts under the subscription key");
        serde_json::from_slice(&plaintext).expect("the payload is JSON")
    }
}

/* ── fixtures ────────────────────────────────────────────────────────── */

/// A session whose assistant has spoken, so it counts as unread.
fn seed_spoken_session(probe: &Probe, id: &str) {
    let workspace = probe.workspace();
    // Owned by this server: the fixture is driven with a send, and only an owned
    // session can be.
    let entries = [
        json!({
            "type": "user",
            "cwd": workspace.to_string_lossy(),
            "timestamp": "2026-01-01T10:00:00.000Z",
            "message": {"content": [{"type": "text", "text": "summarise the report"}]},
        }),
        json!({
            "type": "assistant",
            "timestamp": "2026-01-01T10:00:30.000Z",
            "message": {"content": [{"type": "text", "text": "All three sections are done and the numbers reconcile."}]},
        }),
    ];
    common::seed_transcript(&probe.managed_root(), "-workspace", id, &entries);
}

/// Run one injected turn to completion.
///
/// `Ending::Died` is the resident-era shape of a failed turn: the CLI dies in
/// the middle of it rather than finishing with a non-zero exit, because with one
/// process per session those are no longer the same event.
async fn run_a_turn(probe: &Probe, id: &str, ending: Ending) -> Value {
    if ending == Ending::Died {
        plan_dying_cli(&probe.workspace(), "", 3);
    }
    probe
        .send_ok("POST", &format!("/api/sessions/claude/{id}/message"), json!({"text": "carry on"}), 200)
        .await;
    probe.wait_for_injection("claude", id).await
}

/// Every endpoint this instance is subscribed to, in the order it first saw them.
///
/// The subscriptions are rows in the instance's state database and there is no
/// route that lists them, so this is the only window onto "what would the next
/// notification actually be sent to" — which is the question both the upsert and
/// the `410` reaper are about.
fn stored_endpoints(probe: &Probe) -> Vec<String> {
    common::rows(&probe.instance_db(), "SELECT endpoint FROM push_subscriptions ORDER BY rowid", ())
}

/// How the turn [`run_a_turn`] drives is meant to end.
#[derive(PartialEq, Eq)]
enum Ending {
    Answered,
    Died,
}

/* ── the REST surface ────────────────────────────────────────────────── */

#[tokio::test]
async fn the_application_server_key_is_minted_once_and_handed_out_as_a_point() {
    let probe = Probe::start().await;

    let first = probe.get_json("/api/push/vapid").await;
    let key = first["publicKey"].as_str().expect("publicKey");
    assert_eq!(
        ece::b64url_decode(key).expect("base64url").len(),
        ece::POINT_LEN,
        "an uncompressed P-256 point, which is what `applicationServerKey` wants"
    );

    let again = probe.get_json("/api/push/vapid").await;
    assert_eq!(again["publicKey"], first["publicKey"], "rotating it would invalidate every device");

    // The key is stored, and stored once: a second row would mean a second
    // identity, and every device subscribed against the loser can never be
    // delivered to again.
    let stored = common::rows(
        &probe.instance_db(),
        "SELECT value FROM kv WHERE scope = ?1 AND key = 'vapidPrivate'",
        (scope::PUSH,),
    );
    assert_eq!(stored.len(), 1, "one key, minted on first use");
    assert_ne!(stored[0], key, "and it is the private half that is kept, not the point handed out");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // `push.json` was `0600` because it held the private half. Nothing reads
        // or writes it any more — the mode has to have moved with the secret
        // rather than lapsed, so it is the database that is asserted on, and the
        // file that is asserted *absent*. A key sitting world-readable beside a
        // locked database would be the exact regression this pins.
        let mode = std::fs::metadata(probe.instance_db()).expect("state.db").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the database holds the private half now");
        assert!(
            !probe.instance_dir().join("push.json").exists(),
            "and there is no second, unprotected copy of it"
        );
    }

    probe.stop().await;
}

#[tokio::test]
async fn a_subscription_is_stored_replaced_and_forgotten() {
    let probe = Probe::start().await;
    let browser = Browser::new();
    let endpoint = "https://push.example.test/one";

    probe
        .send_ok("POST", "/api/push/subscriptions", browser.subscription(endpoint), 201)
        .await;
    // No route lists the subscribed devices — a management UI is the "future"
    // the `label` field is kept for — so the store is where this has to be
    // asked. `endpoint` is the primary key, which is the whole of what makes a
    // re-subscribe idempotent.
    assert_eq!(stored_endpoints(&probe), [endpoint]);

    // The browser re-subscribed under the same endpoint.
    probe
        .send_ok("POST", "/api/push/subscriptions", Browser::new().subscription(endpoint), 201)
        .await;
    assert_eq!(
        stored_endpoints(&probe),
        [endpoint],
        "the endpoint is the identity, so this replaced rather than added"
    );

    let (status, body) = probe.send("DELETE", "/api/push/subscriptions", json!({"endpoint": endpoint})).await;
    assert_eq!(status, 200, "{body}");
    assert!(stored_endpoints(&probe).is_empty());

    let (status, _) = probe.send("DELETE", "/api/push/subscriptions", json!({"endpoint": endpoint})).await;
    assert_eq!(status, 200, "unsubscribing twice is not an error");

    probe.stop().await;
}

/// A bad row would sit in the file failing every notification forever, and the
/// browser that sent it would never hear about it — so it is refused on arrival.
#[tokio::test]
async fn a_malformed_subscription_is_refused_on_arrival() {
    let probe = Probe::start().await;
    let browser = Browser::new();

    let mut wrong_scheme = browser.subscription("http://push.example.test/one");
    wrong_scheme["endpoint"] = json!("http://push.example.test/one");
    let (status, body) = probe.send("POST", "/api/push/subscriptions", wrong_scheme).await;
    assert_eq!(status, 400, "a real push service is never plain HTTP: {body}");

    let mut short_auth = browser.subscription("https://push.example.test/one");
    short_auth["keys"]["auth"] = json!(ece::b64url(&[1u8; 8]));
    let (status, body) = probe.send("POST", "/api/push/subscriptions", short_auth).await;
    assert_eq!(status, 400, "the auth secret is exactly 16 bytes: {body}");

    let mut compressed = browser.subscription("https://push.example.test/one");
    compressed["keys"]["p256dh"] = json!(ece::b64url(&[0x02; ece::POINT_LEN]));
    let (status, body) = probe.send("POST", "/api/push/subscriptions", compressed).await;
    assert_eq!(status, 400, "RFC 8291 wants an uncompressed point: {body}");

    // The right length and `0x04` tag, but not a point on the curve: it would
    // fail every send, so it is refused here rather than stored.
    let mut off_curve = browser.subscription("https://push.example.test/one");
    off_curve["keys"]["p256dh"] = json!(ece::b64url(&[0x04; ece::POINT_LEN]));
    let (status, body) = probe.send("POST", "/api/push/subscriptions", off_curve).await;
    assert_eq!(status, 400, "an off-curve point is not a usable key: {body}");

    probe.stop().await;
}

/* ── the trigger ─────────────────────────────────────────────────────── */

#[tokio::test]
async fn a_finished_turn_delivers_one_notification_the_browser_can_open() {
    allow_loopback_endpoints();
    let probe = Probe::start().await;
    let endpoint = FakeEndpoint::start(201).await;
    let browser = Browser::new();
    seed_spoken_session(&probe, DELIVERED_ID);
    probe
        .send_ok("POST", "/api/push/subscriptions", browser.subscription(&endpoint.url), 201)
        .await;

    let injection = run_a_turn(&probe, DELIVERED_ID, Ending::Answered).await;
    assert_eq!(injection["state"], "ok", "{injection}");

    endpoint.wait_for(1).await;
    let captured = endpoint.take();
    assert_eq!(captured.len(), 1, "one turn, one notification");
    let request = &captured[0];

    assert_eq!(request.header("content-encoding"), Some("aes128gcm"), "RFC 8291's only encoding");
    assert_eq!(request.header("content-type"), Some("application/octet-stream"));
    assert_eq!(request.header("ttl"), Some("86400"));
    let authorization = request.header("authorization").expect("an Authorization header");
    assert!(authorization.starts_with("vapid t="), "RFC 8292 §3: {authorization}");
    assert!(authorization.contains(", k="), "…and it names the key it was signed with");

    let payload = browser.open(request);
    assert_eq!(
        payload["tag"],
        format!("claude/{DELIVERED_ID}"),
        "the tag is the session, so the shade collapses"
    );
    assert_eq!(
        payload["url"],
        format!("/sessions/claude/{DELIVERED_ID}"),
        "and tapping it opens that session"
    );
    assert_eq!(payload["title"], "summarise the report", "the session's own title");
    assert_eq!(
        payload["body"], "All three sections are done and the numbers reconcile.",
        "the last thing the assistant actually said"
    );

    // A second turn inside the window is the same session saying the same thing
    // again; the shade gets one line, not two.
    let injection = run_a_turn(&probe, DELIVERED_ID, Ending::Answered).await;
    assert_eq!(injection["state"], "ok", "{injection}");
    endpoint.settle().await;
    assert_eq!(endpoint.count(), 0, "throttled: the 60 s window is still open");

    probe.stop().await;
}

/// RFC 8030 §7.3: a `410` means the browser has unregistered and no retry will
/// bring it back. Keeping the row would mean failing forever, once per turn.
#[tokio::test]
async fn a_gone_subscription_is_forgotten_on_the_spot() {
    allow_loopback_endpoints();
    let probe = Probe::start().await;
    let endpoint = FakeEndpoint::start(410).await;
    let browser = Browser::new();
    seed_spoken_session(&probe, GONE_ID);
    probe
        .send_ok("POST", "/api/push/subscriptions", browser.subscription(&endpoint.url), 201)
        .await;

    run_a_turn(&probe, GONE_ID, Ending::Answered).await;
    endpoint.wait_for(1).await;

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if stored_endpoints(&probe).is_empty() {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "the dead subscription was never reaped");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    probe.stop().await;
}

/// The whole point of the unread rule: if you are already looking at it, the
/// screen has told you everything the notification would.
#[tokio::test]
async fn a_turn_that_finishes_on_a_session_already_seen_is_not_announced() {
    allow_loopback_endpoints();
    let probe = Probe::start().await;
    let endpoint = FakeEndpoint::start(201).await;
    seed_spoken_session(&probe, SEEN_ID);
    probe
        .send_ok("POST", "/api/push/subscriptions", Browser::new().subscription(&endpoint.url), 201)
        .await;
    probe
        .send_ok("POST", &format!("/api/sessions/claude/{SEEN_ID}/seen"), json!({}), 200)
        .await;

    let injection = run_a_turn(&probe, SEEN_ID, Ending::Answered).await;
    assert_eq!(injection["state"], "ok", "{injection}");
    endpoint.settle().await;
    assert_eq!(endpoint.count(), 0, "nothing to tell someone who is already looking");

    probe.stop().await;
}

/// A run that died is not going to tell you itself, so this arm ignores the
/// unread rule entirely — the session below has been marked seen and it still
/// goes out.
#[tokio::test]
async fn a_failed_run_is_announced_even_to_someone_who_is_watching() {
    allow_loopback_endpoints();
    let probe = Probe::start().await;
    let endpoint = FakeEndpoint::start(201).await;
    let browser = Browser::new();
    seed_spoken_session(&probe, FAILED_ID);
    probe
        .send_ok("POST", "/api/push/subscriptions", browser.subscription(&endpoint.url), 201)
        .await;
    probe
        .send_ok("POST", &format!("/api/sessions/claude/{FAILED_ID}/seen"), json!({}), 200)
        .await;

    let injection = run_a_turn(&probe, FAILED_ID, Ending::Died).await;
    assert_eq!(injection["state"], "failed", "{injection}");

    endpoint.wait_for(1).await;
    let captured = endpoint.take();
    let payload = browser.open(&captured[0]);
    let body = payload["body"].as_str().expect("a body");
    assert!(body.starts_with("Run failed:"), "it says what happened: {body}");
    assert_eq!(payload["tag"], format!("claude/{FAILED_ID}"));

    probe.stop().await;
}

/// A subscription endpoint that `307`s to a loopback address must not turn the
/// send path into an SSRF. `endpoint_is_acceptable` only vets the first hop, so
/// the guarantee is the client's: it follows no redirect. The canary the
/// redirect points at is a service that would only ever be reachable from the
/// server's own host, and it must never be hit.
#[tokio::test]
async fn a_push_endpoint_redirect_is_never_followed_to_a_loopback_canary() {
    allow_loopback_endpoints();
    let probe = Probe::start().await;
    // Where the redirect points: a loopback-only service the send path must not
    // be tricked into reaching.
    let canary = FakeEndpoint::start(200).await;
    let redirector = FakeEndpoint::start_redirect(canary.url.clone()).await;
    let browser = Browser::new();
    seed_spoken_session(&probe, REDIRECT_ID);
    probe
        .send_ok("POST", "/api/push/subscriptions", browser.subscription(&redirector.url), 201)
        .await;

    let injection = run_a_turn(&probe, REDIRECT_ID, Ending::Answered).await;
    assert_eq!(injection["state"], "ok", "{injection}");

    // The first hop is reached — the send genuinely went out...
    redirector.wait_for(1).await;
    // ...and the redirect is refused: nothing follows it to the canary.
    canary.settle().await;
    assert_eq!(canary.count(), 0, "the send path must not follow a push endpoint's redirect");

    probe.stop().await;
}
