//! Cloud's routes and the remote-view gate (`droppen_cloud`) on the state the
//! server builds (`crate::api::test_support`); some serve it and attach the
//! link the way `app` does. They reach cloud's insides through
//! `droppen_cloud::testing`.

use std::sync::Arc;

use gotcha::axum::extract::FromRef;
use gotcha::{Json, State};

use droppen_agent::fingerprint::machine_fingerprint;
use droppen_agent::status::Standing;
use droppen_auth::MaybeIdentity;
use droppen_http::error::ApiError;
use droppen_http::extract::ApiJson;

use crate::api::AppState;
use droppen_cloud::link_handoff::{self, PageOrigin};
use droppen_cloud::testing::{CloudStateTesting, CloudTesting, HubWebsTesting, TestAttempt};
use droppen_cloud::*;

fn attempt(owner: &str, expires_ms: u64) -> Option<TestAttempt> {
    Some(TestAttempt {
        id: "a1".into(),
        nonce: "n1".into(),
        owner: owner.into(),
        hub: "wss://api.droppen.app".into(),
        hub_web: DEFAULT_HUB.into(),
        aggregate: true,
        expires_ms,
        handoff: None,
    })
}

fn code_of(e: ApiError) -> (u16, String) {
    (e.status().as_u16(), e.body()["error"].as_str().unwrap_or_default().to_string())
}

/// Chain v3 review: disconnecting the computer and turning aggregation
/// off deleted only the legacy pending key; other devices' machine keys
/// and copies stayed. The whole directory goes now.
#[tokio::test]
async fn leaving_or_turning_aggregation_off_deletes_the_whole_aggregator_directory() {
    let home = tempfile::tempdir().unwrap();
    let state = crate::api::test_support::state(home.path()).await;
    state.drive.auth.users.add("alice", "correct horse battery staple").await.unwrap();
    let alice = || {
        MaybeIdentity(Some(droppen_http::identity::AuthIdentity {
            username: "alice".into(),
            token_sha256: None,
        }))
    };
    let dir = droppen_aggregator::dir(&state.cloud.paths());
    let fill = || {
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["keys.json", "copies.db", "device-key.json"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
    };
    fill();
    let _ = post_aggregator(State(CloudState::from_ref(&state)), alice(), ApiJson(AggregatorRequest { enabled: false }))
        .await
        .unwrap();
    assert!(!dir.exists(), "turning aggregation off deletes other devices' keys and copies");
    assert!(
        !state.cloud.aggregator_switch().is_on(),
        "the switch this server hands the link reads the API's off in cloud.json"
    );
    let _ = post_aggregator(State(CloudState::from_ref(&state)), alice(), ApiJson(AggregatorRequest { enabled: true }))
        .await
        .unwrap();
    assert!(
        state.cloud.aggregator_switch().is_on(),
        "and on again once the API turns it back on"
    );
    fill();
    let _ = post_unlink(State(CloudState::from_ref(&state)), alice(), ApiJson(UnlinkRequest { delete_copies: None }))
        .await
        .unwrap();
    assert!(!dir.exists(), "disconnecting deletes them too");
}

/// A pre-chain-v3 pending key is deleted at boot, whatever the link state,
/// so an install that carries the leftover does not report `pending` (and
/// hide the other devices' copies) for ever.
#[tokio::test]
async fn a_pre_chain_v3_pending_key_is_not_pending_after_the_next_boot() {
    let home = tempfile::tempdir().unwrap();
    let state = crate::api::test_support::state(home.path()).await;
    let key = droppen_aggregator::key_path(&state.cloud.paths());
    std::fs::create_dir_all(key.parent().unwrap()).unwrap();
    std::fs::write(&key, b"{}").unwrap();
    assert_eq!(CloudState::from_ref(&state).status_for(None).await.aggregator.state, "pending");

    // The next boot builds the state from the same home again.
    let rebooted = crate::api::test_support::state(home.path()).await;
    assert!(!key.exists(), "the leftover key is deleted at startup");
    assert_ne!(CloudState::from_ref(&rebooted).status_for(None).await.aggregator.state, "pending");
}

/// `docs/host-identity-rotation.md` §3.8 「只剩退役钥时」:
/// only a computer whose identity key was renewed elsewhere starts again
/// with a new identity — its seed archived, never used again, the
/// enrollment forgotten, a fresh seed — and is then signed in like a new one.
#[tokio::test]
async fn only_a_computer_renewed_elsewhere_starts_again_with_a_new_identity() {
    let home = tempfile::tempdir().unwrap();
    let state = crate::api::test_support::state(home.path()).await;
    state.drive.auth.users.add("alice", "correct horse battery staple").await.unwrap();
    let alice = || {
        MaybeIdentity(Some(droppen_http::identity::AuthIdentity {
            username: "alice".into(),
            token_sha256: None,
        }))
    };
    let paths = state.cloud.paths().clone();
    let files = droppen_agent::AgentFiles::of(&paths);
    let old = state.cloud.control.identity().unwrap();
    let mut st = droppen_agent::state::AgentState::enrolling("wss://hub.example", "a-1", "m-1", [9; 16], 5);
    st.save(&files).unwrap();
    let refused = post_re_enroll_fresh(State(CloudState::from_ref(&state)), alice()).await.unwrap_err();
    assert_eq!(code_of(refused), (409, "not_renewed_elsewhere".into()));
    assert_eq!(state.cloud.control.identity().unwrap().sign_public(), old.sign_public(), "nothing changed");
    // Refused by the hub as renewed, without a chain to show for it: not
    // evidence, nothing to start again for.
    st.rotation.refused_ms = Some(6);
    st.save(&files).unwrap();
    let refused = post_re_enroll_fresh(State(CloudState::from_ref(&state)), alice()).await.unwrap_err();
    assert_eq!(code_of(refused), (409, "not_renewed_elsewhere".into()));
    let shown = get_cloud(State(CloudState::from_ref(&state)), alice()).await.unwrap().0.link.unwrap().rotation;
    assert_eq!((shown.state.as_str(), shown.reason.as_deref()), ("refused", Some("rotated")));
    st.rotation.foreign_ms = Some(6);
    st.rotation.foreign_reason = Some(droppen_agent::rotation::FOREIGN_RENEWED_ELSEWHERE.into());
    st.save(&files).unwrap();
    let before = get_cloud(State(CloudState::from_ref(&state)), alice()).await.unwrap().0;
    let rotation = before.link.unwrap().rotation;
    assert_eq!((rotation.state.as_str(), rotation.reason.as_deref()), ("foreign", Some("renewed_elsewhere")));
    let fresh = post_re_enroll_fresh(State(CloudState::from_ref(&state)), alice()).await.unwrap().0;
    assert_eq!(fresh.state, "unenrolled");
    let new = state.cloud.control.identity().unwrap();
    assert_ne!(new.sign_public(), old.sign_public(), "a fresh seed");
    assert_eq!(fresh.fingerprint, machine_fingerprint(&new));
    let archived: Vec<_> = std::fs::read_dir(paths.instance_dir().join("agent"))
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with("machine-key.retired-"))
        .collect();
    assert_eq!(archived.len(), 1, "the retired seed is archived: {archived:?}");
    let after = get_cloud(State(CloudState::from_ref(&state)), alice()).await.unwrap().0;
    assert_eq!(after.state, "none", "back at entering a registration code");
    assert!(after.link.is_none());
}

