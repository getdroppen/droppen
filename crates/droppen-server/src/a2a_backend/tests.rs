//! The backend end to end: A2A tasks driven through the providers the entry
//! registers and the alerts it wires, over a fake `claude` that a test steers
//! by what its prompts say.
//!
//! The fake is a resident like the real one — `system:init`, then one turn
//! per message on stdin, each written to a transcript where the real CLI
//! writes it — and reads its instructions out of the prompt, so a test needs
//! no files in the session's directory (which, for a worktree, it does not
//! know beforehand):
//!
//! * `[ask]` — stop on an `AskUserQuestion` and wait for the answer, which
//!   the reply then repeats;
//! * `[hold]` — say "Working on it" and hold the turn until the test drops
//!   `go` into the fake's control directory; an interrupt ends it first;
//! * `[fail]` — end the turn with an error result;
//! * `[write <name>]` — create `<name>` in the session's directory;
//! * `[pr]` — name a pull request in the reply;
//! * `[limit]` — the first time, refuse the turn with a session limit, as
//!   claude does when an account's five hours have run out.
//!
//! A `refuse-start` file in the control directory makes it exit before it
//! says it is listening. Every spawn is logged (`spawns.jsonl`), and every
//! line it reads (`stdin.jsonl`).

use std::path::{Path, PathBuf};

use droppen_a2a::backend::{AnswerValue, Prompt, RunAccount, SessionRef};

use super::*;

/// The id every test's caller has.
const CALLER: &str = "a2a:token:dont-do";

/// The fake `claude`, with `{control}` its control directory.
const FAKE_CLAUDE: &str = r#"#!/usr/bin/env python3
import datetime, json, os, queue, re, sys, threading, time, uuid

CONTROL = {control}

def log(name, value):
    with open(os.path.join(CONTROL, name), "a") as f:
        f.write(json.dumps(value) + "\n")

args = sys.argv[1:]
session = None
for i, arg in enumerate(args[:-1]):
    if arg in ("--session-id", "--resume"):
        session = args[i + 1]
log("spawns.jsonl", {"argv": args, "cwd": os.getcwd()})
if os.path.exists(os.path.join(CONTROL, "refuse-start")):
    sys.stderr.write("this claude cannot start\n")
    sys.exit(2)

# The root the real CLI writes to: the one it is pointed at, else `~/.claude`
# of the home the server reads (the test's, beside the control directory).
root = os.environ.get("CLAUDE_CONFIG_DIR") or os.path.join(os.path.dirname(CONTROL), ".claude")
transcript = os.path.join(root, "projects", "-a2a", "%s.jsonl" % session)
os.makedirs(os.path.dirname(transcript), exist_ok=True)

def stamp():
    return datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%f")[:-3] + "Z"

def out(value):
    sys.stdout.write(json.dumps(value) + "\n")
    sys.stdout.flush()

def record(entry):
    entry.update(uuid=str(uuid.uuid4()), timestamp=stamp(), sessionId=session, cwd=os.getcwd())
    with open(transcript, "a") as f:
        f.write(json.dumps(entry) + "\n")

def say(text):
    record({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "text", "text": text}]}})
    out({"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}})

def result(error=None):
    if error:
        out({"type": "result", "subtype": error, "is_error": True})
    else:
        out({"type": "result", "subtype": "success", "is_error": False})

lines = queue.Queue()
def read():
    for line in sys.stdin:
        lines.put(line)
    lines.put(None)
threading.Thread(target=read, daemon=True).start()

# Messages written while a turn runs: buffered, and run as turns of their own.
buffered = []

def answer_control(message):
    out({"type": "control_response", "response": {"subtype": "success", "request_id": message.get("request_id")}})

def mid_turn(wanted, idle=None):
    """Read stdin until `wanted(message)` answers; an interrupt answers False.
    `idle` is asked between reads, and its truthy answer is returned."""
    deadline = time.time() + 60
    while time.time() < deadline:
        if idle:
            done = idle()
            if done:
                return done
        try:
            line = lines.get(timeout=0.05)
        except queue.Empty:
            continue
        if line is None:
            sys.exit(0)
        message = json.loads(line)
        log("stdin.jsonl", message)
        if message.get("type") == "control_request":
            answer_control(message)
            if message.get("request", {}).get("subtype") == "interrupt":
                return False
            continue
        found = wanted(message)
        if found:
            return found
        if message.get("type") == "user":
            buffered.append(message)
    sys.exit(97)

asked = 0
def ask():
    global asked
    asked += 1
    rid = "req-%d-%s" % (asked, session[:8])
    questions = [{"question": "Which colour?", "header": "Colour", "multiSelect": False,
                  "options": [{"label": "Red", "description": "Warm"}, {"label": "Blue", "description": "Cool"}]}]
    out({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "toolu_" + rid, "name": "AskUserQuestion", "input": {}}]}})
    out({"type": "control_request", "request_id": rid, "request": {"subtype": "can_use_tool", "tool_name": "AskUserQuestion",
         "display_name": "AskUserQuestion", "input": {"questions": questions}, "tool_use_id": "toolu_" + rid,
         "requires_user_interaction": True}})
    def answered(message):
        response = message.get("response", {})
        if message.get("type") == "control_response" and response.get("request_id") == rid:
            return response.get("response") or {"denied": True}
        return None
    found = mid_turn(answered)
    if found is False:
        out({"type": "control_cancel_request", "request_id": rid})
    return found

def released():
    go = os.path.join(CONTROL, "go")
    if os.path.exists(go):
        os.remove(go)
        return True
    return None

