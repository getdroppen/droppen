//! The account store, over HTTP, against a synthetic home.

mod support;

use serde_json::{json, Value};
use support::{common, Probe};

const SECRET: &str = "sk-ant-oat01-TESTSECRETVALUE0123456789";

async fn create(probe: &Probe, label: &str, secret: &str) -> Value {
    probe
        .send_ok(
            "POST",
            "/api/accounts",
            json!({"label": label, "kind": "oauth_token", "secret": secret}),
            201,
        )
        .await
}

/// One account out of the list, by id — the list holds more than one.
async fn stored_account(probe: &Probe, id: &str) -> Value {
    probe.get_json("/api/accounts").await["accounts"]
        .as_array()
        .expect("array")
        .iter()
        .find(|account| account["id"] == id)
        .cloned()
        .unwrap_or_else(|| panic!("account {id} is not in the list"))
}

#[tokio::test]
async fn an_account_round_trips_and_never_shows_its_secret() {
    let probe = Probe::start().await;

    let empty = probe.get_json("/api/accounts").await;
    assert_eq!(empty["accounts"].as_array().expect("array").len(), 0);
    assert_eq!(empty["defaultAccountId"], Value::Null, "an absent default is null, not missing");

    let account = create(&probe, "Work", SECRET).await;
    let id = account["id"].as_str().expect("id").to_string();
    assert_eq!(account["label"], "Work");
    assert_eq!(account["kind"], "oauth_token");
    assert_eq!(account["usageEnabled"], false, "only a real verify may turn this on");
    assert_eq!(
        account["email"],
        Value::Null,
        "an unverified account has no address, and the key is absent"
    );

    let listed = probe.get_json("/api/accounts").await;
    assert_eq!(listed["accounts"].as_array().expect("array").len(), 1);
    assert_eq!(
        listed["defaultAccountId"],
        Value::Null,
        "adding an account must not silently move new sessions off the local login"
    );

    // Nothing the API ever returns may carry the token or a hash of it.
    let raw = probe.get("/api/accounts").await.1;
    assert!(!raw.contains(SECRET), "the token reached a response body: {raw}");
    assert!(!raw.contains("secretSha256"), "not even the hash: {raw}");

    let renamed = probe
        .send_ok("PATCH", &format!("/api/accounts/{id}"), json!({"label": "Home"}), 200)
        .await;
    assert_eq!(renamed["label"], "Home");
    assert_ne!(renamed["updatedAt"], account["updatedAt"], "a patch restamps the account");

    let defaulted = probe.send_ok("PUT", "/api/accounts/default", json!({"accountId": id}), 200).await;
    assert_eq!(defaulted["defaultAccountId"], Value::String(id.clone()));

    let deleted = probe.send_ok("DELETE", &format!("/api/accounts/{id}"), json!({}), 200).await;
    assert_eq!(deleted["accepted"], true);
    assert_eq!(
        probe.get_json("/api/accounts").await["defaultAccountId"],
        Value::Null,
        "the default went with it"
    );

    probe.stop().await;
}

/// The three things a CLI's default can be, over HTTP — an account, the local
/// login somebody chose, or nothing chosen ("automatic") — and where
/// `GET /api/accounts/start` says a start that names no account runs for each.
#[tokio::test]
async fn the_default_is_an_account_a_chosen_local_login_or_automatic_and_start_says_where() {
    let probe = Probe::start().await;
    let start = |query: &'static str| {
        let probe = &probe;
        async move { probe.get_json(&format!("/api/accounts/start{query}")).await }
    };
    assert_eq!(start("").await, json!({"accountId": null, "reason": "local", "model": null, "placed": true}), "a fresh install");

    let id = create(&probe, "Work", SECRET).await["id"].as_str().expect("id").to_string();
    probe.send_ok("PUT", "/api/accounts/default", json!({"accountId": id}), 200).await;
    assert_eq!(start("?provider=claude&model=opus").await, json!({"accountId": id, "reason": "default", "model": null, "placed": true}));
    assert_eq!(
        start("?provider=codex").await,
        json!({"accountId": null, "reason": "local", "model": null, "placed": true}),
        "the other CLI keeps its own default"
    );

    let chosen = probe.send_ok("PUT", "/api/accounts/default", json!({"accountId": null}), 200).await;
    assert_eq!(chosen["defaultAccountId"], Value::Null);
    assert_eq!(chosen["local"]["claude"]["isDefault"], true, "null chooses the local login");
    assert_eq!(start("").await, json!({"accountId": null, "reason": "local_chosen", "model": null, "placed": true}));

    let (status, body) = probe
        .send("PUT", "/api/accounts/default", json!({"accountId": id, "automatic": true}))
        .await;
    assert_eq!(status, 400, "automatic clears the default; it names no account: {body}");
    assert_eq!(
        probe.get_json("/api/accounts").await["local"]["claude"]["isDefault"],
        true,
        "and a refused request changes nothing"
    );

    let automatic = probe
        .send_ok("PUT", "/api/accounts/default", json!({"accountId": null, "automatic": true}), 200)
        .await;
    assert_eq!(automatic["defaultAccountId"], Value::Null);
    assert_eq!(automatic["local"]["claude"]["isDefault"], false, "cleared: nothing chosen, as on a fresh install");
    assert_eq!(start("").await, json!({"accountId": null, "reason": "local", "model": null, "placed": true}));

    probe.stop().await;
}

