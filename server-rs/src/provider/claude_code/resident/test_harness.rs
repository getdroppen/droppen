//! The resident tests' harness: a driver run over a fake `claude` the test
//! controls, on a registry whose timings the test sets. A module of its own,
//! outside the tests, because the server's feed tests run a resident too.

use super::*;
use std::collections::HashMap;
use std::path::Path;
use droppen_instance::instance::InstanceDirs;
use droppen_instance::paths::Paths;

/// The id every fake claude in this module answers under.
pub(crate) const SESSION: &str = "11111111-1111-4111-8111-111111111111";

/// The id a `--fork-session` answers under instead.
pub(super) const FORKED: &str = "22222222-2222-4222-8222-222222222222";

/// A driver, the fake claude it is driving, and the files both write.
pub(crate) struct Harness {
    pub(crate) _home: tempfile::TempDir,
    /// Claude Code's runs, which the driver under test is one of.
    pub(super) registry: Arc<ClaudeCodeRuns>,
    /// The injection registry over them, as a server's `Drive` holds it.
    pub(crate) injections: Arc<InjectionRegistry>,
    /// Every provider over `registry`, as the routes and the feeds ask
    /// them, and the spawn gate they lend.
    pub(crate) providers: Arc<droppen_provider::ProviderRegistry>,
    pub(super) tx: mpsc::UnboundedSender<Cmd>,
    pub(super) shared: Arc<Mutex<ResidentState>>,
    /// Every line the fake claude read off its stdin, in order.
    pub(super) log: PathBuf,
    /// Touched to let the fake claude finish the turn it is holding.
    pub(super) go: PathBuf,
    /// Where the `claude` a respawn will find lives.
    pub(super) bin: PathBuf,
    /// One line of argv per process the driver has started for itself.
    pub(super) spawns: PathBuf,
    /// Every line a *replacement* process read off its stdin.
    pub(super) after: PathBuf,
    pub(super) shim: tokio::task::JoinHandle<Result<(), String>>,
    pub(super) driver: tokio::task::JoinHandle<()>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.driver.abort();
        self.shim.abort();
        // A respawn starts a real shim and a real process, and neither is a
        // child of anything this test owns — that is the whole point of the
        // shim. Aborting the task that was talking to one leaves it running
        // against a `tempdir` that is about to go, so the journal is read
        // for what to end. claude leads a process group of its own, and the
        // shim follows its child out.
        for record in self.registry.host.journal.read() {
            // SAFETY: `kill` delivers a signal to a process group and cannot
            // corrupt this process' own state.
            unsafe { libc::kill(-(record.cli_pid as libc::pid_t), libc::SIGKILL) };
        }
    }
}

/// The logins a resident test's accounts launch under: Claude Code's own,
/// whose launch and check these tests are about, and a stand-in for
/// Codex's, which no Claude Code run asks.
pub(super) fn logins() -> droppen_accounts::login::Logins {
    droppen_accounts::login::Logins {
        claude: Arc::new(crate::provider::claude_code::login::ClaudeCodeLogin),
        codex: droppen_accounts::login::Logins::stand_ins().codex,
    }
}

/// A registry with timings a test can wait out.
///
/// Built by hand rather than through [`InjectionRegistry::new`] on
/// purpose: `new` reads the timings out of the environment, which is
/// process-global and shared with every other test in this binary.
pub(super) fn registry(home: &Path, stall: Duration, tick: Duration, step: Duration, settle: Duration, absorb_poll: Duration) -> (Arc<InjectionRegistry>, Arc<ClaudeCodeRuns>) {
    let paths = Arc::new(Paths::new(home.to_path_buf()));
    let dirs = InstanceDirs::new(&paths.data_dir(), droppen_instance::instance::DEFAULT_INSTANCE).expect("valid name");
    dirs.create().expect("instance directories");
    let host = droppen_provider::testing::host(
        paths.clone(),
        &dirs,
        Arc::new(droppen_accounts::store::AccountsStore::new(paths.clone(), logins())),
        Arc::new(droppen_accounts::quota::QuotaCache::new(paths.clone(), logins())),
        Duration::from_secs(600),
        step,
        tick,
    );
    let claude = Arc::new(ClaudeCodeRuns {
        residents: Mutex::new(HashMap::new()),
        stall_after: stall,
        resident_idle: None,
        // Short enough that the ceiling on a parked settings change can be
        // watched inside a test, and far past anything else here waits for.
        await_background: Some(Duration::from_millis(400)),
        // The production default, set here rather than read for the reason
        // the timings are: no test's answer may depend on the shell.
        rotate_ahead: Some(droppen_accounts::rotation::AHEAD_DEFAULT_PERCENT),
        // This test's own, so the corroboration probe reads a directory the
        // test controls rather than whatever `/tmp` happens to hold.
        task_root: home.join("task-root"),
        recovery_backoff: [Duration::from_millis(30); 3],
        steer_settle: settle,
        absorb_poll,
        // Recording on, into this test's own instance directory, for the
        // same reason the timings are set by hand: `Recorders::new` reads
        // the environment, and `a_session_records_its_whole_life` needs the
        // answer to be yes whatever the developer's shell says.
        recorders: crate::provider::claude_code::record::Recorders::new_at(dirs.session_logs(), None, paths.clone()),
        decoration: droppen_provider::testing::no_decoration(),
        reading_recheck: Mutex::new(None),
        host: host.clone(),
    });
    let injections = droppen_provider::testing::injection_registry_on(host, dirs, droppen_provider::testing::no_decoration());
    (injections, claude)
}

