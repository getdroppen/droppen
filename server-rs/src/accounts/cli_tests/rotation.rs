//! Rotation, with each CLI's own login saying whether its sessions move,
//! which family a model is and whether its accounts keep a reading each.

use std::sync::Arc;

use droppen_accounts::quota::readings::UsageLogin;
use droppen_accounts::quota::{QuotaCache, QuotaWindow};
use droppen_accounts::rotation::*;
use droppen_accounts::store::{Account, AccountKind, AccountProvider, AccountsStore};
use droppen_http::openapi_ext::{Nullable, Optional};
use droppen_sessions::account::LimitClass;

const NOW: i64 = 1_790_000_000_000;
const MINUTE: i64 = 60_000;

fn account(id: &str, windows: Vec<Window>) -> PoolAccount {
    PoolAccount {
        id: id.to_string(),
        label: format!("{id}@example"),
        in_pool: true,
        resting: Vec::new(),
        windows,
    }
}

fn public(provider: AccountProvider, kind: AccountKind, rotation: bool) -> Account {
    Account {
        id: "11111111-2222-3333-4444-555555555555".into(),
        label: "a".into(),
        kind,
        email: Optional(None),
        usage_enabled: false,
        created_at: String::new(),
        updated_at: String::new(),
        unverified: Optional(None),
        provider,
        env_keys: Optional(None),
        rest: Vec::new(),
        rotation,
    }
}

fn local(state: droppen_accounts::local_login::LocalLoginState, is_default: bool) -> droppen_accounts::local_login::LocalLogin {
    droppen_accounts::local_login::LocalLogin {
        state,
        rest: Vec::new(),
        is_default,
    }
}

/// The trigger, class by class: the long windows and the dead credential
/// move a session, a request-rate blip and an unclassified failure do not.
#[test]
fn only_limits_that_outlast_a_respawn_trigger_a_rotation() {
    let logins = crate::app::logins();
    let pooled = public(AccountProvider::Claude, AccountKind::OauthToken, true);
    for class in [LimitClass::Session, LimitClass::Weekly, LimitClass::Credit, LimitClass::Auth] {
        assert!(triggers(&logins, class, Some(&pooled), false), "{class:?} moves the session");
    }
    for class in [LimitClass::Rate, LimitClass::Other] {
        assert!(!triggers(&logins, class, Some(&pooled), false), "{class:?} does not");
        assert!(!triggers(&logins, class, Some(&pooled), true), "{class:?} does not, even rebound by hand");
    }
    assert!(!triggers(&logins, LimitClass::Session, None, true), "a session on no account never moves");
    let outside = public(AccountProvider::Claude, AccountKind::Env, false);
    assert!(
        !triggers(&logins, LimitClass::Session, Some(&outside), false),
        "nor one on an account outside the pool"
    );
    assert!(
        triggers(&logins, LimitClass::Session, Some(&outside), true),
        "…unless somebody already bound the session elsewhere, and the move is theirs"
    );
    let codex = public(AccountProvider::Codex, AccountKind::Env, true);
    assert!(!triggers(&logins, LimitClass::Session, Some(&codex), true), "nor a codex one, ever");
}

/// [`default_for_new_session`] against the store: the account a new claude
/// session asking for `model` should start on, given the configured default
/// and `DROPPEN_ROTATE_AHEAD_PERCENT`.
async fn default_for(accounts: &AccountsStore, quota: &QuotaCache, default_id: &str, model: Option<&str>) -> String {
    let pool = pool_of(accounts, quota, AccountProvider::Claude).await;
    let family = accounts.logins().claude.model_family(model);
    default_for_new_session(default_id, &pool, family, ahead_threshold_from_env(), droppen_accounts::rest::now_ms())
}

