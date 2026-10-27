//! The two account wizards, end to end through a real pty.
//!
//! Mirrors the former TypeScript server's `setup-token.test.ts` and the spawn
//! half of the former TypeScript server's `usage-login.test.ts`: a fake
//! `claude` prints a token the way the real one does — trailing ANSI-wrapped
//! hint and all — and the assertions are about what the server did with it.
//! Nothing here contacts Anthropic, and no real `claude` is ever run.
//!
//! The fake is driven by files under the server's *data* directory rather than
//! by arguments, because the wizard chooses the working directory itself:
//! `setup-token` runs in `<data>/setup-token/<captureId>`, so `../..` from the
//! script's own `$PWD` is the one place a test can leave it a note.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde_json::{json, Value};

mod support;

use support::common;

/// The token the fake prints; 70 body characters, as a real one has.
fn token() -> String {
    format!("sk-ant-oat01-{}", "A".repeat(70))
}

/// The authorization URL the fake prints, as a terminal hyperlink.
const AUTH_URL: &str = "https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c250a&scope=user%3Ainference&state=EDR4cIjOHOlb";

/// A `claude` that answers the wizard sub-commands and the credential check.
///
/// `setup-token` prints the token followed immediately by an ANSI-wrapped hint,
/// which is the shape that once got spliced onto the token itself. `auth login`
/// records the environment it was handed and then stays up, the way a real login
/// waits for the browser. `-p` with a token in the environment is the credential
/// check, and stands in for Anthropic: it accepts everything unless the test has
/// said which token is the real one.
///
/// Markers in the *data* directory choose the variant, because the wizard picks
/// its own working directory: `setup-token` runs in `<data>/setup-token/<id>`
/// and the check in `<data>/verify/<id>/cwd`, so `..` from each script's own
/// `$PWD` is the one place a test can leave it a note.
///
/// * `linger` — stay up after printing, so the replay buffer can be inspected;
/// * `oauth` — print the authorization URL and wait for a pasted code first;
/// * `wrap` — break the token across two lines, the way the CLI's own wrapper
///   breaks it at the terminal width;
/// * `only-whole-tokens` — the one token the check will accept;
/// * `rate-limited` — the check answers 429, which is neither a yes nor a no.
fn fake_claude() -> String {
    let token = token();
    format!(
        r#"#!/bin/sh
dir="$PWD"
if [ "$1" = "setup-token" ]; then
  data="$dir/../.."
  env > "$data/setup-env.txt"
  # Read once, with no waiting: the width the CLI would wrap at has to be right
  # from the first instant, because the first thing it prints is already wrapped
  # by then.
  printf '%s' "$(stty size 2>/dev/null)" > "$data/tty-size.txt"
  # A real wizard takes a browser round trip to get here; this one only has to
  # take long enough that the terminal it runs in is observable at all.
  sleep 0.05
  if [ -f "$data/oauth" ]; then
    printf 'Browser did not open? Use the url below to sign in\r\n'
    printf '\033]8;id=xf3kl4;%s\007%s\033]8;;\007\r\n' '{url}' '{visible}'
    printf '\r\nPaste code here if prompted > '
    read -r code
    printf '%s' "$code" > "$data/code.txt"
    printf '\r\n'
  fi
  printf 'Here is your token:\r\n'
  if [ -f "$data/wrap" ]; then
    printf '%s\r\n%s' '{head}' '{tail}'
  else
    printf '%s' '{token}'
  fi
  printf '\033[2mStore this token somewhere safe\033[0m\r\n'
  if [ -f "$data/linger" ]; then sleep 30; fi
  exit 0
fi
if [ "$1" = "auth" ] && [ "$2" = "login" ]; then
  env > "$dir/login-env.tmp"
  mv "$dir/login-env.tmp" "$dir/login-env.txt"
  sleep 30
  exit 0
fi
if [ "$1" = "-p" ] && [ -n "$CLAUDE_CODE_OAUTH_TOKEN" ]; then
  data="$dir/../../.."
  echo check >> "$data/checks.txt"
  if [ -f "$data/only-whole-tokens" ] && [ "$CLAUDE_CODE_OAUTH_TOKEN" != "$(cat "$data/only-whole-tokens")" ]; then
    printf 'Invalid bearer token\n' >&2
    exit 1
  fi
  if [ -f "$data/rate-limited" ]; then
    printf 'API Error: 429 Too Many Requests\n' >&2
    exit 1
  fi
  exit 0
fi
exit 0
"#,
        url = AUTH_URL,
        // Wrapped to 80 columns on screen; the whole URL is in the escape.
        visible = &AUTH_URL[..80],
        // 80 columns of token, then the rest — a plausible-looking truncation.
        head = &token[..80],
        tail = &token[80..],
    )
}

