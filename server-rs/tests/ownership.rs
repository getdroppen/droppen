//! Whose sessions a server lists, and what it may do with one it did not make.
//!
//! The model in one line: a server owns the sessions *it* made, and everything
//! else — the user's own `~/.claude`, another remote-hand's work, a codex thread
//! started from a terminal — is external. External is readable and refuses to be
//! driven. (Earlier builds could adopt one by copying it into their own root;
//! that is gone, and the copies it made are ordinary owned sessions.)
//!
//! The tests here are deliberately end-to-end over two live servers, because the
//! failure they exist to prevent is exactly the one no single-server test can
//! see: two remote-hands on one machine resuming one transcript and interleaving
//! two conversations into it.

mod support;

use std::path::Path;

use serde_json::{json, Value};
use support::{common, Probe};

/// The session ids one probe's server has recorded as its own, under `source`.
///
/// The registry is a table in that instance's state database, and no route
/// exposes it. That is deliberate rather than an oversight: a sibling must not
/// be able to see, or be told about, what this server made. So this is the only
/// window onto it, and the tests that use it are the ones about the registry
/// itself rather than about what it makes the list say.
fn owned(probe: &Probe, source: &str) -> Vec<String> {
    common::rows(
        &probe.instance_db(),
        "SELECT session_id FROM owned_sessions WHERE source = ?1 ORDER BY session_id",
        (source,),
    )
}

const OURS: &str = "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa";
const THEIRS: &str = "bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb";

