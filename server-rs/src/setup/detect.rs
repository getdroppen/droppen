//! Asking each CLI about itself, and reading what it says.
//!
//! Every question here is one the CLI answers about itself — its version, its
//! own login status — and every answer is read by a parser that is a pure
//! function of the [`RunOutput`], so each format the CLIs print is pinned by a
//! test with the text in it. The orchestration ([`detect`]) runs the questions
//! through the [`Runner`] concurrently and puts the answers together.
//!
//! # What is never asked
//!
//! Detection reads no credential. It does not open `~/.claude/.credentials.json`,
//! `~/.claude.json` or `~/.codex/auth.json`, does not run `security` against
//! the keychain, and runs no `/usage` probe (that takes a machine-wide lock
//! and spends rate-limit budget). `codex login status` prints a masked API key;
//! [`codex_login`] reads only the words before it, and the output is dropped
//! the moment it has been parsed — it is never stored, logged or reported.
//! `git --version` is never run on macOS: without the command line tools it is
//! the thing that pops Apple's install dialog, so `xcode-select -p` is asked
//! instead.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::locate::{self, Search};
use super::runner::{Invocation, RpcCall, RunOutput, Runner};
use super::{GitState, LoginState, SetupHelpers, SetupOs, ToolFound, ToolId, ToolLogin};
use crate::openapi_ext::Nullable;
use crate::process_env::{resolve_on_path, Env};

/// The oldest `claude` this server's session driver is known to work with.
///
/// Every resident session is started with `--input-format stream-json
/// --output-format stream-json --verbose` (read against claude 2.1.231,
/// `inject/resident.rs`) and, since `AskUserQuestion` landed, with
/// `--permission-prompt-tool stdio`, whose `control_request` protocol the driver
/// answers was read off claude 2.1.295 (`inject/questions.rs`). Every spawn
/// depends on both, so the floor is the later of the two: an older CLI may
/// accept the flag and answer the protocol differently, which would reach the
/// user as sessions that fail for no visible reason. Claude Code updates itself
/// in the background, so in practice this only ever catches an install that
/// stopped updating — exactly the one worth a "needs update".
pub const MIN_CLAUDE: &str = "2.1.295";

/// The oldest `codex` this server's session driver is known to work with.
///
/// Codex sessions are driven through a persistent `codex app-server`
/// (`turn/start`, `turn/steer`, `turn/interrupt`), verified against codex-cli
/// 0.153.4 — the version the README names — and the quota panel and this
/// guide ask the same app server `account/rateLimits/read` / `account/read`.
/// No older version was ever run against the driver, so none is claimed.
pub const MIN_CODEX: &str = "0.153.4";

/// One question's budget: `--version`, `login status`, `xcode-select -p`.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(10);
/// `claude auth status` gets as long as the account verification gives it.
pub const CLAUDE_STATUS_TIMEOUT: Duration = Duration::from_secs(20);

impl ToolId {
    /// The minimum version this server supports.
    pub fn min_version(self) -> &'static str {
        match self {
            ToolId::Claude => MIN_CLAUDE,
            ToolId::Codex => MIN_CODEX,
        }
    }
}

// ------------------------------------------------------------------- parsing

/// The first `x.y.z` in a `--version` answer: `2.1.211 (Claude Code)` and
/// `codex-cli 0.163.0` alike.
pub fn parse_version(text: &str) -> Option<String> {
    static VERSION: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = VERSION.get_or_init(|| regex::Regex::new(r"(\d+)\.(\d+)\.(\d+)").expect("valid version pattern"));
    re.captures(text).map(|caps| format!("{}.{}.{}", &caps[1], &caps[2], &caps[3]))
}

/// `version >= min`, numerically, part by part.
pub fn at_least(version: &str, min: &str) -> bool {
    let parts = |text: &str| -> Option<Vec<u64>> { text.split('.').map(|part| part.parse().ok()).collect() };
    match (parts(version), parts(min)) {
        (Some(version), Some(min)) => version >= min,
        _ => false,
    }
}