def text_of(message):
    content = message.get("message", {}).get("content", "")
    if isinstance(content, str):
        return content
    return "\n".join(block.get("text", "") for block in content if block.get("type") == "text")

def turn(message):
    text = text_of(message)
    record({"type": "user", "message": {"role": "user", "content": text}})
    for name in re.findall(r"\[write ([\w.-]+)\]", text):
        with open(name, "w") as f:
            f.write("written by the task\n")
    if "[ask]" in text:
        answer = ask()
        if answer is False:
            result("error_during_execution")
            return
        given = answer.get("updatedInput", {})
        say("Answered: %s %s" % (json.dumps(given.get("answers", {}), sort_keys=True), json.dumps(given.get("annotations", {}), sort_keys=True)))
    if "[hold]" in text:
        say("Working on it")
        if not mid_turn(lambda message: None, released):
            result("error_during_execution")
            return
    if "[fail]" in text:
        result("error_during_execution")
        return
    limited = os.path.join(CONTROL, "limit-hit")
    if "[limit]" in text and not os.path.exists(limited):
        open(limited, "w").close()
        out({"type": "rate_limit_event", "rate_limit_info": {"status": "rejected", "rateLimitType": "five_hour",
             "resetsAt": int(time.time()) + 7200, "overageStatus": "rejected", "isUsingOverage": False}})
        out({"type": "result", "subtype": "success", "is_error": True, "result": "You have hit your session limit", "api_error_status": 429})
        return
    words = " ".join(re.sub(r"\[[^\]]*\]", " ", text).split())
    reply = "Done: %s." % words[-60:]
    if "[pr]" in text:
        reply += " Opened https://github.com/acme/widgets/pull/42."
    say(reply)
    result()

out({"type": "system", "subtype": "init", "session_id": session})
while True:
    if buffered:
        message = buffered.pop(0)
        out({"type": "system", "subtype": "init", "session_id": session})
    else:
        line = lines.get()
        if line is None:
            break
        message = json.loads(line)
        log("stdin.jsonl", message)
    if message.get("type") == "control_request":
        answer_control(message)
    elif message.get("type") == "user":
        turn(message)
"#;

/// Codex's provider's own App Server fixture, which runs every turn to its end
/// at once and writes no transcript.
const FAKE_CODEX: &str = include_str!("../../../droppen-provider-codex/tests/support/fake_codex.py");

/// The server's stand-in ACP agent, which answers a prompt with a few
/// sentences and ends its turn.
const FAKE_ACP_AGENT: &str = include_str!("../../tests/support/fake_acp_agent.py");

/// A server's state over a scratch home with the fake `claude` on its
/// children's `PATH`, its A2A backend, and a git repository for the entry.
struct Harness {
    home: tempfile::TempDir,
    _path: droppen_base::process_env::ChildPathGuard,
    state: crate::api::AppState,
    sessions: Arc<A2aSessions>,
    events: mpsc::UnboundedReceiver<BackendEvent>,
    /// Every event so far, for a failure to show.
    seen: Vec<BackendEvent>,
    /// The fake's control directory.
    control: PathBuf,
    /// The entry's directory: a repository with one commit, on `main`.
    work: PathBuf,
}

/// Run git in `dir`, as a test does it: no config of the machine's own.
fn git(dir: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .stdout(std::process::Stdio::null())
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?}");
}

