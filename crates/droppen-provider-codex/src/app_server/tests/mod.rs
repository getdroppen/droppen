//! Unit and protocol-fixture tests for the Codex App Server driver.

mod fake_codex;

use fake_codex::codex;

use super::*;

#[test]
fn codex_permission_aliases_map_onto_exec_flags() {
    assert_eq!(codex_permission_flags("auto").expect("auto"), ["--sandbox", "workspace-write"]);
    assert_eq!(
        codex_permission_flags("workspace-write").expect("alias"),
        ["--sandbox", "workspace-write"]
    );
    assert_eq!(
        codex_permission_flags("bypass-all").expect("bypass"),
        ["--dangerously-bypass-approvals-and-sandbox"]
    );
    assert!(codex_permission_flags("nonsense").is_err());
}
use droppen_instance::instance::InstanceDirs;
use droppen_instance::paths::Paths;
use serde_json::json;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::{
    unix::{OwnedReadHalf, OwnedWriteHalf},
    UnixListener,
};

/// Where this binary's session shim is: Codex's spawns under test run it
/// as this module's `session_shim`
/// ([`droppen_provider::testing::shims_run_as`]).
pub(super) const SHIMS: &str = module_path!();

/// Not a test: the session shim Codex's spawns under test run
/// ([`droppen_provider::testing::session_shim`]).
#[test]
#[ignore = "the session shim a Codex test's spawn runs, not a test"]
fn session_shim() {
    droppen_provider::testing::session_shim();
}

/// The logins the accounts are handed here: Codex's own, and a stand-in
/// for Claude Code's, which nothing here starts.
pub(super) fn logins() -> droppen_accounts::login::Logins {
    droppen_accounts::login::Logins {
        codex: Arc::new(crate::login::CodexLogin),
        ..droppen_accounts::login::Logins::stand_ins()
    }
}

/// The providers over `registry`: Codex alone, the one these tests drive.
fn codex_only(registry: &Arc<InjectionRegistry>) -> Arc<droppen_provider::ProviderRegistry> {
    droppen_provider::ProviderRegistry::new(registry.clone(), |providers, registry| {
        providers.register(Arc::new(Codex::new(registry.clone())))
    })
}

struct Harness {
    driver: Driver,
    /// The mock app server's end of the driver's link.
    socket: MockSocket,
    /// The providers over the driver's registry, for their spawn gate.
    providers: Arc<droppen_provider::ProviderRegistry>,
    _home: tempfile::TempDir,
}

/// A mock app server's end of a driver's link: what the driver writes is
/// read from it, and what is sent on it the driver reads as the app
/// server's.
struct MockSocket {
    input: BufReader<OwnedReadHalf>,
    output: OwnedWriteHalf,
}

impl MockSocket {
    /// The next line the driver wrote, byte for byte, newline included.
    async fn raw(&mut self) -> String {
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(2), self.input.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        line
    }
    /// The next request the driver wrote.
    async fn request(&mut self) -> Value {
        serde_json::from_str(&self.raw().await).unwrap()
    }
    /// Nothing more is written for this long after the last line: a
    /// request the driver writes is on the socket before its command
    /// returns.
    async fn writes_nothing(&mut self) {
        let mut line = String::new();
        let written = tokio::time::timeout(Duration::from_millis(300), self.input.read_line(&mut line)).await;
        assert!(written.is_err(), "nothing is written: {line}");
    }
    /// `value` as the app server's next line.
    async fn send(&mut self, value: Value) {
        self.output.write_all(format!("{value}\n").as_bytes()).await.unwrap();
    }
}

/// The Codex providers over a fresh registry at `home`, and a link to a
/// mock app server on a socket there, with the app server's end of it.
async fn mock_app_server(home: &std::path::Path) -> (Arc<droppen_provider::ProviderRegistry>, shim::Link, MockSocket) {
    let paths = Arc::new(Paths::new(home.into()));
    let dirs = InstanceDirs::new(&paths.data_dir(), "codex-test").unwrap();
    dirs.create().unwrap();
    let accounts = Arc::new(droppen_accounts::store::AccountsStore::new(paths.clone(), logins()));
    let quota = Arc::new(droppen_accounts::quota::QuotaCache::new(paths.clone(), logins()));
    let decoration = droppen_provider::testing::no_decoration();
    let registry = InjectionRegistry::new(paths, &dirs, accounts, quota, decoration);
    let providers = codex_only(&registry);
    let socket = home.join("mock.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let connector = tokio::spawn(async move { shim::connect_in_process(&socket).await.unwrap() });
    let (stream, _) = listener.accept().await.unwrap();
    let (read, mut output) = stream.into_split();
    let mut input = BufReader::new(read);
    let mut hello = String::new();
    input.read_line(&mut hello).await.unwrap();
    output
        .write_all(format!("{{\"shim_hello\":{{\"claude_pid\":{},\"total\":0}}}}\n", std::process::id()).as_bytes())
        .await
        .unwrap();
    let link = connector.await.unwrap();
    (providers, link, MockSocket { input, output })
}

/// The run record of the mock app server's session, `test-thread`.
fn mock_record() -> runs::RunRecord<CodexRun> {
    runs::RunRecord {
        run_id: new_uuid(),
        source: Source::Codex,
        session_id: "test-thread".into(),
        cwd: None,
        cli_pid: std::process::id(),
        started_at: iso8601(SystemTime::now()),
        pid_token: procs::start_token(std::process::id()),
        resident: true,
        shim: None,
        payload: CodexRun {
            codex: Some(State::default()),
            ..CodexRun::default()
        },
    }
}

/// What the sessions' feeds and the session list were told, in order: a
/// feed's poke by its key, and a list's refresh by the session it names.
#[derive(Default)]
struct Told(Mutex<Vec<String>>);

impl Told {
    /// Everything told since the last call.
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
}

impl droppen_provider::SessionFeeds for Told {
    fn poke(&self, key: &str) {
        self.0.lock().unwrap().push(format!("feed {key}"));
    }
}

impl droppen_http::refresh::RefreshSink for Told {
    fn request_refresh(&self, expected: Option<(&dyn std::fmt::Display, &str)>) {
        let named = expected.map(|(source, id)| format!(" {source}/{id}")).unwrap_or_default();
        self.0.lock().unwrap().push(format!("list{named}"));
    }

    fn publish(&self, _frame: String) {}
}

impl Harness {
    async fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let (providers, link, socket) = mock_app_server(home.path()).await;
        let (_tx, rx) = mpsc::unbounded_channel();
        let driver = Driver {
            lease: providers.gate().lease(Source::Codex, "test-thread").unwrap(),
            runs: codex(&providers).clone(),
            record: mock_record(),
            link,
            state: State::default(),
            shared: Arc::new(Mutex::new(State::default())),
            rx,
            replies: HashMap::new(),
            pending_since: Instant::now(),
            interrupt: None,
            stop_next: None,
            starting: Default::default(),
            archive_reply: None,
            archive_since: Instant::now(),
            retiring: false,
            // Milliseconds, not the half-minute a real shutdown is given:
            // every test here drives a mock socket whose "CLI" is this very
            // process, so the live branch would otherwise wait out the lot.
            retire_wait: Duration::from_millis(100),
        };
        Self {
            driver,
            socket,
            providers,
            _home: home,
        }
    }
    /// A handle wired to this driver's shared state, as the registry holds
    /// one. The receiver comes back with it so the caller can keep the
    /// handle *alive* — a dropped one reads as a driver that has stopped.
    fn handle(&self) -> (Handle, mpsc::UnboundedReceiver<Cmd>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Handle {
                tx,
                shared: self.driver.shared.clone(),
                account: None,
                starting: self.driver.starting.clone(),
            },
            rx,
        )
    }
    async fn event(&mut self, value: Value) {
        self.socket.send(value).await;
        let output = self.driver.link.recv().await.unwrap();
        self.driver.output(output).await.unwrap();
    }
    /// The next line the driver wrote ([`MockSocket::raw`]).
    async fn raw(&mut self) -> String {
        self.socket.raw().await
    }
    /// The next request the driver wrote ([`MockSocket::request`]).
    async fn request(&mut self) -> Value {
        self.socket.request().await
    }
    async fn enqueue(&mut self, text: &str) -> String {
        self.enqueue_with(text, &SendOptions::default()).await
    }
    /// [`Harness::enqueue`] with `options`: a message its client named,
    /// say.
    async fn enqueue_with(&mut self, text: &str, options: &SendOptions) -> String {
        let message = message(text, options).unwrap();
        let id = message.id.clone();
        let (tx, rx) = oneshot::channel();
        self.driver.command(Cmd::Deliver(message, tx)).await.unwrap();
        rx.await.unwrap().unwrap();
        id
    }
    async fn start(&mut self) -> Value {
        self.enqueue("first").await;
        self.driver.submit(0, false).await.unwrap();
        let request = self.request().await;
        self.event(json!({"method":"turn/started","params":{"threadId":"test-thread","turn":{"id":"turn-1","status":"inProgress"}}}))
            .await;
        self.event(json!({"id":request["id"],"result":{"turn":{"id":"turn-1","status":"inProgress"}}}))
            .await;
        request
    }
    /// Nothing more is written for a while ([`MockSocket::writes_nothing`]).
    async fn writes_nothing(&mut self) {
        self.socket.writes_nothing().await;
    }
    async fn complete(&mut self, status: &str) {
        self.event(json!({"method":"turn/completed","params":{"threadId":"test-thread","turn":{"id":"turn-1","status":status,"error":{"message":"test failure"}}}})).await;
    }
}