/// A start that lets the answer choose its model, over HTTP: the default is
/// resting on fable, the head of its own (static) list, so the answer is the
/// default with the next of its models it can run — and a start sent with that
/// model lands there, where one sent with fable would not.
#[tokio::test]
async fn a_start_choosing_its_model_gets_a_login_and_a_model_that_land_together() {
    let probe = Probe::start().await;
    let default = create(&probe, "Default", SECRET).await["id"].as_str().expect("id").to_string();
    let spare = create(&probe, "Spare", "sk-ant-oat01-TESTSECRETVALUE9876543210").await["id"]
        .as_str()
        .expect("id")
        .to_string();
    probe.send_ok("PUT", "/api/accounts/default", json!({"accountId": default}), 200).await;
    assert_eq!(
        probe.get_json("/api/accounts/start?chooseModel=true").await,
        json!({"accountId": default, "reason": "default", "model": null, "placed": true}),
        "nothing resting: any model lands on the default, so none is chosen"
    );

    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_millis() as i64;
    rusqlite::Connection::open(probe.instance_db())
        .expect("instance db")
        .execute(
            "INSERT INTO account_rest (account_id, scope, class, until_ms, reason, session_id, source, observed_ms)
             VALUES (?1, 'fable', 'weekly', ?2, 'weekly limit', NULL, 'claude:result', ?3)",
            (&default, now + 3_600_000, now),
        )
        .expect("a fable rest");

    assert_eq!(
        probe.get_json("/api/accounts/start?chooseModel=true").await,
        json!({"accountId": default, "reason": "default", "model": "opus", "placed": true}),
        "the default, with the first of its own models it can still run"
    );
    assert_eq!(
        probe.get_json("/api/accounts/start?model=fable").await,
        json!({"accountId": spare, "reason": "rotated", "model": null, "placed": true}),
        "a start that names fable goes to the spare"
    );
    assert_eq!(
        probe.get_json("/api/accounts/start?model=opus&chooseModel=true").await,
        json!({"accountId": default, "reason": "default", "model": null, "placed": true}),
        "a named model is placed as named; chooseModel is for a start that names none"
    );

    probe.stop().await;
}

/// Where the OAuth token is kept, and who else on this machine can read it.
///
/// This used to be about `accounts.json` — its bytes, and its `0600`. The file
/// is neither read nor written any more, which moves the security property
/// rather than retiring it: the token is in the database now, so that is what
/// has to be `0600`, and the file has to be *absent* rather than left behind
/// holding a second, world-readable copy of the same secret.
#[tokio::test]
async fn the_store_that_holds_the_token_is_private_to_its_owner() {
    let probe = Probe::start().await;
    create(&probe, "Work", SECRET).await;

    // The secret is stored in the clear — that is the whole point of the store,
    // and the reason for everything below it. remote-hand has to hand the real
    // token to a `claude` it spawns; there is no hash that would do.
    let stored = common::rows(&probe.config_db(), "SELECT payload FROM accounts", ());
    assert_eq!(stored.len(), 1);
    assert!(stored[0].contains(SECRET), "the credential is what is kept");

    assert!(
        !probe.config_dir().join("accounts.json").exists(),
        "and there is no second copy of it under a mode nothing sets any more"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(probe.config_db()).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "a store holding credentials must not be readable by anyone else");
    }

    probe.stop().await;
}