/// The same rule against a real store: the default resting under the
/// five-hour window sends a new session to the next pool account, a
/// family rest only a session of that family, and nothing else moves.
#[tokio::test]
async fn a_new_session_passes_over_a_resting_default() {
    use droppen_accounts::limits::Verdict;
    use droppen_accounts::rest::{now_ms, Observation};
    use droppen_accounts::store::Credential;
    use droppen_instance::paths::Paths;

    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Arc::new(Paths::new(dir.path().to_path_buf()));
    let store = AccountsStore::new(paths.clone(), crate::app::logins());
    let quota = QuotaCache::new(paths, crate::app::logins());
    let create = |label: &'static str| {
        let store = &store;
        async move {
            store
                .create(label, AccountProvider::Claude, Credential::Oauth(format!("sk-ant-oat01-test-{label}")))
                .await
                .expect("account")
        }
    };
    let default = create("default").await;
    let other = create("other").await;
    assert_eq!(
        default_for(&store, &quota, &default.id, Some("fable")).await,
        default.id,
        "not resting: stays"
    );

    let rest = |scope: &str| Observation {
        account_id: default.id.clone(),
        session_id: Some("s".into()),
        source: "claude:result",
        model: None,
        verdict: Verdict {
            class: if scope == "all" { LimitClass::Session } else { LimitClass::Weekly },
            scope: scope.to_string(),
            until_ms: Some(now_ms() + 60 * MINUTE),
        },
        raw: "You have hit your limit".into(),
        reason: "You have hit your limit".into(),
        may_rest: true,
    };
    store.limit_log().record(&rest("fable"), now_ms()).expect("rest");
    assert_eq!(default_for(&store, &quota, &default.id, Some("claude-fable-5-1")).await, other.id);
    assert_eq!(default_for(&store, &quota, &default.id, Some("opus")).await, default.id);
    assert_eq!(
        default_for(&store, &quota, &default.id, None).await,
        other.id,
        "a session that names no model might be a fable one, so it is kept off the fable rest"
    );

    store.limit_log().record(&rest("all"), now_ms()).expect("rest");
    assert_eq!(default_for(&store, &quota, &default.id, None).await, other.id);
}

/// A pool is ranked by readings only for a CLI whose accounts keep one
/// each: a claude account's windows come from its reading, and a codex
/// account carries none, whatever is filed under its id.
#[tokio::test]
async fn only_a_cli_that_reads_each_login_ranks_its_pool_by_readings() {
    use droppen_accounts::store::Credential;
    use droppen_base::env_block::parse_env_block;
    use droppen_instance::paths::Paths;

    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Arc::new(Paths::new(dir.path().to_path_buf()));
    let store = AccountsStore::new(paths.clone(), crate::app::logins());
    let quota = Arc::new(QuotaCache::new(paths, crate::app::logins()));
    let claude = store
        .create("Claude", AccountProvider::Claude, Credential::Oauth("sk-ant-oat01-test-pool".to_string()))
        .await
        .expect("account");
    let codex = store
        .create(
            "Codex",
            AccountProvider::Codex,
            Credential::Env(parse_env_block("OPENAI_API_KEY=not-a-real-openai-key").expect("block")),
        )
        .await
        .expect("account");
    let window = QuotaWindow {
        label: "5h".to_string(),
        used_percent: 40.0,
        remaining_percent: 60.0,
        resets_at: Nullable(None),
        window_minutes: 300,
        updated_at: droppen_http::openapi_ext::Optional(None),
    };
    quota.ingest_session(UsageLogin::Account(claude.id.clone()), vec![window.clone()]);
    quota.ingest_session(UsageLogin::Account(codex.id.clone()), vec![window]);

    let windows = |pool: Vec<PoolAccount>| pool.iter().map(|account| account.windows.len()).collect::<Vec<_>>();
    assert_eq!(windows(pool_of(&store, &quota, AccountProvider::Claude).await), [1]);
    assert_eq!(
        windows(pool_of(&store, &quota, AccountProvider::Codex).await),
        [0],
        "a codex account ranks by the order it is listed in"
    );
}