/// A caller as the guard hands it over: signed in, with its login's digest.
async fn signed_in(state: &AppState, user: &str) -> MaybeIdentity {
    let login = state.drive.auth.sessions.issue(user, None).await.unwrap();
    MaybeIdentity(Some(droppen_http::identity::AuthIdentity {
        username: user.into(),
        token_sha256: Some(droppen_auth::sessions::digest_of(&login.token)),
    }))
}

/// Make `user` the owner, as a sign-in from the local settings does.
async fn own(state: &AppState, user: &str) {
    let since = CloudState::from_ref(state).accounts().await.get(user).cloned().unwrap();
    state
        .cloud
        .update_file(|f| {
            f.owner = Some(user.into());
            f.accounts_since.insert(user.into(), since);
        })
        .unwrap();
}

/// Pretend the link is enrolled (a device completes it): something to claim.
fn enrolled(state: &AppState) {
    droppen_agent::state::AgentState::enrolling("wss://hub.example", "a-1", "m-1", [7; 16], 1)
        .save(&droppen_agent::AgentFiles::of(&state.cloud.paths()))
        .unwrap();
}

fn access(user: &str, remote: bool) -> ApiJson<AccessRequest> {
    ApiJson(AccessRequest { user: user.into(), remote })
}

/// S1-7: who sees the other devices here, by the one rule — and only
/// whoever manages the link is told about every local account.
#[tokio::test]
async fn the_status_says_who_sees_the_other_devices_here() {
    let home = tempfile::tempdir().unwrap();
    let state = crate::api::test_support::state(home.path()).await;
    let none = CloudState::from_ref(&state).status_for(None).await;
    assert!(!none.sees_other_devices && none.accounts.is_empty(), "local authentication off: nobody");
    for user in ["alice", "bob", "carol"] {
        state.drive.auth.users.add(user, "correct horse battery staple").await.unwrap();
    }
    own(&state, "alice").await;
    let alice = signed_in(&state, "alice").await;
    let _ = put_access(State(CloudState::from_ref(&state)), alice.clone(), access("carol", true)).await.unwrap();
    let _ = put_access(State(CloudState::from_ref(&state)), alice.clone(), access("bob", false)).await.unwrap();
    let status = CloudState::from_ref(&state).status_for(Some("alice")).await;
    assert!(status.sees_other_devices);
    assert!(!status.can_claim, "it has an owner");
    let rows: Vec<(String, bool, bool)> = status.accounts.iter().map(|a| (a.user.clone(), a.remote, a.owner)).collect();
    assert_eq!(
        rows,
        vec![("alice".into(), true, true), ("bob".into(), false, false), ("carol".into(), true, false)]
    );
    let bob = CloudState::from_ref(&state).status_for(Some("bob")).await;
    assert!(!bob.sees_other_devices && bob.accounts.is_empty());
    assert!(CloudState::from_ref(&state).status_for(Some("carol")).await.sees_other_devices);
    // The gateway's own gate, the same rule.
    let carol = signed_in(&state, "carol").await;
    assert_eq!(remote_viewer(&CloudState::from_ref(&state), &alice).await.unwrap(), "alice");
    assert_eq!(remote_viewer(&CloudState::from_ref(&state), &carol).await.unwrap(), "carol");
    assert_eq!(
        code_of(remote_viewer(&CloudState::from_ref(&state), &signed_in(&state, "bob").await).await.unwrap_err()),
        (403, "no_remote_access".into())
    );
    let bridged = MaybeIdentity(Some(droppen_http::identity::AuthIdentity {
        username: "cloud:d-1".into(),
        token_sha256: None,
    }));
    assert_eq!(
        code_of(remote_viewer(&CloudState::from_ref(&state), &bridged).await.unwrap_err()),
        (403, "not_from_cloud".into())
    );
    // A caller whose login cannot be named: refused (fail closed).
    let unnamed = MaybeIdentity(Some(droppen_http::identity::AuthIdentity {
        username: "alice".into(),
        token_sha256: None,
    }));
    assert_eq!(code_of(remote_viewer(&CloudState::from_ref(&state), &unnamed).await.unwrap_err()), (401, "unauthorized".into()));
    // A login signed out since its ticket was minted: refused.
    let MaybeIdentity(Some(identity)) = &carol else { unreachable!() };
    state.drive.auth.sessions.revoke(identity.token_sha256.as_deref().unwrap()).await;
    assert_eq!(code_of(remote_viewer(&CloudState::from_ref(&state), &carol).await.unwrap_err()).0, 401);
    // Only the owner says who else sees them.
    let bob = signed_in(&state, "bob").await;
    assert_eq!(
        code_of(put_access(State(CloudState::from_ref(&state)), bob.clone(), access("bob", true)).await.unwrap_err()),
        (403, "not_owner".into())
    );
    // The owner's account gone: anybody may manage the link, but nobody sees.
    state.drive.auth.users.remove("alice").await.unwrap();
    assert_eq!(
        code_of(remote_viewer(&CloudState::from_ref(&state), &signed_in(&state, "carol").await).await.unwrap_err()),
        (403, "no_remote_access".into())
    );
    assert!(!CloudState::from_ref(&state).status_for(Some("bob")).await.sees_other_devices);
    // A cloud.json that does not read: nobody either.
    std::fs::write(LinkFile::of(&state.cloud.paths()).path(), b"{\"owner\": tr").unwrap();
    assert!(state.cloud.read_file_strict().is_none());
    assert!(!CloudState::from_ref(&state).status_for(Some("carol")).await.sees_other_devices);
}