/// Put the fake `claude` on `PATH`, once per test binary.
///
/// `PATH` is process-global and the harness runs tests in threads, so this is
/// the one piece of shared mutable state here — and it says the same thing for
/// every test in the binary, which is why it can be set once and never changed.
/// Everything a single test needs to vary lives under that test's own data
/// directory instead.
fn ensure_fake_claude_on_path() {
    let script = fake_claude();
    support::fake_bin("remote-hand-wizard-bin-", &[("claude", &script)]);
}

async fn start() -> common::TestServer {
    ensure_fake_claude_on_path();
    common::TestServer::start().await
}

fn data_dir(server: &common::TestServer) -> PathBuf {
    server.home.join(".local/share/remote-hand")
}

/// `POST`/`GET` a JSON route, asserting the status so the happy path is one line.
async fn post(server: &common::TestServer, path: &str, body: Value, expect: u16) -> Value {
    let (status, raw) = server.json("POST", path, &body).await;
    assert_eq!(status, expect, "POST {path} -> {status}: {raw}");
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("POST {path} returned non-JSON ({e}): {raw}"))
}

/// Poll a capture until it leaves `pending`.
async fn settle(server: &common::TestServer, capture_id: &str) -> Value {
    let path = format!("/api/accounts/setup-token/{capture_id}");
    let path = path.as_str();
    common::wait_for_async(
        &format!("capture {capture_id} to settle"),
        Duration::from_secs(15),
        move || async move {
            let status = server.get_json(path).await;
            (status["status"] != json!("pending")).then_some(status)
        },
    )
    .await
}

/// Wait until `predicate` holds of the live terminal list, or give up.
async fn wait_for_terminals(server: &common::TestServer, what: &str, predicate: impl Fn(&Value) -> bool) -> Value {
    let predicate = &predicate;
    common::wait_for_async(&format!("terminals to {what}"), Duration::from_secs(15), move || async move {
        let list = server.get_json("/api/terminals").await;
        predicate(&list).then_some(list)
    })
    .await
}

/// Wait until a path stops existing, which the wizard's cleanup does off-thread.
async fn wait_gone(path: &Path) {
    common::wait_for_async(
        &format!("{} to be removed", path.display()),
        Duration::from_secs(15),
        move || async move { (!path.exists()).then_some(()) },
    )
    .await
}

fn lists_terminal(list: &Value, id: &str) -> bool {
    list["terminals"]
        .as_array()
        .expect("terminals array")
        .iter()
        .any(|terminal| terminal["id"] == json!(id))
}

#[tokio::test]
async fn the_wizard_captures_the_token_stores_the_account_and_cleans_up_after_itself() {
    let server = start().await;
    let started = post(&server, "/api/accounts/setup-token", json!({ "label": "Wizard" }), 201).await;
    let capture_id = started["captureId"].as_str().expect("captureId").to_string();
    let terminal_id = started["terminalId"].as_str().expect("terminalId").to_string();

    // The wizard terminal is an ordinary one: it lists and attaches like any other.
    assert!(lists_terminal(&server.get_json("/api/terminals").await, &terminal_id));
    let scratch = data_dir(&server).join("setup-token").join(&capture_id);
    assert!(scratch.join("claude").is_dir(), "the throwaway config dir should exist");

    let capture = settle(&server, &capture_id).await;
    assert_eq!(capture["status"], json!("captured"), "{capture}");
    let account_id = capture["accountId"].as_str().expect("accountId").to_string();
    assert!(capture.get("reused").is_none(), "a first capture is not a reuse: {capture}");
    // Nothing in the status payload resembles the token.
    assert!(!capture.to_string().contains("sk-ant-oat01-"), "{capture}");

    let accounts = server.get_json("/api/accounts").await;
    let stored = accounts["accounts"].as_array().expect("accounts");
    assert_eq!(stored.len(), 1, "{accounts}");
    assert_eq!(stored[0]["label"], json!("Wizard"));
    assert_eq!(stored[0]["kind"], json!("oauth_token"));
    assert_eq!(stored[0]["id"], json!(account_id));

    // The first defence against a wrapped token: the wizard's pty is far wider
    // than anything the CLI prints, from the instant the CLI starts, and no
    // attached browser can narrow it afterwards.
    assert_eq!(
        std::fs::read_to_string(data_dir(&server).join("tty-size.txt")).expect("the size the CLI saw"),
        "40 400",
        "the wizard has to run wide enough that nothing it prints is wrapped"
    );

    wait_gone(&scratch).await;
    server.stop().await;
}