/// One written line: `template` with each placeholder replaced by the
/// value that differs from run to run, and the newline that ends it.
fn wire(template: &str, values: &[(&str, &str)]) -> String {
    let mut line = values.iter().fold(template.to_owned(), |line, (from, to)| line.replace(from, to));
    line.push('\n');
    line
}

/// The `id` member of a written line, for the requests that mint one.
fn id_of(line: &str) -> String {
    serde_json::from_str::<Value>(line).unwrap()["id"].as_str().unwrap().to_owned()
}

/// The codex local login is `signed_out` on a refused credential, a rest
/// with no end of its own. A turn that completes on it proves the
/// credential works again and ends that rest; a failed one does not, and a
/// quota rest is left alone.
#[tokio::test]
async fn a_completed_turn_ends_a_refused_credential_rest_on_its_login() {
    use droppen_accounts::limits::Verdict;
    use droppen_sessions::account::LimitClass;
    use droppen_accounts::local_login::LocalLoginState;
    use droppen_accounts::rest::{now_ms, Observation, LOCAL_CODEX};
    let mut h = Harness::new().await;
    h.start().await;
    let accounts = h.driver.runs.host.accounts.clone();
    let rest = |class: LimitClass, until_ms: Option<i64>| Observation {
        account_id: LOCAL_CODEX.to_string(),
        session_id: Some("test-thread".into()),
        source: "codex:turn",
        model: None,
        verdict: Verdict {
            class,
            scope: "all".into(),
            until_ms,
        },
        raw: "401 Unauthorized".into(),
        reason: String::new(),
        may_rest: true,
    };
    accounts.limit_log().record(&rest(LimitClass::Auth, None), now_ms()).expect("refused");
    assert_eq!(accounts.local_logins().codex.state, LocalLoginState::SignedOut);

    h.complete("failed").await;
    assert_eq!(accounts.local_logins().codex.state, LocalLoginState::SignedOut, "a failed turn proves nothing");

    accounts
        .limit_log()
        .record(&rest(LimitClass::Session, Some(now_ms() + 3_600_000)), now_ms())
        .expect("spent");
    h.complete("completed").await;
    let local = accounts.local_logins().codex;
    assert_eq!(local.state, LocalLoginState::Unknown, "signed in again, as far as anything says");
    assert_eq!(
        local.rest.iter().map(|rest| rest.class).collect::<Vec<_>>(),
        vec![LimitClass::Session],
        "only the refused credential ends"
    );
}

/// A failed turn that is an account limit, and a rate-limit update with a
/// full window, land in the limit log — and nothing else about the driver
/// changes: the failed turn pauses the queue exactly as any failed turn
/// always has.
#[tokio::test]
async fn a_usage_limit_is_recorded_and_changes_nothing_else() {
    use droppen_sessions::account::LimitClass;
    let mut h = Harness::new().await;
    h.start().await;
    let log = h.driver.runs.host.accounts.limit_log();

    h.event(json!({"method":"account/rateLimits/updated","params":{"rateLimits":{"primary":{"usedPercent":40,"resetsAt":1790000000}}}}))
        .await;
    assert!(log.events(None, 10).is_empty(), "a reading is not an event");

    // A full window resetting in 2100, then the turn failing on it.
    h.event(json!({"method":"account/rateLimits/updated","params":{"rateLimits":{"primary":{"usedPercent":100,"windowDurationMins":300,"resetsAt":4_102_444_800i64}}}}))
        .await;
    h.event(json!({"method":"turn/completed","params":{"threadId":"test-thread","turn":{"id":"turn-1","status":"failed","error":{"message":"You've hit your usage limit. Try again later.","codexErrorInfo":"usageLimitExceeded"}}}}))
        .await;
    assert!(h.driver.state.paused, "the queue pauses on a failed turn, as it always did");
    assert!(h.driver.state.active.is_none());

    let events = log.events(None, 10);
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!((events[0].source.as_str(), events[0].class), ("codex:turn", LimitClass::Session));
    assert_eq!(events[0].account_id.0, None, "the machine's own codex login goes out as null");
    assert_eq!(events[0].session_id.0.as_deref(), Some("test-thread"));
    assert_eq!((events[1].source.as_str(), events[1].class), ("codex:rate_limits", LimitClass::Rate));
    assert!(events[1].until_at.0.as_deref().is_some_and(|at| at.starts_with("2100-01-01")));

    // One rest for the whole account: the known reset, which the usage
    // limit's thirty-minute default did not cut short.
    let rests = log.active_rests(droppen_accounts::rest::now_ms());
    let local = &rests[droppen_accounts::rest::LOCAL_CODEX];
    assert_eq!(local.len(), 1);
    assert_eq!(local[0].class, LimitClass::Rate);
    assert!(local[0].until_at.0.as_deref().is_some_and(|at| at.starts_with("2100-01-01")));

    // A failure that is not a limit is not an event.
    h.complete("failed").await;
    assert_eq!(log.events(None, 10).len(), 2);
}