/// The user's decision of 2026-10-19: an ownerless link (enrolled from the
/// command line) shows nobody the other devices until someone who manages
/// it says "I added this device" — and what anybody allowed while nobody
/// owned it does not come with the claim (review of S1-7, round 3).
#[tokio::test]
async fn an_ownerless_link_is_seen_by_nobody_until_it_is_claimed() {
    let home = tempfile::tempdir().unwrap();
    let state = crate::api::test_support::state(home.path()).await;
    for user in ["alice", "bob", "mallory"] {
        state.drive.auth.users.add(user, "correct horse battery staple").await.unwrap();
    }
    let alice = signed_in(&state, "alice").await;
    let bob = signed_in(&state, "bob").await;
    let mallory = signed_in(&state, "mallory").await;
    // No cloud.json at all (enrolled from the command line): nobody sees.
    assert_eq!(
        code_of(remote_viewer(&CloudState::from_ref(&state), &alice).await.unwrap_err()),
        (403, "no_remote_access".into())
    );
    // Not signed in to Droppen: nothing to claim.
    assert_eq!(
        code_of(post_claim(State(CloudState::from_ref(&state)), alice.clone()).await.unwrap_err()),
        (409, "not_linked".into())
    );
    assert!(!CloudState::from_ref(&state).status_for(Some("alice")).await.can_claim);
    enrolled(&state);
    assert!(CloudState::from_ref(&state).status_for(Some("alice")).await.can_claim);
    assert!(CloudState::from_ref(&state).may_manage(&state.cloud.read_file(), "bob").await, "anybody manages it");
    // Anybody may manage it, but nobody may allow anybody while nobody owns it.
    assert_eq!(
        code_of(
            put_access(State(CloudState::from_ref(&state)), mallory.clone(), access("mallory", true))
                .await
                .unwrap_err()
        ),
        (403, "not_owner".into())
    );
    // An allowance written by hand meanwhile does not survive a claim either.
    let t = CloudState::from_ref(&state).accounts().await;
    state
        .cloud
        .update_file(|f| {
            f.remote_access.insert("mallory".into(), true);
            f.accounts_since.insert("mallory".into(), t["mallory"].clone());
        })
        .unwrap();
    let claimed = post_claim(State(CloudState::from_ref(&state)), bob.clone()).await.unwrap();
    assert!(claimed.sees_other_devices && !claimed.can_claim);
    assert_eq!(remote_viewer(&CloudState::from_ref(&state), &bob).await.unwrap(), "bob", "claimed: bob alone");
    assert!(remote_viewer(&CloudState::from_ref(&state), &mallory).await.is_err(), "nothing allowed before the claim holds");
    assert!(remote_viewer(&CloudState::from_ref(&state), &alice).await.is_err());
    assert!(state.cloud.read_file().remote_access.is_empty());
    // A second claim: refused.
    assert_eq!(
        code_of(post_claim(State(CloudState::from_ref(&state)), alice.clone()).await.unwrap_err()).1,
        "not_owner",
        "only the owner manages it now"
    );
}