/// The bounded wait every fake CLI in here uses to sit on `$GO`, prepended
/// to every script [`Harness::build`] writes.
///
/// A stub that outlives its test is not a slow test, it is a fork bomb. The
/// harness' `tempdir` goes with the panic, `$GO` can then never appear, and
/// a reparented `sh` polls a path that no longer exists at fifty spawns a
/// second for as long as the machine stays up. Enough of them and the box
/// stops resolving user names. So the wait is bounded twice over:
///
/// * it gives up the instant the directory holding `$GO` is gone, which is
///   the leak exactly — the release file cannot arrive from a `tempdir`
///   that has been removed, so there is nothing left to wait for; and
/// * it gives up after thirty seconds regardless, which covers the case
///   [`Drop`] cannot — a test binary killed outright never unwinds, so the
///   directory survives and the anchor above never fires.
///
/// Thirty seconds rather than the eight the stepped `await` helpers use
/// because `$GO` gates a whole turn while those only wait on a nudge a test
/// is about to make: [`until`] gives up at ten seconds, and a cap near it
/// would race the assertion for which failure gets reported. Giving up
/// *exits* rather than falling through, and with a code no test expects, so
/// a genuine hang still fails loudly instead of passing quietly.
pub(super) const WAIT_GUARD: &str = r#"
GO_DIR=$(dirname "$GO")
await_go() {
  n=0
  while [ ! -f "$GO" ]; do
[ -d "$GO_DIR" ] || exit 97
[ "$n" -lt 1500 ] || exit 97
sleep 0.02
n=$((n + 1))
  done
}
"#;

/// The lines a real claude writes before it will take anything.
///
/// Two of the three are the point. `hook_started` comes *before* `init`, so
/// a driver that took the first line as the handshake would already be
/// wrong; and the junk line proves the tolerant parse, since a reader that
/// refused it would never reach the init behind it.
pub(super) fn preamble() -> String {
    [
        r#"printf '%s\n' '{"type":"system","subtype":"hook_started"}'"#.to_string(),
        r#"printf '%s\n' 'this line is not json'"#.to_string(),
        format!(r#"printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}"}}'"#),
    ]
    .join("\n")
}

/// The ordinary body: log every message, hold the turn until released.
pub(super) fn hold_until_released() -> &'static str {
    r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in
*control_request*) : ;;
*) await_go
   rm -f "$GO"
   printf '%s\n' '{"type":"assistant","message":{}}'
   printf '%s\n' '{"type":"result","subtype":"success","is_error":false}' ;;
  esac
done
"#
}

/// The CLI that runs a turn of its own when a background task finishes.
///
/// The production sequence, one step per file the test touches: a subagent's
/// stream is forwarded while the session sits between turns, then the
/// notification wakes the main agent and it runs a whole turn — `init`, an
/// assistant message of its own, `result` — with nothing on stdin to have
/// started it. See [`Driver::on_speech`].
///
/// Written as a subshell beside the ordinary read loop because that is
/// exactly what the real thing is: output arriving when nothing has been
/// asked for. The two never print at the same time — the loop only answers a
/// prompt, and no prompt can be delivered until the woken turn's `result`
/// has gone out — so the interleaving one shared pipe would allow cannot
/// happen here.
///
/// Each step ends with a `background_tasks_changed` the driver visibly acts
/// on, so a test can wait for "everything up to here has been read" instead
/// of sleeping and hoping. Every wait is bounded: a test that fails halfway
/// leaves no subshell polling a `tempdir` that has gone.
pub(super) fn wakes_itself_for_a_finished_task() -> String {
    format!(
        r#"
dir=$(dirname "$GO")
await() {{ n=0; while [ ! -f "$dir/$1" ] && [ "$n" -lt 400 ]; do [ -d "$dir" ] || exit 97; sleep 0.02; n=$((n + 1)); done; }}
(
  await subagent
  printf '%s\n' '{{"type":"system","subtype":"task_progress","task_id":"t1"}}'
  printf '%s\n' '{{"type":"assistant","parent_tool_use_id":"toolu_child","message":{{"model":"claude-opus-5"}}}}'
  printf '%s\n' '{{"type":"user","parent_tool_use_id":"toolu_child","message":{{}}}}'
  printf '%s\n' '{{"type":"system","subtype":"background_tasks_changed","tasks":[{{"task_id":"t1"}}]}}'
  await wake
  printf '%s\n' '{{"type":"system","subtype":"init","session_id":"{SESSION}"}}'
  printf '%s\n' '{{"type":"system","subtype":"background_tasks_changed","tasks":[]}}'
  await speak
  printf '%s\n' '{{"type":"assistant","message":{{"model":"claude-fable-5"}}}}'
  await finish
  printf '%s\n' '{{"type":"result","subtype":"success","is_error":false}}'
) &
{}
"#,
        hold_until_released()
    )
}