/// A `--version` answer, or `None` for one that did not run or said nothing
/// that looks like a version.
pub fn version_of(output: &RunOutput) -> Option<String> {
    if !output.succeeded() {
        return None;
    }
    parse_version(&output.stdout).or_else(|| parse_version(&output.stderr))
}

/// No login known: the CLI could not be asked, or did not answer.
pub fn unknown_login() -> ToolLogin {
    ToolLogin {
        state: LoginState::Unknown,
        method: Nullable(None),
        plan: Nullable(None),
        email: Nullable(None),
        usable: Nullable(None),
    }
}

fn signed_out() -> ToolLogin {
    ToolLogin {
        state: LoginState::SignedOut,
        ..unknown_login()
    }
}

/// `claude auth status --json`.
///
/// The exit code is the documented answer — 0 signed in, 1 signed out — and
/// anything else, a timeout included, is `unknown` rather than a guess. The
/// JSON is read for what it adds: `authMethod` (documented), and `email`,
/// `subscriptionType`, `rateLimitTier` (observed, undocumented — used when
/// present). A banner around the JSON is tolerated
/// ([`crate::accounts::parse_auth_status`]).
///
/// `usable` is a best effort: `subscriptionType: "free"` is a
/// login whose plan does not include Claude Code. Any other subscription says
/// yes; no subscription (a Console or API-key login) says nothing.
pub fn claude_login(output: &RunOutput) -> ToolLogin {
    match output.code {
        Some(0) => {}
        Some(1) => return signed_out(),
        _ => return unknown_login(),
    }
    let status = crate::accounts::parse_auth_status(&output.stdout).unwrap_or_default();
    if status.get("loggedIn").and_then(Value::as_bool) == Some(false) {
        return signed_out();
    }
    let text = |key: &str| {
        status
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let subscription = text("subscriptionType");
    let tier = text("rateLimitTier");
    let usable = subscription.as_deref().map(|plan| !plan.eq_ignore_ascii_case("free"));
    ToolLogin {
        state: LoginState::SignedIn,
        method: Nullable(text("authMethod").filter(|method| method != "none")),
        plan: Nullable(
            subscription
                .as_deref()
                .map(|plan| crate::quota::claude_plan_label(Some(plan), tier.as_deref())),
        ),
        email: Nullable(text("email")),
        usable: Nullable(usable),
    }
}

/// `codex login status`: plain text, on stderr or stdout depending on the
/// version, exit 0 or 1.
///
/// `Logged in using an API key - sk-…` prints a masked key after the dash.
/// Only the words before ` - ` are ever compared, and nothing of the output
/// outlives this function.
pub fn codex_login(output: &RunOutput) -> ToolLogin {
    if output.timed_out || output.spawn_failed {
        return unknown_login();
    }
    let lines = output.stdout.lines().chain(output.stderr.lines());
    for line in lines {
        let head = line.split(" - ").next().unwrap_or_default().trim();
        let method = if head.starts_with("Logged in using ChatGPT") {
            "chatgpt"
        } else if head.starts_with("Logged in using an API key") {
            "apiKey"
        } else if head.starts_with("Not logged in") {
            return signed_out();
        } else {
            continue;
        };
        return ToolLogin {
            state: LoginState::SignedIn,
            method: Nullable(Some(method.to_string())),
            ..unknown_login()
        };
    }
    unknown_login()
}

/// The plan and address out of `account/read`'s `{account: {type, email,
/// planType}}`. Nothing else in the reply is read.
pub fn codex_account(result: &Value) -> (Option<String>, Option<String>) {
    let account = result.get("account");
    let text = |key: &str| {
        account
            .and_then(|account| account.get(key))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    (text("planType"), text("email"))
}

/// `node --version` → `22.3.0`.
pub fn node_version(output: &RunOutput) -> Option<String> {
    version_of(output)
}

/// `xcode-select -p`: exit 0 means the command line tools (and so git) are
/// there; any other exit means they are not; no answer means nobody knows.
pub fn git_state(output: &RunOutput) -> GitState {
    match output.code {
        Some(0) => GitState::Ready,
        Some(_) => GitState::Missing,
        None => GitState::Unknown,
    }
}

// --------------------------------------------------------------- the probe

/// Everything detection looks with: built from the live process for the
/// server, by hand for a test.
#[derive(Debug, Clone)]
pub struct Probe {
    /// The user's home: the fallbacks are under it and every CLI runs in it.
    pub home: PathBuf,
    /// The environment children get ([`crate::process_env::clean_env_bare`]).
    pub env: Env,
    /// The environment a `claude` gets ([`crate::process_env::claude_env_bare`]):
    /// no `CLAUDE_CONFIG_DIR`, so `auth status` speaks for the local login.
    pub claude_env: Env,
    /// The `PATH` the process was started with.
    pub inherited_path: String,
    /// The directories the user adopted.
    pub overlay: Vec<PathBuf>,
    /// Homebrew's prefixes, searched as fallbacks.
    pub system_dirs: Vec<PathBuf>,
    /// Whether this is a Mac: `sw_vers` and `xcode-select` are asked only there.
    pub macos: bool,
}

impl Probe {
    /// The server's own view, right now.
    pub fn live(home: PathBuf, overlay: Vec<PathBuf>) -> Self {
        Self {
            home,
            env: crate::process_env::clean_env_bare(),
            claude_env: crate::process_env::claude_env_bare(),
            inherited_path: crate::process_env::inherited_path(),
            overlay,
            system_dirs: vec![PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin")],
            macos: cfg!(target_os = "macos"),
        }
    }

    /// What [`locate::locate`] looks with.
    pub(crate) fn search(&self) -> Search<'_> {
        Search {
            home: &self.home,
            env: &self.env,
            inherited_path: &self.inherited_path,
            overlay: &self.overlay,
            system_dirs: &self.system_dirs,
        }
    }

    fn ask(&self, program: &Path, args: &[&str], env: &Env, timeout: Duration) -> Invocation {
        let mut invocation = Invocation::new(program.to_string_lossy(), args, timeout);
        invocation.env = env.clone();
        invocation.cwd = Some(self.home.clone());
        invocation
    }
}

/// What detection found about one CLI.
#[derive(Debug, Clone)]
pub struct ToolProbe {
    /// Which CLI.
    pub id: ToolId,
    /// Where it is, if anywhere.
    pub found: Option<ToolFound>,
    /// What `--version` said.
    pub version: Option<String>,
    /// Found, version read, and at least the minimum.
    pub supported: bool,
    /// What its own status command said.
    pub login: ToolLogin,
}

/// What detection found, before anything request-specific is added.
#[derive(Debug, Clone)]
pub struct Detection {
    /// The operating system.
    pub os: SetupOs,
    /// Homebrew, Node and git.
    pub helpers: SetupHelpers,
    /// Claude, then Codex.
    pub tools: Vec<ToolProbe>,
}

/// Ask everything, concurrently.
pub async fn detect(runner: &Arc<dyn Runner>, probe: &Probe) -> Detection {
    let (os, helpers, claude, codex) = tokio::join!(
        os(runner, probe),
        helpers(runner, probe),
        tool(runner, probe, ToolId::Claude),
        tool(runner, probe, ToolId::Codex)
    );
    Detection {
        os,
        helpers,
        tools: vec![claude, codex],
    }
}

/// `darwin`/`arm64`, and the macOS version from `sw_vers` on a Mac.
async fn os(runner: &Arc<dyn Runner>, probe: &Probe) -> SetupOs {
    let macos = if probe.macos {
        let output = runner
            .run(probe.ask(Path::new("/usr/bin/sw_vers"), &["-productVersion"], &probe.env, STEP_TIMEOUT))
            .await;
        output
            .succeeded()
            .then(|| output.stdout.trim().to_string())
            .filter(|version| !version.is_empty())
    } else {
        None
    };
    SetupOs {
        platform: crate::api::node_platform().to_string(),
        macos: Nullable(macos),
        arch: match std::env::consts::ARCH {
            "aarch64" => "arm64".to_string(),
            other => other.to_string(),
        },
    }
}

/// Homebrew on the `PATH`, Node's version, git via `xcode-select -p`.
async fn helpers(runner: &Arc<dyn Runner>, probe: &Probe) -> SetupHelpers {
    let brew = resolve_on_path("brew", &probe.env).is_some();
    let node = async {
        let path = resolve_on_path("node", &probe.env)?;
        node_version(&runner.run(probe.ask(&path, &["--version"], &probe.env, STEP_TIMEOUT)).await)
    };
    let git = async {
        if probe.macos {
            git_state(
                &runner
                    .run(probe.ask(Path::new("/usr/bin/xcode-select"), &["-p"], &probe.env, STEP_TIMEOUT))
                    .await,
            )
        } else if resolve_on_path("git", &probe.env).is_some() {
            GitState::Ready
        } else {
            GitState::Missing
        }
    };
    let (node, git) = tokio::join!(node, git);
    SetupHelpers {
        brew,
        node: Nullable(node),
        git,
    }
}

/// Locate one CLI, read its version, and — when it is new enough to have the
/// command — ask it whether it is signed in.
pub async fn tool(runner: &Arc<dyn Runner>, probe: &Probe, id: ToolId) -> ToolProbe {
    let Some(found) = locate::locate(id, &probe.search()) else {
        return ToolProbe {
            id,
            found: None,
            version: None,
            supported: false,
            login: unknown_login(),
        };
    };
    let path = PathBuf::from(&found.path);
    let env = match id {
        ToolId::Claude => &probe.claude_env,
        ToolId::Codex => &probe.env,
    };
    let version = version_of(&runner.run(probe.ask(&path, &["--version"], env, STEP_TIMEOUT)).await);
    let supported = version.as_deref().is_some_and(|version| at_least(version, id.min_version()));
    // An older CLI may not have the status command at all, and its usage error
    // would read as "signed out"; the card says "needs update" either way.
    let login = if supported {
        login(runner, probe, id, &path).await
    } else {
        unknown_login()
    };
    ToolProbe {
        id,
        found: Some(found),
        version,
        supported,
        login,
    }
}

/// The CLI's own answer to "who is signed in": `claude auth status --json`,
/// or `codex login status` plus `account/read` for the plan and address.
pub async fn login(runner: &Arc<dyn Runner>, probe: &Probe, id: ToolId, path: &Path) -> ToolLogin {
    match id {
        ToolId::Claude => claude_login(
            &runner
                .run(probe.ask(path, &["auth", "status", "--json"], &probe.claude_env, CLAUDE_STATUS_TIMEOUT))
                .await,
        ),
        ToolId::Codex => {
            let mut login = codex_status(runner, probe, path).await;
            if login.state == LoginState::SignedIn {
                let call = RpcCall {
                    program: path.to_path_buf(),
                    cwd: probe.home.clone(),
                    env: probe.env.clone(),
                    method: "account/read",
                    timeout: STEP_TIMEOUT,
                };
                if let Some(result) = runner.rpc(call).await {
                    let (plan, email) = codex_account(&result);
                    login.plan = Nullable(plan);
                    login.email = Nullable(email);
                }
            }
            login
        }
    }
}

/// `codex login status` alone: the cheap question a sign-in poll asks.
pub async fn codex_status(runner: &Arc<dyn Runner>, probe: &Probe, path: &Path) -> ToolLogin {
    codex_login(&runner.run(probe.ask(path, &["login", "status"], &probe.env, STEP_TIMEOUT)).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::runner::fixture::Fixture;
    use crate::setup::InstallMethod;
    use std::os::unix::fs::PermissionsExt;

    fn marker(path: &Path) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path, "not a program; never executed\n").expect("write");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    #[test]
    fn both_version_formats_are_read() {
        assert_eq!(parse_version("2.1.211 (Claude Code)\n").as_deref(), Some("2.1.211"));
        assert_eq!(parse_version("codex-cli 0.163.0\n").as_deref(), Some("0.163.0"));
        assert_eq!(parse_version("v22.3.0").as_deref(), Some("22.3.0"));
        assert_eq!(parse_version("codex-cli 0.164.0-alpha.2").as_deref(), Some("0.164.0"));
        assert_eq!(parse_version("no version here"), None);
        assert_eq!(version_of(&RunOutput::exited(0, "2.1.300 (Claude Code)", "")).as_deref(), Some("2.1.300"));
        assert_eq!(version_of(&RunOutput::exited(1, "2.1.300", "")), None, "a failed run says nothing");
        assert_eq!(version_of(&RunOutput::timeout()), None);
    }

    #[test]
    fn versions_compare_by_number_not_by_text() {
        assert!(at_least("2.1.295", MIN_CLAUDE));
        assert!(at_least("2.1.1000", "2.1.295"));
        assert!(at_least("3.0.0", "2.1.295"));
        assert!(!at_least("2.1.99", "2.1.295"), "99 < 295 though '9' > '2'");
        assert!(!at_least("1.9.999", "2.1.295"));
        assert!(at_least("0.163.0", MIN_CODEX));
        assert!(!at_least("0.153.3", MIN_CODEX));
        assert!(!at_least("garbage", MIN_CODEX));
    }

    #[test]
    fn claude_s_status_reads_signed_in_signed_out_and_a_banner() {
        let signed_in = RunOutput::exited(
            0,
            r#"{"loggedIn":true,"authMethod":"claude.ai","email":"ada@example.com","subscriptionType":"max","rateLimitTier":"default_claude_max_20x"}"#,
            "",
        );
        let login = claude_login(&signed_in);
        assert_eq!(login.state, LoginState::SignedIn);
        assert_eq!(login.method.0.as_deref(), Some("claude.ai"));
        assert_eq!(login.plan.0.as_deref(), Some("Max 20x"));
        assert_eq!(login.email.0.as_deref(), Some("ada@example.com"));
        assert_eq!(login.usable.0, Some(true));

        let banner = RunOutput::exited(
            0,
            "Update available! Run claude update\n{\"loggedIn\": true, \"authMethod\": \"console\"}\nThanks\n",
            "",
        );
        let login = claude_login(&banner);
        assert_eq!(login.state, LoginState::SignedIn);
        assert_eq!(login.method.0.as_deref(), Some("console"));
        assert_eq!(login.plan.0, None, "a Console login has no subscription to name");
        assert_eq!(login.usable.0, None, "and says nothing about one");

        let out = claude_login(&RunOutput::exited(1, r#"{"loggedIn":false,"authMethod":"none"}"#, ""));
        assert_eq!(out.state, LoginState::SignedOut);
        assert_eq!(out.method.0, None);

        let free = claude_login(&RunOutput::exited(
            0,
            r#"{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"free"}"#,
            "",
        ));
        assert_eq!(free.usable.0, Some(false), "H9: a free plan does not include Claude Code");
        assert_eq!(free.plan.0.as_deref(), Some("Free"));

        assert_eq!(claude_login(&RunOutput::timeout()).state, LoginState::Unknown);
        assert_eq!(claude_login(&RunOutput::failed_to_start()).state, LoginState::Unknown);
        assert_eq!(claude_login(&RunOutput::exited(2, "", "boom")).state, LoginState::Unknown);
    }

    #[test]
    fn codex_s_status_reads_four_texts_and_never_the_key() {
        let chatgpt = codex_login(&RunOutput::exited(0, "", "Logged in using ChatGPT\n"));
        assert_eq!(chatgpt.state, LoginState::SignedIn);
        assert_eq!(chatgpt.method.0.as_deref(), Some("chatgpt"));

        let key = codex_login(&RunOutput::exited(
            0,
            "Logged in using an API key - sk-proj-***************************abcd\n",
            "",
        ));
        assert_eq!(key.state, LoginState::SignedIn);
        assert_eq!(key.method.0.as_deref(), Some("apiKey"));
        let serialized = serde_json::to_string(&key).expect("json");
        assert!(!serialized.contains("sk-"), "the masked key never reaches the report: {serialized}");

        assert_eq!(codex_login(&RunOutput::exited(1, "", "Not logged in\n")).state, LoginState::SignedOut);
        assert_eq!(
            codex_login(&RunOutput::exited(0, "something new entirely\n", "")).state,
            LoginState::Unknown,
            "an answer nobody wrote a parser for is not a guess"
        );
        assert_eq!(codex_login(&RunOutput::timeout()).state, LoginState::Unknown);
        assert_eq!(codex_login(&RunOutput::failed_to_start()).state, LoginState::Unknown);
    }

    #[test]
    fn codex_s_account_gives_its_plan_and_address_and_nothing_else() {
        let (plan, email) = codex_account(&serde_json::json!({
            "account": {"type": "chatgpt", "email": "ada@example.com", "planType": "plus", "credits": {"balance": 3}},
        }));
        assert_eq!(plan.as_deref(), Some("plus"));
        assert_eq!(email.as_deref(), Some("ada@example.com"));
        assert_eq!(codex_account(&serde_json::json!({"account": {"type": "apiKey"}})), (None, None));
        assert_eq!(codex_account(&serde_json::json!(null)), (None, None));
    }

    #[test]
    fn git_is_ready_missing_or_unknown_by_xcode_select_s_exit() {
        assert_eq!(
            git_state(&RunOutput::exited(0, "/Library/Developer/CommandLineTools\n", "")),
            GitState::Ready
        );
        assert_eq!(
            git_state(&RunOutput::exited(2, "", "xcode-select: error: unable to get active developer directory")),
            GitState::Missing
        );
        assert_eq!(git_state(&RunOutput::timeout()), GitState::Unknown);
    }

    /// A temp home with a native claude and an npm codex, and a fixture that
    /// answers every question detection is allowed to ask.
    fn scene() -> (tempfile::TempDir, Probe, Arc<Fixture>) {
        let home = tempfile::tempdir().expect("tempdir");
        let bin = home.path().join(".local/bin");
        let claude = home.path().join(".local/share/claude/versions/2.1.300");
        marker(&claude);
        std::fs::create_dir_all(&bin).expect("bin");
        std::os::unix::fs::symlink(&claude, bin.join("claude")).expect("link");
        let codex = home.path().join("npm/lib/node_modules/@openai/codex/bin/codex.js");
        marker(&codex);
        std::os::unix::fs::symlink(&codex, bin.join("codex")).expect("link");
        marker(&bin.join("node"));
        marker(&bin.join("brew"));

        let env: Env = [("PATH".to_string(), bin.to_string_lossy().into_owned())].into_iter().collect();
        let probe = Probe {
            home: home.path().to_path_buf(),
            env: env.clone(),
            claude_env: env,
            inherited_path: bin.to_string_lossy().into_owned(),
            overlay: Vec::new(),
            system_dirs: Vec::new(),
            macos: true,
        };
        let fixture = Fixture::new();
        fixture
            .answer("sw_vers -productVersion", RunOutput::exited(0, "15.3\n", ""))
            .answer("xcode-select -p", RunOutput::exited(2, "", "xcode-select: error"))
            .answer("node --version", RunOutput::exited(0, "v22.3.0\n", ""))
            .answer("claude --version", RunOutput::exited(0, "2.1.300 (Claude Code)\n", ""))
            .answer(
                "claude auth status --json",
                RunOutput::exited(0, r#"{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"pro"}"#, ""),
            )
            .answer("codex --version", RunOutput::exited(0, "codex-cli 0.163.0\n", ""))
            .answer(
                "codex login status",
                RunOutput::exited(0, "", "Logged in using an API key - sk-proj-****abcd\n"),
            )
            .answer_rpc("account/read", serde_json::json!({"account": {"type": "apiKey"}}));
        (home, probe, fixture)
    }

    #[tokio::test]
    async fn detection_asks_exactly_these_questions_and_nothing_that_reads_a_credential() {
        let (_home, probe, fixture) = scene();
        let runner: Arc<dyn Runner> = fixture.clone();
        let detection = detect(&runner, &probe).await;

        let mut lines = fixture.lines();
        lines.sort();
        assert_eq!(
            lines,
            [
                "claude --version",
                "claude auth status --json",
                "codex --version",
                "codex app-server account/read",
                "codex login status",
                "node --version",
                "sw_vers -productVersion",
                "xcode-select -p",
            ]
        );
        for asked in fixture.asked() {
            assert!(asked.stdin.is_none(), "detection pipes nothing into {}", asked.line());
            assert!(asked.program.starts_with('/'), "every program is run by its full path: {}", asked.program);
            assert!(
                !["security", "cat", "git", "sh", "bash", "zsh", "open"].contains(&asked.name.as_str()),
                "detection must not run {}",
                asked.line()
            );
            for arg in &asked.args {
                for secret in [".credentials.json", ".claude.json", "auth.json", "/usage"] {
                    assert!(!arg.contains(secret), "{} names {secret}", asked.line());
                }
            }
        }

        assert_eq!(detection.os.macos.0.as_deref(), Some("15.3"));
        assert!(detection.helpers.brew);
        assert_eq!(detection.helpers.node.0.as_deref(), Some("22.3.0"));
        assert_eq!(detection.helpers.git, GitState::Missing);
        let claude = &detection.tools[0];
        assert_eq!(claude.found.as_ref().map(|found| found.method), Some(InstallMethod::Native));
        assert_eq!(claude.version.as_deref(), Some("2.1.300"));
        assert!(claude.supported);
        assert_eq!(claude.login.plan.0.as_deref(), Some("Pro"));
        let codex = &detection.tools[1];
        assert_eq!(codex.found.as_ref().map(|found| found.method), Some(InstallMethod::Npm));
        assert_eq!(codex.login.method.0.as_deref(), Some("apiKey"));
    }

    #[tokio::test]
    async fn a_cli_too_old_is_not_asked_a_question_it_may_not_have() {
        let (_home, probe, fixture) = scene();
        fixture.answer("claude --version", RunOutput::exited(0, "1.0.88 (Claude Code)\n", ""));
        let runner: Arc<dyn Runner> = fixture.clone();
        let claude = tool(&runner, &probe, ToolId::Claude).await;
        assert_eq!(claude.version.as_deref(), Some("1.0.88"));
        assert!(!claude.supported);
        assert_eq!(claude.login.state, LoginState::Unknown);
        assert!(!fixture.lines().iter().any(|line| line.starts_with("claude auth")));
    }

    #[tokio::test]
    async fn a_missing_cli_is_never_run() {
        let home = tempfile::tempdir().expect("tempdir");
        let probe = Probe {
            home: home.path().to_path_buf(),
            env: [("PATH".to_string(), home.path().join("nothing").to_string_lossy().into_owned())]
                .into_iter()
                .collect(),
            claude_env: Env::new(),
            inherited_path: String::new(),
            overlay: Vec::new(),
            system_dirs: Vec::new(),
            macos: false,
        };
        let fixture = Fixture::new();
        let runner: Arc<dyn Runner> = fixture.clone();
        let detection = detect(&runner, &probe).await;
        assert!(detection.tools.iter().all(|tool| tool.found.is_none() && !tool.supported));
        assert_eq!(detection.helpers.git, GitState::Missing, "no git on the PATH off a Mac");
        assert!(fixture.lines().is_empty(), "nothing to ask: {:?}", fixture.lines());
    }
}