/// Review of S1-7, round 3: of two claims at once, one wins; the other
/// hears `has_owner`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_claims_at_once_make_one_owner() {
    let home = tempfile::tempdir().unwrap();
    let state = crate::api::test_support::state(home.path()).await;
    let names: Vec<String> = (0..8).map(|n| format!("u{n}")).collect();
    for user in &names {
        state.drive.auth.users.add(user, "correct horse battery staple").await.unwrap();
    }
    enrolled(&state);
    let mut callers = Vec::new();
    for user in &names {
        callers.push(signed_in(&state, user).await);
    }
    let claims = callers.into_iter().map(|who| {
        let state = state.clone();
        tokio::spawn(async move { post_claim(State(CloudState::from_ref(&state)), who).await.map(|_| ()) })
    });
    let claims: Vec<_> = claims.collect();
    let mut won = 0;
    for claim in claims {
        match claim.await.unwrap() {
            Ok(()) => won += 1,
            // Lost under the file's lock (`has_owner`), or after the winner wrote (`not_owner`).
            Err(e) => {
                let code = code_of(e);
                assert!(code == (409, "has_owner".into()) || code == (403, "not_owner".into()), "{code:?}");
            }
        }
    }
    assert_eq!(won, 1, "one owner");
    let owner = state.cloud.read_file().owner.unwrap();
    assert!(names.contains(&owner));
}

/// Review of S1-7, round 3: an account removed and made again under the
/// owner's or an allowed name inherits nothing.
#[tokio::test]
async fn an_account_made_again_under_the_same_name_inherits_nothing() {
    let home = tempfile::tempdir().unwrap();
    let state = crate::api::test_support::state(home.path()).await;
    for user in ["alice", "carol"] {
        state.drive.auth.users.add(user, "correct horse battery staple").await.unwrap();
    }
    own(&state, "alice").await;
    let alice = signed_in(&state, "alice").await;
    let _ = put_access(State(CloudState::from_ref(&state)), alice.clone(), access("carol", true)).await.unwrap();
    assert!(remote_viewer(&CloudState::from_ref(&state), &signed_in(&state, "carol").await).await.is_ok());
    // carol goes, and a new carol comes (a ms later at least).
    state.drive.auth.users.remove("carol").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    state.drive.auth.users.add("carol", "another horse battery staple").await.unwrap();
    assert_eq!(
        code_of(remote_viewer(&CloudState::from_ref(&state), &signed_in(&state, "carol").await).await.unwrap_err()),
        (403, "no_remote_access".into())
    );
    // So does alice: the new one is not the owner — nobody is, until a claim.
    state.drive.auth.users.remove("alice").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    state.drive.auth.users.add("alice", "another horse battery staple").await.unwrap();
    let again = signed_in(&state, "alice").await;
    assert!(remote_viewer(&CloudState::from_ref(&state), &again).await.is_err());
    enrolled(&state);
    assert!(CloudState::from_ref(&state).status_for(Some("alice")).await.can_claim, "an ownerless link again");
}

/// A port nothing listens on: a hub that cannot be reached.
fn dead_hub() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    format!("ws://127.0.0.1:{port}")
}

fn valid_code() -> String {
    rh_proto::identity::EnrollCode {
        account_fingerprint: [3; 16],
        secret: [4; 16],
    }
    .to_string()
}