#[tokio::test]
async fn the_same_token_twice_is_a_409_naming_the_first_label() {
    let probe = Probe::start().await;
    create(&probe, "Work", SECRET).await;

    let (status, body) = probe
        .send("POST", "/api/accounts", json!({"label": "Home", "kind": "oauth_token", "secret": SECRET}))
        .await;
    assert_eq!(status, 409);
    assert_eq!(body["error"], "duplicate_account");
    assert_eq!(body["existingLabel"], "Work", "the UI shows which account already has it");
    assert!(!body.to_string().contains(SECRET), "a conflict must not echo the token back: {body}");

    probe.stop().await;
}

#[tokio::test]
async fn an_unusable_label_or_token_is_a_400() {
    let probe = Probe::start().await;

    let (status, body) = probe
        .send("POST", "/api/accounts", json!({"label": "  ", "kind": "oauth_token", "secret": SECRET}))
        .await;
    assert_eq!(status, 400);
    assert_eq!(body["error"], "the label is empty");

    let (status, body) = probe
        .send(
            "POST",
            "/api/accounts",
            json!({"label": "Work", "kind": "oauth_token", "secret": "has space"}),
        )
        .await;
    assert_eq!(status, 400);
    assert_eq!(body["error"], "the token contains whitespace");

    probe.stop().await;
}

#[tokio::test]
async fn an_unknown_account_is_a_404_everywhere_it_can_be_named() {
    let probe = Probe::start().await;
    let missing = "11111111-2222-3333-4444-555555555555";

    for (method, path, body) in [
        ("PATCH", format!("/api/accounts/{missing}"), json!({"label": "x"})),
        ("DELETE", format!("/api/accounts/{missing}"), json!({})),
        ("POST", format!("/api/accounts/{missing}/usage-verify"), json!({})),
        ("PUT", "/api/accounts/default".to_string(), json!({"accountId": missing})),
    ] {
        let (status, value) = probe.send(method, &path, body).await;
        assert_eq!(status, 404, "{method} {path} -> {status}: {value}");
        assert_eq!(value["error"], "account not found");
    }

    // An id that is not a uuid names no account either. The TypeScript answered
    // `400` here (zod rejected the path parameter before the handler ran), but
    // the contract documents only `404` on these routes, so `404` is what a
    // client written against it is prepared for.
    for (method, path) in [
        ("DELETE", "/api/accounts/not-a-uuid".to_string()),
        ("PATCH", "/api/accounts/not-a-uuid".to_string()),
        ("POST", "/api/accounts/not-a-uuid/usage-verify".to_string()),
    ] {
        let (status, body) = probe.send(method, &path, json!({"label": "x"})).await;
        assert_eq!(status, 404, "{method} {path} -> {status}: {body}");
        assert_eq!(body["error"], "account not found");
    }

    probe.stop().await;
}