#[tokio::test]
async fn the_captured_token_is_not_left_sitting_in_the_terminals_replay_buffer() {
    let server = start().await;
    // The CLI stays up after printing, so the terminal is still attachable when
    // the assertion runs — which is the only moment the buffer could leak.
    std::fs::create_dir_all(data_dir(&server)).expect("data dir");
    std::fs::write(data_dir(&server).join("linger"), "").expect("linger marker");

    let started = post(&server, "/api/accounts/setup-token", json!({ "label": "Replay" }), 201).await;
    let capture_id = started["captureId"].as_str().expect("captureId").to_string();
    let terminal_id = started["terminalId"].as_str().expect("terminalId").to_string();
    assert_eq!(settle(&server, &capture_id).await["status"], json!("captured"));

    // Whoever attaches now must not be repainted the token.
    let url = format!("ws://{}/api/terminals/{terminal_id}/ws", server.addr);
    let (mut socket, _) = tokio_tungstenite::connect_async(&url).await.expect("attach");
    let replay = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("a replay frame")
        .expect("a frame")
        .expect("a text frame")
        .into_text()
        .expect("text");
    assert!(!replay.contains("sk-ant-oat01-"), "the replay still held a credential: {replay:?}");
    drop(socket);

    // The grace period passes and the wizard terminal is reaped for good.
    wait_for_terminals(&server, "dropped the wizard terminal", |list| !lists_terminal(list, &terminal_id)).await;
    server.stop().await;
}

#[tokio::test]
async fn re_authorizing_a_token_already_on_file_keeps_the_stored_label_and_says_so() {
    let server = start().await;
    let first = post(&server, "/api/accounts/setup-token", json!({ "label": "Original" }), 201).await;
    let first = settle(&server, first["captureId"].as_str().expect("captureId")).await;
    assert_eq!(first["status"], json!("captured"), "{first}");

    let again = post(&server, "/api/accounts/setup-token", json!({ "label": "Typed something else" }), 201).await;
    let again = settle(&server, again["captureId"].as_str().expect("captureId")).await;
    assert_eq!(again["status"], json!("captured"), "{again}");
    assert_eq!(again["reused"], json!(true), "{again}");
    assert_eq!(again["accountId"], first["accountId"], "the same token is the same account");

    // One account, still under the label it was first stored with.
    let accounts = server.get_json("/api/accounts").await;
    let labels: Vec<&str> = accounts["accounts"]
        .as_array()
        .expect("accounts")
        .iter()
        .map(|a| a["label"].as_str().expect("label"))
        .collect();
    assert_eq!(labels, ["Original"], "{accounts}");
    server.stop().await;
}

/// Leave the fake a note in the server's data directory.
fn seed(server: &common::TestServer, name: &str, body: &str) {
    let data = data_dir(server);
    std::fs::create_dir_all(&data).expect("data dir");
    std::fs::write(data.join(name), body).expect("marker");
}