/// Put a `claude` in `bin` that a real spawn will find and run.
pub(super) fn write_claude(bin: &Path, body: &str) {
    let path = bin.join("claude");
    std::fs::write(&path, body).expect("fake claude");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
}

/// A recipe whose environment can actually start the `claude` in `bin`.
pub(super) fn recipe(bin: &Path, spawns: &Path, after: &Path, fingerprint: Fingerprint) -> Recipe {
    let mut env = Env::new();
    env.insert("PATH".into(), format!("{}:/bin:/usr/bin", bin.display()));
    env.insert("SPAWNS".into(), spawns.to_string_lossy().into_owned());
    env.insert("AFTER".into(), after.to_string_lossy().into_owned());
    env.insert("REFUSE".into(), bin.join("refuse").to_string_lossy().into_owned());
    Recipe {
        args: vec![],
        env,
        workdir: Some(bin.to_path_buf()),
        fingerprint,
    }
}

/// The `claude` a respawn finds on the recipe's `PATH`, by default.
///
/// The same handshake as [`preamble`] and the same one-result-per-line body,
/// plus the two things a recovery test has to be able to see: which argv the
/// driver chose — `--resume`, or `--resume --fork-session` — and which id the
/// replacement answered under. A `--fork-session` announces a *different* id,
/// because that is exactly what the real CLI does and what makes the second
/// rung worth having.
pub(super) fn replacement_claude(session: &str, forked: &str) -> String {
    format!(
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$SPAWNS"
[ -f "$REFUSE" ] && exit 91
answer='{session}'
case "$*" in *--fork-session*) answer='{forked}' ;; esac
printf '%s\n' '{{"type":"system","subtype":"hook_started"}}'
printf '{{"type":"system","subtype":"init","session_id":"%s"}}\n' "$answer"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$AFTER"
  case "$line" in
*control_request*) : ;;
*) printf '%s\n' '{{"type":"result","subtype":"success","is_error":false}}' ;;
  esac
done
"#
    )
}

/// What a harness' driver is told it inherited.
///
/// One argument rather than three because [`Harness::build`] already takes
/// as many as anything should, and because the three only ever vary
/// together: they are the whole of what [`ClaudeCodeRuns::attached`]
/// reconstructs from a journal record.
/// What a replayed background task has left on disk to be believed by.
///
/// The distinction the last three draw is the whole point of reading a
/// directory's *entries*: claude creates a transcript when a background
/// agent starts and then streams into it for as long as the agent runs, so
/// a directory whose mtime is minutes old routinely holds a file written a
/// second ago. A probe that reads only the directory calls that quiet.
#[derive(Clone, Copy)]
pub(super) enum Witness {
    /// Nothing was written anywhere: the task is a ghost.
    None,
    /// A transcript written now, in a directory created now — a task that
    /// started moments before the attach.
    Fresh,
    /// A transcript written now, in a directory last touched long ago — a
    /// task that started minutes ago and is still streaming. The live case
    /// this harness exists to pin down.
    StreamingIntoAnOldDirectory,
    /// A transcript and a directory both long untouched: the agent is gone.
    Stale,
    /// No transcript at all, and a task output file written now — a
    /// background *command*, which writes nothing under `subagents/`.
    TaskOutputOnly,
}

impl Witness {
    /// The shell that leaves this witness, run by the fake claude before it
    /// writes its history.
    ///
    /// `touch -t` backdates rather than sleeps: the quiet window is thirty
    /// seconds and no test may wait that long. Backdating the directory
    /// *after* the file is created is what makes the two differ at all —
    /// creating an entry is precisely what moves a directory's mtime, so the
    /// order is the test.
    pub(super) fn script(self, subagents: &Path, tasks: &Path) -> String {
        const LONG_AGO: &str = "202001010000";
        let (dir, file) = (subagents.display(), "agent-ghost.jsonl");
        match self {
            Witness::None => String::new(),
            Witness::Fresh => format!("mkdir -p {dir} && printf '{{}}\\n' > {dir}/{file}\n"),
            Witness::StreamingIntoAnOldDirectory => format!("mkdir -p {dir} && printf '{{}}\\n' > {dir}/{file} && touch -t {LONG_AGO} {dir}\n"),
            Witness::Stale => format!(
                "mkdir -p {dir} && printf '{{}}\\n' > {dir}/{file} \
                 && touch -t {LONG_AGO} {dir}/{file} && touch -t {LONG_AGO} {dir}\n"
            ),
            Witness::TaskOutputOnly => {
                format!("mkdir -p {0} && printf 'working\\n' > {0}/ghost.output\n", tasks.display())
            }
        }
    }
}