/// The browser hand-off (§4.9) over HTTP, as the server serves it — the
/// login guard outermost: the attempt carries a droppen.app address for a
/// page that said its origin; droppen.app's sealed answer comes back
/// without a login and opens only for its attempt and state, once, before
/// the attempt expires; the page that waits sees how it ended.
#[tokio::test]
async fn droppen_app_s_answer_comes_back_through_the_browser_bound_to_its_attempt() {
    let home = tempfile::tempdir().unwrap();
    let (state, addr) = crate::api::test_support::served(home.path()).await;
    // What a link needs, as the owning server attaches it.
    state.cloud.control.attach(droppen_agent::link::LinkEnv {
        addr,
        credential: "test".into(),
        streams: Arc::new(crate::stream_source::SessionStreams::new(
            state.drive.paths.clone(),
            state.drive.transcripts.clone(),
            state.drive.clone(),
        )),
        host: Arc::new(crate::app::PushLinkHost),
        cancel: tokio_util::sync::CancellationToken::new(),
        aggregator_switch: state.cloud.aggregator_switch(),
    });
    let base = format!("http://{addr}");
    let client = reqwest::Client::new();
    let hub = dead_hub();
    let link = |token: Option<String>, origin: Option<String>| {
        let mut request = client
            .post(format!("{base}/api/cloud/link"))
            .json(&serde_json::json!({"hub": hub.clone()}));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(origin) = origin {
            request = request.header("Origin", origin);
        }
        request.send()
    };
    let give_back = |attempt: String, sealed: String| {
        client
            .post(format!("{base}/api/cloud/link/return"))
            .header("Origin", base.clone())
            .json(&serde_json::json!({"attempt": attempt, "sealed": sealed}))
            .send()
    };
    let error_of = |body: serde_json::Value| body["error"].as_str().unwrap_or_default().to_string();

    // No account yet: the device is in its setup state, which is no way
    // round the attempt — an answer for none finds nothing.
    let res = give_back("a".repeat(32), "AAAA".into()).await.unwrap();
    assert_eq!(res.status(), 404);

    state.drive.auth.users.add("alice", "correct horse battery staple").await.unwrap();
    let token = state.drive.auth.sessions.issue("alice", None).await.unwrap().token;

    // Without an Origin there is no page to come back to.
    let res = link(Some(token.clone()), None).await.unwrap();
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    assert!(body["handoffUrl"].is_null(), "{body}");
    let first = body["attempt"].as_str().unwrap().to_string();

    // From the page: the droppen.app address, with everything in its fragment.
    let res = link(Some(token.clone()), Some(base.clone())).await.unwrap();
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    let url = body["handoffUrl"].as_str().unwrap().to_string();
    let attempt = body["attempt"].as_str().unwrap().to_string();
    assert_ne!(attempt, first, "a new attempt replaces the first");
    assert!(url.starts_with(&format!("{}/link#droppen-link=", web_origin(&hub))), "{url}");
    assert!(!url.contains('?'));
    let status = CloudState::from_ref(&state).status_for(Some("alice")).await;
    assert_eq!(
        status.attempt.as_ref().and_then(|a| a.handoff_url.clone()).as_deref(),
        Some(url.as_str()),
        "the opener sees it"
    );
    let bob = CloudState::from_ref(&state).status_for(Some("bob")).await;
    assert!(bob.attempt.unwrap().handoff_url.is_none(), "nobody else does");

    let reply = serde_json::json!({"code": valid_code(), "email": "me@example.com"});
    // The first attempt is gone: its answer finds nothing.
    let (_, sealed) = droppen_cloud::testing::answer(&url, &reply, None);
    let res = give_back(first, sealed.clone()).await.unwrap();
    assert_eq!(res.status(), 404);
    // A page that never saw the address cannot answer for it (login CSRF), and uses nothing up.
    let (_, forged) = droppen_cloud::testing::answer(&url, &reply, Some([0; 16]));
    let res = give_back(attempt.clone(), forged).await.unwrap();
    assert_eq!(res.status(), 403);
    assert_eq!(error_of(res.json().await.unwrap()), "bad_reply");
    let res = give_back(attempt.clone(), "not base64!".into()).await.unwrap();
    assert_eq!(res.status(), 400);
    // A device bridged from Droppen never answers, even with the right reply.
    let bridged = post_return(
        State(CloudState::from_ref(&state)),
        MaybeIdentity(Some(droppen_http::identity::AuthIdentity {
            username: "cloud:d-1".into(),
            token_sha256: None,
        })),
        ApiJson(ReturnRequest {
            attempt: attempt.clone(),
            sealed: sealed.clone(),
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(code_of(bridged), (403, "not_from_cloud".into()));
    assert!(state.cloud.live_attempt().is_some(), "still there");

    // The real answer, without a login: taken, and the attempt is used up
    // (the hub is unreachable here, so the enrollment itself fails).
    let res = give_back(attempt.clone(), sealed.clone()).await.unwrap();
    assert_eq!(res.status(), 502);
    assert_eq!(error_of(res.json().await.unwrap()), "hub_unreachable");
    assert!(state.cloud.live_attempt().is_none(), "used up");
    let res = give_back(attempt.clone(), sealed).await.unwrap();
    assert_eq!(res.status(), 404, "once only");
    // The page that waits sees how it ended; nobody else does.
    let seen = CloudState::from_ref(&state).status_for(Some("alice")).await.handoff.expect("an outcome");
    assert_eq!((seen.attempt.as_str(), seen.outcome.as_str()), (attempt.as_str(), "failed"));
    assert_eq!(seen.error.as_deref(), Some("hub_unreachable"));
    assert!(CloudState::from_ref(&state).status_for(Some("bob")).await.handoff.is_none());
    assert!(CloudState::from_ref(&state).status_for(None).await.handoff.is_none());
}

/// A server that owns its instance and listens, as `return` needs it (the
/// link can start), with the local account `alice` — and, with `enroll`,
/// already in an account ([`linked`]): the state, and alice's attempt
/// opened from the server's own page towards `hub` (`None`: the default,
/// or the hub an enrolled device stays with).
async fn opened_from_a_page(home: &std::path::Path, hub: Option<String>, enroll: bool) -> (AppState, LinkAttempt) {
    opened_from_a_page_with(home, hub, enroll, false).await
}

/// [`opened_from_a_page`], asking the hub where its app is when
/// `discover` ([`droppen_cloud::hub_web`]).
async fn opened_from_a_page_with(home: &std::path::Path, hub: Option<String>, enroll: bool, discover: bool) -> (AppState, LinkAttempt) {
    let (state, addr) = crate::api::test_support::served(home).await;
    state.cloud.hub_webs.set_discover(discover);
    state.cloud.control.attach(droppen_agent::link::LinkEnv {
        addr,
        credential: "test".into(),
        streams: Arc::new(crate::stream_source::SessionStreams::new(
            state.drive.paths.clone(),
            state.drive.transcripts.clone(),
            state.drive.clone(),
        )),
        host: Arc::new(crate::app::PushLinkHost),
        cancel: tokio_util::sync::CancellationToken::new(),
        aggregator_switch: state.cloud.aggregator_switch(),
    });
    state.drive.auth.users.add("alice", "correct horse battery staple").await.unwrap();
    if enroll {
        linked(&state);
    }
    let alice = signed_in(&state, "alice").await;
    let Json(opened) = post_link(
        State(CloudState::from_ref(&state)),
        alice,
        PageOrigin(Some(format!("http://{addr}"))),
        ApiJson(LinkRequest { hub, aggregate: None }),
    )
    .await
    .unwrap();
    (state, opened)
}

/// Pretend this device is in an account and runs AI in it (`linked`): a
/// verified chain that adds it, saved as the link would.
fn linked(state: &AppState) {
    use rh_proto::chain::{genesis_draft, Action, ChainClock, ChainState, DeviceInfo, Signer};
    use rh_proto::identity::{fingerprint, MachineIdentity};
    let ak = MachineIdentity::from_seed(&[1; 32]);
    let device = MachineIdentity::from_seed(&[2; 32]);
    let machine = MachineIdentity::from_seed(&[3; 32]);
    let info = DeviceInfo {
        id: "d-1".into(),
        sign_key: device.sign_public(),
        dh_key: device.dh_public(),
        full: true,
    };
    let genesis = genesis_draft("a-1", ak.sign_public(), info, None, 1_000).sign(ak.signing_key());
    let mut chain = ChainState::verify(std::slice::from_ref(&genesis), None, &mut ChainClock::history()).unwrap();
    let add = chain
        .draft(Signer::Device("d-1".into()), Action::AddMachine(machine.info("m-1")), 2_000)
        .sign(device.signing_key());
    chain.apply(&add, &mut ChainClock::history()).unwrap();
    let mut st = droppen_agent::state::AgentState::enrolling("wss://hub.example", "a-1", "m-1", fingerprint(&ak.sign_public()), 5_000);
    assert_eq!(st.absorb(&[genesis, add], 5_000).error, None);
    st.save(&droppen_agent::AgentFiles::of(&state.cloud.paths())).unwrap();
    assert_eq!(state.cloud.control.status().state, Standing::Linked);
}

/// The sign-in opens where the hub says its app is
/// (`/.well-known/droppen-hub`, [`droppen_cloud::hub_web`]): the popup's
/// origin, the hand-off address and the status the page opens the popup
/// from — while the agent keeps signing in to the hub itself.
#[tokio::test]
async fn the_sign_in_opens_the_app_origin_the_hub_names() {
    use droppen_cloud::testing::{hub_answering, port};
    let (hub, asked) = hub_answering(|hub| {
        let doc = serde_json::json!({"origin": hub, "app": format!("http://my.localhost:{}", port(hub))});
        (200, None, doc.to_string())
    })
    .await;
    let app = format!("http://my.localhost:{}", port(&hub));
    let home = tempfile::tempdir().unwrap();
    let (state, opened) = opened_from_a_page_with(home.path(), Some(hub.clone()), false, true).await;
    assert_eq!(opened.hub_web, app);
    let url = opened.handoff_url.clone().expect("opened from a page");
    assert!(url.starts_with(&format!("{app}/link#droppen-link=")), "{url}");
    let status = CloudState::from_ref(&state).status_for(Some("alice")).await;
    assert_eq!((status.hub.as_str(), status.hub_web.as_str()), (hub.as_str(), app.as_str()), "the agent stays on the hub");
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
    // The answer through the browser is bound to the page's origin, not
    // to the host droppen.app's page was on: it opens as before.
    let reply = serde_json::json!({"code": valid_code()});
    let (attempt, sealed) = droppen_cloud::testing::answer(&url, &reply, None);
    let held = state.cloud.live_attempt().expect("the attempt");
    assert_eq!(attempt, held.id);
    assert!(held.handoff.as_ref().unwrap().open(&attempt, &link_handoff::sealed_bytes(&sealed).unwrap()).is_ok());
}

/// Review of the hand-off: between taking the attempt and the hub's
/// answer the attempt is gone, and the page that waits read that as "ran
/// out". Through `return` itself, while the hub keeps the enrollment
/// waiting: every look finds the attempt or that its answer was taken
/// (`receiving`), never neither — and only the page of the one who
/// opened it sees the answer.
#[tokio::test]
async fn while_it_enrolls_the_page_that_waits_sees_the_answer_was_taken() {
    let home = tempfile::tempdir().unwrap();
    // A hub that takes the connection and never answers: the enrollment waits.
    let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hub = format!("ws://{}", silent.local_addr().unwrap());
    let (state, opened) = opened_from_a_page(home.path(), Some(hub), false).await;
    let url = opened.handoff_url.clone().expect("opened from a page");
    let reply = serde_json::json!({"code": valid_code()});
    let (_, sealed) = droppen_cloud::testing::answer(&url, &reply, None);
    let before = CloudState::from_ref(&state).status_for(Some("alice")).await;
    assert!(before.attempt.is_some() && before.handoff.is_none());

    let returning = tokio::spawn(post_return(
        State(CloudState::from_ref(&state)),
        MaybeIdentity(None),
        ApiJson(ReturnRequest {
            attempt: opened.attempt.clone(),
            sealed,
        }),
    ));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let seen = CloudState::from_ref(&state).status_for(Some("alice")).await;
        match (seen.attempt.is_some(), seen.handoff) {
            (true, None) => {}
            (false, Some(handoff)) => {
                assert_eq!(
                    (handoff.attempt.as_str(), handoff.outcome.as_str()),
                    (opened.attempt.as_str(), "receiving")
                );
                break;
            }
            (attempt, handoff) => panic!("attempt {attempt}, an answer {}", handoff.is_some()),
        }
        assert!(std::time::Instant::now() < deadline, "the answer was never taken");
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(!returning.is_finished(), "still enrolling: the hub has not answered");
    assert!(CloudState::from_ref(&state).status_for(Some("bob")).await.handoff.is_none());
    assert!(CloudState::from_ref(&state).status_for(None).await.handoff.is_none());
    returning.abort();
    drop(silent);
}

/// Review of the hand-off: whether this device is in an account already
/// is said only to a reply that opens — not to anybody who reaches the
/// port — and saying it uses the attempt up.
#[tokio::test]
async fn only_a_reply_that_opens_learns_the_device_is_in_an_account() {
    let home = tempfile::tempdir().unwrap();
    let (state, opened) = opened_from_a_page(home.path(), None, true).await;
    let url = opened.handoff_url.clone().expect("opened from a page");
    let reply = serde_json::json!({"code": valid_code()});
    let give_back = |sealed: String| {
        post_return(
            State(CloudState::from_ref(&state)),
            MaybeIdentity(None),
            ApiJson(ReturnRequest {
                attempt: opened.attempt.clone(),
                sealed,
            }),
        )
    };

    let (_, forged) = droppen_cloud::testing::answer(&url, &reply, Some([0; 16]));
    assert_eq!(code_of(give_back(forged).await.unwrap_err()), (403, "bad_reply".into()));
    assert!(state.cloud.live_attempt().is_some(), "a refusal leaves the attempt be");

    let (_, sealed) = droppen_cloud::testing::answer(&url, &reply, None);
    assert_eq!(code_of(give_back(sealed).await.unwrap_err()), (409, "already_enrolled".into()));
    assert!(state.cloud.live_attempt().is_none(), "used up");
    let seen = CloudState::from_ref(&state).status_for(Some("alice")).await.handoff.expect("an outcome");
    assert_eq!(
        (seen.outcome.as_str(), seen.error.as_deref()),
        ("failed", Some("already_enrolled"))
    );
}

/// The device's owner by the local capability (`remote-hand-server open`)
/// while it has no user account: it manages the link and sees the other
/// devices, whatever `cloud.json` names — and stops the moment the first
/// account exists, to which a link it signed in passes, allowances cleared.
#[tokio::test]
async fn the_local_owner_owns_the_link_until_the_first_account_takes_it() {
    let home = tempfile::tempdir().unwrap();
    let state = crate::api::test_support::state(home.path()).await;
    state
        .drive
        .auth
        .users
        .add_app("droppen-app", "correct horse battery staple")
        .await
        .unwrap();
    let owner = signed_in(&state, droppen_auth::LOCAL_OWNER).await;
    enrolled(&state);
    // Signed in by the local owner: what `post_finish` records.
    state
        .cloud
        .update_file(|f| {
            f.owner = Some(droppen_auth::LOCAL_OWNER.into());
            f.linked_at = Some(1);
            f.remote_access.insert(droppen_auth::LOCAL_OWNER.into(), true);
            f.remote_access.insert("droppen-app".into(), true);
        })
        .unwrap();
    assert_eq!(remote_viewer(&CloudState::from_ref(&state), &owner).await.unwrap(), droppen_auth::LOCAL_OWNER);
    let status = CloudState::from_ref(&state).status_for(Some(droppen_auth::LOCAL_OWNER)).await;
    assert!(
        status.can_manage && status.sees_other_devices && !status.can_claim,
        "the owner, with nothing to claim"
    );
    let app = signed_in(&state, "droppen-app").await;
    assert!(remote_viewer(&CloudState::from_ref(&state), &app).await.is_err(), "the app's own account is no viewer by itself");
    assert!(!CloudState::from_ref(&state).may_manage(&state.cloud.read_file(), "droppen-app").await);

    // The first account: the link is ada's, bound to her account, and the
    // owner's sessions are over.
    state.drive.auth.users.add("ada", "correct horse battery staple").await.unwrap();
    state.drive.auth.sessions.revoke_user(droppen_auth::LOCAL_OWNER, None).await;
    state.cloud.hand_over_from_local_owner(&state.drive.auth.users).await;
    let file = state.cloud.read_file();
    assert_eq!(file.owner.as_deref(), Some("ada"));
    assert_eq!(file.remote_access.keys().collect::<Vec<_>>(), ["ada"], "allowances cleared");
    assert_eq!(file.accounts_since.get("ada"), CloudState::from_ref(&state).accounts().await.get("ada"));
    let ada = signed_in(&state, "ada").await;
    assert_eq!(remote_viewer(&CloudState::from_ref(&state), &ada).await.unwrap(), "ada");
    assert!(remote_viewer(&CloudState::from_ref(&state), &owner).await.is_err(), "the owner's session is gone");
    let fresh = signed_in(&state, droppen_auth::LOCAL_OWNER).await;
    assert!(
        remote_viewer(&CloudState::from_ref(&state), &fresh).await.is_err(),
        "and no owner session counts with an account"
    );
    assert!(!CloudState::from_ref(&state).may_manage(&state.cloud.read_file(), droppen_auth::LOCAL_OWNER).await);
    assert!(!CloudState::from_ref(&state).status_for(Some(droppen_auth::LOCAL_OWNER)).await.sees_other_devices);
}

/// The desktop app signs this device in to Droppen from its own window,
/// whose login is the app's app-internal account: on the app's server that
/// account may manage a link nobody owns — it is the app's, for the
/// operating-system user who runs it — but not one somebody else owns.
/// Anywhere else an app-internal account is never the "anybody" of an
/// ownerless link.
#[tokio::test]
async fn the_desktop_apps_own_account_may_sign_the_device_in_on_the_apps_server() {
    let home = tempfile::tempdir().unwrap();
    let mut state = crate::api::test_support::state(home.path()).await;
    state
        .drive
        .auth
        .users
        .add_app("droppen-app", "correct horse battery staple")
        .await
        .unwrap();
    assert!(
        !CloudState::from_ref(&state).may_manage(&state.cloud.read_file(), "droppen-app").await,
        "not on a server the app does not run"
    );
    state.desktop_app = true;
    assert!(CloudState::from_ref(&state).may_manage(&state.cloud.read_file(), "droppen-app").await, "on the app's own server");

    // A link a person's account owns stays theirs to manage.
    state.drive.auth.users.add("ada", "correct horse battery staple").await.unwrap();
    let since = CloudState::from_ref(&state).accounts().await.get("ada").cloned().unwrap();
    state
        .cloud
        .update_file(|f| {
            f.owner = Some("ada".into());
            f.linked_at = Some(1);
            f.accounts_since.insert("ada".into(), since);
        })
        .unwrap();
    assert!(!CloudState::from_ref(&state).may_manage(&state.cloud.read_file(), "droppen-app").await);
    assert!(CloudState::from_ref(&state).may_manage(&state.cloud.read_file(), "ada").await);
}

/// A sign-in the local owner began before the first account was made,
/// and whose answer came back after: the owner is nobody's manager any
/// more, so it cannot finish — an ownerless link included.
#[tokio::test]
async fn the_local_owner_manages_nothing_once_the_device_has_an_account() {
    let home = tempfile::tempdir().unwrap();
    let state = crate::api::test_support::state(home.path()).await;
    let owner = droppen_auth::LOCAL_OWNER;
    assert!(CloudState::from_ref(&state).may_manage(&state.cloud.read_file(), owner).await, "the owner, while there is no account");
    state.drive.auth.users.add("ada", "correct horse battery staple").await.unwrap();
    let file = state.cloud.read_file();
    assert!(file.owner.is_none(), "nobody owns the link");
    assert!(!CloudState::from_ref(&state).may_manage(&file, owner).await, "not the owner any more");
    assert!(CloudState::from_ref(&state).may_manage(&file, "ada").await, "an account manages an ownerless link");
    // What `post_return` asks of the attempt's opener.
    let attempt = attempt(owner, droppen_base::time::now_ms_u64() + 60_000).unwrap();
    assert!(!CloudState::from_ref(&state).may_manage(&file, &attempt.owner).await);
}