impl Harness {
    async fn new() -> Self {
        // The crate's own shim hook is for its own tests: this binary's shim is the server's.
        droppen_provider::testing::shims_run_as(crate::provider_tests::SHIMS);
        let home = tempfile::tempdir().expect("tempdir");
        let control = home.path().join("control");
        let bin = home.path().join("bin");
        std::fs::create_dir_all(&control).expect("control dir");
        std::fs::create_dir_all(&bin).expect("bin dir");
        let script = FAKE_CLAUDE.replace("{control}", &serde_json::to_string(&control).expect("a path"));
        std::fs::write(bin.join("claude"), script).expect("the fake claude");
        std::fs::write(bin.join("codex"), FAKE_CODEX).expect("the fake codex");
        // Where the Codex fixture finds it is in a test.
        std::fs::write(home.path().join(".rh-test-root"), "").expect("the test root");
        for cli in ["claude", "codex"] {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(bin.join(cli), std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        let path = droppen_base::process_env::ChildPathGuard::take();
        droppen_base::process_env::set_child_path(Some(format!("{}:{}", bin.display(), droppen_base::process_env::inherited_path())));
        let work = home.path().join("work");
        std::fs::create_dir_all(&work).expect("work dir");
        git(&work, &["init", "-q", "-b", "main", "."]);
        std::fs::write(work.join("README.md"), "the widget\n").expect("a file");
        git(&work, &["add", "."]);
        git(&work, &["commit", "-qm", "first"]);
        let state = crate::api::test_support::state(home.path()).await;
        let sessions = state.a2a_sessions.clone();
        let events = sessions.tap();
        sessions.adopted();
        Self {
            home,
            _path: path,
            state,
            sessions,
            events,
            seen: Vec::new(),
            control,
            work,
        }
    }

    fn work(&self) -> String {
        self.work.to_string_lossy().into_owned()
    }

    fn run(&self, cwd: &str, isolation: Isolation) -> RunSpec {
        RunSpec {
            source: EntrySource::Claude,
            acp_agent: None,
            cwd: cwd.to_string(),
            account: RunAccount::Local,
            model: None,
            effort: None,
            permission: None,
            isolation,
        }
    }

    /// Start task `id` in context `context` with `text`, in `run`.
    async fn start(&self, id: &str, context: &str, run: &RunSpec, text: &str) -> Result<Started, BackendError> {
        self.sessions
            .start(StartSession {
                task: task(id, context, None, &run.cwd, None),
                run: run.clone(),
                prompt: prompt(text),
                client_message_id: format!("a2a:{CALLER}:{id}"),
            })
            .await
    }

    /// Send task `id` to the session `started` made, as `kind`, under
    /// message `message`.
    async fn send(&self, id: &str, context: &str, started: &Started, kind: SendKind, message: &str, text: &str) -> Result<(), BackendError> {
        self.sessions
            .send(SendToSession {
                task: task(id, context, started.session_id.clone(), &started.cwd, started.branch.clone()),
                kind,
                prompt: prompt(text),
                client_message_id: format!("a2a:{CALLER}:{message}"),
            })
            .await
    }

    /// The next event `matches` takes, the ones before it kept in `seen`.
    async fn event(&mut self, what: &str, matches: impl Fn(&BackendEvent) -> bool) -> BackendEvent {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            match tokio::time::timeout_at(deadline, self.events.recv()).await {
                Ok(Some(event)) => {
                    self.seen.push(event.clone());
                    if matches(&event) {
                        return event;
                    }
                }
                Ok(None) => panic!("the backend's events stopped; saw {:#?}", self.seen),
                Err(_) => panic!("{what}: not reported in time; saw {:#?}", self.seen),
            }
        }
    }

    /// Task `id`'s turn ended: how.
    async fn ended(&mut self, id: &str) -> TurnOutcome {
        let id = id.to_string();
        match self
            .event(
                &format!("{id}'s end"),
                |event| matches!(event, BackendEvent::TurnEnded { task_id, .. } if *task_id == id),
            )
            .await
        {
            BackendEvent::TurnEnded { outcome, .. } => outcome,
            _ => unreachable!(),
        }
    }

    /// Task `id`'s turn ended well: its artifacts.
    async fn completed(&mut self, id: &str) -> TurnResult {
        match self.ended(id).await {
            TurnOutcome::Completed(result) => result,
            other => panic!("{id} ended {other:?}; saw {:#?}", self.seen),
        }
    }

    /// Task `id`'s message went into its turn.
    async fn turn_started(&mut self, id: &str) {
        let id = id.to_string();
        self.event(
            &format!("{id}'s turn"),
            |event| matches!(event, BackendEvent::TurnStarted { task_id } if *task_id == id),
        )
        .await;
    }

    /// What task `id` was reported doing in the next `wait`.
    async fn quiet(&mut self, id: &str, wait: Duration) -> Vec<BackendEvent> {
        let deadline = tokio::time::Instant::now() + wait;
        let mut about = Vec::new();
        while let Ok(Some(event)) = tokio::time::timeout_at(deadline, self.events.recv()).await {
            self.seen.push(event.clone());
            if event_task(&event) == Some(id) {
                about.push(event);
            }
        }
        about
    }

    /// Let the fake's held turn finish.
    fn release(&self) {
        std::fs::write(self.control.join("go"), "").expect("go");
    }

    /// Every user message the fake read off its stdin.
    fn read(&self) -> Vec<String> {
        std::fs::read_to_string(self.control.join("stdin.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter(|line| line.contains("\"user\""))
            .map(str::to_string)
            .collect()
    }

    /// Every process the fake was started as: its arguments and directory.
    fn spawns(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.control.join("spawns.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).expect("a spawn"))
            .collect()
    }

    /// Wait until session `id`'s resident is between turns with nothing queued.
    async fn settled(&self, id: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let idle = self
                .state
                .drive
                .providers
                .resident_info(claude(), id)
                .is_some_and(|resident| resident.phase == ResidentPhase::Idle && resident.queued == 0);
            if idle {
                return;
            }
            assert!(tokio::time::Instant::now() < deadline, "session {id} never settled");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Forget every task followed in memory, as a restarted server has: what
    /// is written down stays.
    fn restart(&self) {
        self.sessions.inner.tasks.lock().unwrap_or_else(PoisonError::into_inner).clear();
    }
}

fn claude() -> Source {
    Source::Claude
}

fn task(id: &str, context: &str, session: Option<String>, cwd: &str, branch: Option<String>) -> TaskRef {
    TaskRef {
        task_id: id.to_string(),
        context_id: context.to_string(),
        entry_id: "widgets".to_string(),
        caller: CALLER.to_string(),
        session: SessionRef {
            source: EntrySource::Claude,
            session_id: session,
            cwd: cwd.to_string(),
            branch,
        },
    }
}

fn prompt(text: &str) -> Prompt {
    Prompt {
        text: text.to_string(),
        images: Vec::new(),
    }
}

fn event_task(event: &BackendEvent) -> Option<&str> {
    match event {
        BackendEvent::TurnStarted { task_id }
        | BackendEvent::Progress { task_id, .. }
        | BackendEvent::Question { task_id, .. }
        | BackendEvent::QuestionResolved { task_id }
        | BackendEvent::TurnEnded { task_id, .. } => Some(task_id),
        BackendEvent::SessionIdentified { .. } | BackendEvent::SessionGone { .. } => None,
    }
}

/// A context id, as the protocol side mints one.
fn context() -> String {
    droppen_provider::common::new_uuid()
}

/// A new session's task is taken into its turn and completes with both
/// artifacts: the reply, and the result naming the session, the directory,
/// the branch it ran on, the file it made and the pull request it named.
#[tokio::test]
async fn a_new_session_completes_with_its_artifacts() {
    let mut harness = Harness::new().await;
    let run = harness.run(&harness.work(), Isolation::None);
    let started = harness
        .start("t1", &context(), &run, "fence-1 [write made.txt] [pr] fix the widget")
        .await
        .expect("started");
    assert_eq!(started.cwd, harness.work());
    assert_eq!(started.branch, None, "no worktree, no branch of its own");
    let session = started.session_id.clone().expect("the session's id");

    harness.turn_started("t1").await;
    let result = harness.completed("t1").await;
    assert_eq!(
        result.summary,
        "Done: fence-1 fix the widget. Opened https://github.com/acme/widgets/pull/42."
    );
    assert_eq!(
        result.result,
        ResultData {
            source: "claude".into(),
            session_id: session,
            host: result.result.host.clone(),
            cwd: harness.work(),
            branch: Some("main".into()),
            changed_files: vec!["made.txt".into()],
            pr_urls: vec!["https://github.com/acme/widgets/pull/42".into()],
            droppen_url: None,
        }
    );
    assert!(!result.result.host.is_empty(), "the device's name");
    assert!(
        harness.sessions.inner.store.task("t1").is_none(),
        "an ended task is not written down any more"
    );
}

/// A second task in the context is the next turn of the same session, on
/// the same process, and ends with its own reply.
#[tokio::test]
async fn a_follow_up_is_the_next_turn_of_the_same_session() {
    let mut harness = Harness::new().await;
    let run = harness.run(&harness.work(), Isolation::None);
    let ctx = context();
    let started = harness.start("t1", &ctx, &run, "fence-1 first").await.expect("started");
    harness.completed("t1").await;

    harness
        .send("t2", &ctx, &started, SendKind::NewTurn, "m2", "fence-2 second")
        .await
        .expect("sent");
    harness.turn_started("t2").await;
    let result = harness.completed("t2").await;
    assert_eq!(result.summary, "Done: fence-2 second.");
    assert_eq!(Some(result.result.session_id), started.session_id);
    assert_eq!(harness.spawns().len(), 1, "one process for both turns");
}

/// A running task says what it is doing, and more sent to it goes into its
/// turn: the task ends with the turn that ran it, not before.
#[tokio::test]
async fn a_running_task_reports_progress_and_takes_more() {
    let mut harness = Harness::new().await;
    let run = harness.run(&harness.work(), Isolation::None);
    let ctx = context();
    let started = harness.start("t1", &ctx, &run, "fence-1 [hold] first part").await.expect("started");
    harness.turn_started("t1").await;
    match harness.event("progress", |event| matches!(event, BackendEvent::Progress { .. })).await {
        BackendEvent::Progress { task_id, text } => assert_eq!((task_id.as_str(), text.as_str()), ("t1", "Working on it")),
        _ => unreachable!(),
    }
    harness
        .send("t1", &ctx, &started, SendKind::Append, "m1b", "fence-1b [write more.txt] second part")
        .await
        .expect("added");
    harness.release();
    let result = harness.completed("t1").await;
    assert_eq!(
        result.summary, "Done: fence-1b second part.",
        "the task's last turn is the added message's"
    );
    assert_eq!(result.result.changed_files, ["more.txt"]);
    assert!(harness.read().iter().any(|line| line.contains("fence-1b")), "the added message ran");
    assert_eq!(
        harness
            .seen
            .iter()
            .filter(|event| matches!(event, BackendEvent::TurnEnded { .. }))
            .count(),
        1
    );
}

/// A Codex session's task: the thread `thread/start` names is the session,
/// the turn it starts is the task's, and the task completes when it ends.
#[tokio::test]
async fn a_codex_session_completes() {
    let mut harness = Harness::new().await;
    let run = RunSpec {
        source: EntrySource::Codex,
        ..harness.run(&harness.work(), Isolation::None)
    };
    let started = harness.start("t1", &context(), &run, "fence-1 for codex").await.expect("started");
    assert_eq!(started.session_id.as_deref(), Some("0199c1a1-2b3c-4d5e-8f60-112233445566"));
    harness.turn_started("t1").await;
    let result = harness.completed("t1").await;
    assert_eq!(result.result.source, "codex");
    assert_eq!(Some(result.result.session_id), started.session_id);
}

/// An entry may run an ACP agent (unlike a schedule, it names the agent): the
/// task's turn is the agent's prompt turn, and it completes when that ends.
#[tokio::test]
async fn an_acp_session_completes() {
    use std::os::unix::fs::PermissionsExt;
    let mut harness = Harness::new().await;
    let agent = harness.home.path().join("bin").join("fake-agent");
    std::fs::write(&agent, FAKE_ACP_AGENT).expect("the fake agent");
    std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let update = droppen_provider_acp::agents::AcpAgentUpdate {
        name: Optional(None),
        command: Optional(Some(agent.to_string_lossy().into_owned())),
        args: Optional(Some(vec!["--acp".into(), "--stdio".into()])),
        env: Optional(None),
    };
    droppen_provider_acp::testing::configure(&harness.state.paths, "fake", &update).expect("configured");
    let run = RunSpec {
        source: EntrySource::Acp,
        acp_agent: Some("fake".into()),
        ..harness.run(&harness.work(), Isolation::None)
    };
    assert_eq!(harness.sessions.check_entry(&run).await, Ok(()));
    assert!(
        harness
            .sessions
            .check_entry(&RunSpec {
                acp_agent: Some("other".into()),
                ..run.clone()
            })
            .await
            .is_err(),
        "an agent that is not configured"
    );
    let started = harness.start("t1", &context(), &run, "fence-1 for the agent").await.expect("started");
    harness.turn_started("t1").await;
    let result = harness.completed("t1").await;
    assert_eq!(result.result.source, "acp");
    assert_eq!(Some(result.result.session_id), started.session_id);
}

/// Store a Claude account `label` in the harness's server, an OAuth one, which
/// is in the rotation's pool.
async fn account(harness: &Harness, label: &str) -> droppen_accounts::store::Account {
    use droppen_accounts::store::{AccountProvider, Credential};
    let token = format!("sk-ant-oat01-test-{}", label.replace('@', "-"));
    harness
        .state
        .drive
        .accounts
        .create(label, AccountProvider::Claude, Credential::Oauth(token))
        .await
        .expect("an account")
}

/// A turn a limit refuses is not the task's end when the driver moves the
/// session to the next account: the task stays working, says so, and
/// completes with the turn the driver continues there.
#[tokio::test]
async fn a_turn_a_limit_moves_to_another_account_is_still_the_task_s() {
    let mut harness = Harness::new().await;
    let first = account(&harness, "a@work").await;
    let next = account(&harness, "b@team").await;
    let run = RunSpec {
        account: RunAccount::Id(first.id.clone()),
        ..harness.run(&harness.work(), Isolation::None)
    };
    let started = harness
        .start("t1", &context(), &run, "fence-1 [limit] heavy work")
        .await
        .expect("started");
    let session = started.session_id.clone().expect("the session");
    harness.turn_started("t1").await;
    let result = harness.completed("t1").await;
    assert!(result.summary.starts_with("Done:"), "the continued turn's reply: {}", result.summary);
    assert!(
        harness
            .seen
            .iter()
            .any(|event| matches!(event, BackendEvent::Progress { text, .. } if text.contains("b@team"))),
        "the move is reported as progress: {:#?}",
        harness.seen
    );
    assert_eq!(
        harness.state.drive.accounts.session_account_id(&session).await.as_deref(),
        Some(next.id.as_str()),
        "the session moved"
    );
    let spawns = harness.spawns();
    assert_eq!(spawns.len(), 2, "a second process, under the next account: {spawns:?}");
    assert!(spawns[1]["argv"].as_array().is_some_and(|argv| argv.iter().any(|arg| arg == "--resume")));
}

/// A turn a limit refuses with nowhere to move the session fails the task,
/// with the limit.
#[tokio::test]
async fn a_turn_a_limit_holds_fails_the_task() {
    let mut harness = Harness::new().await;
    let only = account(&harness, "a@work").await;
    let run = RunSpec {
        account: RunAccount::Id(only.id.clone()),
        ..harness.run(&harness.work(), Isolation::None)
    };
    harness
        .start("t1", &context(), &run, "fence-1 [limit] heavy work")
        .await
        .expect("started");
    match harness.ended("t1").await {
        TurnOutcome::Failed(why) => assert!(why.contains("session limit"), "{why}"),
        other => panic!("{other:?}"),
    }
}

/// A turn a limit refuses on a session nothing moves (the CLI's own login)
/// fails the task, and the turn of the next task queued behind it is that
/// task's, not this one's.
#[tokio::test]
async fn a_limit_nothing_moves_fails_the_task_and_not_the_next() {
    let mut harness = Harness::new().await;
    let run = harness.run(&harness.work(), Isolation::None);
    let ctx = context();
    let started = harness
        .start("t1", &ctx, &run, "fence-1 [hold] [limit] heavy work")
        .await
        .expect("started");
    harness.turn_started("t1").await;
    harness
        .send("t2", &ctx, &started, SendKind::NewTurn, "m2", "fence-2 next")
        .await
        .expect("queued");
    harness.release();
    match harness.ended("t1").await {
        TurnOutcome::Failed(why) => assert!(why.contains("session limit"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(harness.completed("t2").await.summary, "Done: fence-2 next.");
}

/// A completed task's artifacts are made with a time limit: past it, the
/// task still completes, with the fields that need no git and no transcript
/// and empty lists for the rest.
#[tokio::test]
async fn a_task_completes_without_the_artifacts_that_take_too_long() {
    let mut harness = Harness::new().await;
    harness.sessions.inner.artifacts_wait_ms.store(0, Ordering::Relaxed);
    let run = harness.run(&harness.work(), Isolation::None);
    let started = harness
        .start("t1", &context(), &run, "fence-1 [write made.txt] [pr] quick")
        .await
        .expect("started");
    let result = harness.completed("t1").await;
    assert_eq!(result.summary, "");
    assert_eq!(
        result.result,
        ResultData {
            source: "claude".into(),
            session_id: started.session_id.expect("the session"),
            host: droppen_http::hostname(),
            cwd: harness.work(),
            branch: None,
            changed_files: Vec::new(),
            pr_urls: Vec::new(),
            droppen_url: None,
        }
    );
}

/// A question stops the task (`INPUT_REQUIRED`); its answer — a choice and
/// words beside it — goes into the turn that asked, which then completes.
/// An answer given through the backend is no news of its own.
#[tokio::test]
async fn a_question_is_answered_with_a_choice_and_words_and_the_task_completes() {
    let mut harness = Harness::new().await;
    let run = harness.run(&harness.work(), Isolation::None);
    let ctx = context();
    let started = harness.start("t1", &ctx, &run, "fence-1 [ask] pick a colour").await.expect("started");
    let question = match harness
        .event("the question", |event| matches!(event, BackendEvent::Question { .. }))
        .await
    {
        BackendEvent::Question { question, .. } => question,
        _ => unreachable!(),
    };
    assert_eq!(question.questions.len(), 1);
    assert_eq!(question.questions[0].question, "Which colour?");
    assert_eq!(question.questions[0].header, "Colour");
    assert_eq!(
        question.questions[0]
            .options
            .iter()
            .map(|option| option.label.as_str())
            .collect::<Vec<_>>(),
        ["Red", "Blue"]
    );

    let answer = a2a::Answer {
        request_id: question.request_id.clone(),
        answers: BTreeMap::from([("Which colour?".to_string(), AnswerValue::One("Blue".into()))]),
        notes: BTreeMap::new(),
        response: Some("the darker one".into()),
    };
    let asked = task("t1", &ctx, started.session_id.clone(), &started.cwd, None);
    harness.sessions.answer(&asked, answer.clone()).await.expect("answered");
    let result = harness.completed("t1").await;
    assert!(result.summary.contains(r#""Which colour?": "Blue""#), "{}", result.summary);
    assert!(
        result.summary.contains("the darker one"),
        "the words go with the choice: {}",
        result.summary
    );
    assert!(
        !harness.seen.iter().any(|event| matches!(event, BackendEvent::QuestionResolved { .. })),
        "{:#?}",
        harness.seen
    );
    assert_eq!(
        harness.sessions.answer(&asked, answer).await,
        Err(BackendError::QuestionClosed),
        "answered once"
    );
}

/// An answer that does not fit the question is refused as invalid, and the
/// task still waits on it: a fitting one afterwards goes in as any would.
#[tokio::test]
async fn an_answer_that_fits_no_question_is_invalid_and_the_task_still_waits() {
    let mut harness = Harness::new().await;
    let run = harness.run(&harness.work(), Isolation::None);
    let ctx = context();
    let started = harness.start("t1", &ctx, &run, "fence-1 [ask] pick a colour").await.expect("started");
    let request_id = match harness
        .event("the question", |event| matches!(event, BackendEvent::Question { .. }))
        .await
    {
        BackendEvent::Question { question, .. } => question.request_id,
        _ => unreachable!(),
    };
    let asked = task("t1", &ctx, started.session_id.clone(), &started.cwd, None);
    let wrong = a2a::Answer {
        request_id: request_id.clone(),
        answers: BTreeMap::from([("Which shade?".to_string(), AnswerValue::One("Blue".into()))]),
        notes: BTreeMap::new(),
        response: None,
    };
    match harness.sessions.answer(&asked, wrong).await {
        Err(BackendError::Invalid(why)) => assert!(why.contains("Which shade?"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(
        harness.quiet("t1", Duration::from_millis(1000)).await.is_empty(),
        "nothing happened to the task"
    );
    let right = a2a::Answer {
        request_id,
        answers: BTreeMap::from([("Which colour?".to_string(), AnswerValue::One("Red".into()))]),
        notes: BTreeMap::new(),
        response: None,
    };
    harness.sessions.answer(&asked, right).await.expect("answered");
    let result = harness.completed("t1").await;
    assert!(result.summary.contains(r#""Which colour?": "Red""#), "{}", result.summary);
}

/// A turn a person stops with Stop in Droppen ends its task canceled, not
/// failed.
#[tokio::test]
async fn a_turn_stopped_in_droppen_cancels_the_task() {
    let mut harness = Harness::new().await;
    let run = harness.run(&harness.work(), Isolation::None);
    let started = harness.start("t1", &context(), &run, "fence-1 [hold] a long one").await.expect("started");
    harness.turn_started("t1").await;
    let session = started.session_id.expect("the session");
    assert!(harness.state.drive.providers.interrupt(claude(), &session), "stopped as the page's Stop does");
    match harness.ended("t1").await {
        TurnOutcome::Canceled(said) => assert_eq!(said, "stopped in Droppen"),
        other => panic!("{other:?}"),
    }
}

/// A question answered on the Droppen page is resolved for the task, which
/// goes back to work and completes.
#[tokio::test]
async fn a_question_answered_in_droppen_resolves_it() {
    let mut harness = Harness::new().await;
    let run = harness.run(&harness.work(), Isolation::None);
    let started = harness.start("t1", &context(), &run, "fence-1 [ask] pick").await.expect("started");
    let request_id = match harness
        .event("the question", |event| matches!(event, BackendEvent::Question { .. }))
        .await
    {
        BackendEvent::Question { question, .. } => question.request_id,
        _ => unreachable!(),
    };
    let reply = Reply::Answer(questions::Answer {
        answers: BTreeMap::from([("Which colour?".to_string(), "Red".to_string())]),
        ..questions::Answer::default()
    });
    let session = started.session_id.expect("the session");
    droppen_drive::routes::settle_question(&harness.state.drive, claude(), &session, &request_id, reply, Some("someone".into()))
        .await
        .expect("answered on the page");
    harness
        .event(
            "the question's close",
            |event| matches!(event, BackendEvent::QuestionResolved { task_id } if task_id == "t1"),
        )
        .await;
    let result = harness.completed("t1").await;
    assert!(result.summary.contains(r#""Which colour?": "Red""#), "{}", result.summary);
}

/// A cancelled task that is still queued is taken out of the queue and never
/// runs; a running one has its turn stopped; and nothing more is said of
/// either. Cancelling again, or a task this backend never had, is fine.
#[tokio::test]
async fn a_queued_task_is_withdrawn_and_a_running_one_interrupted() {
    let mut harness = Harness::new().await;
    let run = harness.run(&harness.work(), Isolation::None);
    let ctx = context();
    let started = harness.start("t1", &ctx, &run, "fence-1 [hold] a long one").await.expect("started");
    harness.turn_started("t1").await;
    harness
        .send("t2", &ctx, &started, SendKind::NewTurn, "m2", "fence-2 waits its turn")
        .await
        .expect("queued");
    assert!(
        harness.quiet("t2", Duration::from_millis(1500)).await.is_empty(),
        "behind a busy turn the task is still submitted"
    );

    let session = started.session_id.clone().expect("the session");
    let second = task("t2", &ctx, started.session_id.clone(), &started.cwd, None);
    harness.sessions.cancel(&second).await.expect("withdrawn");
    let queued = harness
        .state
        .drive
        .providers
        .resident_info(claude(), &session)
        .expect("the resident")
        .queued;
    assert_eq!(queued, 0, "the message left the queue");

    let first = task("t1", &ctx, started.session_id.clone(), &started.cwd, None);
    harness.sessions.cancel(&first).await.expect("interrupted");
    harness.settled(&session).await;
    let injection = harness.state.drive.providers.injection_for(claude(), &session).expect("the run");
    assert!(injection.interrupted, "the turn was stopped");
    assert!(
        harness.quiet("t1", Duration::from_millis(1500)).await.is_empty(),
        "a cancelled task is not reported on"
    );
    assert!(
        !harness.read().iter().any(|line| line.contains("fence-2")),
        "the withdrawn message never ran"
    );

    harness.sessions.cancel(&first).await.expect("twice");
    harness
        .sessions
        .cancel(&task("never", &ctx, None, &started.cwd, None))
        .await
        .expect("one it never had");
}

/// A cancel that arrives while the task's start is still on its way in is
/// not lost: whichever comes first, the task is never reported as ended, and
/// its session is not left running its turn. A start of a task cancelled
/// before it starts nothing.
#[tokio::test]
async fn a_task_cancelled_while_it_starts_is_stopped() {
    let mut harness = Harness::new().await;
    let run = harness.run(&harness.work(), Isolation::None);
    let ctx = context();
    let cancel = task("t1", &ctx, None, &run.cwd, None);
    let (started, cancelled) = tokio::join!(
        harness.start("t1", &ctx, &run, "fence-1 [hold] cancelled at once"),
        harness.sessions.cancel(&cancel)
    );
    cancelled.expect("cancelled");
    if let Ok(started) = started {
        harness.settled(&started.session_id.expect("the session")).await;
    }
    let early = task("t2", &context(), None, &run.cwd, None);
    harness.sessions.cancel(&early).await.expect("cancelled before it started");
    assert!(
        harness.start("t2", &early.context_id, &run, "fence-2 never").await.is_err(),
        "a start of a cancelled task starts nothing"
    );
    let about = harness.quiet("t1", Duration::from_millis(1500)).await;
    assert!(!about.iter().any(|event| matches!(event, BackendEvent::TurnEnded { .. })), "{about:#?}");
    assert!(!harness.read().iter().any(|line| line.contains("fence-2")));
}

/// A turn the CLI fails fails the task, with what the CLI said; a CLI that
/// cannot start at all fails the start.
#[tokio::test]
async fn a_cli_failure_fails_the_task() {
    let mut harness = Harness::new().await;
    let run = harness.run(&harness.work(), Isolation::None);
    harness.start("t1", &context(), &run, "fence-1 [fail] break").await.expect("started");
    match harness.ended("t1").await {
        TurnOutcome::Failed(why) => assert!(why.contains("error_during_execution"), "{why}"),
        other => panic!("{other:?}"),
    }

    // A claude that exits before it says it is listening is found out after
    // the start answered (its id is this server's own), once the recovery
    // ladder has tried to bring it back: the start, or the task, fails.
    std::fs::write(harness.control.join("refuse-start"), "").expect("refuse");
    match harness.start("t2", &context(), &run, "fence-2 never").await {
        Err(BackendError::Failed(why)) => assert!(!why.is_empty()),
        Ok(_) => match harness.ended("t2").await {
            TurnOutcome::Failed(why) => assert!(!why.is_empty()),
            other => panic!("a CLI that cannot start: {other:?}"),
        },
        other => panic!("a CLI that cannot start: {other:?}"),
    }
    assert!(
        harness.sessions.inner.store.task("t2").is_none(),
        "nothing is followed of a task that failed"
    );
}

/// A context gets a worktree of its own, `<cwd>/.worktrees/a2a-<context>`
/// on `a2a/<context>`, and its session runs in it: what the task changes is
/// there and not in the entry's checkout, and the result says so. The same
/// context's worktree is never made twice, and a directory outside git
/// cannot be given one.
#[tokio::test]
async fn a_worktree_is_made_used_and_never_reused() {
    let mut harness = Harness::new().await;
    let run = harness.run(&harness.work(), Isolation::Worktree);
    let ctx = context();
    let started = harness
        .start("t1", &ctx, &run, "fence-1 [write notes.txt] take notes")
        .await
        .expect("started");
    let worktree = harness.work.join(".worktrees").join(format!("a2a-{ctx}"));
    assert_eq!(started.cwd, worktree.to_string_lossy());
    assert_eq!(started.branch.as_deref(), Some(format!("a2a/{ctx}").as_str()));
    let result = harness.completed("t1").await;
    assert_eq!(
        harness.spawns()[0]["cwd"].as_str(),
        Some(started.cwd.as_str()),
        "the session ran in the worktree"
    );
    assert!(worktree.join("notes.txt").is_file() && !harness.work.join("notes.txt").exists());
    assert_eq!(result.result.cwd, started.cwd);
    assert_eq!(result.result.branch, started.branch);
    assert_eq!(result.result.changed_files, ["notes.txt"]);

    match harness.start("t2", &ctx, &run, "fence-2 again").await {
        Err(BackendError::Rejected {
            code: Some(ErrorCode::IsolationFailed),
            message,
        }) => assert!(message.contains("never reused"), "{message}"),
        other => panic!("a second worktree for one context: {other:?}"),
    }
    let plain = harness.home.path().join("plain");
    std::fs::create_dir_all(&plain).expect("a directory");
    let outside = harness.run(&plain.to_string_lossy(), Isolation::Worktree);
    match harness.start("t3", &context(), &outside, "fence-3 nowhere").await {
        Err(BackendError::Rejected {
            code: Some(ErrorCode::IsolationFailed),
            ..
        }) => {}
        other => panic!("a worktree outside git: {other:?}"),
    }
    assert!(
        harness.sessions.check_entry(&outside).await.is_err(),
        "and the entry says so when it is saved"
    );
    assert!(harness
        .sessions
        .check_entry(&harness.run(&plain.to_string_lossy(), Isolation::None))
        .await
        .is_ok());
}

/// After a restart the tasks are picked up from what was written down: one
/// still queued is submitted, one whose turn runs is working and completes
/// when it ends, and one whose turn ended while the server was down ends
/// with its artifacts.
#[tokio::test]
async fn a_task_is_picked_up_again_after_a_restart() {
    let mut harness = Harness::new().await;
    let run = harness.run(&harness.work(), Isolation::None);
    let ctx = context();
    let started = harness.start("t1", &ctx, &run, "fence-1 [hold] long").await.expect("started");
    harness.turn_started("t1").await;
    harness
        .send("t2", &ctx, &started, SendKind::NewTurn, "m2", "fence-2 [hold] queued")
        .await
        .expect("queued");
    harness.restart();

    let session = started.session_id.clone();
    let first = task("t1", &ctx, session.clone(), &started.cwd, None);
    let second = task("t2", &ctx, session.clone(), &started.cwd, None);
    assert_eq!(harness.sessions.resume(&second).await, Ok(Resumed::Submitted));
    assert_eq!(harness.sessions.resume(&first).await, Ok(Resumed::Working));
    harness.release();
    // The reply is the turn's last text, "Working on it" and all: nothing
    // came between them.
    assert_eq!(harness.completed("t1").await.summary, "Working on it\n\nDone: fence-1 long.");
    harness.turn_started("t2").await;

    // The second turn ends while nobody is following it.
    harness.restart();
    harness.release();
    harness.settled(session.as_deref().expect("the session")).await;
    match harness.sessions.resume(&second).await {
        Ok(Resumed::Ended(TurnOutcome::Completed(result))) => assert_eq!(result.summary, "Working on it\n\nDone: fence-2 queued."),
        other => panic!("{other:?}"),
    }
    assert!(harness.sessions.inner.store.task("t2").is_none());
    assert!(matches!(
        harness.sessions.resume(&task("gone", &ctx, session, &started.cwd, None)).await,
        Ok(Resumed::Gone(_))
    ));
}

/// An entry is checked for what would not run: a CLI that is not here, flags
/// its CLI would not take, a directory that is not one, an account that is
/// not stored, an ACP agent for a CLI that runs none.
#[tokio::test]
async fn an_entry_that_would_not_run_is_refused() {
    let harness = Harness::new().await;
    let ok = harness.run(&harness.work(), Isolation::Worktree);
    assert_eq!(harness.sessions.check_entry(&ok).await, Ok(()));
    let refused = [
        RunSpec {
            source: EntrySource::Tck,
            ..ok.clone()
        },
        RunSpec {
            model: Some("--dangerously".into()),
            ..ok.clone()
        },
        RunSpec {
            cwd: "relative".into(),
            ..ok.clone()
        },
        RunSpec {
            cwd: harness.work.join("missing").to_string_lossy().into_owned(),
            ..ok.clone()
        },
        RunSpec {
            account: RunAccount::Id("11111111-1111-4111-8111-111111111111".into()),
            ..ok.clone()
        },
        RunSpec {
            acp_agent: Some("fake".into()),
            ..ok.clone()
        },
    ];
    for run in refused {
        assert!(harness.sessions.check_entry(&run).await.is_err(), "{run:?}");
    }
}

/// An image a task sends is stored as an upload, under a name of the
/// backend's own whatever the part called it, and too many, too large or
/// other kinds of images are refused.
#[tokio::test]
async fn images_are_stored_under_names_of_their_own_and_bounded() {
    let harness = Harness::new().await;
    let image = |bytes: usize| a2a::Image {
        media_type: "image/png".into(),
        filename: Some("../../etc/passwd".into()),
        bytes: vec![0u8; bytes],
    };
    let stored = harness.sessions.inner.store_images(&[image(8)]).await.expect("stored");
    let path = Path::new(&stored[0]);
    assert!(path.starts_with(harness.state.paths.uploads_dir()), "{path:?}");
    assert_eq!(path.file_name().and_then(|name| name.to_str()), Some("a2a-image-1.png"));
    assert!(check_images(&[image(MAX_IMAGE_BYTES + 1)]).is_err());
    assert!(check_images(&vec![image(1); MAX_IMAGES + 1]).is_err());
    assert!(
        check_images(&[image(MAX_IMAGE_BYTES), image(MAX_IMAGE_BYTES), image(MAX_IMAGE_BYTES)]).is_err(),
        "together"
    );
    assert!(check_images(&[a2a::Image {
        media_type: "image/svg+xml".into(),
        ..image(1)
    }])
    .is_err());
}