pub(super) struct Adoption {
    /// `Some(witness)` for a driver that finds history waiting for it,
    /// `None` for a fresh spawn; see [`Harness::start_attached`].
    pub(super) attach: Option<Witness>,
    /// What the record says the running process was started with. `None` is
    /// a record from before the journal held a fingerprint.
    pub(super) live: Option<Fingerprint>,
    /// The backlog the record was holding, as the journal kept it: no
    /// recipe, because a journal cannot carry an environment.
    pub(super) queued: Vec<Queued>,
    /// The fingerprint of the recipe the driver holds, when it is not the
    /// default: a process started under an account, for the rotation tests.
    pub(super) recipe: Option<Fingerprint>,
    /// A queue the record says was held after a limit, as a server that
    /// restarted mid-hold finds it.
    pub(super) hold: Option<Hold>,
    /// The shim's replay ring, when a test needs one small enough to lose
    /// the start of the history; `None` is the production size.
    pub(super) ring: Option<usize>,
    /// What the session's transcript says when the driver attaches, written
    /// and backdated past [`TRANSCRIPT_SETTLES_AFTER`] — so the transcript is
    /// a witness the replay's settle really asks. Attach only.
    pub(crate) transcript: Option<String>,
    /// How often the driver reads the transcript for folded steers. Short
    /// by default, so a test watching for one sees it inside its own wait;
    /// [`NEVER_SETTLES`] for a test that has to show the feed's nudge alone
    /// is enough.
    pub(super) absorb_poll: Duration,
}

impl Default for Adoption {
    /// A fresh spawn, whose settings are known because this driver chose
    /// them. Hand-written rather than derived precisely because the derived
    /// `None` would mean "unknown", which is a different test.
    fn default() -> Self {
        Adoption {
            attach: None,
            live: Some(Fingerprint::default()),
            queued: Vec::new(),
            recipe: None,
            hold: None,
            ring: None,
            transcript: None,
            absorb_poll: Duration::from_millis(50),
        }
    }
}

/// A settle window no test can wait out.
///
/// The default for every harness but the two steering scenarios, and the
/// right default: with it, a delivered turn is ended by claude's own
/// `result` and by nothing else, which is what every other test in here is
/// about. The two that *are* about the window ask for a millisecond one.
pub(super) const NEVER_SETTLES: Duration = Duration::from_secs(3600);

impl Harness {
    /// Run `body` as the fake claude, with a driver attached to it.
    pub(super) async fn start(body: &str, stall: Duration, tick: Duration, step: Duration) -> Harness {
        Harness::start_with_handshake(&preamble(), body, stall, tick, step).await
    }

    /// The ordinary timings, but with a settle window a test can watch.
    ///
    /// The steering scenarios and nothing else: what they are testing *is*
    /// the window, so it has to be short enough to sit through and long
    /// enough that a fake claude which answers immediately still wins the
    /// race it is supposed to win. `step` is the interrupt ladder's, taken
    /// here because the one thing the two timings must not do is arrive in
    /// the wrong order — production's are two seconds and five.
    pub(super) async fn settling(body: &str, settle: Duration, step: Duration) -> Harness {
        Harness::build(
            &preamble(),
            body,
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            step,
            settle,
            Adoption::default(),
        )
        .await
    }

    /// A driver that finds history waiting for it, as a re-attach does.
    ///
    /// The shim is started, the fake claude writes `history` and touches a
    /// marker, and only then does the driver connect — so `link.total()` is
    /// non-zero and the driver walks the history through
    /// [`Driver::replay`] exactly as [`ClaudeCodeRuns::attached`] makes
    /// it. Waiting on the marker rather than on a sleep is what keeps the
    /// arrangement from being a race.
    ///
    /// `corroborate` writes a subagent transcript from inside the fake
    /// claude, which is the only place it can be written *before* the
    /// driver settles its replay — and where a real one would come from.
    pub(super) async fn start_attached(history: &str, body: &str, stall: Duration, tick: Duration, step: Duration, witness: Witness) -> Harness {
        Harness::build(
            history,
            body,
            stall,
            tick,
            step,
            NEVER_SETTLES,
            Adoption {
                attach: Some(witness),
                ..Adoption::default()
            },
        )
        .await
    }

    /// A driver that took over a resident whose record says `live`, holding
    /// `queued`, exactly as [`ClaudeCodeRuns::attached`] builds one.
    ///
    /// The production scenario in one call: `None` is the record written
    /// before the journal carried a fingerprint at all.
    pub(super) async fn start_inherited(live: Option<Fingerprint>, queued: Vec<Queued>) -> Harness {
        Harness::build(
            &preamble(),
            hold_until_released(),
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            Duration::from_millis(120),
            NEVER_SETTLES,
            Adoption {
                attach: None,
                live,
                queued,
                recipe: None,
                hold: None,
                ..Adoption::default()
            },
        )
        .await
    }

    pub(super) async fn start_with_handshake(handshake: &str, body: &str, stall: Duration, tick: Duration, step: Duration) -> Harness {
        Harness::build(handshake, body, stall, tick, step, NEVER_SETTLES, Adoption::default()).await
    }