/// Poll a capture until it reports the URL the user has to authorize.
async fn wait_for_auth_url(server: &common::TestServer, capture_id: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let status = server.get_json(&format!("/api/accounts/setup-token/{capture_id}")).await;
        if let Some(url) = status["authUrl"].as_str() {
            return url.to_string();
        }
        assert!(Instant::now() < deadline, "capture {capture_id} never printed a URL: {status}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// The 401 the capture's rejoining and the credential check exist for, end to end.
///
/// The CLI hard-wraps at the terminal width, so the token arrives as two lines.
/// The first line on its own is the right length, the right alphabet and the
/// right prefix — indistinguishable from a whole token until it is *used*. So
/// the capture offers both readings and the check decides, and what lands in the
/// store is the token that works.
#[tokio::test]
async fn a_wrapped_token_is_stored_whole_because_the_truncated_reading_is_refused() {
    let server = start().await;
    seed(&server, "wrap", "");
    seed(&server, "only-whole-tokens", &token());

    let started = post(&server, "/api/accounts/setup-token", json!({ "label": "Wrapped" }), 201).await;
    let capture = settle(&server, started["captureId"].as_str().expect("captureId")).await;
    assert_eq!(capture["status"], json!("captured"), "{capture}");

    // The whole token is the one on file: storing it again is a duplicate, and
    // the first 80 characters of it are refused before they can be stored at all.
    let (status, body) = server
        .json(
            "POST",
            "/api/accounts",
            &json!({"label": "Same", "kind": "oauth_token", "secret": token()}),
        )
        .await;
    assert_eq!(status, 409, "the stored secret is the whole token: {body}");
    let (status, body) = server
        .json(
            "POST",
            "/api/accounts",
            &json!({"label": "Short", "kind": "oauth_token", "secret": &token()[..80]}),
        )
        .await;
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("Invalid bearer token"), "the CLI's own words reach the user: {body}");
    assert!(!body.contains("sk-ant-oat01-A"), "and the token itself does not: {body}");
    server.stop().await;
}

/// The failure that must never end in a stored token: the terminal wrapped it,
/// so there are two readings, and the check that would say which is which cannot
/// run. Storing either one is a coin flip whose losing side is an account that
/// 401s on every message — so the capture fails and says why.
///
/// This is the re-authorization case as it actually happens: the account is
/// re-authorized *because* its quota ran out, and a quota that has run out is
/// exactly what answers a check with a 429.
#[tokio::test]
async fn a_wrapped_token_is_not_stored_when_the_check_cannot_say_which_reading_is_right() {
    let server = start().await;
    seed(&server, "wrap", "");
    seed(&server, "rate-limited", "");

    let started = post(&server, "/api/accounts/setup-token", json!({ "label": "Wrapped" }), 201).await;
    let capture = settle(&server, started["captureId"].as_str().expect("captureId")).await;
    assert_eq!(capture["status"], json!("error"), "{capture}");
    let error = capture["error"].as_str().expect("an error to show the user");
    assert!(error.contains("rate-limited"), "the reason the user can act on: {error}");
    assert!(error.contains("run the wizard again"), "{error}");
    assert!(!error.contains("sk-ant-oat01-"), "and never the token itself: {error}");

    let accounts = server.get_json("/api/accounts").await;
    assert_eq!(
        accounts["accounts"].as_array().expect("accounts").len(),
        0,
        "nothing may be stored on a coin flip: {accounts}"
    );
    assert_eq!(
        std::fs::read_to_string(data_dir(&server).join("checks.txt"))
            .expect("checks")
            .lines()
            .count(),
        1,
        "a second probe would fail the same way, so it is not spent"
    );
    server.stop().await;
}

/// The same rule one step later: the truncated reading was *refused*, and the
/// check on the whole one then hit a 429. The refused reading must not become
/// the fallback — that would write to disk the exact token Anthropic had just
/// turned down.
#[tokio::test]
async fn a_reading_anthropic_refused_is_never_stored_when_the_next_check_fails() {
    let server = start().await;
    seed(&server, "wrap", "");
    seed(&server, "only-whole-tokens", &token());
    seed(&server, "rate-limited", "");

    let started = post(&server, "/api/accounts/setup-token", json!({ "label": "Wrapped" }), 201).await;
    let capture = settle(&server, started["captureId"].as_str().expect("captureId")).await;
    assert_eq!(capture["status"], json!("error"), "{capture}");
    assert!(capture["error"].as_str().expect("error").contains("rate-limited"), "{capture}");

    let accounts = server.get_json("/api/accounts").await;
    assert_eq!(accounts["accounts"].as_array().expect("accounts").len(), 0, "{accounts}");
    assert_eq!(
        std::fs::read_to_string(data_dir(&server).join("checks.txt"))
            .expect("checks")
            .lines()
            .count(),
        2,
        "the refusal of the first reading does not stop the second being tried"
    );
    server.stop().await;
}

/// Nothing opens a browser here: the URL is reported to the client as text, and
/// the code the user copies back is typed into the pty on their behalf.
#[tokio::test]
async fn the_authorization_url_is_reported_and_a_pasted_code_is_typed_into_the_wizard() {
    let server = start().await;
    seed(&server, "oauth", "");

    let started = post(&server, "/api/accounts/setup-token", json!({ "label": "OAuth" }), 201).await;
    let capture_id = started["captureId"].as_str().expect("captureId").to_string();

    // The whole URL, not the 80 columns of it the CLI printed on screen.
    assert_eq!(wait_for_auth_url(&server, &capture_id).await, AUTH_URL);

    // Nothing ran a browser opener: the wizard's PATH shadows every one of them.
    let env = std::fs::read_to_string(data_dir(&server).join("setup-env.txt")).expect("the wizard's environment");
    let var = |key: &str| {
        env.lines()
            .find_map(|line| line.strip_prefix(key))
            .unwrap_or_else(|| panic!("no {key} in the wizard's environment"))
            .to_string()
    };
    let stubs = data_dir(&server).join("setup-token").join(&capture_id).join("bin");
    assert!(
        var("PATH=").starts_with(&format!("{}:", stubs.display())),
        "the no-op openers have to win over the real ones: {}",
        var("PATH=")
    );
    assert_eq!(var("BROWSER="), stubs.join("open").to_string_lossy());

    post(
        &server,
        &format!("/api/accounts/setup-token/{capture_id}/code"),
        json!({ "code": "  auth-code-1234#state  " }),
        200,
    )
    .await;
    let capture = settle(&server, &capture_id).await;
    assert_eq!(capture["status"], json!("captured"), "{capture}");
    assert_eq!(
        std::fs::read_to_string(data_dir(&server).join("code.txt")).expect("the code the wizard read"),
        "auth-code-1234#state",
        "the code reaches the CLI trimmed, on one line"
    );

    // A capture that has already finished is not waiting for anything.
    let (status, body) = server
        .json(
            "POST",
            &format!("/api/accounts/setup-token/{capture_id}/code"),
            &json!({ "code": "again" }),
        )
        .await;
    assert_eq!(status, 409, "{body}");
    server.stop().await;
}

/// Re-authorization keeps the account, which is the whole point of it: sessions
/// are bound to an account *id*, so a session mid-conversation carries on under
/// the same account with a credential that works again.
#[tokio::test]
async fn re_authorizing_an_account_swaps_its_credential_and_keeps_its_id() {
    let server = start().await;
    let stale = format!("sk-ant-oat01-{}", "S".repeat(70));
    let account = post(
        &server,
        "/api/accounts",
        json!({ "label": "Work", "kind": "oauth_token", "secret": stale }),
        201,
    )
    .await;
    let id = account["id"].as_str().expect("id").to_string();

    let started = post(&server, &format!("/api/accounts/{id}/setup-token"), json!({}), 201).await;
    let capture = settle(&server, started["captureId"].as_str().expect("captureId")).await;
    assert_eq!(capture["status"], json!("captured"), "{capture}");
    assert_eq!(capture["accountId"], json!(id), "the same account, re-authorized");

    let accounts = server.get_json("/api/accounts").await;
    let stored = accounts["accounts"].as_array().expect("accounts");
    assert_eq!(stored.len(), 1, "a re-auth adds nothing: {accounts}");
    assert_eq!(stored[0]["id"], json!(id));
    assert_eq!(stored[0]["label"], json!("Work"), "and renames nothing");

    // The old credential is off the books: storing it now is not a duplicate.
    post(
        &server,
        "/api/accounts",
        json!({ "label": "Stale", "kind": "oauth_token", "secret": stale }),
        201,
    )
    .await;
    // …and the new one is, so it is the one that was written.
    let (status, body) = server
        .json(
            "POST",
            "/api/accounts",
            &json!({"label": "Fresh", "kind": "oauth_token", "secret": token()}),
        )
        .await;
    assert_eq!(status, 409, "{body}");
    server.stop().await;
}

/// An API key has no OAuth flow to re-run; it is re-entered instead.
#[tokio::test]
async fn re_authorizing_an_env_account_is_refused_before_anything_is_spawned() {
    let server = start().await;
    let account = post(
        &server,
        "/api/accounts",
        json!({ "label": "Key", "kind": "env", "env": "ANTHROPIC_API_KEY=sk-ant-api03-key" }),
        201,
    )
    .await;
    let id = account["id"].as_str().expect("id").to_string();

    let (status, body) = server.json("POST", &format!("/api/accounts/{id}/setup-token"), &json!({})).await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).expect("json"),
        json!({ "error": "setup_token_not_supported" })
    );

    // A pasted replacement is how that account is re-authorized, and it is
    // checked before it is stored just like a new one is.
    let patched = server
        .json(
            "PATCH",
            &format!("/api/accounts/{id}"),
            &json!({ "env": "ANTHROPIC_API_KEY=sk-ant-api03-fresh" }),
        )
        .await;
    assert_eq!(patched.0, 200, "{}", patched.1);
    assert_eq!(
        serde_json::from_str::<Value>(&patched.1).expect("json")["id"],
        json!(id),
        "a pasted re-auth keeps the id too"
    );

    let (status, body) = server.json("POST", &format!("/api/accounts/{}/setup-token", uuid()), &json!({})).await;
    assert_eq!(status, 404, "{body}");
    server.stop().await;
}