/// The order of the rules, pinned without a store: a configured default
/// first (moved off a rest, for claude only), then a chosen local login,
/// then a local login nothing condemns; signed out, the first free account
/// *in the pool*, and with none the local login after all.
#[test]
fn a_start_nobody_pointed_takes_a_pool_account_only_for_a_signed_out_local_login() {
    use droppen_accounts::local_login::LocalLoginState::{SignedIn, SignedOut, Unknown};
    let start = |provider, default: Option<&str>, login, pool: &[PoolAccount], family| {
        let picked = start_choice(crate::app::logins().of(provider).as_ref(), default, &login, pool, family, None, NOW);
        (picked.account_id.0, picked.reason)
    };
    let gateway = PoolAccount {
        in_pool: false,
        ..account("gateway", Vec::new())
    };
    let resting = PoolAccount {
        resting: vec!["all".into()],
        ..account("resting", Vec::new())
    };
    let opus_spent = PoolAccount {
        resting: vec!["opus".into()],
        ..account("opus-spent", Vec::new())
    };
    let spare = account("spare", Vec::new());
    let pool = [gateway.clone(), resting.clone(), opus_spent.clone(), spare.clone()];
    let claude = AccountProvider::Claude;

    assert_eq!(start(claude, None, local(Unknown, false), &pool, None), (None, StartReason::Local));
    assert_eq!(start(claude, None, local(SignedIn, false), &pool, None), (None, StartReason::Local));
    assert_eq!(
        start(claude, None, local(SignedOut, false), &pool, Some("sonnet")),
        (Some("opus-spent".into()), StartReason::Fallback),
        "the gateway first in the list is out of the pool, and an account resting on everything is passed over"
    );
    assert_eq!(
        start(claude, None, local(SignedOut, false), &pool, Some("opus")),
        (Some("spare".into()), StartReason::Fallback),
        "a family rest keeps a session of that family off it"
    );
    assert_eq!(
        start(claude, None, local(SignedOut, true), &pool, None),
        (None, StartReason::LocalChosen),
        "a local login somebody chose stays chosen, signed out or not"
    );
    assert_eq!(
        start(claude, None, local(SignedOut, false), &[gateway.clone(), resting.clone()], None),
        (None, StartReason::LocalSignedOut),
        "nothing free in the pool: the local login and its sign-in error, never the gateway"
    );
    assert_eq!(
        start(claude, None, local(SignedOut, false), &[], None),
        (None, StartReason::LocalSignedOut)
    );

    let mut resting_default = resting.clone();
    resting_default.id = "default".into();
    let with_default = [resting_default, spare.clone()];
    assert_eq!(
        start(claude, Some("default"), local(SignedOut, false), &with_default, None),
        (Some("spare".into()), StartReason::Rotated),
        "a resting default is moved as it always was"
    );
    assert_eq!(
        start(AccountProvider::Codex, Some("default"), local(SignedOut, false), &with_default, None),
        (Some("default".into()), StartReason::Default),
        "a codex default never moves"
    );
    assert_eq!(
        start(claude, Some("gateway"), local(SignedOut, false), &pool, None),
        (Some("gateway".into()), StartReason::Default),
        "a default is the user's choice, in the pool or not"
    );
}