    pub(super) async fn build(
        handshake: &str,
        body: &str,
        stall: Duration,
        tick: Duration,
        step: Duration,
        settle: Duration,
        adoption: Adoption,
    ) -> Harness {
        let home = tempfile::tempdir().expect("tempdir");
        Harness::build_in(home, handshake, body, stall, tick, step, settle, adoption).await
    }

    /// [`Harness::build`] in a home the caller has already put things in —
    /// the accounts a rotation test needs before the driver is told which
    /// one it runs under.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn build_in(
        home: tempfile::TempDir,
        handshake: &str,
        body: &str,
        stall: Duration,
        tick: Duration,
        step: Duration,
        settle: Duration,
        adoption: Adoption,
    ) -> Harness {
        let Adoption {
            attach,
            live,
            queued,
            recipe: recipe_fingerprint,
            hold,
            ring,
            transcript: tail,
            absorb_poll,
        } = adoption;
        let (injections, registry) = registry(home.path(), stall, tick, step, settle, absorb_poll);
        // Claude Code's provider, over this harness's own runs.
        let providers = droppen_provider::ProviderRegistry::new(injections.clone(), |providers, injections| {
            providers.register(Arc::new(ClaudeCode::on(registry.clone(), injections)));
        });
        let log = home.path().join("stdin.log");
        let go = home.path().join("go");
        let bin = home.path().join("bin");
        let spawns = home.path().join("spawns.txt");
        let after = home.path().join("after.log");
        std::fs::create_dir_all(&bin).expect("bin");
        write_claude(&bin, &replacement_claude(SESSION, FORKED));
        let marked = home.path().join("history-written");
        // The subagent transcript a corroborated background task leaves
        // behind, written where a real one is: under the session's own
        // transcript, by the process itself.
        let transcript = transcript_path(home.path());
        let subagents = crate::provider::claude_code::sessions::transcript::subagent_dir(&transcript);
        // Where claude would put this session's task output: under the root
        // the test registry owns, behind a slug the probe never has to
        // guess — it finds the session id one level down instead.
        let tasks = registry.task_root.join("a-project").join(SESSION).join("tasks");
        let prologue = match attach {
            None => String::new(),
            Some(witness) => {
                // The session's own transcript exists either way, so the
                // corroboration probe really runs: with no transcript at all
                // it would answer "nothing running" without looking, and the
                // test would prove nothing about the look.
                let witness = witness.script(&subagents, &tasks);
                let tail = match &tail {
                    Some(tail) => format!("printf '%s\\n' '{tail}' > {0} && touch -t 202001010000 {0}\n", transcript.display()),
                    None => String::new(),
                };
                format!("touch {}\n{witness}{tail}touch {}\n", transcript.display(), marked.display())
            }
        };
        // `WAIT_GUARD` goes in ahead of everything a body might do, so
        // every fake CLI — including any written later — has a bounded,
        // self-destructing `await_go` rather than a loop that can outlive
        // the `tempdir` two lines above it.
        let script = format!(
            "LOG={}\nGO={}\nTRANSCRIPT={}\n{WAIT_GUARD}{}\n{}{}",
            log.display(),
            go.display(),
            transcript.display(),
            handshake,
            prologue,
            body
        );

        let sidecars = Sidecars::new(registry.host.shim_dir.clone(), "resident-under-test");
        sidecars.prepare().expect("sidecars");
        let shim = tokio::spawn(shim::Args {
            socket: sidecars.socket().to_path_buf(),
            stderr_file: sidecars.stderr(),
            exit_file: sidecars.exit(),
            buffer_bytes: ring.unwrap_or(shim::DEFAULT_BUFFER_BYTES),
            command: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), script],
        }
        .run());
        if attach.is_some() {
            until("the history to be written", || marked.exists()).await;
        }
        let link = shim::connect_in_process(sidecars.socket()).await.expect("the shim answers");
        // Exactly what `ClaudeCodeRuns::attached` computes: a shim that
        // has buffered bytes has history, and history has to be replayed
        // before anything in it may be acted on. A fresh spawn never
        // replays, however fast its CLI got going.
        let replaying = attach.is_some() && link.total() > 0;
        assert_eq!(replaying, attach.is_some(), "an attach has history to replay and a spawn has none");

        // The journal record this driver will keep up to date. Every harness
        // has one — a driver whose record names nothing writes its progress
        // into the void, and two of the things worth asserting about a
        // restart are what it wrote down.
        registry.host.journal.add(runs::RunRecord {
            run_id: "record-under-test".to_string(),
            source: Source::Claude,
            session_id: SESSION.to_string(),
            cwd: None,
            cli_pid: link.claude_pid(),
            started_at: iso8601(std::time::SystemTime::now()),
            pid_token: procs::start_token(link.claude_pid()),
            resident: true,
            shim: Some(sidecars.id().to_string()),
            payload: ClaudeRun {
                fingerprint: live.clone(),
                turn_at: None,
                queued: queued.clone(),
                hold: hold.clone(),
            },
        });
        // Split exactly as an attach splits it, so a harness given a backlog
        // inherits it under the same rules the real thing applies.
        let Inherited { delivered_ahead, queue } = inherited_queue(SESSION, &queued, live.as_ref());

        let shared = Arc::new(Mutex::new(ResidentState {
            phase: Phase::Starting,
            stalled: false,
            queued: delivered_ahead.len() + queue.len(),
            queued_messages: delivered_ahead
                .iter()
                .map(|pending| pending.preview(true))
                .chain(queue.iter().map(|pending| pending.preview(false)))
                .collect(),
            background: 0,
            tasks: Vec::new(),
            awaiting_background: false,
            turn_started_at: Some(iso8601(SystemTime::now())),
            question: None,
        }));
        let (tx, rx) = mpsc::unbounded_channel();
        registry.residents.lock().expect("residents").insert(
            SESSION.to_string(),
            Handle {
                tx: tx.clone(),
                shared: shared.clone(),
            },
        );
        let recorder = registry.recorders.open(SESSION);
        let driver = tokio::spawn(
            Driver {
                runs: registry.clone(),
                id: SESSION.to_string(),
                sidecars: sidecars.clone(),
                link,
                record: "record-under-test".to_string(),
                recorder,
                live,
                recipe: Some(recipe(&bin, &spawns, &after, recipe_fingerprint.unwrap_or_default())),
                tx: tx.clone(),
                rx,
                shared: shared.clone(),
                lease: providers.gate().empty_lease(),
                queue,
                delivered_ahead,
                absorb: AbsorbWatch::default(),
                turn: None,
                ready: false,
                heard: Instant::now(),
                replaying,
                replay_open: false,
                replay_marked: false,
                pending_turn: None,
                idle_since: None,
                seq: 0,
                fatal: None,
                background: BackgroundTasks::default(),
                awaiting_background: false,
                awaiting_since: None,
                deaths: VecDeque::new(),
                rung: 0,
                recovering: false,
                retiring: false,
                limit_hint: limits::HintSlot::default(),
                ahead_noted: None,
                // Read back out of the journal, the way an attach reads it,
                // so a held queue is restored by the same round trip.
                hold: registry
                    .host.journal
                    .read()
                    .into_iter()
                    .find(|record| record.run_id == "record-under-test")
                    .and_then(|record| record.decode::<ClaudeRun>().ok())
                    .and_then(|record| record.payload.hold),
                asked: Vec::new(),
                replay_asked: Vec::new(),
                replay_refusals: Vec::new(),
                replay_declines: Vec::new(),
                replay_gapped: false,
                draft: registry.draft_writer(SESSION),
                phase_moved: std::sync::atomic::AtomicBool::new(false),
            }
            .run(),
        );
        Harness {
            _home: home,
            registry,
            injections,
            providers,
            tx,
            shared,
            log,
            go,
            bin,
            spawns,
            after,
            shim,
            driver,
        }
    }

    /// The ordinary harness: nothing stalls, nothing ticks in the test's
    /// lifetime, and the ladder is fast enough to watch.
    pub(super) async fn ordinary() -> Harness {
        Harness::ordinary_with(hold_until_released()).await
    }

    /// The ordinary timings around a different `claude`.
    pub(crate) async fn ordinary_with(body: &str) -> Harness {
        Harness::start(body, Duration::from_secs(3600), Duration::from_secs(3600), Duration::from_millis(120)).await
    }

    /// A message that fits the running process, and could start another.
    pub(crate) fn send(&self, text: &str) {
        self.send_with(text, self.recipe(Fingerprint::default()));
    }

    /// The recipe a message carries, pointing at this harness' own `claude`.
    pub(super) fn recipe(&self, fingerprint: Fingerprint) -> Recipe {
        recipe(&self.bin, &self.spawns, &self.after, fingerprint)
    }

    /// Overwrite the `claude` the next respawn will find.
    pub(super) fn plan_claude(&self, body: &str) {
        write_claude(&self.bin, body);
    }

    /// Make every spawn from here on fail after it has started.
    pub(super) fn refuse_to_start(&self) {
        std::fs::write(self.bin.join("refuse"), b"").expect("refuse");
    }

    /// One entry per process the driver started for itself, argv and all.
    pub(super) fn spawns(&self) -> Vec<String> {
        std::fs::read_to_string(&self.spawns)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// What the journal says the process behind this session was started
    /// with — the thing the *next* server would read.
    ///
    /// Found by session rather than by run id, so it follows a respawn to
    /// the record that replaced the one the harness started with.
    pub(super) fn recorded_fingerprint(&self) -> Option<Fingerprint> {
        self.registry
            .host.journal
            .read()
            .into_iter()
            .find(|record| record.session_id == SESSION)
            .and_then(|record| record.decode::<ClaudeRun>().ok())
            .and_then(|record| record.payload.fingerprint)
    }

    /// Every line a replacement process read off its stdin.
    pub(super) fn after(&self) -> Vec<String> {
        std::fs::read_to_string(&self.after)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// A message that wants the process `recipe` describes.
    pub(super) fn send_with(&self, text: &str, recipe: Recipe) {
        self.post(Pending::new(text, recipe));
    }

    /// Put a message in the driver's inbox and walk away, without waiting
    /// for it to be written down: these tests poll for what they expect.
    /// [`a_message_sent_behind_a_turn_is_on_disk_before_its_send_is_answered`]
    /// is the one that goes through [`ClaudeCodeRuns::deliver`] instead.
    pub(super) fn post(&self, pending: Pending) {
        let (recorded, _) = tokio::sync::oneshot::channel();
        self.tx
            .send(Cmd::Deliver {
                pending: Box::new(pending),
                recorded,
            })
            .expect("the driver is listening");
    }

    /// A message a client named, sent the way the send route sends one:
    /// the id claimed and answered first, the message delivered under the
    /// queue id the claim chose. Returns that queue id.
    pub(super) fn named(&self, text: &str) -> String {
        let receipts = self.registry.host.receipts();
        let client = uuid::Uuid::new_v4().to_string();
        let now = droppen_provider::receipts::now_ms();
        let qid = match receipts
            .claim(&client, droppen_provider::receipts::Kind::Message, Source::Claude, Some(SESSION), now)
            .expect("claim")
        {
            droppen_provider::receipts::Claim::Owned { qid } => qid,
            other => panic!("a fresh id is owned, got {other:?}"),
        };
        receipts.complete(&client, "{}", None, now);
        self.post(Pending::with_id(qid.clone(), text, self.recipe(Fingerprint::default())));
        qid
    }

    /// The receipt of the named message travelling as `qid`.
    pub(super) fn receipt(&self, qid: &str) -> Option<ReceiptStatus> {
        self.registry
            .host.receipts()
            .for_session(Source::Claude, SESSION, droppen_provider::receipts::now_ms())
            .into_iter()
            .find(|receipt| receipt.qid.0.as_deref() == Some(qid))
            .map(|receipt| receipt.status)
    }

    /// Let the fake claude finish the turn it is holding.
    pub(crate) fn release(&self) {
        std::fs::write(&self.go, b"").expect("go");
    }

    /// Let [`wakes_itself_for_a_finished_task`] take its next step.
    pub(super) fn nudge(&self, step: &str) {
        let home = self.go.parent().expect("the go file has a directory");
        std::fs::write(home.join(step), b"").expect("nudge");
    }

    /// This driver's journal record, as the next server would read it.
    pub(super) fn record(&self) -> runs::RunRecord<ClaudeRun> {
        self.registry
            .host.journal
            .read()
            .into_iter()
            .find(|record| record.run_id == "record-under-test")
            .expect("the journal still holds this driver's record")
            .decode()
            .expect("a Claude Code record")
    }

    pub(crate) fn state(&self) -> ResidentState {
        self.shared.lock().expect("mirror").clone()
    }

    /// The queue as a row would show it: steered first, then waiting.
    pub(super) fn queued(&self) -> Vec<droppen_sessions::QueuedMessage> {
        self.state().queued_messages
    }

    /// The id of the queued message whose text contains `needle`.
    pub(super) fn qid(&self, needle: &str) -> String {
        self.queued()
            .into_iter()
            .find(|message| message.text.contains(needle))
            .unwrap_or_else(|| panic!("no queued message matching {needle}: {:?}", self.queued()))
            .id
    }

    pub(super) async fn steer(&self, qid: &str) -> QueueOutcome {
        self.registry.steer_queued(SESSION, qid).await
    }

    pub(super) async fn withdraw(&self, qid: &str) -> QueueOutcome {
        self.registry.drop_queued(SESSION, qid).await
    }

    pub(super) fn injection(&self) -> Option<droppen_sessions::Injection> {
        self.registry.host.run_injection(Source::Claude, SESSION)
    }

    pub(super) fn injection_for(&self, id: &str) -> Option<droppen_sessions::Injection> {
        self.registry.host.run_injection(Source::Claude, id)
    }

    /// Every provider over this harness's registry, as the routes and the
    /// feeds ask them.
    pub(crate) fn providers(&self) -> Arc<droppen_provider::ProviderRegistry> {
        self.providers.clone()
    }

    /// Every line the fake claude has read, in the order it read them.
    pub(super) fn seen(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// This session's flight recording, in the order it was written.
    pub(super) fn recording(&self) -> Vec<crate::provider::claude_code::record::Event> {
        let paths = Paths::new(self._home.path().to_path_buf());
        let dirs = InstanceDirs::new(&paths.data_dir(), droppen_instance::instance::DEFAULT_INSTANCE).expect("valid name");
        crate::provider::claude_code::record::read_log(&dirs.session_logs().join(format!("{SESSION}.ndjson")))
            .unwrap_or_default()
            .into_iter()
            .map(|line| line.event)
            .collect()
    }

    /// Where the transcript of this session would be if a real claude were
    /// writing one, and the directory its subagents write into.
    pub(crate) fn transcript(&self) -> PathBuf {
        transcript_path(self._home.path())
    }
}

/// The transcript path for [`SESSION`] under a harness home.
///
/// A free function because the harness needs it before there is a harness:
/// the fake claude's script has to name it.
pub(super) fn transcript_path(home: &Path) -> PathBuf {
    let paths = Paths::new(home.to_path_buf());
    let project = crate::provider::claude_code::claude_roots::ClaudeRoots::of(&paths).managed_projects().join("-w-repo");
    std::fs::create_dir_all(&project).expect("project dir");
    project.join(format!("{SESSION}.jsonl"))
}

/// Poll until `check` passes, or fail rather than hang forever.
pub(crate) async fn until(what: &str, mut check: impl FnMut() -> bool) {
    for _ in 0..400 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}");
}

impl Harness {
    /// A driver whose process runs under an account, in a home that
    /// already holds the accounts.
    pub(super) async fn rotating(home: tempfile::TempDir, body: &str, live: Fingerprint) -> Harness {
        Harness::build_in(
            home,
            &preamble(),
            body,
            Duration::from_secs(3600),
            Duration::from_secs(3600),
            Duration::from_millis(120),
            NEVER_SETTLES,
            Adoption {
                attach: None,
                live: Some(live.clone()),
                queued: Vec::new(),
                recipe: Some(live),
                hold: None,
                ring: None,
                transcript: None,
                ..Adoption::default()
            },
        )
        .await
    }

    /// The argv lines of the processes a rotation started.
    pub(super) fn respawns(&self) -> Vec<String> {
        self.spawns().into_iter().filter(|line| !line.starts_with("token=")).collect()
    }

    /// The token each of those processes was handed.
    pub(super) fn tokens(&self) -> Vec<String> {
        self.spawns()
            .into_iter()
            .filter_map(|line| line.strip_prefix("token=").map(str::to_owned))
            .collect()
    }

    pub(super) fn latest_rotation(&self) -> Option<droppen_sessions::account::SessionRotation> {
        self.registry.host.accounts.rotation_log().latest(SESSION)
    }
}

/// Backdate a directory or a file the fake claude wrote, so nothing about it
/// reads as background work still streaming.
pub(super) fn quiet(path: &Path) {
    let when = SystemTime::now() - Duration::from_secs(3600);
    let file = std::fs::File::open(path).expect("open to backdate");
    file.set_times(std::fs::FileTimes::new().set_accessed(when).set_modified(when))
        .expect("backdate");
}

impl Harness {
    /// [`Harness::rotating`] with a watchdog that ticks inside the test, a
    /// backlog and a hold the journal record already holds.
    pub(super) async fn rotating_with(
        home: tempfile::TempDir,
        body: &str,
        live: Fingerprint,
        tick: Duration,
        queued: Vec<Queued>,
        hold: Option<Hold>,
    ) -> Harness {
        Harness::build_in(
            home,
            &preamble(),
            body,
            Duration::from_secs(3600),
            tick,
            Duration::from_millis(120),
            NEVER_SETTLES,
            Adoption {
                attach: None,
                live: Some(live.clone()),
                queued,
                recipe: Some(live),
                hold,
                ring: None,
                transcript: None,
                ..Adoption::default()
            },
        )
        .await
    }

    pub(super) fn rotations(&self) -> Vec<droppen_sessions::account::SessionRotation> {
        self.registry.host.accounts.rotation_log().history(SESSION, 20)
    }
}

/// A CLI that thinks for as long as the test holds it: a count on stdout
/// and a subagent's line beside it, then — on `$GO` — the finished text,
/// its entry in the transcript and the `result`.
pub(crate) fn thinks_until_released() -> String {
    let said = r#"{"type":"assistant","uuid":"u-early","message":{"id":"msg_early","role":"assistant","content":[{"type":"text","text":"said once the think was done"}]}}"#;
    let entry = format!(
        r#"{{"type":"assistant","uuid":"u-early","message":{{"id":"msg_early","role":"assistant","content":[{{"type":"text","text":"said once the think was done"}}]}},"timestamp":"2026-10-10T00:00:02.000Z","cwd":"/tmp","sessionId":"{SESSION}"}}"#
    );
    format!(
        r#"
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG"
  case "$line" in
*control_request*) : ;;
*) printf '%s\n' '{{"type":"system","subtype":"thinking_tokens","estimated_tokens":40,"estimated_tokens_delta":40}}'
   printf '%s\n' '{{"type":"assistant","parent_tool_use_id":"toolu_child","uuid":"u-child","message":{{"content":[{{"type":"text","text":"a subagent"}}]}}}}'
   printf '%s\n' '{{"type":"system","subtype":"thinking_tokens","estimated_tokens":42,"estimated_tokens_delta":2}}'
   await_go
   rm -f "$GO"
   printf '%s\n' '{said}'
   printf '%s\n' '{entry}' >> "$TRANSCRIPT"
   printf '%s\n' '{{"type":"result","subtype":"success","is_error":false}}' ;;
  esac
done
"#
    )
}

/// A step of a spawn, as [`droppen_provider::testing::Step`] records it.
pub(super) fn step(what: &'static str, gate: bool, claim: Option<bool>) -> droppen_provider::testing::Step {
    droppen_provider::testing::Step { what, gate, claim }
}