#[tokio::test]
async fn polling_an_unknown_capture_is_a_404() {
    let server = start().await;
    let (status, body) = server.get(&format!("/api/accounts/setup-token/{}", uuid())).await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).expect("json"),
        json!({ "error": "capture not found" })
    );
    server.stop().await;
}

#[tokio::test]
async fn an_env_account_is_rejected_before_anything_is_spawned() {
    let server = start().await;
    let account = post(
        &server,
        "/api/accounts",
        json!({ "label": "Key", "kind": "env", "env": "ANTHROPIC_API_KEY=sk-ant-api03-key" }),
        201,
    )
    .await;
    let id = account["id"].as_str().expect("id").to_string();

    let (status, body) = server.json("POST", &format!("/api/accounts/{id}/usage-login"), &json!({})).await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).expect("json"),
        json!({ "error": "usage_not_supported" })
    );

    // Nothing was spawned and nothing was created for an account that can never
    // hold a login.
    let terminals = server.get_json("/api/terminals").await;
    assert_eq!(terminals["terminals"].as_array().expect("terminals").len(), 0, "{terminals}");
    assert!(
        !data_dir(&server).join("accounts").join(&id).join("claude").exists(),
        "the probe directory should not have been created"
    );
    server.stop().await;
}

#[tokio::test]
async fn an_oauth_account_gets_a_login_terminal_pointed_at_its_own_probe_directory() {
    let server = start().await;
    let account = post(
        &server,
        "/api/accounts",
        json!({ "label": "Work", "kind": "oauth_token", "secret": token() }),
        201,
    )
    .await;
    let id = account["id"].as_str().expect("id").to_string();

    let started = post(&server, &format!("/api/accounts/{id}/usage-login"), json!({}), 201).await;
    let terminal_id = started["terminalId"].as_str().expect("terminalId").to_string();

    let probe = data_dir(&server).join("accounts").join(&id);
    let list = wait_for_terminals(&server, "listed the login terminal", |list| lists_terminal(list, &terminal_id)).await;
    let terminal = list["terminals"]
        .as_array()
        .expect("terminals")
        .iter()
        .find(|t| t["id"] == json!(terminal_id))
        .expect("the login terminal");
    assert_eq!(terminal["title"], json!("claude auth login — Work"));
    assert_eq!(terminal["cwd"], json!(probe.join("cwd").to_string_lossy()));

    // The CLI dumps its environment into the empty cwd it was given.
    let dump = probe.join("cwd").join("login-env.txt");
    let deadline = Instant::now() + Duration::from_secs(15);
    while !dump.exists() {
        assert!(Instant::now() < deadline, "the login CLI never recorded its environment");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let env = std::fs::read_to_string(&dump).expect("env dump");
    let config_dir = env
        .lines()
        .find_map(|line| line.strip_prefix("CLAUDE_CONFIG_DIR="))
        .expect("CLAUDE_CONFIG_DIR");
    assert_eq!(
        Path::new(config_dir),
        probe.join("claude"),
        "the login writes into the account's own probe root"
    );

    server.stop().await;
}

#[tokio::test]
async fn a_usage_login_for_an_unknown_account_is_a_404() {
    let server = start().await;
    let (status, body) = server.json("POST", &format!("/api/accounts/{}/usage-login", uuid()), &json!({})).await;
    assert_eq!(status, 404, "{body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).expect("json"),
        json!({ "error": "account not found" })
    );
    server.stop().await;
}

fn uuid() -> String {
    "11111111-2222-4333-8444-555555555555".to_string()
}