#[test]
fn turn_params_preserve_settings_and_attachments() {
    let m = message(
        "hello",
        &SendOptions {
            model: Some("gpt-5".into()),
            effort: Some("high".into()),
            permission: Some("auto".into()),
            images: vec!["/uploads/a.png".into()],
            files: vec!["/uploads/a.txt".into()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(m.params["sandboxPolicy"]["type"], "workspaceWrite");
    assert_eq!(m.params["approvalPolicy"], "never");
    assert_eq!(m.params["model"], "gpt-5");
    assert_eq!(m.params["effort"], "high");
    assert_eq!(m.params["input"][1], json!({"type":"localImage","path":"/uploads/a.png"}));
    assert!(m.params["input"][0]["text"].as_str().unwrap().contains("/uploads/a.txt"));
    // The bytes the journal keeps and every later turn/start is cloned from.
    assert_eq!(
        serde_json::to_string(&m.params).unwrap(),
        r#"{"input":[{"type":"text","text":"hello\n\n[1 attached file(s) — view with the Read tool:]\n- /uploads/a.txt"},{"type":"localImage","path":"/uploads/a.png"}],"approvalPolicy":"never","model":"gpt-5","effort":"high","sandboxPolicy":{"type":"workspaceWrite"}}"#
    );
    assert!(message(
        "x",
        &SendOptions {
            permission: Some("invalid".into()),
            ..Default::default()
        }
    )
    .is_err());
}

/// The sandbox is the one security boundary a headless turn has, so the
/// turn params of every permission mode, of none and of one codex does not
/// know are pinned byte for byte, outside a chat and in one. A chat is
/// read-only whatever mode it names, an unknown one included; outside a
/// chat an unknown mode is refused, and no mode at all names no sandbox and
/// leaves the thread's own in force.
#[test]
fn turn_params_name_the_sandbox_each_permission_mode_maps_to() {
    const UNNAMED: &str = r#"{"input":[{"type":"text","text":"hi"}],"approvalPolicy":"never"}"#;
    const WRITE: &str = r#"{"input":[{"type":"text","text":"hi"}],"approvalPolicy":"never","sandboxPolicy":{"type":"workspaceWrite"}}"#;
    const READ: &str = r#"{"input":[{"type":"text","text":"hi"}],"approvalPolicy":"never","sandboxPolicy":{"type":"readOnly"}}"#;
    const FULL: &str = r#"{"input":[{"type":"text","text":"hi"}],"approvalPolicy":"never","sandboxPolicy":{"type":"dangerFullAccess"}}"#;
    const REFUSED: &str = "unknown codex permission mode: invalid";
    let params = |permission: Option<&str>, chat: bool| {
        let options = SendOptions {
            permission: permission.map(str::to_owned),
            chat,
            ..Default::default()
        };
        message("hi", &options)
            .map(|message| serde_json::to_string(&message.params).unwrap())
            .map_err(|error| error.to_string())
    };
    let cases = [
        (None, false, Ok(UNNAMED)),
        (Some("auto"), false, Ok(WRITE)),
        (Some("workspace-write"), false, Ok(WRITE)),
        (Some("read-only"), false, Ok(READ)),
        (Some("full-access"), false, Ok(FULL)),
        (Some("danger-full-access"), false, Ok(FULL)),
        (Some("bypass-all"), false, Ok(FULL)),
        (Some("invalid"), false, Err(REFUSED)),
        (None, true, Ok(READ)),
        (Some("auto"), true, Ok(READ)),
        (Some("workspace-write"), true, Ok(READ)),
        (Some("read-only"), true, Ok(READ)),
        (Some("full-access"), true, Ok(READ)),
        (Some("danger-full-access"), true, Ok(READ)),
        (Some("bypass-all"), true, Ok(READ)),
        (Some("invalid"), true, Ok(READ)),
    ];
    for (permission, chat, expected) in cases {
        let expected = expected.map(str::to_owned).map_err(str::to_owned);
        assert_eq!(params(permission, chat), expected, "permission {permission:?}, chat {chat}");
    }
}

/// What the driver writes for a turn, a steer into it, the interrupt that
/// ends it and an archive, byte for byte, as codex reads them. Only the
/// ids the driver mints are filled in. None of them carries a `jsonrpc`
/// member, unlike the quota probe's requests to the same protocol.
#[tokio::test]
async fn the_driver_writes_turn_steer_interrupt_and_archive_requests_byte_for_byte() {
    let mut h = Harness::new().await;
    let first = h.enqueue("first").await;
    h.driver.submit(0, false).await.unwrap();
    assert_eq!(
        h.raw().await,
        wire(
            r#"{"id":"start:QID","method":"turn/start","params":{"input":[{"type":"text","text":"first"}],"approvalPolicy":"never","threadId":"test-thread","clientUserMessageId":"QID"}}"#,
            &[("QID", &first)]
        )
    );
    h.event(json!({"method":"turn/started","params":{"threadId":"test-thread","turn":{"id":"turn-1","status":"inProgress"}}}))
        .await;
    h.event(json!({"id":format!("start:{first}"),"result":{"turn":{"id":"turn-1","status":"inProgress"}}}))
        .await;

    let correction = h.enqueue("correction").await;
    let (reply, rx) = oneshot::channel();
    h.driver
        .command(Cmd::Queue {
            id: correction.clone(),
            steer: true,
            reply,
        })
        .await
        .unwrap();
    assert_eq!(
        h.raw().await,
        wire(
            r#"{"id":"steer:QID","method":"turn/steer","params":{"threadId":"test-thread","expectedTurnId":"turn-1","input":[{"type":"text","text":"correction"}],"clientUserMessageId":"QID"}}"#,
            &[("QID", &correction)]
        )
    );
    h.event(json!({"id":format!("steer:{correction}"),"result":{"turnId":"turn-1"}})).await;
    assert_eq!(rx.await.unwrap(), QueueOutcome::Done);

    h.driver.command(Cmd::Interrupt(Stop::Turn("turn-1".into()))).await.unwrap();
    let interrupt = h.raw().await;
    let id = id_of(&interrupt);
    assert_eq!(
        interrupt,
        wire(
            r#"{"id":"UUID","method":"turn/interrupt","params":{"threadId":"test-thread","turnId":"turn-1"}}"#,
            &[("UUID", &id)]
        )
    );
    h.event(json!({"id":id,"result":{}})).await;
    h.complete("interrupted").await;

    let (reply, rx) = oneshot::channel();
    h.driver.command(Cmd::Archive(reply)).await.unwrap();
    let archive = h.raw().await;
    let id = id_of(&archive);
    assert_eq!(
        archive,
        wire(
            r#"{"id":"archive:UUID","method":"thread/archive","params":{"threadId":"test-thread"}}"#,
            &[("UUID", id.strip_prefix("archive:").unwrap())]
        )
    );
    h.event(json!({"id":id,"result":{}})).await;
    rx.await.unwrap().unwrap();
}

/// A stored row that already names a thread and a message, where the
/// turn's request puts them, is sent naming each once: the thread it runs
/// on and the message it is.
#[tokio::test]
async fn a_stored_rows_own_thread_and_message_give_way_to_the_ones_sent() {
    let mut h = Harness::new().await;
    let stored: Message = serde_json::from_value(json!({
        "id": "queued-1",
        "text": "first",
        "params": {
            "input": [{"type": "text", "text": "first"}],
            "approvalPolicy": "never",
            "threadId": "stale-thread",
            "clientUserMessageId": "stale-message",
        },
        "request": null,
    }))
    .unwrap();
    let (reply, rx) = oneshot::channel();
    h.driver.command(Cmd::Deliver(stored, reply)).await.unwrap();
    rx.await.unwrap().unwrap();
    h.driver.submit(0, false).await.unwrap();
    assert_eq!(
        h.raw().await,
        wire(
            r#"{"id":"start:queued-1","method":"turn/start","params":{"input":[{"type":"text","text":"first"}],"approvalPolicy":"never","threadId":"test-thread","clientUserMessageId":"queued-1"}}"#,
            &[]
        )
    );
}

/// Switching account retires the running process — but only when it has
/// really gone. A CLI that ignores the EOF must leave everything exactly
/// where it was, because the caller's next move is to start a second
/// app-server on the same thread under a different key.
#[tokio::test]
async fn a_process_that_will_not_exit_keeps_its_claim_instead_of_being_replaced() {
    let mut h = Harness::new().await;
    let run_id = h.driver.record.run_id.clone();
    h.driver.runs.host.journal.add(h.driver.record.clone());
    let (handle, _rx) = h.handle();
    h.driver.runs.codex_servers.lock().unwrap().insert("test-thread".into(), handle);

    // `cli_pid` is this test process, which is not going anywhere.
    let (reply, rx) = oneshot::channel();
    h.driver.command(Cmd::Retire(reply)).await.unwrap();
    let error = rx.await.unwrap().expect_err("a process that is still there has not retired");
    assert!(error.to_string().contains("did not accept the shutdown"), "{error}");

    assert!(!h.driver.retiring, "the driver stays in its loop");
    assert!(h.driver.lease.holds(), "and keeps the whole-machine claim");
    assert!(
        h.driver.runs.host.journal.read().iter().any(|record| record.run_id == run_id),
        "the run is still the one on disk"
    );
    assert!(
        h.driver.runs.codex_handle("test-thread").is_some(),
        "so a send is refused rather than double-spawned"
    );
}

/// The other half: once the process is gone, the claim is released *before*
/// the reply, so the caller can spawn the replacement immediately.
#[tokio::test]
async fn a_retired_process_hands_over_its_claim_and_its_journal_entry() {
    let mut h = Harness::new().await;
    let run_id = h.driver.record.run_id.clone();
    h.driver.runs.host.journal.add(h.driver.record.clone());
    let (handle, _rx) = h.handle();
    h.driver.runs.codex_servers.lock().unwrap().insert("test-thread".into(), handle);
    // A pid that has really been and gone, reaped before it is named here:
    // a made-up number risks `kill(-1, 0)` and other pids that are not pids.
    let mut gone = std::process::Command::new("true").spawn().expect("spawn");
    let gone_pid = gone.id();
    gone.wait().expect("reap");
    h.driver.record.cli_pid = gone_pid;
    h.driver.record.pid_token = None;
    assert!(!procs::is_same_process(gone_pid, None), "the fixture must really be dead");

    let (reply, rx) = oneshot::channel();
    h.driver.command(Cmd::Retire(reply)).await.unwrap();
    rx.await.unwrap().expect("the process is gone, so the session is free");

    assert!(h.driver.retiring, "the loop is asked to end without reporting a death");
    assert!(!h.driver.lease.holds(), "the claim is released before the caller is told");
    assert!(!h.driver.runs.host.journal.read().iter().any(|record| record.run_id == run_id));
    assert!(h.driver.runs.codex_handle("test-thread").is_none());
}

#[tokio::test]
async fn archive_uses_the_resident_and_removes_its_journal_only_after_ack() {
    let mut h = Harness::new().await;
    let (reply, rx) = oneshot::channel();
    h.driver.command(Cmd::Archive(reply)).await.unwrap();
    let request = h.request().await;
    assert_eq!(request["method"], "thread/archive");
    assert_eq!(request["params"]["threadId"], "test-thread");
    let saved = h.driver.runs.host.journal.read().pop().unwrap().decode::<CodexRun>().unwrap().payload.codex.unwrap();
    assert_eq!(saved.archive_request.as_deref(), request["id"].as_str());
    assert!(!saved.archived);
    h.event(json!({"id":request["id"],"result":{}})).await;
    rx.await.unwrap().unwrap();
    assert!(h.driver.shared.lock().unwrap().archived);
    assert!(h.driver.runs.host.journal.read().is_empty());
    assert!(h.driver.state.injection.is_none());
}

#[tokio::test]
async fn archive_rejection_preserves_the_resident_and_allows_delivery() {
    let mut h = Harness::new().await;
    let (reply, rx) = oneshot::channel();
    h.driver.command(Cmd::Archive(reply)).await.unwrap();
    let request = h.request().await;
    h.event(json!({"id":request["id"],"error":{"code":-32600,"message":"archive refused"}}))
        .await;
    assert!(rx.await.unwrap().unwrap_err().to_string().contains("archive refused"));
    assert!(!h.driver.state.archived);
    assert!(h.driver.state.archive_request.is_none());
    assert_eq!(h.driver.runs.host.journal.read().len(), 1);
    h.enqueue("still usable").await;
    assert_eq!(h.driver.state.queue[0].text, "still usable");
}

#[tokio::test]
async fn archive_refuses_running_and_queued_work_without_dropping_it() {
    for running in [false, true] {
        let mut h = Harness::new().await;
        if running {
            h.start().await;
        } else {
            h.enqueue("keep me").await;
        }
        let before = serde_json::to_value(&h.driver.state).unwrap();
        let (reply, rx) = oneshot::channel();
        h.driver.command(Cmd::Archive(reply)).await.unwrap();
        assert!(rx.await.unwrap().is_err());
        assert_eq!(serde_json::to_value(&h.driver.state).unwrap(), before);
    }
}

#[tokio::test]
async fn archive_timeout_blocks_delivery_and_reconciles_late_ack_after_replay() {
    let mut h = Harness::new().await;
    let (reply, rx) = oneshot::channel();
    h.driver.command(Cmd::Archive(reply)).await.unwrap();
    let request = h.request().await;
    h.driver.archive_since = Instant::now() - RPC_TIMEOUT - Duration::from_secs(1);
    h.driver.tick().await.unwrap();
    assert!(rx.await.unwrap().unwrap_err().to_string().contains("timed out"));
    let (reply, rx) = oneshot::channel();
    h.driver
        .command(Cmd::Deliver(message("too late", &SendOptions::default()).unwrap(), reply))
        .await
        .unwrap();
    assert!(rx.await.unwrap().is_err());
    assert!(h.driver.state.queue.is_empty());
    h.driver.state = h.driver.runs.host.journal.read().pop().unwrap().decode::<CodexRun>().unwrap().payload.codex.unwrap();
    assert_eq!(h.driver.state.archive_request.as_deref(), request["id"].as_str());
    h.event(json!({"id":request["id"],"result":{}})).await;
    assert!(h.driver.state.archived);
    assert!(h.driver.runs.host.journal.read().is_empty());
}

#[tokio::test]
async fn archive_gate_prevents_a_concurrent_send_from_resurrecting_the_session() {
    let mut h = Harness::new().await;
    let registry = h.driver.runs.clone();
    let (tx, rx) = mpsc::unbounded_channel();
    h.driver.rx = rx;
    let handle = Handle {
        tx,
        shared: h.driver.shared.clone(),
        account: None,
        starting: Default::default(),
    };
    registry.codex_servers.lock().unwrap().insert("test-thread".into(), handle.clone());
    let driver = tokio::spawn(h.driver.run());
    let (archiver, providers) = (registry.clone(), h.providers.clone());
    let archive = tokio::spawn(async move { archiver.archive_codex(providers.gate(), "test-thread").await });
    let request = h.socket.request().await;
    assert_eq!(request["method"], "thread/archive");
    let (sender, providers) = (registry.clone(), h.providers.clone());
    let send = tokio::spawn(async move {
        sender
            .codex_deliver(providers.gate(), Some("test-thread"), None, "stale send", &SendOptions::default(), None)
            .await
    });
    h.socket.send(json!({"id":request["id"],"result":{}})).await;
    archive.await.unwrap().unwrap();
    assert!(send.await.unwrap().unwrap_err().to_string().contains("archived"));
    driver.await.unwrap();
    assert!(!handle.visible());
    assert!(registry.host.journal.read().is_empty());
    assert!(!registry.host.claim_held(Source::Codex, "test-thread"), "the driver released its claim");
}

#[tokio::test]
async fn native_steer_is_acknowledged_without_ending_the_turn() {
    let mut h = Harness::new().await;
    h.start().await;
    let id = h.enqueue("correction").await;
    let (reply, rx) = oneshot::channel();
    h.driver
        .command(Cmd::Queue {
            id: id.clone(),
            steer: true,
            reply,
        })
        .await
        .unwrap();
    let request = h.request().await;
    assert_eq!(request["method"], "turn/steer");
    assert_eq!(request["params"]["expectedTurnId"], "turn-1");
    assert_eq!(request["params"]["input"][0]["text"], "correction");
    assert!(h.driver.state.queue[0].request.is_some());
    h.event(json!({"id":request["id"],"result":{"turnId":"turn-1"}})).await;
    assert_eq!(rx.await.unwrap(), QueueOutcome::Done);
    assert!(h.driver.state.queue.is_empty());
    assert_eq!(h.driver.state.active.as_deref(), Some("turn-1"));
    assert_eq!(h.driver.state.injection.as_ref().unwrap().state, InjectionRunState::Running);
}

#[tokio::test]
async fn rejected_steer_preserves_the_message_and_current_turn() {
    let mut h = Harness::new().await;
    h.start().await;
    let id = h.enqueue("correction").await;
    let (reply, rx) = oneshot::channel();
    h.driver
        .command(Cmd::Queue {
            id: id.clone(),
            steer: true,
            reply,
        })
        .await
        .unwrap();
    let request = h.request().await;
    h.event(json!({"id":request["id"],"error":{"code":-32600,"message":"turn changed"}}))
        .await;
    assert!(matches!(rx.await.unwrap(), QueueOutcome::Refused { reason } if reason.contains("turn changed")));
    assert_eq!(h.driver.state.queue[0].id, id);
    assert!(h.driver.state.queue[0].request.is_none());
    assert_eq!(h.driver.state.active.as_deref(), Some("turn-1"));
}

#[tokio::test]
async fn interrupt_waits_for_completion_and_keeps_the_queue() {
    let mut h = Harness::new().await;
    h.start().await;
    h.enqueue("next").await;
    h.driver.command(Cmd::Interrupt(Stop::Turn("turn-1".into()))).await.unwrap();
    let request = h.request().await;
    assert_eq!(request["method"], "turn/interrupt");
    assert_eq!(request["params"]["turnId"], "turn-1");
    h.event(json!({"id":request["id"],"result":{}})).await;
    assert!(h.driver.state.active.is_some());
    h.complete("interrupted").await;
    assert!(h.driver.state.active.is_none());
    assert!(h.driver.state.injection.as_ref().unwrap().interrupted);
    assert_eq!(h.driver.state.queue.len(), 1);
    assert!(!h.driver.state.paused);
}

#[tokio::test]
async fn stop_during_start_handshake_is_not_lost() {
    let mut h = Harness::new().await;
    let first = h.enqueue("first").await;
    h.driver.submit(0, false).await.unwrap();
    h.request().await;
    let (handle, mut stops) = h.handle();
    assert!(handle.interrupt(), "a Stop while the start is in flight is taken");
    let stop = stops.try_recv().expect("the Stop");
    assert!(
        matches!(&stop, Cmd::Interrupt(Stop::Message(message)) if *message == first),
        "it names the message"
    );
    h.driver.command(stop).await.unwrap();
    h.event(json!({"method":"turn/started","params":{"threadId":"test-thread","turn":{"id":"turn-1"}}}))
        .await;
    assert_eq!(h.request().await["method"], "turn/interrupt");
}

/// A Stop that reaches the driver with nothing to stop is dropped, not
/// kept for a later turn. Here a respawn's queue is paused behind a
/// message the dead app server may have taken, with the message sent while
/// the new one started behind it. The user then withdraws the stale
/// message and sends the other now: that turn runs, and nothing stops it.
#[tokio::test]
async fn a_stop_with_nothing_to_stop_is_not_kept_for_a_later_turn() {
    let mut h = Harness::new().await;
    // As a respawn reads its state back: the message the old app server
    // may have taken, uncertain, and the queue paused behind it.
    let stale = h.enqueue("possibly delivered").await;
    h.driver.state.queue[0].uncertain = true;
    h.driver.state.paused = true;
    let sent = h.enqueue("sent while the app server started").await;
    h.driver.command(Cmd::Interrupt(Stop::Message(sent.clone()))).await.unwrap();
    assert!(!h.driver.state.interrupt_requested, "nothing runs, so no Stop is kept");
    assert!(
        h.driver.stop_next.is_none(),
        "nor kept for the message, which starts only when the user says"
    );

    let (reply, dropped) = oneshot::channel();
    h.driver
        .command(Cmd::Queue {
            id: stale,
            steer: false,
            reply,
        })
        .await
        .unwrap();
    assert_eq!(dropped.await.unwrap(), QueueOutcome::Done);
    let (reply, _sent_now) = oneshot::channel();
    h.driver
        .command(Cmd::Queue {
            id: sent,
            steer: true,
            reply,
        })
        .await
        .unwrap();
    assert_eq!(h.request().await["method"], "turn/start");
    h.event(json!({"method":"turn/started","params":{"threadId":"test-thread","turn":{"id":"turn-9","status":"inProgress"}}}))
        .await;
    h.writes_nothing().await;
    assert!(h.driver.state.active.is_some(), "the turn runs");
}

/// A Stop the driver reads after the turn it meant has ended, with nothing
/// queued, is dropped too: kept, it would stop the next turn.
#[tokio::test]
async fn a_stop_read_after_its_turn_ended_is_not_kept() {
    let mut h = Harness::new().await;
    h.start().await;
    h.complete("completed").await;
    h.driver.command(Cmd::Interrupt(Stop::Turn("turn-1".into()))).await.unwrap();
    assert!(!h.driver.state.interrupt_requested, "nothing runs, so no Stop is kept");
}

/// A Stop names the turn running, and stops no other. Read after that
/// turn completed and the message queued behind it started the next, it
/// stops nothing: the next turn runs.
#[tokio::test]
async fn a_stop_for_a_completed_turn_does_not_stop_the_next() {
    let mut h = Harness::new().await;
    h.start().await;
    h.enqueue("second").await;
    let (handle, mut stops) = h.handle();
    assert!(handle.interrupt(), "a Stop for the running turn is taken");
    let stop = stops.try_recv().expect("the Stop");
    assert!(matches!(&stop, Cmd::Interrupt(Stop::Turn(turn)) if turn == "turn-1"), "it names the turn");

    // The turn completes, and the queue's next turn starts, before the
    // driver reads the Stop.
    h.complete("completed").await;
    assert!(!h.driver.state.paused, "the queue is not paused");
    h.driver.submit(0, false).await.unwrap();
    let start = h.request().await;
    assert_eq!(start["method"], "turn/start");
    h.event(json!({"method":"turn/started","params":{"threadId":"test-thread","turn":{"id":"turn-2","status":"inProgress"}}}))
        .await;
    h.event(json!({"id":start["id"],"result":{"turn":{"id":"turn-2","status":"inProgress"}}}))
        .await;

    h.driver.command(stop).await.unwrap();
    assert!(!h.driver.state.interrupt_requested, "the Stop's turn is over");
    h.writes_nothing().await;
    assert_eq!(h.driver.state.active.as_deref(), Some("turn-2"), "the next turn runs");
}

/// A Stop sent while a message's `turn/start` is in flight names that
/// message. Read after the turn it started has completed and the next
/// message's turn has started, it stops nothing: here before codex has
/// answered the next message's start, so nothing but the first turn's end
/// says the first message's turn is over.
#[tokio::test]
async fn a_stop_for_a_started_message_does_not_stop_the_next() {
    let mut h = Harness::new().await;
    h.enqueue("first").await;
    h.driver.submit(0, false).await.unwrap();
    let start = h.request().await;
    let (handle, mut stops) = h.handle();
    assert!(handle.interrupt(), "a Stop while the start is in flight is taken");
    let stop = stops.try_recv().expect("the Stop");

    h.event(json!({"method":"turn/started","params":{"threadId":"test-thread","turn":{"id":"turn-1","status":"inProgress"}}}))
        .await;
    h.event(json!({"id":start["id"],"result":{"turn":{"id":"turn-1","status":"inProgress"}}}))
        .await;
    h.complete("completed").await;
    h.enqueue("second").await;
    h.driver.submit(0, false).await.unwrap();
    let start = h.request().await;
    h.event(json!({"method":"turn/started","params":{"threadId":"test-thread","turn":{"id":"turn-2","status":"inProgress"}}}))
        .await;

    h.driver.command(stop).await.unwrap();
    assert!(!h.driver.state.interrupt_requested, "the Stop's turn is over");
    h.writes_nothing().await;
    h.event(json!({"id":start["id"],"result":{"turn":{"id":"turn-2","status":"inProgress"}}}))
        .await;
    h.writes_nothing().await;
}

/// A Stop that names a message stops the turn it started, when that turn
/// is the one running by the time the driver reads it.
#[tokio::test]
async fn a_stop_for_a_message_stops_the_turn_it_started() {
    let mut h = Harness::new().await;
    let first = h.enqueue("first").await;
    h.driver.submit(0, false).await.unwrap();
    let start = h.request().await;
    h.event(json!({"id":start["id"],"result":{"turn":{"id":"turn-1","status":"inProgress"}}}))
        .await;
    assert!(h.driver.state.queue.is_empty(), "acknowledged, so off the queue");

    h.driver.command(Cmd::Interrupt(Stop::Message(first))).await.unwrap();
    let interrupt = h.request().await;
    assert_eq!(interrupt["method"], "turn/interrupt");
    assert_eq!(interrupt["params"]["turnId"], "turn-1");
}

/// A Stop for a message at the head of a paused queue is not kept: the
/// message starts only when the user says so, and then it runs.
#[tokio::test]
async fn a_stop_for_a_message_in_a_paused_queue_is_not_kept() {
    let mut h = Harness::new().await;
    let first = h.enqueue("first").await;
    h.driver.state.paused = true;
    h.driver.command(Cmd::Interrupt(Stop::Message(first.clone()))).await.unwrap();
    assert!(h.driver.stop_next.is_none(), "not kept for the message");

    let (reply, _sent_now) = oneshot::channel();
    h.driver
        .command(Cmd::Queue {
            id: first,
            steer: true,
            reply,
        })
        .await
        .unwrap();
    assert_eq!(h.request().await["method"], "turn/start");
    h.event(json!({"method":"turn/started","params":{"threadId":"test-thread","turn":{"id":"turn-1","status":"inProgress"}}}))
        .await;
    h.writes_nothing().await;
}

/// A Stop sent while the head of a queue that is not paused is about to
/// start, with nothing running, names that message; read before its
/// `turn/start` went out, it is kept for that start, and its turn is
/// interrupted once it starts.
#[tokio::test]
async fn a_stop_for_the_message_about_to_start_stops_its_turn() {
    let mut h = Harness::new().await;
    let first = h.enqueue("first").await;
    let (handle, mut stops) = h.handle();
    assert!(handle.interrupt(), "a Stop for the message about to start is taken");
    let stop = stops.try_recv().expect("the Stop");
    assert!(
        matches!(&stop, Cmd::Interrupt(Stop::Message(message)) if *message == first),
        "it names the message"
    );
    h.driver.command(stop).await.unwrap();
    assert!(!h.driver.state.interrupt_requested, "nothing runs yet");

    h.driver.submit(0, false).await.unwrap();
    assert_eq!(h.request().await["method"], "turn/start");
    h.event(json!({"method":"turn/started","params":{"threadId":"test-thread","turn":{"id":"turn-1","status":"inProgress"}}}))
        .await;
    let interrupt = h.request().await;
    assert_eq!(interrupt["method"], "turn/interrupt");
    assert_eq!(interrupt["params"]["turnId"], "turn-1");
}

/// A Stop sent while an app server starts names the message it starts for,
/// and is kept for that message's own start when other work runs first.
/// Here the resume reports a turn in progress: that turn runs on, and the
/// message's turn, after it, is interrupted.
#[tokio::test]
async fn a_stop_during_a_start_is_kept_for_its_message_behind_a_resumed_turn() {
    let mut h = Harness::new().await;
    // As the resume leaves the driver: the turn in progress, started by no
    // message of this driver's.
    h.driver.state.active = Some("turn-0".into());
    let sent = h.enqueue("sent while the app server started").await;
    h.driver.command(Cmd::Interrupt(Stop::Message(sent))).await.unwrap();
    assert!(!h.driver.state.interrupt_requested, "the resumed turn is not the message's");
    h.writes_nothing().await;

    h.event(json!({"method":"turn/completed","params":{"threadId":"test-thread","turn":{"id":"turn-0","status":"completed"}}}))
        .await;
    h.driver.submit(0, false).await.unwrap();
    assert_eq!(h.request().await["method"], "turn/start");
    h.event(json!({"method":"turn/started","params":{"threadId":"test-thread","turn":{"id":"turn-1","status":"inProgress"}}}))
        .await;
    let interrupt = h.request().await;
    assert_eq!(interrupt["method"], "turn/interrupt");
    assert_eq!(interrupt["params"]["turnId"], "turn-1");
}

/// The same, behind a message the process before this one left queued
/// and never sent, which the respawn starts first: that turn runs, and the
/// message's turn, after it, is interrupted.
#[tokio::test]
async fn a_stop_during_a_start_is_kept_for_its_message_behind_one_left_queued() {
    let mut h = Harness::new().await;
    // As the respawn leaves the driver: the message left queued, unsent,
    // and the queue not paused.
    h.enqueue("left by the process before").await;
    let sent = h.enqueue("sent while the app server started").await;
    h.driver.command(Cmd::Interrupt(Stop::Message(sent))).await.unwrap();

    h.driver.submit(0, false).await.unwrap();
    let start = h.request().await;
    assert_eq!(start["method"], "turn/start");
    h.event(json!({"method":"turn/started","params":{"threadId":"test-thread","turn":{"id":"turn-1","status":"inProgress"}}}))
        .await;
    h.writes_nothing().await;
    h.event(json!({"id":start["id"],"result":{"turn":{"id":"turn-1","status":"inProgress"}}}))
        .await;
    h.complete("completed").await;

    h.driver.submit(0, false).await.unwrap();
    assert_eq!(h.request().await["method"], "turn/start");
    h.event(json!({"method":"turn/started","params":{"threadId":"test-thread","turn":{"id":"turn-2","status":"inProgress"}}}))
        .await;
    let interrupt = h.request().await;
    assert_eq!(interrupt["method"], "turn/interrupt");
    assert_eq!(interrupt["params"]["turnId"], "turn-2");
}

/// A Stop kept for a message goes when the queue pauses: the message then
/// starts only when the user says so, and a start they ask for later runs.
#[tokio::test]
async fn a_stop_kept_for_a_message_goes_when_the_queue_pauses() {
    let mut h = Harness::new().await;
    h.start().await;
    let sent = h.enqueue("second").await;
    h.driver.command(Cmd::Interrupt(Stop::Message(sent.clone()))).await.unwrap();
    h.complete("failed").await;
    assert!(h.driver.state.paused, "a failed turn pauses the queue");

    // The user sends the message now.
    let (reply, _sent_now) = oneshot::channel();
    h.driver
        .command(Cmd::Queue {
            id: sent,
            steer: true,
            reply,
        })
        .await
        .unwrap();
    assert_eq!(h.request().await["method"], "turn/start");
    h.event(json!({"method":"turn/started","params":{"threadId":"test-thread","turn":{"id":"turn-2","status":"inProgress"}}}))
        .await;
    h.writes_nothing().await;
}

/// A Stop kept for a message goes when the message leaves the queue: one
/// its client sends again by the same name later runs.
#[tokio::test]
async fn a_stop_kept_for_a_message_goes_when_the_message_leaves_the_queue() {
    let mut h = Harness::new().await;
    h.start().await;
    let named = SendOptions {
        message_id: Some("named-by-its-client".into()),
        ..SendOptions::default()
    };
    let sent = h.enqueue_with("second", &named).await;
    h.driver.command(Cmd::Interrupt(Stop::Message(sent.clone()))).await.unwrap();
    let (reply, dropped) = oneshot::channel();
    h.driver
        .command(Cmd::Queue {
            id: sent,
            steer: false,
            reply,
        })
        .await
        .unwrap();
    assert_eq!(dropped.await.unwrap(), QueueOutcome::Done);

    h.enqueue_with("second, again", &named).await;
    h.complete("completed").await;
    h.driver.submit(0, false).await.unwrap();
    assert_eq!(h.request().await["method"], "turn/start");
    h.event(json!({"method":"turn/started","params":{"threadId":"test-thread","turn":{"id":"turn-2","status":"inProgress"}}}))
        .await;
    h.writes_nothing().await;
}

/// A Stop on a session with nothing running and its queue paused is
/// refused, as on one with nothing queued: no message there starts by
/// itself, so there is nothing for it to stop.
#[tokio::test]
async fn a_stop_with_nothing_running_and_the_queue_paused_is_refused() {
    let mut h = Harness::new().await;
    h.start().await;
    h.enqueue("second").await;
    h.complete("failed").await;
    assert!(h.driver.state.paused, "a failed turn pauses the queue");
    let (handle, mut stops) = h.handle();
    assert!(!handle.interrupt(), "nothing to stop");
    assert!(stops.try_recv().is_err(), "and nothing is sent");
}

/// A start of the mock app server's session for one message, through
/// `codex_starting` and `codex_attached`, with a Stop sent through the
/// session's handle: through the reserved handle, once `codex_starting`
/// has put it in the map (`before_attach`), or once the driver is attached
/// but before it has taken the message. The turn the message starts is
/// interrupted, and the session reads as working on it.
async fn a_start_stopped(before_attach: bool) {
    let home = tempfile::tempdir().unwrap();
    let (providers, link, mut socket) = mock_app_server(home.path()).await;
    let runs = codex(&providers).clone();
    let first = message("first", &SendOptions::default()).unwrap();
    let (starting, delivery) = runs.codex_starting("test-thread", None, first).unwrap();
    if before_attach {
        let handle = runs.codex_handle("test-thread").expect("reserved");
        assert!(handle.interrupt(), "a Stop through the reserved handle is taken");
    }
    let lease = providers.gate().lease(Source::Codex, "test-thread").unwrap();
    let handle = runs.codex_attached(mock_record(), link, lease, Some(starting));
    if !before_attach {
        // One task at a time here: the driver has not run yet, and the
        // message is still in its channel.
        assert!(handle.interrupt(), "a Stop before the driver took the message is taken");
    }
    delivery.taken().await.expect("the message is taken");

    assert_eq!(socket.request().await["method"], "turn/start");
    socket
        .send(json!({"method":"turn/started","params":{"threadId":"test-thread","turn":{"id":"turn-1","status":"inProgress"}}}))
        .await;
    let interrupt = socket.request().await;
    assert_eq!(interrupt["method"], "turn/interrupt");
    assert_eq!(interrupt["params"]["turnId"], "turn-1");
    assert_eq!(handle.state().phase, Phase::Working, "and no longer starting");
}

#[tokio::test]
async fn a_stop_through_the_reserved_handle_before_the_attach_stops_the_turn() {
    a_start_stopped(true).await;
}

#[tokio::test]
async fn a_stop_before_the_attached_driver_takes_the_message_stops_the_turn() {
    a_start_stopped(false).await;
}

/// A Stop in the gap between the attach and the driver taking the
/// message names the message, as it does a moment before the attach, and
/// not what the attach copied in: here a turn the resume reports in
/// progress, which runs on. The message's turn, after it, is interrupted.
#[tokio::test]
async fn a_stop_before_the_attached_driver_takes_the_message_names_it_not_the_resumed_turn() {
    let home = tempfile::tempdir().unwrap();
    let (providers, link, mut socket) = mock_app_server(home.path()).await;
    let runs = codex(&providers).clone();
    let first = message("first", &SendOptions::default()).unwrap();
    let (starting, delivery) = runs.codex_starting("test-thread", None, first).unwrap();
    let lease = providers.gate().lease(Source::Codex, "test-thread").unwrap();
    let mut record = mock_record();
    record.payload.codex.as_mut().expect("its state").active = Some("turn-0".into());
    let handle = runs.codex_attached(record, link, lease, Some(starting));
    // One task at a time here: the driver has not taken the message yet.
    assert!(handle.interrupt(), "a Stop in the gap is taken");
    delivery.taken().await.expect("the message is taken");
    socket.writes_nothing().await;

    socket
        .send(json!({"method":"turn/completed","params":{"threadId":"test-thread","turn":{"id":"turn-0","status":"completed"}}}))
        .await;
    assert_eq!(socket.request().await["method"], "turn/start");
    socket
        .send(json!({"method":"turn/started","params":{"threadId":"test-thread","turn":{"id":"turn-1","status":"inProgress"}}}))
        .await;
    let interrupt = socket.request().await;
    assert_eq!(interrupt["method"], "turn/interrupt");
    assert_eq!(interrupt["params"]["turnId"], "turn-1");
}

/// An app server lost while its session still reads as starting is not
/// left starting: the driver clears the handle's [`Handle::starting`] when
/// it loses the link, and Stop then answers as it does when nothing runs.
///
/// The link has to fail before the driver takes the start's message, which
/// with that message queued is a race `select!` may settle either way. So
/// the message is taken out of the reserved channel first: the driver is
/// attached to the same placeholder with no command pending, which is what
/// its first poll sees when the link fails ahead of the message it would
/// otherwise read.
#[tokio::test]
async fn an_app_server_lost_before_the_start_is_taken_is_no_longer_starting() {
    let home = tempfile::tempdir().unwrap();
    let (providers, link, socket) = mock_app_server(home.path()).await;
    let runs = codex(&providers).clone();
    let first = message("first", &SendOptions::default()).unwrap();
    let (mut starting, _delivery) = runs.codex_starting("test-thread", None, first).unwrap();
    let handle = starting.handle.clone();
    assert!(
        matches!(starting.rx.as_mut().unwrap().try_recv(), Ok(Cmd::Deliver(..))),
        "the start's message is in its channel"
    );
    // The app server goes before the driver can take it.
    drop(socket);
    let lease = providers.gate().lease(Source::Codex, "test-thread").unwrap();
    runs.codex_attached(mock_record(), link, lease, Some(starting));

    for _ in 0..500 {
        if handle.state().stalled {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(handle.state().stalled, "the driver saw its app server go");
    assert!(!handle.starting(), "the lost start is not left starting");
    assert_eq!(handle.state().phase, Phase::Idle, "the session reads as nothing running");
    assert!(!handle.interrupt(), "a Stop has nothing to stop");
    assert!(!providers.interrupt(Source::Codex, "test-thread"), "as the route answers it");
}

/// The placeholder a start puts in the map is announced when it goes in:
/// the session's feed and the list learn at once that the session is
/// starting, rather than at their next poll.
#[tokio::test]
async fn a_codex_start_announces_its_placeholder() {
    let home = tempfile::tempdir().unwrap();
    let (providers, _link, _socket) = mock_app_server(home.path()).await;
    let runs = codex(&providers).clone();
    let told = Arc::new(Told::default());
    runs.host
        .attach_listeners(told.clone(), droppen_http::refresh::Refresh::new(told.clone()));
    let announced = [
        format!("feed {}", runs.host.key_of(Source::Codex, "test-thread")),
        format!("list {}/test-thread", Source::Codex),
    ];
    let first = message("first", &SendOptions::default()).unwrap();
    let (starting, _delivery) = runs.codex_starting("test-thread", None, first).unwrap();
    assert_eq!(told.take(), announced, "the starting app server is announced");
    assert_eq!(
        runs.codex_handle("test-thread").expect("reserved").state().phase,
        Phase::Starting,
        "the session is on view as starting"
    );
    drop(starting);
}

/// A start given up on is announced too: the handle it puts back — or the
/// session it leaves without one — reaches the feed and the list at once,
/// so a client stops reading the session as starting without a poll.
#[tokio::test]
async fn a_codex_start_given_up_on_announces_the_placeholder_going() {
    let home = tempfile::tempdir().unwrap();
    let (providers, _link, _socket) = mock_app_server(home.path()).await;
    let runs = codex(&providers).clone();
    let told = Arc::new(Told::default());
    runs.host
        .attach_listeners(told.clone(), droppen_http::refresh::Refresh::new(told.clone()));
    let announced = [
        format!("feed {}", runs.host.key_of(Source::Codex, "test-thread")),
        format!("list {}/test-thread", Source::Codex),
    ];
    let first = message("first", &SendOptions::default()).unwrap();
    let (starting, delivery) = runs.codex_starting("test-thread", None, first).unwrap();
    told.take();

    drop(starting);
    assert!(runs.codex_handle("test-thread").is_none(), "the reservation is given back");
    assert_eq!(told.take(), announced, "so is the start given up on");
    assert!(delivery.taken().await.is_err(), "the message on its way is answered");
}

#[tokio::test]
async fn late_start_response_does_not_resurrect_a_completed_turn() {
    let mut h = Harness::new().await;
    h.enqueue("first").await;
    h.driver.submit(0, false).await.unwrap();
    let request = h.request().await;
    h.complete("completed").await;
    h.event(json!({"id":request["id"],"result":{"turn":{"id":"turn-1","status":"inProgress"}}}))
        .await;
    assert!(h.driver.state.active.is_none());
    assert!(h.driver.state.queue.is_empty());
    assert_eq!(h.driver.state.injection.as_ref().unwrap().state, InjectionRunState::Ok);
}

#[tokio::test]
async fn queue_and_pending_rpc_are_durable_before_acknowledgement() {
    let mut h = Harness::new().await;
    h.start().await;
    h.enqueue("next").await;
    h.driver.submit(0, true).await.unwrap();
    let request = h.request().await;
    let saved = h.driver.runs.host.journal.read().pop().unwrap().decode::<CodexRun>().unwrap().payload.codex.unwrap();
    assert_eq!(saved.queue[0].request.as_deref(), request["id"].as_str());
    // Reconstruct exactly the state adoption starts with, then consume the
    // response produced while remote-hand was disconnected.
    h.driver.state = serde_json::from_value(serde_json::to_value(saved).unwrap()).unwrap();
    h.event(json!({"id":request["id"],"result":{"turnId":"turn-1"}})).await;
    assert!(h.driver.runs.host.journal.read().remove(0).decode::<CodexRun>().unwrap().payload.codex.as_ref().unwrap().queue.is_empty());
    assert_eq!(h.driver.state.active.as_deref(), Some("turn-1"));
}

#[tokio::test]
async fn failed_turn_pauses_queue_and_other_thread_events_are_ignored() {
    let mut h = Harness::new().await;
    h.start().await;
    h.enqueue("next").await;
    h.event(json!({"method":"turn/completed","params":{"threadId":"other-thread","turn":{"id":"turn-1","status":"completed"}}}))
        .await;
    assert!(h.driver.state.active.is_some());
    h.complete("failed").await;
    assert!(h.driver.state.paused);
    assert_eq!(h.driver.state.queue[0].text, "next");
}

#[tokio::test]
async fn unexpected_approval_requests_are_rejected_not_approved() {
    let mut h = Harness::new().await;
    h.event(json!({"id":42,"method":"item/commandExecution/requestApproval","params":{}}))
        .await;
    let response = h.request().await;
    assert_eq!(response["id"], 42);
    assert_eq!(response["error"]["code"], -32601);
    assert!(response.get("result").is_none());
    // Byte for byte, with the request's id echoed in the type it came in.
    h.event(json!({"id":"req-7","method":"item/fileChange/requestApproval","params":{}}))
        .await;
    assert_eq!(
        h.raw().await,
        wire(r#"{"id":"req-7","error":{"code":-32601,"message":"interactive requests are not supported by Droppen"}}"#, &[])
    );
    h.event(json!({"id":43,"method":"item/commandExecution/requestApproval","params":{}}))
        .await;
    assert_eq!(
        h.raw().await,
        wire(r#"{"id":43,"error":{"code":-32601,"message":"interactive requests are not supported by Droppen"}}"#, &[])
    );
}

#[tokio::test]
async fn dead_process_retains_queue_and_allows_explicit_withdrawal() {
    let mut h = Harness::new().await;
    let id = h.enqueue("possibly delivered").await;
    h.driver.submit(0, false).await.unwrap();
    h.request().await;
    let record = h.driver.runs.host.journal.read().pop().unwrap().decode().unwrap();
    h.driver.runs.codex_dead(record);
    let handle = h.driver.runs.codex_handle("test-thread").unwrap();
    assert!(!handle.alive());
    assert!(handle.visible());
    assert_eq!(handle.info().queued, 1);
    assert!(!handle.info().queued_messages[0].steered, "withdrawal is safe once the process is dead");
    assert!(h.driver.runs.host.journal.read().remove(0).decode::<CodexRun>().unwrap().payload.codex.as_ref().unwrap().queue[0].uncertain);
    assert_eq!(h.driver.runs.codex_queue_action("test-thread", &id, false).await, QueueOutcome::Done);
    assert!(h.driver.runs.host.journal.read().remove(0).decode::<CodexRun>().unwrap().payload.codex.as_ref().unwrap().queue.is_empty());
}

#[tokio::test]
async fn acknowledgement_timeout_never_resubmits_and_late_ack_still_reconciles() {
    let mut h = Harness::new().await;
    h.enqueue("first").await;
    h.driver.submit(0, false).await.unwrap();
    let request = h.request().await;
    h.driver.pending_since = Instant::now() - RPC_TIMEOUT - Duration::from_secs(1);
    h.driver.tick().await.unwrap();
    assert!(h.driver.state.paused);
    assert!(h.driver.state.queue[0].request.is_some());
    h.event(json!({"id":request["id"],"result":{"turn":{"id":"turn-1","status":"inProgress"}}}))
        .await;
    assert!(h.driver.state.queue.is_empty());
    assert_eq!(h.driver.state.active.as_deref(), Some("turn-1"));
    assert_eq!(h.driver.state.injection.as_ref().unwrap().state, InjectionRunState::Running);
}

#[tokio::test]
async fn steering_cannot_change_model_or_drop_a_written_message() {
    let mut h = Harness::new().await;
    h.start().await;
    let id = h.enqueue("use different model").await;
    h.driver.state.queue[0].params["model"] = json!("other-model");
    let (reply, rx) = oneshot::channel();
    h.driver
        .command(Cmd::Queue {
            id: id.clone(),
            steer: true,
            reply,
        })
        .await
        .unwrap();
    assert!(matches!(rx.await.unwrap(), QueueOutcome::Refused { .. }));
    h.driver.state.queue[0].params.as_object_mut().unwrap().remove("model");
    h.driver.submit(0, true).await.unwrap();
    h.request().await;
    let (reply, rx) = oneshot::channel();
    h.driver.command(Cmd::Queue { id, steer: false, reply }).await.unwrap();
    assert_eq!(rx.await.unwrap(), QueueOutcome::AlreadySteered);
}

/// Codex gets the same receipts: a named message is queued until codex
/// acknowledges the `turn/start` that carries it, sent once it has, and
/// failed when it is withdrawn instead.
#[tokio::test]
async fn a_named_messages_receipt_follows_it_into_codex_and_out_of_the_queue() {
    use droppen_provider::receipts::{Claim, Kind};
    use droppen_sessions::delivery::ReceiptStatus;
    let mut h = Harness::new().await;
    let receipts = h.driver.runs.host.receipts().clone();
    let named = |text: &str| {
        let client = new_uuid();
        let now = droppen_provider::receipts::now_ms();
        let Claim::Owned { qid } = receipts.claim(&client, Kind::Message, Source::Codex, Some("test-thread"), now).unwrap() else {
            panic!("a fresh id is owned");
        };
        receipts.complete(&client, "{}", None, now);
        let options = SendOptions {
            message_id: Some(qid.clone()),
            ..SendOptions::default()
        };
        let message = message(text, &options).unwrap();
        assert_eq!(message.id, qid, "the message travels under the queue id the claim chose");
        message
    };
    let status = |receipts: &droppen_provider::receipts::Receipts, qid: &str| {
        receipts
            .for_session(Source::Codex, "test-thread", droppen_provider::receipts::now_ms())
            .into_iter()
            .find(|receipt| receipt.qid.0.as_deref() == Some(qid))
            .map(|receipt| receipt.status)
    };

    let first = named("first");
    let first_id = first.id.clone();
    let (tx, rx) = oneshot::channel();
    h.driver.command(Cmd::Deliver(first, tx)).await.unwrap();
    rx.await.unwrap().unwrap();
    h.driver.submit(0, false).await.unwrap();
    let request = h.request().await;
    assert_eq!(request["params"]["clientUserMessageId"], first_id.as_str());
    assert_eq!(status(&receipts, &first_id), Some(ReceiptStatus::Queued), "written, not yet acknowledged");
    h.event(json!({"id":request["id"],"result":{"turn":{"id":"turn-1","status":"inProgress"}}}))
        .await;
    assert_eq!(status(&receipts, &first_id), Some(ReceiptStatus::Sent));

    let second = named("withdrawn");
    let second_id = second.id.clone();
    let (tx, rx) = oneshot::channel();
    h.driver.command(Cmd::Deliver(second, tx)).await.unwrap();
    rx.await.unwrap().unwrap();
    let (reply, rx) = oneshot::channel();
    h.driver
        .command(Cmd::Queue {
            id: second_id.clone(),
            steer: false,
            reply,
        })
        .await
        .unwrap();
    assert_eq!(rx.await.unwrap(), QueueOutcome::Done);
    assert_eq!(status(&receipts, &second_id), Some(ReceiptStatus::Failed));
}