/// A start that lets the answer choose its model: the login decides the
/// model (the head of its own list) and the model's family can decide the
/// login. Asked one at a time they chase each other; together they settle
/// on a login and one of its own models that the rule places there.
#[test]
fn a_start_choosing_its_model_settles_on_a_login_and_a_model_placed_on_it() {
    use droppen_accounts::local_login::LocalLoginState::{SignedIn, SignedOut};
    let ids = |models: &[&str]| models.iter().map(|model| model.to_string()).collect::<Vec<_>>();
    // D, the default, was probed and its list leads with opus — on which
    // D is resting. E, in the pool, was not probed: the static list, fable first.
    let d_models = ids(&["opus[1m]", "fable", "sonnet"]);
    let e_models = ids(&["fable", "opus", "sonnet", "haiku"]);
    let inputs = StartInputs {
        login: crate::app::logins().claude.clone(),
        default: Some("d".into()),
        local: local(SignedIn, false),
        pool: vec![
            PoolAccount {
                resting: vec!["opus".into()],
                ..account("d", Vec::new())
            },
            account("e", Vec::new()),
        ],
        ahead: None,
        now_ms: NOW,
    };
    let on = |start: AccountStart| start.account_id.0;
    assert_eq!(on(inputs.place(Some("opus[1m]"))).as_deref(), Some("e"), "one at a time: D's head goes to E");
    assert_eq!(on(inputs.place(Some("fable"))).as_deref(), Some("d"), "and E's head comes back to D");
    assert!(inputs.model_matters());
    assert_eq!(inputs.start_order(), vec![Some("d".to_string()), Some("e".to_string()), None]);
    assert_eq!(
        inputs.own_model(Some("d"), &d_models),
        Some("fable"),
        "together: D, with the first of its own models D can still run"
    );
    assert_eq!(inputs.own_model(Some("e"), &e_models), Some("opus"), "E only takes what D cannot");

    // No default, the local login signed out: E rests on opus, F is free.
    // A model-less start goes to F; a fable start to E (listed first).
    let signed_out = StartInputs {
        default: None,
        local: local(SignedOut, false),
        pool: vec![
            PoolAccount {
                resting: vec!["opus".into()],
                ..account("e", Vec::new())
            },
            account("f", Vec::new()),
        ],
        ..inputs.clone()
    };
    assert!(signed_out.model_matters());
    assert_eq!(signed_out.start_order(), vec![Some("f".to_string()), Some("e".to_string()), None]);
    assert_eq!(
        signed_out.own_model(Some("f"), &e_models),
        Some("opus"),
        "F's head, fable, would land on E: F with opus, which E cannot take"
    );
    assert_eq!(signed_out.own_model(None, &e_models), None, "a stand-in is free: nothing lands on the local login");

    // The same, as the endpoint asks it: every login's list in start order.
    let lists = vec![(Some("d".to_string()), d_models.clone()), (Some("e".to_string()), e_models.clone()), (None, e_models.clone())];
    let chosen = inputs.choose(&lists);
    assert_eq!(
        (chosen.account_id.0.as_deref(), chosen.model.0.as_deref(), chosen.placed),
        (Some("d"), Some("fable"), true)
    );
    // Lists that leave no login a model of its own it lands on: D's (a
    // restricted plan, probed) holds only opus, on which D rests; E's only
    // fable, which lands back on D. The login a model-less start gets (E),
    // with the head of its list — `placed: false`, so a start names it.
    let lists = vec![
        (Some("d".to_string()), ids(&["opus[1m]", "opus"])),
        (Some("e".to_string()), ids(&["fable"])),
        (None, ids(&["fable"])),
    ];
    let stranded = inputs.choose(&lists);
    assert_eq!(
        (stranded.account_id.0.as_deref(), stranded.model.0.as_deref(), stranded.placed),
        (Some("e"), Some("fable"), false)
    );
    assert!(inputs.still_names("e"), "and E is the rule's choice for it while nothing changes");
    assert!(!inputs.still_names("d"));

    // Nothing family-dependent: any model lands on the same login.
    let plain = StartInputs {
        pool: vec![account("d", Vec::new()), account("e", Vec::new())],
        ..inputs.clone()
    };
    assert!(!plain.model_matters());
    let codex = StartInputs {
        login: crate::app::logins().codex.clone(),
        ..inputs.clone()
    };
    assert!(!codex.model_matters(), "codex models name no family");
}

/// A model names a family only by its own CLI's naming: `opus` is a claude
/// family, so a claude start for it may stand in on an account resting
/// under `fable`, while for codex it names none, and every rest stands in
/// the way of it.
#[test]
fn a_family_is_named_by_the_start_s_own_cli() {
    use droppen_accounts::local_login::LocalLoginState::SignedOut;
    let codex = StartInputs {
        login: crate::app::logins().codex.clone(),
        default: None,
        local: local(SignedOut, false),
        pool: vec![PoolAccount {
            resting: vec!["fable".into()],
            ..account("x", Vec::new())
        }],
        ahead: None,
        now_ms: NOW,
    };
    let claude = StartInputs {
        login: crate::app::logins().claude.clone(),
        ..codex.clone()
    };
    assert_eq!(claude.place(Some("opus")).account_id.0.as_deref(), Some("x"), "a fable rest is not an opus one");
    assert_eq!(
        codex.place(Some("opus")).account_id.0,
        None,
        "a codex model that reads like claude's names no family: the rest stands in the way"
    );
}