/// The ids in one listing, in the order the server returned them.
async fn ids(probe: &Probe, query: &str) -> Vec<String> {
    probe.get_json(&format!("/api/sessions{query}")).await["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .map(|row| row["id"].as_str().expect("id").to_string())
        .collect()
}

// ------------------------------------------------------------- two instances

/// The headline. Two servers, one machine, one `~/.claude`: neither lists the
/// other's sessions, and both can see the user's own.
#[tokio::test]
async fn one_instances_sessions_are_never_another_instances_list() {
    let a = Probe::start().await;
    let workspace = a.workspace();
    // A session `a` owns, and one the user made in their own root.
    a.seed_session(&a.managed_root(), OURS, &workspace);
    a.seed_session(&a.local_root(), THEIRS, &workspace);

    let b = a.sibling("second").await;
    // A session `b` owns, to prove the isolation runs both ways.
    let b_owned = "cccccccc-3333-4333-8333-cccccccccccc";
    b.seed_session(&b.managed_root(), b_owned, &workspace);

    // The hard case: a session `a` created on the *local login*. Its transcript
    // is in the `~/.claude` both servers read, so only `a`'s own registry says
    // it is `a`'s — and `b` must take the registry's silence as the answer.
    let a_local = a
        .send_ok(
            "POST",
            "/api/sessions/new",
            json!({"source": "claude", "cwd": workspace.to_string_lossy(), "text": "start", "accountId": null}),
            200,
        )
        .await["id"]
        .as_str()
        .expect("id")
        .to_string();
    a.wait_for_spawn(&workspace).await;
    // The real CLI would write this; the fake one only records its argv.
    a.seed_session(&a.local_root(), &a_local, &workspace);

    let mut a_owned = ids(&a, "").await;
    a_owned.sort();
    assert_eq!(
        a_owned,
        sorted(vec![OURS, &a_local]),
        "a owns what it put in its root and what it created on the local login"
    );
    assert_eq!(ids(&b, "").await, vec![b_owned], "and b owns neither of them");
    assert_eq!(
        sorted_ids(&b, "?scope=external").await,
        sorted(vec![THEIRS, &a_local]),
        "a's local-login session is external to b — visible, and not b's to drive"
    );
    assert_eq!(ids(&a, "?scope=external").await, vec![THEIRS], "while to a it is not external at all");

    // The one place both instances' rows could leak into: `scope=all`.
    assert_eq!(
        sorted_ids(&a, "?scope=all").await,
        sorted(vec![OURS, THEIRS, &a_local]),
        "b's session is not a's to see"
    );

    // And a session it cannot list is a session it cannot open.
    assert_eq!(a.get(&format!("/api/sessions/claude/{b_owned}")).await.0, 404);
    // b can read a's local-login session, and must refuse to drive it.
    let (status, body) = b
        .send("POST", &format!("/api/sessions/claude/{a_local}/message"), json!({"text": "not yours"}))
        .await;
    assert_eq!(status, 409);
    assert_eq!(body["error"], "external_session");

    b.stop().await;
    a.stop().await;
}

/// A session this server started on the local login is its own: sendable, listed
/// as owned, and still spawning against the user's own `~/.claude`.
///
/// Ownership decides *who may resume*; the config root decides *which login
/// resumes*. This is the one session where the two answers differ, and both have
/// to come out right.
#[tokio::test]
async fn a_session_started_on_the_local_login_is_this_servers_and_keeps_that_login() {
    let home = tempfile::tempdir().expect("tempdir");
    let probe = Probe::start_at(home.path()).await;
    let workspace = probe.workspace();

    let id = probe
        .send_ok(
            "POST",
            "/api/sessions/new",
            json!({"source": "claude", "cwd": workspace.to_string_lossy(), "text": "start", "accountId": null}),
            200,
        )
        .await["id"]
        .as_str()
        .expect("id")
        .to_string();

    let spawn = probe.wait_for_spawn(&workspace).await;
    assert_eq!(spawn.flag("--session-id"), Some(id.as_str()));
    assert_eq!(
        spawn.env("CLAUDE_CONFIG_DIR"),
        None,
        "the local login means the user's own root, exactly as before"
    );
    // The real CLI would have written the transcript there; the fake one does not.
    probe.seed_session(&probe.local_root(), &id, &workspace);

    assert_eq!(ids(&probe, "").await, vec![id.clone()], "a session this server made is in its own list");
    let detail = probe.get_json(&format!("/api/sessions/claude/{id}")).await;
    assert_eq!(detail["summary"]["scope"], "owned");
    assert_eq!(detail["claudeRoot"], "local", "owned, and still in the user's root");
    assert_eq!(detail["canSend"], true);

    // The second message is the one the registry exists for.
    let (status, body) = probe
        .send("POST", &format!("/api/sessions/claude/{id}/message"), json!({"text": "carry on"}))
        .await;
    assert_eq!(
        status, 200,
        "refusing to continue a session this server just started would be absurd: {body}"
    );
    // No second spawn to inspect: the login a session runs on is fixed when its
    // process starts, and the second message goes to that same process. Which is
    // the stronger statement — there is no *opportunity* for a resume to change
    // root, because there is no resume.
    assert_eq!(
        support::wait_for_prompts(&workspace, 2).await,
        vec!["start".to_string(), "carry on".to_string()],
        "the second message reached the process the first one started"
    );
    assert_eq!(support::call_count(&workspace), 1, "and started no second claude to do it");

    // Binding an account is refused rather than silently forking the
    // conversation: an account spawn needs the transcript in this server's own
    // root, and nothing moves it there.
    let account = probe
        .send_ok(
            "POST",
            "/api/accounts",
            json!({"label": "Work", "kind": "oauth_token", "secret": "sk-ant-oat01-TESTSECRETVALUE0123456789"}),
            201,
        )
        .await["id"]
        .as_str()
        .expect("account id")
        .to_string();
    let (status, body) = probe
        .send("PUT", &format!("/api/sessions/claude/{id}/account"), json!({"accountId": account}))
        .await;
    assert_eq!(status, 409);
    assert_eq!(body["error"], "local_session_cannot_use_account");
    assert!(body["hint"].as_str().is_some_and(|hint| !hint.contains("adopt")), "{body}");
    assert_eq!(owned(&probe, "claude"), [id.as_str()], "and the session is still this server's own");

    probe.wait_for_injection("claude", &id).await;
    probe.stop().await;

    // The registry is a file, so the next server on this home knows it too.
    let restarted = Probe::start_at(home.path()).await;
    assert_eq!(ids(&restarted, "").await, vec![id.clone()], "ownership survives a restart");
    restarted.stop().await;
}

/// An instant chat has no account either, and must keep working end to end.
#[tokio::test]
async fn a_cli_chat_without_a_default_account_is_this_servers_own() {
    let probe = Probe::start().await;
    let created = probe
        .send_ok("POST", "/api/chats/new", json!({"backend": "claude", "text": "hello"}), 200)
        .await;
    let id = created["id"].as_str().expect("id").to_string();

    assert_eq!(
        owned(&probe, "claude"),
        [id],
        "a chat is a session this server started, on the local login"
    );

    probe.stop().await;
}

/// Sorted ids, for assertions that do not care about list order.
fn sorted(ids: Vec<&str>) -> Vec<String> {
    let mut ids: Vec<String> = ids.into_iter().map(str::to_owned).collect();
    ids.sort();
    ids
}

async fn sorted_ids(probe: &Probe, query: &str) -> Vec<String> {
    let mut found = ids(probe, query).await;
    found.sort();
    found
}

/// The Claude root every earlier build shared is claimed by `default`, once,
/// and left strictly alone by anybody else.
#[tokio::test]
async fn the_pre_instance_claude_root_is_claimed_by_default_and_only_once() {
    let home = tempfile::tempdir().expect("tempdir");
    let legacy = home.path().join(".local/share/remote-hand/claude/projects/-workspace");
    std::fs::create_dir_all(&legacy).expect("legacy tree");
    std::fs::write(
        legacy.join(format!("{OURS}.jsonl")),
        format!(
            "{}\n",
            json!({"type": "user", "cwd": "/workspace", "timestamp": "2024-05-01T10:00:00.000Z"})
        ),
    )
    .expect("legacy session");

    // A named instance boots first and must not touch it.
    let named = Probe::start_named_at(home.path(), "second").await;
    assert!(ids(&named, "?scope=all").await.is_empty(), "a named instance takes nothing");
    assert!(legacy.is_dir(), "and leaves the directory where it found it");
    named.stop().await;

    // `default` claims it, and the sessions come with it.
    let first = Probe::start_at(home.path()).await;
    assert_eq!(ids(&first, "").await, vec![OURS], "the moved sessions are this instance's own");
    assert!(!home.path().join(".local/share/remote-hand/claude").exists(), "the old name is gone");
    first.stop().await;

    // Every later boot finds nothing to do and changes nothing.
    let second = Probe::start_at(home.path()).await;
    assert_eq!(ids(&second, "").await, vec![OURS], "the claim is idempotent");
    second.stop().await;
}

// ---------------------------------------------------- earlier adoptions

/// What an adoption by an earlier build left: a copy in this server's own root
/// beside the user's original, under the same id. Adopting is gone, but the copy
/// is still this server's session — listed once, sendable, resumed from the
/// copy, renamed and archived like any other — and the original stays put.
#[tokio::test]
async fn a_session_adopted_by_an_earlier_build_keeps_working() {
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    let external = probe.seed_session(&probe.local_root(), THEIRS, &workspace);
    let copy = probe.seed_session(&probe.managed_root(), THEIRS, &workspace);
    let before = std::fs::read(&external).expect("read the original");

    // The owned copy shadows the row it came from: one session, not two.
    assert_eq!(ids(&probe, "").await, vec![THEIRS]);
    assert_eq!(ids(&probe, "?scope=external").await, Vec::<String>::new(), "the external row is shadowed");
    assert_eq!(ids(&probe, "?scope=all").await, vec![THEIRS]);

    let detail = probe.get_json(&format!("/api/sessions/claude/{THEIRS}")).await;
    assert_eq!(detail["summary"]["scope"], "owned");
    assert_eq!(detail["claudeRoot"], "managed");
    assert_eq!(detail["canSend"], true);

    // It can be driven, against the root it was copied into.
    let (status, body) = probe
        .send("POST", &format!("/api/sessions/claude/{THEIRS}/message"), json!({"text": "carry on"}))
        .await;
    assert_eq!(status, 200, "a session adopted earlier is this server's to send to: {body}");
    let spawn = probe.wait_for_spawn(&workspace).await;
    assert_eq!(spawn.flag("--resume"), Some(THEIRS));
    assert_eq!(
        spawn.env("CLAUDE_CONFIG_DIR"),
        Some(probe.managed_root().to_string_lossy().as_ref()),
        "the resume has to run against the copy, not the original"
    );
    probe.wait_for_injection("claude", THEIRS).await;

    // Renamed and archived like any session this server made.
    probe
        .send_ok("PUT", &format!("/api/sessions/claude/{THEIRS}/title"), json!({"title": "mine"}), 200)
        .await;
    probe
        .send_ok("POST", &format!("/api/sessions/claude/{THEIRS}/archive"), json!({}), 200)
        .await;
    assert!(copy.is_file(), "archiving hides the copy rather than deleting it");
    assert_eq!(
        std::fs::read(&external).expect("the original"),
        before,
        "the user's own file is untouched"
    );

    probe.stop().await;
}

/// Browse-only means *every* mutation, not just the send.
///
/// Two of these leave this server entirely: the titles and the archived list are
/// machine-wide — one store in the config directory that every instance shares —
/// so a rename or an archive here changes what the server that actually owns the
/// session shows; and `codex archive` mutates the user's own Codex state for a
/// thread this server never started.
#[tokio::test]
async fn nothing_that_changes_an_external_session_is_allowed() {
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    probe.seed_session(&probe.local_root(), THEIRS, &workspace);
    let codex_id = "0199c1a1-2b3c-4d5e-8f60-112233445566";
    seed_rollout(&probe.home, codex_id, &workspace);

    let refusals = [
        ("PUT", format!("/api/sessions/claude/{THEIRS}/title"), json!({"title": "mine now"})),
        ("POST", format!("/api/sessions/claude/{THEIRS}/archive"), json!({})),
        ("PUT", format!("/api/sessions/codex/{codex_id}/title"), json!({"title": "mine now"})),
        ("POST", format!("/api/sessions/codex/{codex_id}/archive"), json!({})),
    ];
    for (method, path, body) in refusals {
        let (status, body) = probe.send(method, &path, body).await;
        assert_eq!(status, 409, "{method} {path} -> {status}: {body}");
        assert_eq!(body["error"], "external_session", "{method} {path}: {body}");
        assert!(body.get("adoptable").is_none(), "nothing offers a takeover any more: {body}");
    }
    // The route that used to take a session over is gone.
    let (status, body) = probe
        .send("POST", &format!("/api/sessions/claude/{THEIRS}/adopt"), json!({}))
        .await;
    assert_eq!(status, 404, "the adopt route is gone: {body}");

    // Nothing was written, and `codex archive` was never even reached.
    assert!(!workspace.join("codex-argv.bin").exists(), "the user's own codex state was never touched");

    // The other two refusals are checked through the API rather than by looking
    // for the `titles.json` and `archived.json` that no longer exist to be
    // looked for — and it is the better question anyway, because it is the one
    // the user would ask. A rename that had gone through comes back as the
    // title; an archive that had would make the session stop resolving at all.
    // Reading being untouched is the whole point of `scope=external`, so the
    // same two requests prove both.
    for path in [format!("/api/sessions/claude/{THEIRS}"), format!("/api/sessions/codex/{codex_id}")] {
        let (status, body) = probe.get(&path).await;
        assert_eq!(status, 200, "an archive that had landed would 404 here: {path} -> {status}: {body}");
        let detail: Value = serde_json::from_str(&body).expect("json");
        assert_ne!(detail["summary"]["title"], "mine now", "no shared title was recorded for {path}");
    }
    // …and so is marking one read: an unread badge that could not be cleared
    // would make browsing worse rather than safer.
    probe
        .send_ok("POST", &format!("/api/sessions/claude/{THEIRS}/seen"), json!({}), 200)
        .await;

    probe.stop().await;
}

// ----------------------------------------------------------- codex ownership

/// One rollout file, in the `sessions/YYYY/MM/DD` tree Codex writes.
fn seed_rollout(home: &Path, id: &str, cwd: &Path) {
    let dir = home.join(".codex/sessions/2026/05/09");
    std::fs::create_dir_all(&dir).expect("rollout dir");
    let body = [
        json!({"type": "session_meta", "timestamp": "2026-05-09T16:39:49.000Z", "payload": {"id": id, "cwd": cwd.to_string_lossy()}}),
        json!({
            "type": "response_item",
            "timestamp": "2026-05-09T16:39:52.000Z",
            "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "hello"}]},
        }),
    ]
    .iter()
    .map(|line| format!("{line}\n"))
    .collect::<String>();
    std::fs::write(dir.join(format!("rollout-2026-05-09T16-39-49-{id}.jsonl")), body).expect("seed rollout");
}

/// Codex ownership is a record rather than a path, so it has to be written by
/// the thing that creates the thread — and read back by the list.
#[tokio::test]
async fn a_codex_thread_this_server_started_is_owned_and_every_other_one_is_not() {
    let probe = Probe::start().await;
    let workspace = probe.workspace();
    let mine = "0199c1a1-2b3c-4d5e-8f60-112233445566";
    std::fs::write(
        workspace.join("codex-stdout.txt"),
        format!("{}\n", json!({"type": "thread.started", "thread_id": mine})),
    )
    .expect("plan codex");

    let created = probe
        .send_ok(
            "POST",
            "/api/sessions/new",
            json!({"source": "codex", "cwd": workspace.to_string_lossy(), "text": "start"}),
            200,
        )
        .await;
    assert_eq!(created["id"], mine);

    // The real CLI would have written this; the fake one only announces the id.
    seed_rollout(&probe.home, mine, &workspace);
    // Somebody else's thread, in the very same `~/.codex`.
    seed_rollout(&probe.home, THEIRS, &workspace);

    assert_eq!(owned(&probe, "codex"), [mine], "the id the CLI announced, not the provisional one");

    assert_eq!(ids(&probe, "?source=codex").await, vec![mine], "only the thread this server made");
    assert_eq!(ids(&probe, "?source=codex&scope=external").await, vec![THEIRS]);

    // A thread nobody here started cannot be driven, and cannot be taken over.
    let (status, body) = probe
        .send("POST", &format!("/api/sessions/codex/{THEIRS}/message"), json!({"text": "hi"}))
        .await;
    assert_eq!(status, 409);
    assert_eq!(body["error"], "external_session");
    assert!(body["hint"].as_str().is_some_and(|hint| !hint.is_empty()), "{body}");

    probe.stop().await;
}

/// Start a codex thread through the API, with the fake CLI announcing `id`.
async fn start_codex_thread(probe: &Probe, id: &str) {
    let workspace = probe.workspace();
    std::fs::write(
        workspace.join("codex-stdout.txt"),
        format!("{}\n", json!({"type": "thread.started", "thread_id": id})),
    )
    .expect("plan codex");
    let created = probe
        .send_ok(
            "POST",
            "/api/sessions/new",
            json!({"source": "codex", "cwd": workspace.to_string_lossy(), "text": "start"}),
            200,
        )
        .await;
    assert_eq!(created["id"], id);
    // The real CLI would have written this; the fake one only announces the id.
    seed_rollout(&probe.home, id, &workspace);
}

/// The codex half of the headline. Claude gets this from a directory — an
/// account session lands in a root the sibling cannot read — and codex cannot,
/// because `~/.codex` is one directory for the whole machine. The claim in the
/// shared data directory is what buys the same silence.
#[tokio::test]
async fn one_instances_codex_threads_are_never_another_instances_list() {
    let a = Probe::start().await;
    let mine = "0199c1a1-2b3c-4d5e-8f60-112233445566";
    start_codex_thread(&a, mine).await;
    // A thread the user started themselves, in the very same `~/.codex`.
    seed_rollout(&a.home, THEIRS, &a.workspace());

    let b = a.sibling("second").await;

    // A sees both: its own as owned, the user's as external.
    assert_eq!(ids(&a, "?source=codex").await, vec![mine], "the thread this server made");
    assert_eq!(ids(&a, "?source=codex&scope=external").await, vec![THEIRS]);

    // B sees only the user's. A's thread is not B's to show, in any scope —
    // listing it as external would still leak that it exists and let B archive
    // or retitle it.
    assert_eq!(
        ids(&b, "?source=codex&scope=all").await,
        vec![THEIRS],
        "a sibling's thread is hidden, not merely undrivable"
    );
    assert!(ids(&b, "?source=codex").await.is_empty(), "and B owns nothing it did not make");

    // Nor can it be reached by typing its URL, which is where a list-only filter
    // would have leaked it back.
    let (status, _) = b.get(&format!("/api/sessions/codex/{mine}")).await;
    assert_eq!(status, 404, "a hidden thread does not resolve");
    let (status, _) = b.get(&format!("/api/sessions/codex/{THEIRS}")).await;
    assert_eq!(status, 200, "while the user's own thread stays browsable from both");

    // The claim is the mechanism, and it names the instance that made it.
    assert_eq!(std::fs::read_to_string(a.claims_dir().join(mine)).expect("claim").trim(), "default");
    assert!(!a.claims_dir().join(THEIRS).exists(), "the user's own threads are claimed by nobody");
    // And B learned all this without its own registry being touched. B has a
    // state database of its own — every list it just answered opened one — so
    // this is an empty table rather than an absent file, which is the stronger
    // of the two claims: the store exists and has nothing in it.
    assert!(owned(&b, "codex").is_empty(), "reading a sibling's claim is not recording ownership");

    b.stop().await;
    a.stop().await;
}

/// The threads that already existed when this bookkeeping did not. Their
/// ownership is in a registry no sibling reads, so each instance's own next boot
/// is what has to withdraw them from everybody else's list.
#[tokio::test]
async fn a_codex_thread_recorded_before_claims_existed_leaves_the_sibling_list_at_boot() {
    let home = tempfile::tempdir().expect("tempdir");
    let mine = "0199c1a1-2b3c-4d5e-8f60-112233445566";

    let a = Probe::start_named_at(home.path(), "default").await;
    start_codex_thread(&a, mine).await;
    // Reproduce a registry written by a build that had no claims: the entry is
    // there, the claim is not.
    std::fs::remove_file(a.claims_dir().join(mine)).expect("unclaim");
    a.stop().await;

    // Which is exactly the bug: the sibling lists a thread it did not make.
    let b = Probe::start_named_at(home.path(), "second").await;
    assert!(
        ids(&b, "?source=codex&scope=all").await.contains(&mine.to_string()),
        "with no claim, the sibling has no way to know whose thread this is"
    );
    b.stop().await;

    // The owning instance boots, mirrors its registry into the shared directory,
    // and the row leaves the sibling's list for good.
    let a = Probe::start_named_at(home.path(), "default").await;
    assert_eq!(
        std::fs::read_to_string(a.claims_dir().join(mine)).expect("claim").trim(),
        "default",
        "the boot migration claimed it"
    );
    let b = Probe::start_named_at(home.path(), "second").await;
    assert!(
        !ids(&b, "?source=codex&scope=all").await.contains(&mine.to_string()),
        "and the sibling stops listing it"
    );
    assert_eq!(ids(&a, "?source=codex").await, vec![mine], "while its owner still has it");

    // A sibling's boot never takes a claim that is not its own.
    assert_eq!(
        std::fs::read_to_string(b.claims_dir().join(mine)).expect("claim").trim(),
        "default",
        "the second instance's boot left the first one's claim alone"
    );

    b.stop().await;
    a.stop().await;
}

/// A claim outlives the thread it names, and the shared directory must not keep
/// one file per deleted rollout forever — but only its own writer may retire it.
#[tokio::test]
async fn boot_drops_this_instances_stale_claims_and_never_a_siblings() {
    let home = tempfile::tempdir().expect("tempdir");
    let mine = "0199c1a1-2b3c-4d5e-8f60-112233445566";

    let a = Probe::start_named_at(home.path(), "default").await;
    start_codex_thread(&a, mine).await;
    // A claim of A's whose rollout never existed, and one belonging to a sibling
    // that is equally stale.
    std::fs::write(a.claims_dir().join(OURS), "default").expect("stale claim");
    std::fs::write(a.claims_dir().join(THEIRS), "second").expect("a sibling's claim");
    a.stop().await;

    let a = Probe::start_named_at(home.path(), "default").await;
    assert!(a.claims_dir().join(mine).exists(), "a live thread keeps its claim");
    assert!(!a.claims_dir().join(OURS).exists(), "our own claim with no rollout is dropped");
    assert!(
        a.claims_dir().join(THEIRS).exists(),
        "a sibling's claim is the sibling's to retire; dropping it would un-hide its thread here"
    );
    a.stop().await;
}