#[tokio::test]
async fn verifying_an_env_account_is_a_409_and_a_live_login_enables_usage() {
    let probe = Probe::start().await;

    let key_account = probe
        .send_ok(
            "POST",
            "/api/accounts",
            json!({"label": "Key", "kind": "env", "env": "ANTHROPIC_API_KEY=sk-ant-api03-KEYVALUE0123456789"}),
            201,
        )
        .await;
    let (status, body) = probe
        .send(
            "POST",
            &format!("/api/accounts/{}/usage-verify", key_account["id"].as_str().expect("id")),
            json!({}),
        )
        .await;
    assert_eq!(status, 409, "API keys authenticate per request; there is no login to verify");
    assert_eq!(body["error"], "usage_not_supported");

    let account = create(&probe, "Work", SECRET).await;
    let id = account["id"].as_str().expect("id");
    // The CLI reads its answer out of the probe directory's own cwd.
    let cwd = probe.data_dir().join("accounts").join(id).join("cwd");
    support::plan_auth(&cwd, r#"Welcome back!{"loggedIn": true, "email": "me@example.com"}"#);

    let verified = probe.send_ok("POST", &format!("/api/accounts/{id}/usage-verify"), json!({}), 200).await;
    assert_eq!(verified["loggedIn"], true);
    assert_eq!(verified["email"], "me@example.com");

    let stored = stored_account(&probe, id).await;
    assert_eq!(stored["usageEnabled"], true, "a verified login is what turns probing on");
    assert_eq!(stored["email"], "me@example.com");

    // With the reply removed the CLI exits non-zero, which is a "no".
    std::fs::remove_file(cwd.join("auth.out")).expect("remove the reply");
    let again = probe.send_ok("POST", &format!("/api/accounts/{id}/usage-verify"), json!({}), 200).await;
    assert_eq!(again["loggedIn"], false);
    assert_eq!(
        stored_account(&probe, id).await["usageEnabled"],
        false,
        "the stored account is made to agree with the CLI"
    );

    probe.stop().await;
}

#[tokio::test]
async fn a_session_binding_follows_the_root_the_session_lives_in() {
    let probe = Probe::start().await;
    let account = create(&probe, "Work", SECRET).await;
    let id = account["id"].as_str().expect("id");

    let managed = "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa";
    let local = "bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb";
    probe.seed_session(&probe.managed_root(), managed, &probe.workspace());
    probe.seed_session(&probe.local_root(), local, &probe.workspace());

    let bound = probe
        .send_ok("PUT", &format!("/api/sessions/claude/{managed}/account"), json!({"accountId": id}), 200)
        .await;
    assert_eq!(bound["accountId"], Value::String(id.to_string()));

    let detail = probe.get_json(&format!("/api/sessions/claude/{managed}")).await;
    assert_eq!(detail["claudeRoot"], "managed");
    assert_eq!(detail["account"]["id"], Value::String(id.to_string()));
    assert_eq!(detail["account"]["label"], "Work");
    assert_eq!(detail["summary"]["accountId"], Value::String(id.to_string()), "both payloads carry it");

    // A session in the user's own root keeps the user's own login, always.
    let (status, body) = probe
        .send("PUT", &format!("/api/sessions/claude/{local}/account"), json!({"accountId": id}))
        .await;
    assert_eq!(status, 409);
    assert_eq!(body["error"], "local_session_cannot_use_account");

    // Unbinding is always allowed.
    let unbound = probe
        .send_ok("PUT", &format!("/api/sessions/claude/{managed}/account"), json!({"accountId": null}), 200)
        .await;
    assert_eq!(unbound["accountId"], Value::Null);
    assert_eq!(probe.get_json(&format!("/api/sessions/claude/{managed}")).await["account"], Value::Null);

    let (status, _) = probe.send("PUT", "/api/sessions/claude/nope/account", json!({"accountId": null})).await;
    assert_eq!(status, 404, "a session that exists in neither root nor in the bindings");

    probe.stop().await;
}

#[tokio::test]
async fn sessions_are_discovered_in_both_roots() {
    let probe = Probe::start().await;
    probe.seed_session(&probe.local_root(), "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa", &probe.workspace());
    probe.seed_session(&probe.managed_root(), "bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb", &probe.workspace());

    // The scan fans out over both roots; the default list shows only the half
    // this server owns, and `scope=all` is what asks for the fan-out itself.
    let owned = probe.get_json("/api/sessions").await;
    assert_eq!(owned["total"], 1);
    assert_eq!(owned["sessions"][0]["claudeRoot"], "managed");

    let list = probe.get_json("/api/sessions?scope=all").await;
    assert_eq!(list["total"], 2, "the scan fans out over the local and the managed root");
    let roots: Vec<&str> = list["sessions"]
        .as_array()
        .expect("array")
        .iter()
        .map(|s| s["claudeRoot"].as_str().expect("claudeRoot"))
        .collect();
    assert!(roots.contains(&"local") && roots.contains(&"managed"), "got {roots:?}");

    let settings = probe.get_json("/api/settings").await;
    assert_eq!(settings["claudeDir"], probe.local_root().join("projects").to_string_lossy().to_string());
    assert_eq!(
        settings["claudeManagedDir"],
        probe.managed_root().join("projects").to_string_lossy().to_string()
    );
    assert_eq!(settings["claudeSessionCount"], 2);

    probe.stop().await;
}