/// The same against a real store, both CLIs: a local login signed out by a
/// refused credential, accounts in and out of the pool, a rest, an
/// explicitly chosen local login and a configured default.
#[tokio::test]
async fn a_start_against_the_store_skips_a_signed_out_local_login_only_for_the_pool() {
    use droppen_accounts::limits::Verdict;
    use droppen_accounts::rest::{now_ms, Observation, LOCAL_CLAUDE, LOCAL_CODEX};
    use droppen_accounts::store::{AccountPatch, Credential};
    use droppen_base::env_block::parse_env_block;
    use droppen_instance::paths::Paths;

    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Arc::new(Paths::new(dir.path().to_path_buf()));
    let store = AccountsStore::new(paths.clone(), crate::app::logins());
    let quota = QuotaCache::new(paths, crate::app::logins());
    let start = |provider: AccountProvider, model: Option<&'static str>| {
        let (store, quota) = (&store, &quota);
        async move {
            let picked = start_for(store, quota, provider, model).await;
            (picked.account_id.0, picked.reason)
        }
    };
    let refused = |account: &str, class: LimitClass, until_ms: Option<i64>| Observation {
        account_id: account.to_string(),
        session_id: Some("s1".into()),
        source: "claude:result",
        model: None,
        verdict: Verdict {
            class,
            scope: "all".into(),
            until_ms,
        },
        raw: "Failed to authenticate: OAuth session expired and could not be refreshed".into(),
        reason: String::new(),
        may_rest: true,
    };

    assert_eq!(start(AccountProvider::Claude, None).await, (None, StartReason::Local), "nothing stored");

    // First in the list: an env block behind a gateway, out of the pool as
    // every env account is unless somebody puts it in.
    let gateway = store
        .create(
            "Gateway",
            AccountProvider::Claude,
            Credential::Env(parse_env_block("ANTHROPIC_AUTH_TOKEN=t\nANTHROPIC_BASE_URL=https://relay.example").expect("block")),
        )
        .await
        .expect("gateway");
    assert!(!gateway.rotation, "an env account is out of the pool by default");
    assert_eq!(
        start(AccountProvider::Claude, None).await,
        (None, StartReason::Local),
        "a local login nobody knows anything about is still where a start goes"
    );

    store
        .limit_log()
        .record(&refused(LOCAL_CLAUDE, LimitClass::Auth, None), now_ms())
        .expect("local refused");
    assert_eq!(
        start(AccountProvider::Claude, None).await,
        (None, StartReason::LocalSignedOut),
        "signed out with only the gateway stored: the local login and its error, never the gateway"
    );

    let work = store
        .create("Work", AccountProvider::Claude, Credential::Oauth("sk-ant-token-work".into()))
        .await
        .expect("work");
    let spare = store
        .create("Spare", AccountProvider::Claude, Credential::Oauth("sk-ant-token-spare".into()))
        .await
        .expect("spare");
    assert_eq!(
        start(AccountProvider::Claude, None).await,
        (Some(work.id.clone()), StartReason::Fallback),
        "the first account in the pool, past the gateway listed before it"
    );

    store
        .limit_log()
        .record(&refused(&work.id, LimitClass::Session, Some(now_ms() + 3_600_000)), now_ms())
        .expect("work resting");
    assert_eq!(
        start(AccountProvider::Claude, None).await,
        (Some(spare.id.clone()), StartReason::Fallback),
        "an account resting on everything is passed over"
    );
    store
        .update(
            &spare.id,
            AccountPatch {
                rotation: Some(false),
                ..AccountPatch::default()
            },
        )
        .await
        .expect("spare out of the pool");
    assert_eq!(
        start(AccountProvider::Claude, None).await,
        (None, StartReason::LocalSignedOut),
        "every pool account resting, the rest out of it: nothing stands in"
    );

    // Codex: its own local login and its own accounts, out of the pool by default.
    assert_eq!(start(AccountProvider::Codex, None).await, (None, StartReason::Local));
    let codex = store
        .create(
            "Codex",
            AccountProvider::Codex,
            Credential::Env(parse_env_block("OPENAI_API_KEY=sk-codex-one").expect("block")),
        )
        .await
        .expect("codex");
    store
        .limit_log()
        .record(&refused(LOCAL_CODEX, LimitClass::Auth, None), now_ms())
        .expect("codex local refused");
    assert_eq!(
        start(AccountProvider::Codex, None).await,
        (None, StartReason::LocalSignedOut),
        "a codex account is out of the pool unless somebody puts it in"
    );
    store
        .update(
            &codex.id,
            AccountPatch {
                rotation: Some(true),
                ..AccountPatch::default()
            },
        )
        .await
        .expect("codex into the pool");
    assert_eq!(start(AccountProvider::Codex, None).await, (Some(codex.id.clone()), StartReason::Fallback));
    assert_eq!(
        start(AccountProvider::Claude, None).await,
        (None, StartReason::LocalSignedOut),
        "each CLI stands in only with its own accounts"
    );

    // The local login chosen in Settings stays chosen, signed out or not.
    store.set_default(None, AccountProvider::Codex).await.expect("local codex");
    assert_eq!(start(AccountProvider::Codex, None).await, (None, StartReason::LocalChosen));

    // A configured default is the user's and always wins — even one out of the pool.
    store.set_default(Some(&gateway.id), AccountProvider::Claude).await.expect("default");
    assert_eq!(start(AccountProvider::Claude, None).await, (Some(gateway.id.clone()), StartReason::Default));
}

/// A codex start goes to its configured default however that default
/// rests, with a free pool account beside it: codex sessions do not move
/// between accounts. Through the store, so the start is handed the login
/// of its own CLI ([`StartInputs::gather`]) from the logins the entry builds.
#[tokio::test]
async fn a_codex_start_stays_on_its_resting_default() {
    use droppen_accounts::limits::Verdict;
    use droppen_accounts::rest::{now_ms, Observation};
    use droppen_accounts::store::{AccountPatch, Credential};
    use droppen_base::env_block::parse_env_block;
    use droppen_instance::paths::Paths;

    let dir = tempfile::tempdir().expect("tempdir");
    let paths = Arc::new(Paths::new(dir.path().to_path_buf()));
    let store = AccountsStore::new(paths.clone(), crate::app::logins());
    let quota = QuotaCache::new(paths, crate::app::logins());
    let mut pooled = Vec::new();
    for label in ["A", "B"] {
        let block = parse_env_block(&format!("OPENAI_API_KEY=key-{label}")).expect("block");
        let account = store
            .create(label, AccountProvider::Codex, Credential::Env(block))
            .await
            .expect("codex account");
        store
            .update(
                &account.id,
                AccountPatch {
                    rotation: Some(true),
                    ..AccountPatch::default()
                },
            )
            .await
            .expect("into the pool");
        pooled.push(account);
    }
    let (a, b) = (&pooled[0], &pooled[1]);
    store.set_default(Some(&a.id), AccountProvider::Codex).await.expect("default");
    store
        .limit_log()
        .record(
            &Observation {
                account_id: a.id.clone(),
                session_id: Some("s".into()),
                source: "codex:turn",
                model: None,
                verdict: Verdict {
                    class: LimitClass::Session,
                    scope: "all".into(),
                    until_ms: Some(now_ms() + 60 * MINUTE),
                },
                raw: "You've hit your usage limit".into(),
                reason: "You've hit your usage limit".into(),
                may_rest: true,
            },
            now_ms(),
        )
        .expect("rest");

    let picked = start_for(&store, &quota, AccountProvider::Codex, None).await;
    assert_eq!(
        (picked.account_id.0, picked.reason),
        (Some(a.id.clone()), StartReason::Default),
        "not {}, the free pool account beside it",
        b.label
    );
}
