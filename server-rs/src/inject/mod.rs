//! Restart-safe CLI drivers: Claude stream-json and Codex App Server.
//!
//! Both residents use the protocol-agnostic session shim. Legacy codex exec
//! runs are still adopted by PID until they finish, but no new exec is spawned.

use std::collections::HashMap;
use std::path::PathBuf;
#[cfg(test)]
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::accounts::store::{AccountError, AccountLaunch, AccountProvider};
use crate::api::error::ApiError;
use crate::claude_roots::{self, ClaudeRoot};
use crate::instance::InstanceDirs;
use crate::process_env::{claude_env, claude_env_bare, clean_env, clean_env_bare, redact_secrets, Env};
use crate::procs;
use crate::record;
use crate::sessions::{claude, codex, iso8601, Injection, InjectionRunState, Paths, ServerId, SessionRef, SessionStatus, Source};
use gotcha::axum::http::StatusCode;
use gotcha::Schematic;

mod app_server;
pub mod drafts;
mod lock;
pub mod questions;
pub mod receipts;
mod resident;
mod runs;
pub mod shim;

pub use resident::{AnswerOutcome, Phase, QueueOutcome, Reply, ResidentState, StopTaskOutcome};

/// How often an adopted run's pid is probed.
///
/// An adopted run has no `Child` handle to `wait()` on — the process that
/// forked it is gone, and the orphan was reparented to pid 1, which is what
/// reaps it. `kill(pid, 0)` turning `ESRCH` is therefore the only completion
/// signal available, and polling is the only way to read it. Two seconds is
/// well inside what a human reads as "the composer unblocked when it
/// finished", and costs one syscall per live run.
const ADOPTED_POLL: Duration = Duration::from_secs(2);
/// What an adopted run whose CLI was cut short is told.
const RESTART_NOTE: &str = "server restarted during this run";
/// Everything that can stop a run before it starts.
#[derive(Debug, Clone, thiserror::Error)]
pub enum InjectError {
    /// A plain refusal, reported to the client verbatim.
    #[error("{0}")]
    Message(String),
    /// Sessions in the user's own `~/.claude` keep using their own login, always.
    #[error("local_session_cannot_use_account")]
    LocalSessionAccount,
    /// A spawn was handed the other CLI's account.
    #[error("this account authenticates {} and cannot run a {} session", account.label(), wanted.label())]
    ProviderMismatch {
        /// The CLI the account belongs to.
        account: AccountProvider,
        /// The CLI being spawned.
        wanted: AccountProvider,
    },
    /// The account store refused.
    #[error("{0}")]
    Account(#[from] AccountError),
    /// A plain I/O failure on the way to a spawn.
    ///
    /// The TypeScript lets these escape as ordinary `Error`s, which
    /// `spawnErrorMessage` (the former TypeScript server's `app.ts`) replaces
    /// with the route's own wording rather than showing the user an `errno`.
    #[error("{0}")]
    Internal(String),
}

impl InjectError {
    /// What the client is told, masking anything the TypeScript would mask.
    pub fn client_message(&self, fallback: &str) -> String {
        match self {
            InjectError::Internal(_) | InjectError::Account(AccountError::Io(_)) => fallback.to_string(),
            other => other.to_string(),
        }
    }
}

impl InjectError {
    fn message(text: impl Into<String>) -> Self {
        InjectError::Message(text.into())
    }
}

/// Which login a spawn should run under.
///
/// The three states are the wire's three states: the field absent, the field
/// `null`, and the field naming an account. They mean different things — `null`
/// is "the local login even though a default is configured", while absent leaves
/// the choice to the caller's fallback.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum AccountChoice {
    /// The `accountId` field was absent.
    #[default]
    Unset,
    /// `accountId: null` — the local login, explicitly.
    Local,
    /// `accountId: "<uuid>"`.
    Account(String),
    /// `accountId: "<uuid>"` with `accountFrom: "start"`: the account
    /// `GET /api/accounts/start` named for a pair no model-less start reaches
    /// (`placed: false`). The start rule's choice, not a person's — logged as
    /// that, and re-checked at the spawn ([`rule_account`]).
    Rule(String),
}

/// Where a request's `accountId` came from, when it was not somebody's pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Schematic)]
#[serde(rename_all = "snake_case")]
pub enum AccountFrom {
    /// `GET /api/accounts/start` named it, with `placed: false`.
    Start,
}

impl AccountChoice {
    /// [`Self::from_request`], with where a named account came from: one the
    /// start rule named is the rule's choice ([`AccountChoice::Rule`]).
    pub fn from_start_request(value: Option<Option<String>>, from: Option<AccountFrom>) -> Self {
        match (Self::from_request(value), from) {
            (AccountChoice::Account(id), Some(AccountFrom::Start)) => AccountChoice::Rule(id),
            (choice, _) => choice,
        }
    }

    /// Build the choice from the double-`Option` a request body deserializes to.
    pub fn from_request(value: Option<Option<String>>) -> Self {
        match value {
            None => AccountChoice::Unset,
            Some(None) => AccountChoice::Local,
            Some(Some(id)) => AccountChoice::Account(id),
        }
    }

    /// The account named, if one was.
    pub fn named(&self) -> Option<&str> {
        match self {
            AccountChoice::Account(id) => Some(id),
            _ => None,
        }
    }
}

/// Everything the composer can ask a spawn to do differently.
#[derive(Debug, Clone, Default)]
pub struct SendOptions {
    /// `--model` / `-m`.
    pub model: Option<String>,
    /// `--effort` / `-c model_reasoning_effort=`.
    pub effort: Option<String>,
    /// `--permission-mode` / `--sandbox`.
    pub permission: Option<String>,
    /// Instant chat: replace the coding persona and disable all tools.
    pub chat: bool,
    /// Image paths from `/api/uploads`.
    pub images: Vec<String>,
    /// Non-image attachment paths from `/api/uploads`.
    pub files: Vec<String>,
    /// Which login to run under.
    pub account: AccountChoice,
    /// New claude sessions only: fall back to the configured default account.
    pub use_default_account: bool,
    /// The queue id the message is to travel under, when the route chose one.
    ///
    /// Set for a message the client named (`clientMessageId`): the route
    /// claims the name and picks the id *before* anything is enqueued, so the
    /// receipt can follow the message from the first instant a driver holds it.
    /// `None` mints one, as every send did before.
    pub message_id: Option<String>,
}

/// Mode names aligned with current Codex terminology, mapped onto what
/// `codex exec` actually supports (approvals are always "never" headless).
/// Raw sandbox values are kept as aliases for older stored preferences.
pub(crate) fn codex_permission_flags(permission: &str) -> Result<Vec<&'static str>, InjectError> {
    Ok(match permission {
        "auto" | "workspace-write" => vec!["--sandbox", "workspace-write"],
        "read-only" => vec!["--sandbox", "read-only"],
        "full-access" | "danger-full-access" => vec!["--sandbox", "danger-full-access"],
        "bypass-all" => vec!["--dangerously-bypass-approvals-and-sandbox"],
        other => return Err(InjectError::message(format!("unknown codex permission mode: {other}"))),
    })
}

/// The instant-chat persona: the text the CLI is actually launched with.
const CHAT_SYSTEM_PROMPT: &str = concat!(
    "You are a friendly general-purpose assistant in a casual chat app. ",
    "This is not a coding tool: have a normal conversation, answer in the user's language, ",
    "and keep answers conversational. You have no tools; never claim to read files or run commands."
);

fn claude_chat_flags() -> Vec<String> {
    vec![
        "--system-prompt".into(),
        CHAT_SYSTEM_PROMPT.into(),
        "--disallowedTools".into(),
        "*".into(),
    ]
}

/// The tools a resident is started without, whatever else it is given.
///
/// The plan-mode pair, because of what `--permission-prompt-tool stdio` does to
/// them: `EnterPlanMode` runs without asking anyone, and the only way back out
/// is `ExitPlanMode`, which is a permission prompt — and every permission
/// prompt a resident raises is refused (see [`questions`]). A model that
/// entered plan mode would be shut in it, read-only, for the rest of the
/// session. Without the prompt tool the CLI never offered either of them to a
/// `-p` process, so nothing is lost by saying so.
const RESIDENT_DISALLOWED_TOOLS: [&str; 2] = ["EnterPlanMode", "ExitPlanMode"];

/// The `error` of a `409` for an attachment that is not there any more.
pub const ATTACHMENT_MISSING: &str = "attachment_missing";

/// The refusal of an attachment that `/api/uploads` handed out and that is no
/// longer on this device: swept with the rest of the old uploads, or deleted.
fn attachment_missing() -> ApiError {
    ApiError::with_body(
        StatusCode::CONFLICT,
        serde_json::json!({
            "error": ATTACHMENT_MISSING,
            "hint": "an attachment is no longer on this device; attach it again",
        }),
    )
}

/// Prove every attachment came from `/api/uploads` and is still there, and
/// return the resolved paths.
///
/// The gate is `isUploadPath` in the former TypeScript server's `app.ts`, and
/// it belongs here rather than in the route because this module is what hands
/// the paths to a child process (`-i`, `--add-dir`, the prompt suffix): a path
/// that escaped the uploads directory would widen what the CLI may read. The
/// *resolved* paths are returned because [`crate::files::assert_under_uploads`]
/// only vouches for what it resolved, not for the spelling the client sent.
///
/// The lexical check comes first and keeps its own refusal. Then the file
/// itself: it must exist, be a regular file, and — symlinks followed — still be
/// inside the uploads directory. One that is gone (a draft kept across the
/// sweep of old uploads) is refused with [`ATTACHMENT_MISSING`] rather than
/// handed to an agent that would find nothing at the path.
pub fn resolve_attachments(paths: &Paths, images: Vec<String>, files: Vec<String>) -> Result<(Vec<String>, Vec<String>), ApiError> {
    if images.is_empty() && files.is_empty() {
        return Ok((images, files));
    }
    let uploads = std::fs::canonicalize(paths.uploads_dir()).ok();
    let resolve = |requested: Vec<String>| -> Result<Vec<String>, ApiError> {
        requested
            .into_iter()
            .map(|path| {
                let resolved = crate::files::assert_under_uploads(paths, std::path::Path::new(&path))?;
                let real = std::fs::canonicalize(&resolved).map_err(|_| attachment_missing())?;
                let inside = uploads.as_ref().is_some_and(|uploads| real != *uploads && real.starts_with(uploads));
                if !inside {
                    return Err(ApiError::new(StatusCode::CONFLICT, "attachment paths must come from /api/uploads"));
                }
                if !std::fs::metadata(&real).is_ok_and(|meta| meta.is_file()) {
                    return Err(attachment_missing());
                }
                Ok(resolved.to_string_lossy().into_owned())
            })
            .collect()
    };
    Ok((resolve(images)?, resolve(files)?))
}

/// The suffix that tells a CLI where its attachments are.
///
/// It is also the contract the transcript parser reads back
/// (`crate::sessions::util::parse_attachment_suffix`), so the wording is fixed.
pub fn attachment_suffix(paths: &[String]) -> String {
    let list = paths.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n");
    format!("\n\n[{} attached file(s) — view with the Read tool:]\n{list}", paths.len())
}

/// claude has no image flag; it views every attachment through the Read tool.
fn with_claude_attachments(text: &str, options: &SendOptions) -> String {
    claude_prompt(text, &options.images, &options.files)
}

/// A prompt as claude reads it: the text, and where its attachments are.
///
/// Public for the one other place a user's words reach claude: an answer to a
/// question it is blocked on, which travels in the permission response rather
/// than as a message, and has to point at an attachment just the same.
pub fn claude_prompt(text: &str, images: &[String], files: &[String]) -> String {
    let attach: Vec<String> = images.iter().chain(files.iter()).cloned().collect();
    if attach.is_empty() {
        text.to_string()
    } else {
        format!("{text}{}", attachment_suffix(&attach))
    }
}

/// codex has no flag for non-image files, so point it at them in the prompt text.
fn with_codex_files(text: &str, options: &SendOptions) -> String {
    if options.files.is_empty() {
        text.to_string()
    } else {
        format!("{text}{}", attachment_suffix(&options.files))
    }
}

/// The settings half of a resident's command line: everything after the
/// session selector, and nothing that varies per message.
///
/// Two things changed with the resident model, and both are visible here. The
/// prompt is gone — it goes in over stdin as stream-json, which is what lets one
/// process take message after message — and with it the `--` guard, because
/// with no positional argument left there is nothing for a variadic flag to
/// swallow. The variadic flags are still last for the same reason they were
/// before: `--add-dir` and `--disallowedTools` would otherwise eat whatever
/// followed them.
///
/// Everything on this line is fixed for the process' whole life, which is what
/// [`resident::Fingerprint`] exists to compare: a message wanting a different
/// model needs a different process, not a different argument.
fn claude_resident_flags(paths: &Paths, options: &SendOptions) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(model) = &options.model {
        args.extend(["--model".to_string(), model.clone()]);
    }
    if let Some(effort) = &options.effort {
        args.extend(["--effort".to_string(), effort.clone()]);
    }
    if let Some(permission) = &options.permission {
        args.extend(["--permission-mode".to_string(), permission.clone()]);
    }
    // Where claude sends what it would otherwise have asked a terminal: back
    // down the stream, as a `control_request` the driver answers. This is what
    // makes `AskUserQuestion` exist at all for a `-p` process — and it routes
    // every other permission prompt the same way, each of which the driver
    // refuses exactly as `--permission-mode` used to. See [`questions`].
    args.extend(["--permission-prompt-tool".to_string(), "stdio".to_string()]);
    // Prepare every process for later screenshots, even when its first prompt
    // is text-only. Adding this flag later would require a process restart.
    args.extend(["--add-dir".into(), paths.uploads_dir().to_string_lossy().into_owned()]);
    // One `--disallowedTools`, not two: it is variadic, so it goes last, and a
    // chat's `*` already covers the plan tools.
    if options.chat {
        args.extend(claude_chat_flags());
    } else {
        args.push("--disallowedTools".into());
        args.extend(RESIDENT_DISALLOWED_TOOLS.iter().map(|tool| (*tool).to_string()));
    }
    args
}

/// Everything a resident spawn needs, resolved against one send.
///
/// Assembled on the send path rather than in the driver because this is where
/// the account store, the uploads directory and the session's own working
/// directory are reachable; the driver only ever compares it and hands it to
/// [`shim::spawn_via_shim`].
fn claude_recipe(paths: &Paths, cwd: Option<&str>, options: &SendOptions, env: Env, login: ClaudeLogin) -> resident::Recipe {
    resident::Recipe {
        args: claude_resident_flags(paths, options),
        env,
        workdir: Some(claude_workdir(paths, cwd)),
        fingerprint: resident::Fingerprint {
            model: options.model.clone(),
            // A send asks for a model; only the CLI can say what it will call
            // it. See [`resident::Fingerprint::resolved`].
            resolved: None,
            effort: options.effort.clone(),
            permission: options.permission.clone(),
            account: login.account,
            credential: login.credential,
            chat: options.chat,
            uploads: true,
            questions: true,
        },
    }
}

/// Which login a claude spawn runs under, in the two parts a resident is
/// compared by.
///
/// The id alone was the whole answer until an env account was edited under a
/// running process: the id says who is billed, and says nothing about what is
/// actually in the environment. So the credential is named too — by its hash,
/// which is the most the store will say about one — and a send that resolves to
/// the same account holding a *different* credential no longer looks like the
/// same login. See [`resident::Fingerprint::credential`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ClaudeLogin {
    /// The account whose credential is in the environment; `None` is the
    /// root's own login.
    account: Option<String>,
    /// The hash of the credential that account held when this was resolved.
    credential: Option<String>,
}

/// That account, plus a name for the credential it holds right now.
///
/// Read here rather than carried out of [`owned_claude_env`] because it is
/// wanted on the path that does not build an environment at all — a local-login
/// send — and because the store is the only thing that may answer it.
async fn claude_login(drive: &crate::drive::Drive, account: Option<String>) -> ClaudeLogin {
    let credential = match account.as_deref() {
        Some(id) => drive.accounts.credential_hash(id).await,
        None => None,
    };
    ClaudeLogin { account, credential }
}

/// Where a resident runs, with a fallback that is somewhere and not anywhere.
///
/// A session records the directory it was started in, and that directory can be
/// renamed or deleted between two messages. Refusing to send is the wrong answer
/// — the conversation is still worth continuing — so the fallback is the home
/// directory, the same one a session with no recorded directory at all gets.
///
/// What it must *never* fall back to is "unset", because an unset working
/// directory means the child inherits the server's, and the server's is wherever
/// somebody happened to start it: a developer's checkout, `/`, the directory a
/// supervisor chose. Handing an agent the source tree of the server driving it,
/// silently, because a path went missing, is not a recovery.
///
/// One `stat` on the send path, which is cheap next to the process this is about
/// to start.
fn claude_workdir(paths: &Paths, cwd: Option<&str>) -> PathBuf {
    match cwd.map(PathBuf::from) {
        Some(dir) if dir.is_dir() => dir,
        _ => paths.home.clone(),
    }
}

// ------------------------------------------------------------ account resolution

/// The environment a spawn against this instance's own Claude root runs with.
///
/// Two independent pieces:
///
/// * **the root.** Every session this server owns lives in
///   `<data>/instances/<name>/claude`, so every spawn against one is pointed
///   there. That is what makes the session resumable *here* and nowhere else.
/// * **the token**, only when an account was chosen. Without one the spawn uses
///   whatever login the root itself holds — which is exactly what an adopted
///   session, or an instance the user logged into directly, needs.
///
/// A `.credentials.json` in the root is only worth a warning in the token case.
/// Older CLIs let file credentials outrank the environment, which is why this
/// used to refuse; claude 2.1.266 was checked on 2026-09-23 with a bogus file
/// next to a valid `CLAUDE_CODE_OAUTH_TOKEN` and used the environment, and the
/// `ANTHROPIC_AUTH_TOKEN` / `ANTHROPIC_API_KEY` tracks sit ahead of the file as
/// well. Refusing would only make the root's own login (which no-account
/// sessions need) and account sessions mutually exclusive.
async fn owned_claude_env(drive: &crate::drive::Drive, account_id: Option<&str>) -> Result<Env, InjectError> {
    managed_claude_env(&drive.paths, &drive.accounts, account_id).await
}

/// [`owned_claude_env`] from the two things it reads, for a caller that holds no
/// [`crate::drive::Drive`] — a session's driver, rotating it onto another
/// account.
async fn managed_claude_env(paths: &Paths, accounts: &crate::accounts::store::AccountsStore, account_id: Option<&str>) -> Result<Env, InjectError> {
    let dir = claude_roots::ensure_managed_root(paths)
        .await
        .map_err(|error| InjectError::Internal(format!("cannot create this instance's Claude root: {error}")))?;
    let mut vars = vec![("CLAUDE_CONFIG_DIR".to_string(), dir.to_string_lossy().into_owned())];
    if let Some(account_id) = account_id {
        if let Some(path) = claude_roots::managed_root_file_credentials(paths).await {
            tracing::warn!(
                path = %path.display(),
                account = account_id,
                "the managed root holds file credentials; the account token in the environment takes precedence on current CLIs"
            );
        }
        let launch = accounts.account_env(account_id).await?;
        // A codex key in `ANTHROPIC_API_KEY` would be a 401 under a label that
        // says otherwise; refuse it here, where the account is still nameable.
        require_provider(&launch, AccountProvider::Claude)?;
        vars.extend(launch.env);
    }
    Ok(claude_env(vars))
}

/// A spawn environment with one account's variables swapped for another's.
///
/// Everything else in `env` — the Claude root, the `rh-notify` decoration, the
/// `PATH` it was started with — is what the session already runs with and is
/// kept exactly. Each of `old`'s variables goes; where the server's own
/// environment would have supplied one of those names had no account set it,
/// that value comes back (`inherited`, which never answers for the harness
/// variables [`claude_env`] strips). Then `new`'s go in on top — the same
/// "account last" order [`managed_claude_env`] builds in.
///
/// Known limitation: `old` is the old account's block as stored *now*. A key
/// deleted from that block after the process started (say `ANTHROPIC_BASE_URL`)
/// is not in `old`, so its stale value survives into the new environment.
fn swap_account_env(env: &Env, old: &[(String, String)], new: &[(String, String)], inherited: impl Fn(&str) -> Option<String>) -> Env {
    let mut env = env.clone();
    for (key, _) in old {
        match inherited(key) {
            Some(value) => env.insert(key.clone(), value),
            None => env.remove(key),
        };
    }
    for (key, value) in new {
        env.insert(key.clone(), value.clone());
    }
    env
}

/// What the server's own environment gives a spawn under `key`, after the strip
/// [`claude_env`] makes.
fn inherited_var(key: &str) -> Option<String> {
    clean_env_bare().remove(key)
}

/// Which login a resume runs under.
///
/// A session is this server's own when its transcript sits in the root this
/// instance owns, or when it is already bound to an account — the binding is
/// written before the first spawn, so it is authoritative even before the CLI
/// has created the transcript.
///
/// A session that is *not* this server's own reaches here only from a caller
/// that has already checked (the send route refuses it with `external_session`),
/// so the local-login branch survives for the account-mismatch report and for
/// anything internal that drives a session directly.
/// The account is returned beside the environment because a resident is
/// fingerprinted by it: two messages billed to different logins cannot share a
/// process, and the resolution — binding, explicit choice, default — happens
/// only here.
async fn resume_claude_env(drive: &crate::drive::Drive, id: &str, options: &SendOptions) -> Result<(Env, ClaudeLogin), InjectError> {
    let found = claude::find_claude_session(&drive.paths, id).await;
    let bound = drive.accounts.session_account_id(id).await;
    let owned = found.map(|f| f.root) == Some(ClaudeRoot::Managed) || bound.is_some();

    if !owned {
        // An explicit account on a session in the user's own root is a mistake
        // worth reporting; `accountId: null` on one is simply what it already does.
        if options.account.named().is_some() {
            return Err(InjectError::LocalSessionAccount);
        }
        return Ok((claude_env_bare(), ClaudeLogin::default()));
    }

    // `accountId: null` means "no account token", which for an owned session is
    // the root's own login rather than the user's `~/.claude`; absent means
    // "whatever this session is already bound to".
    let account_id = match &options.account {
        AccountChoice::Local => None,
        // Only a new start carries a rule's account; on a resume it is a name.
        AccountChoice::Account(id) | AccountChoice::Rule(id) => Some(id.clone()),
        AccountChoice::Unset => bound.clone(),
    };
    let env = owned_claude_env(drive, account_id.as_deref()).await?;
    if let Some(chosen) = options.account.named() {
        if Some(chosen) != bound.as_deref() {
            drive.accounts.bind_session_account(id, Some(chosen), AccountProvider::Claude).await?;
            // Somebody's choice: nothing moves it ahead of a limit for a while.
            drive.accounts.rotation_log().note_hand_binding(id, crate::accounts::rest::now_ms());
        }
    }
    let login = claude_login(drive, account_id).await;
    Ok((env, login))
}

/// Refuse an account that belongs to the other CLI.
fn require_provider(launch: &AccountLaunch, wanted: AccountProvider) -> Result<(), InjectError> {
    if launch.provider == wanted {
        return Ok(());
    }
    Err(InjectError::ProviderMismatch {
        account: launch.provider,
        wanted,
    })
}

/// Which account a codex spawn runs under, and what it puts into it.
///
/// The same three-state resolution [`resume_claude_env`] does, and for the same
/// reason: `accountId: null` is "the machine's own codex login" even when a
/// default is configured, absent is "whatever this thread is already bound to",
/// and a named account both wins and is written down.
///
/// `None` is the answer for a machine with no codex accounts, and it is the
/// answer that matters most: it must produce exactly the spawn this server made
/// before any of this existed — no extra argv, no extra environment, codex's own
/// `~/.codex/auth.json` and nothing else.
async fn codex_account(drive: &crate::drive::Drive, id: Option<&str>, options: &SendOptions) -> Result<Option<CodexAccount>, InjectError> {
    let bound = match id {
        Some(id) => drive.accounts.session_account_id(id).await,
        None => None,
    };
    let chosen = match &options.account {
        AccountChoice::Local => None,
        AccountChoice::Account(id) => Some(id.clone()),
        AccountChoice::Rule(named) => rule_account(drive, AccountProvider::Codex, named, options.model.as_deref()).await,
        AccountChoice::Unset => match bound.clone() {
            Some(bound) => Some(bound),
            // The configured default, or — with none, and `~/.codex` known to
            // be signed out — a codex account in the pool, if one is free.
            None if options.use_default_account => start_account(drive, AccountProvider::Codex, options.model.as_deref()).await,
            None => None,
        },
    };
    let Some(account_id) = chosen else { return Ok(None) };
    let launch = drive.accounts.account_env(&account_id).await?;
    require_provider(&launch, AccountProvider::Codex)?;
    // An explicit choice on a thread that already exists is also a re-binding:
    // the next message must find the account this one asked for.
    if let (Some(id), Some(named)) = (id, options.account.named()) {
        if Some(named) != bound.as_deref() {
            drive.accounts.bind_session_account(id, Some(named), AccountProvider::Codex).await?;
        }
    }
    Ok(Some(CodexAccount { id: account_id, launch }))
}

/// One codex account, resolved and ready to be spawned under.
#[derive(Debug, Clone)]
pub(crate) struct CodexAccount {
    /// Which account it is, so a live process can be compared against it.
    pub(crate) id: String,
    /// What it contributes to the spawn.
    pub(crate) launch: AccountLaunch,
}

/// Give a claude spawn the means to notify its user, when there is a channel.
///
/// Claude only, for now, and the reason is the skill rather than the program:
/// `rh-notify` would work perfectly well on a `codex exec` command line, but
/// nothing would ever tell codex that it exists — skills are a Claude Code
/// mechanism, and a `codex` thread's id is not even known until after the spawn
/// that would have carried the token. See [`crate::notify::inject`].
///
/// Deliberately infallible. A session that cannot notify is a session; a session
/// that would not start because a notification channel could not be set up is a
/// bug with a very unhappy user attached.
async fn notify_decorate(drive: &crate::drive::Drive, id: &str, env: &mut Env) {
    crate::notify::inject::decorate(&drive.paths, &drive.instance, Source::Claude, id, env).await;
}

// ------------------------------------------------------------------ operations

/// What a send did with the message.
#[derive(Debug, Clone, Copy, Default)]
pub struct Sent {
    /// The message went in behind a turn that is still running, and will be
    /// delivered when that turn ends.
    ///
    /// Only a resident can say this: a message to a busy session used to be a
    /// refusal, because a second `--print` child against one transcript is two
    /// agents writing over each other. One process taking messages in order has
    /// no such problem, so the composer no longer has to be blocked by a turn.
    pub queued: bool,
}

/// Hand a message to the CLI that owns an existing session.
pub async fn send_to_session(
    drive: &crate::drive::Drive,
    source: Source,
    id: &str,
    cwd: Option<&str>,
    text: &str,
    options: &SendOptions,
) -> Result<Sent, InjectError> {
    if source == Source::Claude {
        // Resolved before anything is built: an unusable account must not spawn.
        let (mut env, login) = resume_claude_env(drive, id, options).await?;
        notify_decorate(drive, id, &mut env).await;
        let recipe = claude_recipe(&drive.paths, cwd, options, env, login);
        // claude has no image flag, so every attachment reaches it as a path in
        // the prompt — which now travels over stdin rather than in argv.
        let prompt = with_claude_attachments(text, options);
        let delivery = drive.injections.deliver(id, &prompt, recipe, options.message_id.clone()).await?;
        return Ok(Sent { queued: delivery.queued });
    }

    // Resolved before anything is built, exactly as the claude branch does: an
    // account that cannot be read must not reach a spawn.
    let account = codex_account(drive, Some(id), options).await?;
    drive
        .injections
        .codex_deliver(Some(id), cwd, text, options, account)
        .await
        .map(|(_, sent)| sent)
}

/// Start a brand-new session in `cwd` and resolve once its canonical id is known.
pub async fn start_new_session(
    drive: &crate::drive::Drive,
    source: Source,
    cwd: &str,
    text: &str,
    options: &SendOptions,
) -> Result<String, InjectError> {
    if source == Source::Claude {
        // The local-login default for a claude spawn; an account overwrites it
        // below with a managed environment, which carries the marker too.
        let mut env = claude_env_bare();
        let id = new_uuid();
        let chosen = match &options.account {
            // The configured default (moved off a rest), or — with none, and
            // the local login known to be signed out — an account in the pool:
            // a chat or a scheduled task started there would fail its first turn.
            AccountChoice::Unset if options.use_default_account => {
                start_account(drive, AccountProvider::Claude, options.model.as_deref()).await
            }
            AccountChoice::Unset | AccountChoice::Local => None,
            AccountChoice::Account(id) => Some(id.clone()),
            AccountChoice::Rule(named) => rule_account(drive, AccountProvider::Claude, named, options.model.as_deref()).await,
        };
        match &chosen {
            Some(account_id) => {
                env = owned_claude_env(drive, Some(account_id)).await?;
                // Bound before the spawn: a crash mid-launch must not leave a
                // managed session nobody can resume.
                drive
                    .accounts
                    .bind_session_account(&id, Some(account_id), AccountProvider::Claude)
                    .await?;
            }
            // The local login writes into the user's own `~/.claude`, so this
            // session's own directory will say it belongs to nobody. It belongs
            // to us: we just made it, and refusing to send the *second* message
            // to a session this server started would be absurd. Recorded before
            // the spawn, like the binding above and for the same reason.
            None => {
                if let Err(error) = crate::sessions::owned_sessions::record(&drive.paths, source, &id).await {
                    tracing::error!(session = %id, %error, "cannot record this claude session as ours; it will list as external");
                }
            }
        }
        // Unlike a resume, a new session waits for the launch: there is no
        // session yet for a failure to be reported against. The resident's own
        // spawn is what is awaited — the id is ours either way, because a
        // session with no transcript is started with `--session-id`.
        notify_decorate(drive, &id, &mut env).await;
        let login = claude_login(drive, chosen).await;
        let recipe = claude_recipe(&drive.paths, Some(cwd), options, env, login);
        let prompt = with_claude_attachments(text, options);
        drive.injections.deliver(&id, &prompt, recipe, options.message_id.clone()).await?;
        return Ok(id);
    }

    let account = codex_account(drive, None, options).await?;
    let account_id = account.as_ref().map(|account| account.id.clone());
    let (id, _) = drive.injections.codex_deliver(None, Some(cwd), text, options, account).await?;
    // Codex has one config root and no way to isolate it, so ownership of a
    // thread is a record rather than a path — written the moment the CLI tells us
    // which thread it actually made. A failure here costs the session its
    // ownership (it lists as external) and nothing else, so it is logged rather
    // than turned into a refusal of a run that has already started.
    if let Err(error) = crate::sessions::owned_sessions::record(&drive.paths, Source::Codex, &id).await {
        tracing::error!(session = %id, %error, "cannot record this codex session as ours; it will list as external");
    }
    // Written beside the ownership record and for the same reason: the thread id
    // does not exist until `thread/start` answers, and the next message has to
    // find the same login this one ran under.
    if let Some(account_id) = account_id {
        if let Err(error) = drive.accounts.bind_session_account(&id, Some(&account_id), AccountProvider::Codex).await {
            tracing::error!(session = %id, %error, "cannot bind this codex session to its account; the next message will fall back to the default");
        }
    }
    Ok(id)
}

/// An account the start rule named for a pair no model-less start reaches
/// (`accountFrom: "start"`), checked again now: still the login a start naming
/// no model gets ([`crate::accounts::rotation::StartInputs::still_names`]) —
/// then it, logged as the rule's choice — or, if the rule has moved on since
/// the page asked, wherever the rule places this start's model now.
async fn rule_account(drive: &crate::drive::Drive, provider: AccountProvider, named: &str, model: Option<&str>) -> Option<String> {
    let inputs = crate::accounts::rotation::StartInputs::gather(&drive.accounts, &drive.quota, provider).await;
    if inputs.still_names(named) {
        tracing::info!(
            provider = provider.label(),
            to = %named,
            model = model.unwrap_or("-"),
            reason = "no login's own models land on that login: the start rule named it",
            "starting the new session on the account the start rule named"
        );
        return Some(named.to_string());
    }
    let fresh = inputs.place(model).account_id.0;
    tracing::info!(
        provider = provider.label(),
        named = %named,
        to = fresh.as_deref().unwrap_or("local login"),
        model = model.unwrap_or("-"),
        reason = "the start rule no longer names that account",
        "placing the new session where the start rule puts it now"
    );
    fresh
}

/// Where a start that names no account runs ([`crate::accounts::rotation::start_for`]),
/// and a line in the log whenever that is not simply the default or the local
/// login: which login the start was moved off, which it went to, and why.
async fn start_account(drive: &crate::drive::Drive, provider: AccountProvider, model: Option<&str>) -> Option<String> {
    use crate::accounts::rotation::StartReason;
    let start = crate::accounts::rotation::start_for(&drive.accounts, &drive.quota, provider, model).await;
    let to = start.account_id.0;
    let provider_name = provider.label();
    let default = match start.reason {
        StartReason::Rotated => drive.accounts.default_account_id(provider).await.unwrap_or_default(),
        _ => String::new(),
    };
    match start.reason {
        StartReason::Rotated => tracing::info!(
            provider = provider_name,
            from = %default,
            to = to.as_deref().unwrap_or("-"),
            model = model.unwrap_or("-"),
            reason = "the default account is resting or about to run out",
            "starting the new session on another account in the pool"
        ),
        StartReason::Fallback => tracing::info!(
            provider = provider_name,
            from = "local login",
            to = to.as_deref().unwrap_or("-"),
            model = model.unwrap_or("-"),
            reason = "no default account, and the machine's own login is signed out",
            "starting the new session on an account in the pool instead of the local login"
        ),
        StartReason::LocalSignedOut => tracing::info!(
            provider = provider_name,
            model = model.unwrap_or("-"),
            reason = "no default account, the machine's own login is signed out, and no account in the pool is free",
            "starting the new session on the signed-out local login; its first turn will say how to sign in"
        ),
        StartReason::Default | StartReason::Local | StartReason::LocalChosen => {}
    }
    to
}

impl InjectionRegistry {
    /// A CLI's own words, fit to be stored and shown.
    ///
    /// Both layers, in the order that matters: the exact stored credentials
    /// first, because those are known rather than guessed and two of the three
    /// kinds look like nothing a pattern could find; then the `sk-…` pattern,
    /// which catches a credential this server never stored — the user's own
    /// login out of `~/.claude`, a key somebody pasted into a prompt.
    pub(crate) fn redact(&self, text: &str) -> String {
        redact_secrets(&self.accounts.redact_known_secrets(text))
    }

    /// `recipe`, rebuilt to bill `to` instead of `from`.
    ///
    /// The command line and the working directory are the session's and stay as
    /// they are; the environment has `from`'s variables swapped for `to`'s (see
    /// [`swap_account_env`]) and the fingerprint names `to` and the credential
    /// it holds now, so the process this recipe starts is the one a message for
    /// `to` fits. A recipe that was not `from`'s to begin with, or a `from` the
    /// store no longer holds, is rebuilt from scratch the way the send path
    /// builds one instead.
    async fn retarget_recipe(&self, session: &str, recipe: &resident::Recipe, from: &str, to: &str) -> Result<resident::Recipe, InjectError> {
        let new = self.accounts.account_env(to).await?;
        require_provider(&new, AccountProvider::Claude)?;
        let old = match recipe.fingerprint.account.as_deref() {
            Some(account) if account == from => self.accounts.account_env(from).await.ok(),
            _ => None,
        };
        let env = match old {
            Some(old) => swap_account_env(&recipe.env, &old.env, &new.env, inherited_var),
            None => self.account_spawn_env(session, to).await?,
        };
        let login = self.login_of(Some(to.to_string())).await;
        let mut fingerprint = recipe.fingerprint.clone();
        fingerprint.account = login.account;
        fingerprint.credential = login.credential;
        fingerprint.resolved = None;
        Ok(resident::Recipe {
            args: recipe.args.clone(),
            env,
            workdir: recipe.workdir.clone(),
            fingerprint,
        })
    }

    /// A recipe for a session's process under `account`, from the settings it
    /// was running with, for a driver that was never handed one: a resident
    /// inherited across a restart that no message has described yet. Built the
    /// way the send path builds one — see [`claude_recipe`].
    async fn recipe_from_settings(
        &self,
        session: &str,
        live: &resident::Fingerprint,
        cwd: Option<&str>,
        account: &str,
    ) -> Result<resident::Recipe, InjectError> {
        let options = SendOptions {
            model: live.model.clone(),
            effort: live.effort.clone(),
            permission: live.permission.clone(),
            chat: live.chat,
            ..SendOptions::default()
        };
        let env = self.account_spawn_env(session, account).await?;
        let login = self.login_of(Some(account.to_string())).await;
        Ok(claude_recipe(&self.paths, cwd, &options, env, login))
    }

    /// The whole environment a spawn of `session` under `account` gets, built
    /// from nothing: the managed root, the account, the `rh-notify` decoration.
    async fn account_spawn_env(&self, session: &str, account: &str) -> Result<Env, InjectError> {
        let mut env = managed_claude_env(&self.paths, &self.accounts, Some(account)).await?;
        crate::notify::inject::decorate(&self.paths, &self.instance, Source::Claude, session, &mut env).await;
        Ok(env)
    }

    /// [`claude_login`], from the store this registry holds.
    async fn login_of(&self, account: Option<String>) -> ClaudeLogin {
        let credential = match account.as_deref() {
            Some(id) => self.accounts.credential_hash(id).await,
            None => None,
        };
        ClaudeLogin { account, credential }
    }

    /// Write a rotation decision down and tell everyone who should hear of it:
    /// the log line, the phone, the session's feed and the list.
    ///
    /// The one `info` line per decision is here, so every path that decides —
    /// a switch, nowhere to go, the cap — is logged the same way. Returns the
    /// record's id, for a switch that learns later how much it interrupted.
    fn record_rotation(&self, decision: &crate::accounts::rotation::Decision, notice: &str) -> Option<i64> {
        let id = match self.accounts.rotation_log().record(decision) {
            Ok(id) => Some(id),
            Err(error) => {
                tracing::warn!(session = %decision.session_id, %error, "cannot record an account rotation");
                None
            }
        };
        tracing::info!(
            session = %decision.session_id,
            from = %decision.from.id,
            to = %decision.to.id,
            class = decision.class.as_str(),
            scope = %decision.scope,
            until = decision.until_ms.map(crate::accounts::rest::iso_ms).as_deref().unwrap_or("-"),
            interrupted = decision.interrupted_tasks,
            outcome = decision.outcome.as_str(),
            trigger = decision.trigger.as_str(),
            used = ?decision.used_percent,
            "decided an account rotation"
        );
        // Known limitation: the failed turn behind this decision has
        // already sent its own "Run failed" notification a moment earlier, so
        // a limit reaches the phone twice — the failure, then what was done
        // about it. The second is the one that bypasses the throttle. A move
        // ahead of a limit has no failure before it, and reaches it once.
        // Every session on one account crosses the threshold together, so a
        // move ahead tends to come in a burst: N sessions from A to B within a
        // few minutes. One push per account pair per window says it; the rest
        // would only bury it. A limit-driven rotation always pushes.
        let ahead_burst = decision.trigger == crate::accounts::rotation::RotationTrigger::Ahead
            && !first_ahead_push(&decision.from.id, &decision.to.id, std::time::Instant::now());
        if !ahead_burst {
            crate::push::notify_rotation(
                self.paths.clone(),
                Source::Claude,
                decision.session_id.clone(),
                decision.outcome,
                decision.trigger,
                notice.to_string(),
            );
        }
        self.announce(Source::Claude, &decision.session_id);
        id
    }

    /// Write a limit a driver saw into this instance's limit log.
    ///
    /// Phase 1 of account rotation, and only that: whatever the driver then
    /// decides (phase 2), nothing here retries, switches or holds. The words
    /// go through both redaction layers here, before anything is stored, and
    /// the log line says what kind of limit it was and until when, never what
    /// the CLI said.
    ///
    /// Synchronous like every state-database write in this server, and only
    /// ever reached on a path where a CLI has already failed.
    pub(crate) fn record_limit(&self, mut observation: crate::accounts::rest::Observation) {
        use crate::accounts::rest;
        observation.raw = self.redact(&observation.raw);
        observation.reason = if observation.reason.is_empty() {
            crate::accounts::limits::reason_line(&observation.raw, observation.verdict.class)
        } else {
            self.redact(&observation.reason)
        };
        match self.accounts.limit_log().record(&observation, rest::now_ms()) {
            Ok(recorded) => tracing::info!(
                account = %observation.account_id,
                session = observation.session_id.as_deref().unwrap_or("-"),
                source = observation.source,
                class = observation.verdict.class.as_str(),
                scope = %observation.verdict.scope,
                until = recorded.until_ms.map(rest::iso_ms).as_deref().unwrap_or("-"),
                rested = recorded.rested,
                duplicate = recorded.duplicate,
                "recorded an account limit"
            ),
            Err(error) => tracing::warn!(
                account = %observation.account_id,
                source = observation.source,
                %error,
                "cannot record an account limit"
            ),
        }
    }

    /// A turn on this login went through, so its credential works: end a
    /// refused-credential (`auth`) rest it carries.
    ///
    /// Such a rest has no end of its own. The claude local login heals by
    /// reading (a reading newer than the refusal outweighs it,
    /// [`crate::accounts::local_login`]); codex has no reading of its login,
    /// and a stored account's rest only ended with a new credential. Without
    /// this, one passing refusal would mark a login signed out for good — and a
    /// signed-out local login hands every start that names no account to a
    /// stand-in ([`crate::accounts::rotation::start_choice`]).
    pub(crate) fn credential_accepted(&self, account_id: &str, session_id: &str) {
        use crate::accounts::limits::LimitClass;
        let log = self.accounts.limit_log();
        if log
            .active_observed_ms(account_id, LimitClass::Auth, crate::accounts::rest::now_ms())
            .is_none()
        {
            return;
        }
        match log.clear_class(account_id, LimitClass::Auth) {
            Ok(_) => tracing::info!(
                account = %account_id,
                session = %session_id,
                "a turn went through on a login resting on a refused credential; the rest is over"
            ),
            Err(error) => tracing::warn!(account = %account_id, %error, "cannot end a refused-credential rest"),
        }
    }
}

fn cli_binary(source: Source) -> &'static str {
    match source {
        Source::Claude => "claude",
        Source::Codex => "codex",
    }
}

fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

// -------------------------------------------------------------------- registry

/// One run's mutable state.
struct RunState {
    state: InjectionRunState,
    error: Option<String>,
    interrupted: bool,
    started_at: String,
    finished_at: Option<String>,
    /// `None` once the child has exited.
    pid: Option<u32>,
    /// What [`procs::start_token`] said about [`RunState::pid`] when the run was
    /// last known to be that process, so a signal can prove it still is.
    ///
    /// `None` on a run this server spawned, and that is not a gap: its CLI is
    /// still an unreaped child of this process, and the kernel cannot hand a pid
    /// out again before its parent has reaped it. Only an inherited pid — a
    /// number the previous server wrote in a file — needs the proof.
    pid_token: Option<String>,
    last_progress: Instant,
    idle_killed: bool,
    /// The session id the watchdog checks the transcript mtime of; a codex run
    /// re-points it once the real thread id is known.
    current_id: String,
    /// Identifies this run in `runs.json`, so completion removes its own entry.
    run_id: String,
    /// This run's cross-instance claim on the session, held for exactly as long
    /// as the CLI is running. `None` once it has finished, and on a run that
    /// never launched.
    lock: Option<lock::InjectLock>,
}

impl RunState {
    fn to_injection(&self) -> Injection {
        Injection {
            state: self.state,
            error: crate::openapi_ext::Nullable(self.error.clone()),
            interrupted: self.interrupted,
            started_at: self.started_at.clone(),
            finished_at: crate::openapi_ext::Nullable(self.finished_at.clone()),
        }
    }
}

/// A live or finished run.
pub struct Run {
    state: Mutex<RunState>,
    /// Flipped once the child has exited and the final state has been written.
    ///
    /// A `watch` rather than a `Notify`: `notify_waiters` stores no permit, so a
    /// waiter that arrives after the run finished — which is exactly what the
    /// codex handshake does when stdout closes first — would wait forever.
    finished: tokio::sync::watch::Sender<bool>,
}

impl Run {
    /// This run's state, in the shape the API speaks.
    pub fn injection(&self) -> Injection {
        self.state.lock().expect("run state poisoned").to_injection()
    }
}

/// Every run remote-hand started, keyed [`SessionRef::scoped_key`].
///
/// The TypeScript keeps two module-level maps. Here they are one registry that
/// lives in the application state, so two servers in one process do not kill
/// each other's children.
pub struct InjectionRegistry {
    /// The stored logins, for one job only: taking the credentials this server
    /// injected back out of whatever the CLI printed. See
    /// [`InjectionRegistry::redact`].
    accounts: Arc<crate::accounts::store::AccountsStore>,
    /// The quota readings, for one job only: every `rate_limit_event` a
    /// resident's claude writes is a reading of the login it runs on, and the
    /// driver files it here. The same handle [`crate::drive::Drive::quota`]
    /// serves the panel from, so a reading is on the panel the moment it lands.
    quota: Arc<crate::quota::QuotaCache>,
    /// This server's name, so the keys below can say whose they are.
    ///
    /// Only ever this process' own: everything keyed by it is in memory and
    /// dies with the registry. See [`InjectionRegistry::key_of`].
    server: ServerId,
    codex_servers: Mutex<HashMap<String, app_server::Handle>>,
    runs: Mutex<HashMap<String, Arc<Run>>>,
    /// The resident claude behind each session, keyed by session id alone —
    /// there is no codex resident, so the source is not part of the key.
    residents: Mutex<HashMap<String, resident::Handle>>,
    /// Serializes the gap between "no resident for this session" and "one in the
    /// map"; see [`InjectionRegistry::deliver`].
    spawn_gate: tokio::sync::Mutex<()>,
    /// Where a resident's shim leaves its socket, stderr tail and tombstone.
    shim_dir: PathBuf,
    /// How long a codex run may make no progress before it is ended.
    idle_timeout: Duration,
    /// How long a claude turn may make no progress before it is *marked*
    /// stalled. Nothing is ever killed for it; see [`resident`].
    stall_after: Duration,
    /// How long each rung of a resident's interrupt ladder waits for the turn to
    /// end before the next signal.
    ///
    /// The ladder starts polite — a `control_request` that ends the turn and
    /// leaves the process alive — and escalates only because a wedged CLI cannot
    /// answer it. Five seconds is well past what a healthy claude takes to write
    /// the turn's `result`, and short enough that a Stop button still feels like
    /// one.
    interrupt_step: Duration,
    /// How often the watchdog checks.
    watchdog_interval: Duration,
    /// How long a settings change may be parked behind claude's background work
    /// before the process is restarted anyway; `None` to wait for ever.
    ///
    /// A ceiling rather than a policy: parking is right, and the tasks a
    /// resident exists to protect can legitimately run for an hour. What this
    /// stops is the *unbounded* case — a background set that is wrong about
    /// work which is not there, holding a message nobody can stop or take back.
    /// See [`resident::Driver::waited_long_enough`].
    await_background: Option<Duration>,
    /// `DROPPEN_ROTATE_AHEAD_PERCENT`: the used percent of a window at which a
    /// session is moved off its account at the next quiet point, before the
    /// limit refuses a turn; `None` when that is switched off. See
    /// [`resident::Driver::rotate_ahead`].
    rotate_ahead: Option<f64>,
    /// Where claude drops a background task's output file.
    ///
    /// The CLI writes one `<task-id>.output` per background task under
    /// `<root>/<cwd-slug>/<session-id>/tasks/`, and streams into it while the
    /// task runs — so for a `local_bash` task, which has no subagent transcript
    /// anywhere, this is the *only* place on disk that says it is still alive.
    /// [`resident::Driver::subagents_recently_active`] reads it for exactly that
    /// reason.
    ///
    /// A root rather than the directory itself because the slug in the middle is
    /// the working directory with its separators and its non-ASCII mangled, and
    /// reproducing that encoding is a guess; looking for the session id one level
    /// down is not. Overridable so a test can point it at a directory it owns.
    task_root: PathBuf,
    /// How long a resident may sit idle before it is retired, or `None` — the
    /// default — for never.
    ///
    /// Off by default because the resident model's whole promise is that the
    /// next message goes into a process that is already there and that nothing
    /// running in the background is ever cut off, and both are weakened by
    /// ending idle processes. It exists because a resident costs three or four
    /// hundred megabytes, and a machine with enough sessions open needs a way to
    /// say so. See [`resident::Driver::idle_shutdown`].
    resident_idle: Option<Duration>,
    /// How long the recovery ladder waits before each restart of a resident
    /// that died unexpectedly: one entry per death inside the breaker's window,
    /// the last reused past the end.
    ///
    /// A field rather than the constant it defaults to so a test can walk the
    /// whole ladder — resume, fork, breaker — without sitting out real
    /// backoffs; production always runs [`resident::RECOVERY_BACKOFF`].
    recovery_backoff: [Duration; 3],
    /// How long a turn opened for a *steered* message is given to prove it
    /// exists before the driver concludes claude answered the message inside
    /// the turn it was handed to.
    ///
    /// The whole of the merged/deferred ambiguity is timed by this, so it is a
    /// field for the same reason [`Self::recovery_backoff`] is: a test has to
    /// be able to watch both outcomes without sitting out the real one.
    /// Production runs [`resident::STEER_SETTLE`], or `DROPPEN_STEER_SETTLE_MS`.
    steer_settle: Duration,
    /// How often a resident holding steered messages looks in the transcript
    /// for claude having taken them into the turn in flight.
    ///
    /// The backstop for a session nobody is watching: a watched one is told the
    /// moment its transcript grows — see [`InjectionRegistry::transcript_grew`].
    /// Production runs [`resident::ABSORB_POLL`], or `DROPPEN_ABSORB_POLL_MS`.
    absorb_poll: Duration,
    /// Where transcripts are discovered, for the idle watchdog.
    paths: Arc<Paths>,
    /// The record of in-flight runs, in this server's own instance directory.
    journal: runs::RunsFile,
    /// Where the per-session claims live: the **shared** data directory, so a
    /// second instance contends with this one rather than ignoring it.
    lock_dir: PathBuf,
    /// Where a session's flight recording goes, if it is being made at all.
    ///
    /// A factory rather than a recorder: one is opened per *resident*, lives on
    /// its driver, and is what carries the session's life into
    /// [`crate::record`]. See [`resident::Driver::recorder`].
    recorders: record::Recorders,
    /// This server's own instance directory, for the one thing a driver builds
    /// an environment for by itself: a process for the account an automatic
    /// rotation moved its session to, which [`crate::notify::inject`] decorates
    /// exactly as the send path does.
    instance: InstanceDirs,
    /// The session feed and the list feed, told when a driver changes something
    /// no file shows. Set once, at startup; empty in tests that build no app.
    listeners: std::sync::OnceLock<Listeners>,
    /// When a quota reading last woke the residents holding a queue; see
    /// [`InjectionRegistry::recheck_holds`].
    reading_recheck: Mutex<Option<Instant>>,
    /// The idempotency claims and receipts of messages a client named, in
    /// this instance's database. The drivers report into it by queue id; see
    /// [`receipts`].
    receipts: Arc<receipts::Receipts>,
    /// The questions claude stopped to ask, in this instance's database. The
    /// drivers open and close them; the answer routes compare-and-set them.
    /// See [`questions`].
    questions: Arc<questions::Questions>,
    /// What each resident claude is thinking right now, as a running count;
    /// written by the drivers, read by the session feeds. See [`drafts`].
    drafts: drafts::Drafts,
}

/// Whoever follows a session from outside the driver: the per-session feed and
/// the session list.
struct Listeners {
    feeds: Arc<crate::feed::FeedRegistry>,
    global: Arc<crate::feed::global::GlobalFeed>,
}

/// A duration from the environment, in milliseconds, or the fallback; under
/// its old name too ([`crate::env::var`]).
fn env_ms(name: &str, fallback: Duration) -> Duration {
    crate::env::var(name)
        .and_then(|raw| raw.parse::<f64>().ok())
        .filter(|ms| ms.is_finite() && *ms > 0.0)
        .map(Duration::from_secs_f64_millis)
        .unwrap_or(fallback)
}

trait DurationFromMillis {
    fn from_secs_f64_millis(ms: f64) -> Duration;
}
impl DurationFromMillis for Duration {
    fn from_secs_f64_millis(ms: f64) -> Duration {
        Duration::from_secs_f64(ms / 1000.0)
    }
}

impl InjectionRegistry {
    /// A registry with the timeouts the environment asks for.
    ///
    /// Runs are killed for being *stuck*, never for taking long: a healthy
    /// agentic run can legitimately work for hours, and a wall-clock cap would
    /// execute it mid-task.
    ///
    /// `instance` says where `runs.json` goes, and it is the only per-instance
    /// thing here: the session claims deliberately live one level up, in the
    /// data directory every instance shares.
    pub fn new(
        paths: Arc<Paths>,
        instance: &InstanceDirs,
        accounts: Arc<crate::accounts::store::AccountsStore>,
        quota: Arc<crate::quota::QuotaCache>,
    ) -> Arc<Self> {
        let receipts = Arc::new(receipts::Receipts::new(crate::db::at(paths.instance_db())));
        let questions = Arc::new(questions::Questions::new(crate::db::at(paths.instance_db())));
        Arc::new(Self {
            receipts,
            questions,
            accounts,
            quota,
            server: ServerId::local(&paths),
            codex_servers: Mutex::new(HashMap::new()),
            runs: Mutex::new(HashMap::new()),
            residents: Mutex::new(HashMap::new()),
            spawn_gate: tokio::sync::Mutex::new(()),
            shim_dir: instance.session_shims(),
            idle_timeout: env_ms("DROPPEN_IDLE_TIMEOUT_MS", Duration::from_secs(20 * 60)),
            // Ten minutes of silence is a *report*, not a sentence: the only
            // thing that happens at this threshold is that the turn is marked.
            stall_after: env_ms("DROPPEN_STALL_MS", Duration::from_secs(10 * 60)),
            interrupt_step: env_ms("DROPPEN_INTERRUPT_STEP_MS", Duration::from_secs(5)),
            watchdog_interval: env_ms("DROPPEN_WATCHDOG_INTERVAL_MS", Duration::from_secs(60)),
            // Half an hour: past every background job short enough that waiting
            // is the obvious answer, and well short of a session that is stuck.
            // Zero means "wait for ever", which is why this does not go through
            // `env_ms` either.
            await_background: match crate::env::var("DROPPEN_AWAIT_BACKGROUND_MS")
                .and_then(|raw| raw.trim().parse::<u64>().ok())
            {
                Some(0) => None,
                Some(ms) => Some(Duration::from_millis(ms)),
                None => Some(Duration::from_secs(30 * 60)),
            },
            rotate_ahead: crate::accounts::rotation::ahead_threshold_from_env(),
            // `/tmp`, not `TMPDIR`: claude writes here, and on macOS `TMPDIR` is
            // a per-process directory under `/var/folders` that it never uses.
            task_root: crate::env::var("DROPPEN_TASK_ROOT").map(PathBuf::from).unwrap_or_else(|| {
                // SAFETY: `getuid` reads the calling process' own uid and cannot fail.
                PathBuf::from(format!("/tmp/claude-{}", unsafe { libc::getuid() }))
            }),
            // Zero, absent and unparseable all mean "never", which is why this
            // is the one timing here that does not go through `env_ms`: its
            // fallback is not a duration.
            resident_idle: crate::env::var("DROPPEN_RESIDENT_IDLE_MS")
                .and_then(|raw| raw.trim().parse::<u64>().ok())
                .filter(|ms| *ms > 0)
                .map(Duration::from_millis),
            recovery_backoff: resident::RECOVERY_BACKOFF,
            steer_settle: env_ms("DROPPEN_STEER_SETTLE_MS", resident::STEER_SETTLE),
            absorb_poll: env_ms("DROPPEN_ABSORB_POLL_MS", resident::ABSORB_POLL),
            journal: runs::RunsFile::new(instance.runs_file()),
            lock_dir: paths.data_dir().join("inject-locks"),
            recorders: record::Recorders::new(paths.clone(), instance),
            instance: instance.clone(),
            listeners: std::sync::OnceLock::new(),
            reading_recheck: Mutex::new(None),
            drafts: drafts::Drafts::default(),
            paths,
        })
    }

    /// Give the drivers someone to tell. Called once, when the application
    /// state is assembled; a second call is ignored.
    pub fn attach_listeners(&self, feeds: Arc<crate::feed::FeedRegistry>, global: Arc<crate::feed::global::GlobalFeed>) {
        let _ = self.listeners.set(Listeners { feeds, global });
    }

    /// Tell the session's feed and the session list that it changed in a way
    /// no file shows: its account binding and its rotation record, today.
    pub(crate) fn announce(&self, source: Source, id: &str) {
        let Some(listeners) = self.listeners.get() else { return };
        listeners.feeds.poke(&self.key_of(source, id));
        listeners.global.request_refresh(Some((source, id)));
    }

    /// Ask the session list, and nothing else, to look again.
    ///
    /// The other narrow half of [`InjectionRegistry::announce`], for a change
    /// the session's own view must hear about in a particular order: a resident
    /// that took a steered message off its queue on the transcript's word pokes
    /// the feed only once that feed has shown the transcript — see
    /// `resident::Driver::on_absorb` — while the list, which shows no
    /// transcript, can be told straight away.
    pub(crate) fn refresh_list(&self) {
        let Some(listeners) = self.listeners.get() else { return };
        listeners.global.request_refresh(None);
    }

    /// Push one session's state to whoever is watching it, and nothing else.
    ///
    /// The narrow half of [`InjectionRegistry::announce`], for a change only
    /// the session's own view shows — a message's receipt — and that the list
    /// has no column for.
    pub(crate) fn poke_feed(&self, source: Source, id: &str) {
        let Some(listeners) = self.listeners.get() else { return };
        listeners.feeds.poke(&self.key_of(source, id));
    }

    /// Follow what a claude session's resident has said and its transcript does
    /// not show yet, or `None` for codex, which has no such window to cover.
    ///
    /// Answers whether or not a resident is running: a page may be open on a
    /// session long before anything is sent to it, and has to be holding the
    /// channel the driver will write into when something is.
    pub fn draft_watch(&self, source: Source, id: &str) -> Option<tokio::sync::watch::Receiver<drafts::Draft>> {
        (source == Source::Claude).then(|| self.drafts.subscribe(&self.key_of(source, id)))
    }

    /// The channel claude session `id`'s draft is published on.
    ///
    /// Its driver's [`drafts::DraftWriter`] is the only thing that writes to it
    /// in a running server; a test of the feed stands in for that driver here.
    pub(crate) fn draft_channel(&self, id: &str) -> Arc<tokio::sync::watch::Sender<drafts::Draft>> {
        self.drafts.channel(&self.key_of(Source::Claude, id))
    }

    /// The writer a driver for session `id` publishes its draft through.
    fn draft_writer(&self, id: &str) -> drafts::DraftWriter {
        drafts::DraftWriter::new(self.draft_channel(id))
    }

    /// The idempotency claims and receipts of messages a client named.
    pub fn receipts(&self) -> &Arc<receipts::Receipts> {
        &self.receipts
    }

    /// The questions claude stopped to ask, and what became of them.
    pub fn questions(&self) -> &Arc<questions::Questions> {
        &self.questions
    }

    /// A driver's word on what became of the message travelling as `qid`.
    ///
    /// Cheap for a message nobody named — the update finds no row — and a
    /// poke of the session's feed for one somebody did, so the receipt reaches
    /// the page without waiting for the next poll.
    pub(crate) fn note_receipt(&self, source: Source, session: &str, qid: &str, status: receipts::ReceiptStatus) {
        if self.receipts.mark(qid, status, session, receipts::now_ms()) {
            self.poke_feed(source, session);
        }
    }

    /// Whether this server's driver for a session still holds `qid` in its
    /// queue: the evidence a claim stranded by a crash was enqueued after all.
    pub fn holds_queued(&self, source: Source, id: &str, qid: &str) -> bool {
        self.resident_info(source, id)
            .is_some_and(|info| info.queued_messages.iter().any(|message| message.id == qid))
    }

    /// The key one session's run, resident or feed entry is filed under here.
    ///
    /// [`SessionRef::scoped_key`], so the server is part of it. Every map it
    /// keys is in this process' memory and dies with it — nothing durable and
    /// nothing on the wire is spelled this way, which is exactly what makes the
    /// prefix free.
    pub(super) fn key_of(&self, source: Source, id: &str) -> String {
        SessionRef::new(self.server.clone(), source, id).scoped_key()
    }

    /// The state of the run for this session, if remote-hand ever started one.
    pub fn injection_for(&self, source: Source, id: &str) -> Option<Injection> {
        if source == Source::Codex {
            if let Some(handle) = self.codex_handle(id) {
                return handle.injection();
            }
        }
        self.runs
            .lock()
            .expect("injections poisoned")
            .get(&self.key_of(source, id))
            .map(|run| run.injection())
    }

    /// Whether a run is in flight for this session.
    pub fn is_running(&self, source: Source, id: &str) -> bool {
        self.injection_for(source, id)
            .map(|i| i.state == InjectionRunState::Running)
            .unwrap_or(false)
    }

    /// Everything the session list needs from this subsystem, in one pass.
    ///
    /// Taken once per listing rather than asked per row: a list of two hundred
    /// sessions would otherwise take the residents lock and the runs lock four
    /// hundred times, against a registry every send and every driver contends
    /// on. Both registries are bounded by sessions this server process has
    /// driven, rather than by transcript or list rows, so the snapshot is cheap
    /// to make and cheap to carry.
    pub fn driver_states(&self) -> crate::sessions::DriverStates {
        // Both sources are filed under `key_of` here. They were not: claude
        // residents went in under the bare session id and codex ones under
        // `codex/{id}`, so the snapshot was the one map in the process that
        // could not be read with a single key. Nothing outside
        // `crate::sessions::DriverStates::resident` ever looked, which is what
        // made the asymmetry safe to remove.
        let mut residents: HashMap<_, _> = self
            .residents
            .lock()
            .expect("residents poisoned")
            .iter()
            .map(|(id, handle)| (self.key_of(Source::Claude, id), handle.info()))
            .collect();
        for (id, handle) in self.codex_servers.lock().unwrap().iter().filter(|(_, h)| h.visible()) {
            residents.insert(self.key_of(Source::Codex, id), handle.info());
        }
        let runs = self.runs.lock().expect("injections poisoned");
        let mut running = std::collections::HashSet::new();
        let mut finished = std::collections::HashMap::new();
        for (id, handle) in self.codex_servers.lock().unwrap().iter() {
            if let Some(injection) = handle.injection() {
                if injection.state == InjectionRunState::Running {
                    running.insert(self.key_of(Source::Codex, id));
                } else if let Some(at) = injection.finished_at.0 {
                    finished.insert(self.key_of(Source::Codex, id), at);
                }
            }
        }
        for (key, run) in runs.iter() {
            let injection = run.injection();
            if injection.state == InjectionRunState::Running {
                running.insert(key.clone());
            } else if let Some(at) = injection.finished_at.0 {
                finished.insert(key.clone(), at);
            }
        }
        crate::sessions::DriverStates::new(self.server.clone(), residents, running, finished)
    }

    /// This server's resident for one session, as the API reports it.
    pub fn resident_info(&self, source: Source, id: &str) -> Option<crate::sessions::ResidentInfo> {
        if source == Source::Codex {
            return self.codex_handle(id).filter(app_server::Handle::visible).map(|h| h.info());
        }
        if source != Source::Claude {
            return None;
        }
        Some(self.residents.lock().expect("residents poisoned").get(id)?.info())
    }

    /// Take a message out of a session's queue before it is written to the CLI.
    ///
    /// Both resident protocols hold durable queues. Legacy exec runs do not.
    pub async fn drop_queued_message(&self, source: Source, id: &str, qid: &str) -> QueueOutcome {
        if source == Source::Codex {
            return self.codex_queue_action(id, qid, false).await;
        }
        if source != Source::Claude {
            return QueueOutcome::NoResident;
        }
        self.drop_queued(id, qid).await
    }

    /// Hand a queued message to claude now, ahead of the turn in flight.
    ///
    /// See [`resident::Driver::steer`] for what that means to the CLI and why
    /// it is a delivery rather than an interruption.
    pub async fn steer_queued_message(&self, source: Source, id: &str, qid: &str) -> QueueOutcome {
        if source == Source::Codex {
            return self.codex_queue_action(id, qid, true).await;
        }
        if source != Source::Claude {
            return QueueOutcome::NoResident;
        }
        self.steer_queued(id, qid).await
    }

    /// Stop the turn remote-hand started. `false` when nothing is running.
    ///
    /// A resident is asked rather than killed: the first rung of its ladder is
    /// a `control_request` that ends the *turn* and leaves the process — and
    /// anything it started in the background — alive. Only a CLI that will not
    /// answer gets signalled, and only by the driver that can tell whether the
    /// turn ended in between.
    ///
    /// For a one-shot codex run there is no such distinction, and the old rule
    /// applies: a pid that no longer names the run it was recorded for is *not*
    /// signalled, and the run is closed out as finished instead — see
    /// [`vanished`] for why that is the only safe direction.
    pub fn interrupt(&self, source: Source, id: &str) -> bool {
        if source == Source::Codex {
            if let Some(h) = self.codex_handle(id).filter(app_server::Handle::alive) {
                return h.interrupt();
            }
        }
        if self.has_resident(source, id) {
            return self.interrupt_resident(id);
        }
        // The handle is cloned out so the map lock is released first: the
        // fail-closed path below rewrites `runs.json` and unlinks a claim, and
        // neither belongs under a lock every route contends on.
        let run = self.runs.lock().expect("injections poisoned").get(&self.key_of(source, id)).cloned();
        let Some(run) = run else { return false };
        let mut state = run.state.lock().expect("run state poisoned");
        if state.state != InjectionRunState::Running {
            return false;
        }
        let Some(pid) = state.pid else { return false };
        if !procs::is_same_process(pid, state.pid_token.as_deref()) {
            tracing::info!(pid, session = %id, source = %source, "the run's pid is no longer its CLI; ending it instead of signalling");
            vanished(&mut state, source, self.idle_timeout);
            let claim = release(self, &mut state);
            drop(state);
            drop(claim);
            let _ = run.finished.send(true);
            return false;
        }
        state.interrupted = true;
        drop(state);
        signal(pid, libc::SIGTERM);
        true
    }

    /// Ask claude to stop one background task, leaving the turn alone.
    ///
    /// The narrow counterpart to [`Injections::interrupt`], and deliberately not
    /// reachable the same way: Stop ends the *turn*, which is a thing this
    /// server started and can therefore take back, while a background task is a
    /// child claude is minding on its own clock. The CLI's `stop_task` control
    /// request is the only thing that can end one of those without ending the
    /// process, so a session with no resident — every codex run, and a claude
    /// session this server is not driving — has no way to do it at all rather
    /// than a worse way.
    pub fn stop_background_task(&self, source: Source, id: &str, task_id: &str) -> StopTaskOutcome {
        if source != Source::Claude || !self.has_resident(source, id) {
            return StopTaskOutcome::NoResident;
        }
        self.stop_resident_task(id, task_id)
    }
}

/// The refusal a claimed session produces, naming the holder when it can.
///
/// The wording of the first clause is the one the in-process map has always
/// produced, deliberately: from the composer's point of view the two are the
/// same condition, and it is the reason the send was refused, not the mechanism
/// that noticed, that belongs in a message to the user.
fn busy_message(holder: Option<u32>) -> String {
    match holder {
        Some(pid) => format!("a message is already running for this session (Droppen pid {pid})"),
        None => "a message is already running for this session".to_string(),
    }
}

// ---------------------------------------------------------------- adoption

/// What one boot's adoption pass found.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Adopted {
    /// Runs whose CLI is still working, now tracked again.
    pub recovered: usize,
    /// Records whose CLI is gone, cleaned up.
    pub finished: usize,
    /// Records left for a later boot, because another instance holds the
    /// session right now. Pathological, and logged when non-zero.
    pub deferred: usize,
}

impl Adopted {
    /// Whether the pass found nothing at all, which is the ordinary case: a
    /// server that stopped with no run in flight leaves nothing to adopt.
    pub fn is_empty(&self) -> bool {
        self.recovered == 0 && self.finished == 0 && self.deferred == 0
    }
}

/// What became of one resident record on the way through adoption.
enum Reattached {
    /// Its stdio is back and a driver has it.
    Driving,
    /// The process is alive but unreachable; the claim comes back out to be
    /// held by a pid watcher instead.
    Watching(lock::InjectLock),
    /// There was nothing left to attach to.
    Buried,
}

impl InjectionRegistry {
    /// Re-attach to every run the previous server left behind.
    ///
    /// Must run **after** the instance flock and **before** the listener: a
    /// `GET /api/sessions/...` answered from a half-scanned `runs.json` would
    /// report a live run as absent and unblock the composer, which is the exact
    /// double-spawn this whole mechanism exists to prevent. It is gated on
    /// ownership for the same reason terminal adoption is — `export-openapi`
    /// runs a throwaway server in-process, and a throwaway server must not
    /// claim a real one's runs.
    pub async fn adopt_all(self: &Arc<Self>) -> Adopted {
        let mut report = Adopted::default();
        for record in self.journal.read() {
            if !procs::is_same_process(record.cli_pid, record.pid_token.as_deref()) {
                if record.codex.is_some() {
                    self.codex_dead(record);
                } else {
                    self.bury(&record).await;
                }
                report.finished += 1;
                continue;
            }
            // The dead server's claim died with its descriptor, so this
            // normally succeeds. It failing means some *other* live server is
            // running against the same session — which can only happen if this
            // instance died and another took the session over while it was
            // down. Adopting on top of that would be a second tracker for one
            // session, so the record is left for a later boot instead: the pid
            // check above is what stops it lingering for ever.
            let claim = match lock::acquire(&self.lock_dir, record.source, &record.session_id) {
                Ok(lock::Outcome::Acquired(claim)) => claim,
                Ok(lock::Outcome::Busy { holder }) => {
                    tracing::warn!(
                        session = %record.session_id,
                        source = %record.source,
                        pid = record.cli_pid,
                        ?holder,
                        "another server is already running this session; not adopting"
                    );
                    report.deferred += 1;
                    continue;
                }
                Err(error) => {
                    tracing::warn!(%error, session = %record.session_id, "cannot claim a session to adopt it");
                    report.deferred += 1;
                    continue;
                }
            };
            // A resident is *re-attached to*, not watched: the shim held the
            // process' stdio open across the restart, so this server gets the
            // conversation back rather than just the knowledge that a pid
            // exists. A `codex exec` child has no such thing and is watched.
            if record.resident || record.codex.is_some() {
                match self.clone().reattach(&record, claim).await {
                    Reattached::Driving => report.recovered += 1,
                    Reattached::Watching(claim) => {
                        self.clone().resume(record, claim);
                        report.recovered += 1;
                    }
                    Reattached::Buried => report.finished += 1,
                }
                continue;
            }
            self.clone().resume(record, claim);
            report.recovered += 1;
        }
        // A question can only be answered into the turn that asked it, and that
        // turn is a process: one that did not come back with its session can
        // never take the answer. Those are expired here, once every survivor is
        // in the map, and stay answerable as an ordinary message. A survivor's
        // own questions are its driver's to settle when its replay ends.
        let held: std::collections::HashSet<String> = self.residents.lock().expect("residents poisoned").keys().cloned().collect();
        let expired = self.questions.expire_unheld(&held, questions::now_ms());
        if expired > 0 {
            tracing::info!(
                expired,
                "questions whose process did not survive the restart can no longer be answered in their turn"
            );
        }
        report
    }

    /// Get a resident's stdio back, or work out why that is not possible.
    ///
    /// The three answers are the three states a shim can be in when its server
    /// comes back, and each is a different amount of bad news:
    ///
    /// * the socket answers and claude is alive → the session is *driveable*
    ///   again, exactly as if this server had started it;
    /// * the socket is gone or will not talk, but the pid is still there → the
    ///   process is running and unreachable. The claim is kept and the pid is
    ///   watched, because the one thing that must not happen is a second claude
    ///   against a transcript the first is still writing;
    /// * the shim is minding a corpse, or the pid went while we were asking →
    ///   there is nothing to attach to, and the record is buried.
    async fn reattach(self: Arc<Self>, record: &runs::RunRecord, claim: lock::InjectLock) -> Reattached {
        let Some(shim_id) = record.shim.as_deref() else {
            // A resident record with no shim named cannot be dialled at all.
            // Only reachable from a record written by a server old enough to
            // predate the field, which is to say never in practice.
            tracing::warn!(session = %record.session_id, "a resident record names no shim; watching its pid instead");
            return Reattached::Watching(claim);
        };
        let sidecars = shim::Sidecars::new(self.shim_dir.clone(), shim_id);
        match shim::attach(&sidecars).await {
            Ok(link) => {
                if record.source == Source::Codex {
                    self.codex_attached(record.clone(), link, claim);
                } else {
                    self.attached(record, sidecars, link, claim);
                }
                Reattached::Driving
            }
            // A tombstone, or a pid that went during the handshake. The process
            // is over; what is left is to say how it ended, which the shim wrote
            // down before it closed the stream.
            Err(shim::LinkError::Corpse(why)) => {
                tracing::info!(session = %record.session_id, %why, "the resident named by this record is already dead");
                drop(claim);
                self.bury_resident(record, &sidecars).await;
                Reattached::Buried
            }
            Err(error) => {
                if !procs::is_same_process(record.cli_pid, record.pid_token.as_deref()) {
                    tracing::info!(session = %record.session_id, %error, "the resident named by this record has gone");
                    drop(claim);
                    self.bury_resident(record, &sidecars).await;
                    return Reattached::Buried;
                }
                // The socket is unreachable and the process is not. Everything
                // the shim was for is lost — the turn's end, its output, the
                // ability to send anything at all — but the session stays
                // claimed, which is the half that must not wait: an untracked
                // resident reads as a session with nothing in flight, so the
                // composer opens and the next message starts a second claude
                // against the transcript the first one is still writing.
                tracing::warn!(
                    session = %record.session_id,
                    pid = record.cli_pid,
                    shim = %shim_id,
                    %error,
                    "cannot re-attach to a resident that is still running; holding its session and watching the pid"
                );
                Reattached::Watching(claim)
            }
        }
    }

    /// Clean up after a resident that did not survive, tombstone and all.
    ///
    /// The generic [`InjectionRegistry::bury`] asks the transcript whether the
    /// turn was cut short, because for a `--print` child that is the only
    /// evidence there is. A resident leaves better: the shim's job on the way
    /// out is to write down how claude ended, and that note is worth more than
    /// an inference — so it is used where it exists and the sidecars are swept
    /// up behind it.
    async fn bury_resident(&self, record: &runs::RunRecord, sidecars: &shim::Sidecars) {
        if record.codex.is_some() {
            self.codex_dead(record.clone());
            return;
        }
        if let Some(exit) = sidecars.read_tombstone() {
            tracing::info!(session = %record.session_id, ?exit, "the resident's shim recorded how it ended");
        }
        self.bury(record).await;
        let _ = std::fs::remove_file(sidecars.exit());
        let _ = std::fs::remove_file(sidecars.stderr());
    }

    /// Put an inherited run back in the map and start watching its pid.
    fn resume(self: Arc<Self>, record: runs::RunRecord, claim: lock::InjectLock) {
        let (finished_tx, _) = tokio::sync::watch::channel(false);
        let run = Arc::new(Run {
            state: Mutex::new(RunState {
                state: InjectionRunState::Running,
                error: None,
                interrupted: false,
                // The original timestamp, so an adopted run does not claim to
                // have started when the server restarted.
                started_at: record.started_at.clone(),
                finished_at: None,
                pid: Some(record.cli_pid),
                // Carried out of the record so that every later signal can make
                // the same check adoption just made, rather than trusting a
                // number whose meaning expires the moment the CLI exits.
                pid_token: record.pid_token.clone(),
                // The pipes died with the process that opened them. There is no
                // way to recover what was already written, and no way to read
                // what comes next: the CLI's stdout now goes nowhere.
                // The old clock is unrecoverable — `Instant` has no meaning
                // across processes — so the run is given a full idle grace
                // period from now. Erring towards patience: killing a healthy
                // run is worse than noticing a stuck one late.
                last_progress: Instant::now(),
                idle_killed: false,
                current_id: record.session_id.clone(),
                run_id: record.run_id.clone(),
                lock: Some(claim),
            }),
            finished: finished_tx,
        });
        self.runs
            .lock()
            .expect("injections poisoned")
            .insert(self.key_of(record.source, &record.session_id), run.clone());
        tracing::info!(
            session = %record.session_id,
            source = %record.source,
            pid = record.cli_pid,
            "adopted a run that outlived the last server"
        );
        tokio::spawn(watch_adopted(run, record, self));
    }

    /// Deal with a record whose CLI is gone.
    ///
    /// The record's mere presence says the run had not finished when the last
    /// server wrote the file, but not how it ended: the CLI may have run to
    /// completion after the server died, or been killed along with it. Only the
    /// transcript can tell the two apart, so that is what is asked — and only
    /// when it says the turn was cut short does anything surface, because a
    /// spurious failure on every restart would be worse than silence.
    async fn bury(&self, record: &runs::RunRecord) {
        self.journal.remove(&record.run_id);
        // Taken and immediately dropped: a lock file whose run is over is
        // debris, and leaving it would keep the session claimed by nobody.
        if let Ok(lock::Outcome::Acquired(stale)) = lock::acquire(&self.lock_dir, record.source, &record.session_id) {
            drop(stale);
        }
        if !self.was_cut_short(record).await {
            tracing::info!(session = %record.session_id, "a run finished while the server was down");
            return;
        }

        let key = self.key_of(record.source, &record.session_id);
        {
            let runs = self.runs.lock().expect("injections poisoned");
            // A run started since boot outranks a note about a dead one.
            if runs.contains_key(&key) {
                return;
            }
        }
        let (finished_tx, _) = tokio::sync::watch::channel(true);
        let run = Arc::new(Run {
            state: Mutex::new(RunState {
                // The existing failure path, with a softer reason than usual —
                // rather than a new state the web app would have to learn.
                state: InjectionRunState::Failed,
                error: Some(RESTART_NOTE.to_string()),
                interrupted: false,
                started_at: record.started_at.clone(),
                finished_at: Some(iso8601(std::time::SystemTime::now())),
                pid: None,
                pid_token: None,
                last_progress: Instant::now(),
                idle_killed: false,
                current_id: record.session_id.clone(),
                run_id: record.run_id.clone(),
                lock: None,
            }),
            finished: finished_tx,
        });
        self.runs.lock().expect("injections poisoned").insert(key, run);
        tracing::info!(session = %record.session_id, source = %record.source, "a run was cut short by a restart");
    }

    /// Whether the session's transcript ends mid-turn.
    ///
    /// Both CLIs already have a "is this session in the middle of something"
    /// reading, derived from the tail of the transcript, and it is exactly the
    /// question here: a turn that ended with the assistant's answer is a turn
    /// that completed, whereas one ending on the user's message or a tool call
    /// never got its reply. Reusing them keeps one definition of mid-turn in
    /// the codebase rather than two that can drift.
    async fn was_cut_short(&self, record: &runs::RunRecord) -> bool {
        match record.source {
            Source::Claude => match claude::find_claude_session_file(&self.paths, &record.session_id).await {
                // No transcript at all: the CLI died before writing a line of
                // the turn it was given.
                None => true,
                Some(path) => claude::claude_activity_status(&path).await == SessionStatus::Running,
            },
            Source::Codex => match codex::find_codex_session(&self.paths, &record.session_id).await {
                None => true,
                // Codex reads `Idle` for a transcript nothing has touched
                // lately, which after a long outage is every transcript. That
                // makes the answer conservative rather than wrong: silence,
                // where claude would have said something.
                Some(session) => session.status == SessionStatus::Running,
            },
        }
    }
}

/// Watch an adopted run's pid until it exits, re-arming the idle watchdog.
///
/// Unlike a newly spawned child, an adopted process has no `Child` handle:
/// completion is a poll rather than a `wait`, and the idle watchdog has only
/// the transcript's mtime to go on — the "the CLI printed something" half of
/// progress is gone with the pipes.
///
/// A resident claude reaches here too, until it can be re-attached through its
/// shim, and the watchdog is skipped for it. Not as a caution: a resident is the
/// *session*, so "quiet for twenty minutes" is what a session between messages
/// looks like, and signalling one would end every conversation on the machine
/// that nobody happened to be talking to. See [`resident::Driver::watchdog`] for
/// why a live resident is only ever marked, never reaped.
async fn watch_adopted(run: Arc<Run>, record: runs::RunRecord, registry: Arc<InjectionRegistry>) {
    let mut since_watchdog = Instant::now();
    loop {
        tokio::time::sleep(ADOPTED_POLL).await;

        if !procs::is_same_process(record.cli_pid, record.pid_token.as_deref()) {
            break;
        }
        if !record.resident && record.codex.is_none() && since_watchdog.elapsed() >= registry.watchdog_interval {
            since_watchdog = Instant::now();
            if let Some(pid) = check_idle(&run, record.source, &registry.paths, registry.idle_timeout).await {
                // Checked again rather than relying on the check at the top of
                // the loop: `check_idle` reads the transcript's mtime, and a pid
                // can stop being this run across that await.
                if !procs::is_same_process(pid, record.pid_token.as_deref()) {
                    tracing::info!(pid, session = %record.session_id, "the idle watchdog's pid is no longer its CLI; not signalling");
                    // Nothing was ended by remote-hand, so the run must not go
                    // on to say it was. The loop's own exit condition — the pid
                    // is gone — is what has actually happened.
                    run.state.lock().expect("run state poisoned").idle_killed = false;
                    break;
                }
                signal(pid, libc::SIGTERM);
            }
        }
    }

    let mut state = run.state.lock().expect("run state poisoned");
    vanished(&mut state, record.source, registry.idle_timeout);
    let claim = if record.codex.is_some() {
        state.lock.take()
    } else {
        release(&registry, &mut state)
    };
    drop(state);
    drop(claim);
    if record.codex.is_some() {
        registry.codex_dead(record);
    }
    let _ = run.finished.send(true);
}

/// Signal one process, having already established that it is the right one.
///
/// Every caller owes that proof: [`procs::is_same_process`] must agree that the pid is still
///   the process its record named — the check
///   [`InjectionRegistry::interrupt`] and [`watch_adopted`] make immediately
///   before calling this.
///
/// Reaching here without one of those is how a Stop button ends a stranger's
/// build, so the check belongs at the call site where the provenance is known,
/// not behind a flag passed down to here.
fn signal(pid: u32, sig: libc::c_int) {
    // SAFETY: a signal to a single pid whose identity the caller has just
    // established; the worst outcome is an `ESRCH`, which is ignored.
    unsafe { libc::kill(pid as libc::pid_t, sig) };
}

/// Give up everything a finished run was holding: its record, and its claim.
///
/// The order matters in one direction — the record goes before the claim,
/// because a claim released first would let another instance start a run this
/// server is still advertising as in flight.
///
/// The claim is handed back rather than dropped here so the caller can release
/// the state lock before its unlink runs.
#[must_use = "the claim is released when this is dropped, which should happen outside the state lock"]
fn release(registry: &InjectionRegistry, state: &mut RunState) -> Option<lock::InjectLock> {
    registry.journal.remove(&state.run_id);
    state.lock.take()
}

/// Write the ending of a run whose pid stopped being its CLI.
///
/// Two places reach this, and they must not word the same fact differently or
/// the state would flicker between them: [`watch_adopted`]'s poll, which is how
/// an inherited run normally ends, and [`InjectionRegistry::interrupt`] when the
/// pid fails the identity guard.
///
/// Failing closed there is the asymmetry the whole pid-reuse guard rests on. A
/// signal cannot be taken back: `SIGTERM` to a recycled pid ends a stranger's
/// process, and no later check undoes it. Declining to signal a run that has in
/// fact already ended costs nothing at all — the run is over either way, and
/// saying so here is the truth rather than a guess.
fn vanished(state: &mut RunState, source: Source, idle_timeout: Duration) {
    state.pid = None;
    state.finished_at = Some(iso8601(std::time::SystemTime::now()));
    // There is no exit status to read: the process that could have reaped this
    // child is gone, and pid 1 got the code. So the run is reported as having
    // ended, which is the one thing actually known, and the transcript says
    // what it achieved. Calling it a failure would paint every successful
    // adopted run red.
    if state.interrupted {
        state.state = InjectionRunState::Failed;
        state.error = Some("interrupted from Droppen".to_string());
    } else if state.idle_killed {
        state.state = InjectionRunState::Failed;
        state.error = Some(format!(
            "{} stopped making progress for {} minutes, so Droppen ended it",
            cli_binary(source),
            (idle_timeout.as_secs_f64() / 60.0).round() as u64
        ));
    } else {
        state.state = InjectionRunState::Ok;
    }
}

/// Node reports signals by name (`SIGTERM`), and the message is user-facing.
fn signal_name(signal: i32) -> String {
    match signal {
        libc::SIGTERM => "SIGTERM".to_string(),
        libc::SIGKILL => "SIGKILL".to_string(),
        libc::SIGINT => "SIGINT".to_string(),
        libc::SIGHUP => "SIGHUP".to_string(),
        libc::SIGSEGV => "SIGSEGV".to_string(),
        other => format!("signal {other}"),
    }
}

/// Whether the run has gone quiet for too long; returns the pid to end.
///
/// Progress is either CLI output (codex exec --json streams events) or growth of
/// the session transcript, which both CLIs write continuously — that is what
/// the live feed renders.
async fn check_idle(run: &Run, source: Source, paths: &Paths, idle_timeout: Duration) -> Option<u32> {
    let current_id = run.state.lock().expect("run state poisoned").current_id.clone();
    let transcript_progress = transcript_progress(source, paths, &current_id).await;

    let mut state = run.state.lock().expect("run state poisoned");
    if let Some(progress) = transcript_progress {
        if progress > state.last_progress {
            state.last_progress = progress;
        }
    }
    if state.last_progress.elapsed() < idle_timeout {
        return None;
    }
    state.idle_killed = true;
    state.pid
}

/// The transcript's last write, expressed on the same clock as `last_progress`.
async fn transcript_progress(source: Source, paths: &Paths, id: &str) -> Option<Instant> {
    let path = match source {
        Source::Claude => claude::find_claude_session_file(paths, id).await,
        // Codex discovery is the sibling agent's; against the current stub this
        // is always `None`, which falls back to output activity exactly as the
        // TypeScript does for a session with no file yet.
        Source::Codex => codex::find_codex_session(paths, id).await.map(|s| std::path::PathBuf::from(s.file_path)),
    }?;
    let modified = tokio::fs::metadata(&path).await.ok()?.modified().ok()?;
    let age = std::time::SystemTime::now().duration_since(modified).ok()?;
    Instant::now().checked_sub(age)
}

/// How long one push stands for every other move ahead between the same two
/// accounts.
const AHEAD_PUSH_WINDOW: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Whether this move ahead from `from` to `to` is the first one in the current
/// [`AHEAD_PUSH_WINDOW`], and so the one that gets a push.
fn first_ahead_push(from: &str, to: &str, now: std::time::Instant) -> bool {
    static LAST: std::sync::OnceLock<std::sync::Mutex<HashMap<(String, String), std::time::Instant>>> = std::sync::OnceLock::new();
    let mut last = LAST.get_or_init(Default::default).lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    last.retain(|_, at| now.saturating_duration_since(*at) < AHEAD_PUSH_WINDOW);
    let key = (from.to_string(), to.to_string());
    if last.contains_key(&key) {
        return false;
    }
    last.insert(key, now);
    true
}

#[cfg(test)]
mod tests {
    /// A burst of moves ahead between the same two accounts pushes once; a
    /// different pair, or the same pair after the window, pushes again.
    #[test]
    fn a_burst_of_moves_ahead_between_two_accounts_pushes_once() {
        let t0 = std::time::Instant::now();
        assert!(super::first_ahead_push("burst-a", "burst-b", t0));
        assert!(!super::first_ahead_push("burst-a", "burst-b", t0 + std::time::Duration::from_secs(60)));
        assert!(super::first_ahead_push("burst-a", "burst-c", t0 + std::time::Duration::from_secs(61)));
        assert!(super::first_ahead_push(
            "burst-a",
            "burst-b",
            t0 + super::AHEAD_PUSH_WINDOW + std::time::Duration::from_secs(1)
        ));
    }

    use super::*;

    /// A rotation keeps everything a session's environment holds but the
    /// account: the old account's variables go (back to the server's own value
    /// where it has one), the new one's come in, the rest is untouched.
    #[test]
    fn swapping_accounts_changes_only_the_account_in_an_environment() {
        let pair = |key: &str, value: &str| (key.to_string(), value.to_string());
        let env: Env = [
            pair("PATH", "/session-bin:/usr/bin"),
            pair("CLAUDE_CONFIG_DIR", "/data/instances/default/claude"),
            pair("DROPPEN_NOTIFY_TOKEN", "minted-for-this-session"),
            pair("ANTHROPIC_AUTH_TOKEN", "relay-token"),
            pair("ANTHROPIC_BASE_URL", "https://relay.example"),
            pair("HTTPS_PROXY", "http://account-proxy"),
        ]
        .into_iter()
        .collect();
        let old = [
            pair("ANTHROPIC_AUTH_TOKEN", "relay-token"),
            pair("ANTHROPIC_BASE_URL", "https://relay.example"),
            pair("HTTPS_PROXY", "http://account-proxy"),
        ];
        let new = [pair("CLAUDE_CODE_OAUTH_TOKEN", "oauth-token")];
        let server = |key: &str| (key == "HTTPS_PROXY").then(|| "http://server-proxy".to_string());

        let swapped = swap_account_env(&env, &old, &new, server);
        let expected: Env = [
            pair("PATH", "/session-bin:/usr/bin"),
            pair("CLAUDE_CONFIG_DIR", "/data/instances/default/claude"),
            pair("DROPPEN_NOTIFY_TOKEN", "minted-for-this-session"),
            pair("HTTPS_PROXY", "http://server-proxy"),
            pair("CLAUDE_CODE_OAUTH_TOKEN", "oauth-token"),
        ]
        .into_iter()
        .collect();
        assert_eq!(swapped, expected);
    }

    fn options() -> SendOptions {
        SendOptions::default()
    }

    fn paths() -> Paths {
        Paths::new(std::path::PathBuf::from("/home/me"))
    }

    /// The prompt is no longer on the command line at all — it goes in over
    /// stdin — and what is left is exactly the settings a resident is pinned
    /// to. Both halves matter: a prompt that leaked back into argv would make
    /// every message a new process again, and a settings flag that went missing
    /// would silently run the turn under the wrong model.
    #[test]
    fn a_resident_carries_its_settings_and_never_the_prompt() {
        let mut options = options();
        options.model = Some("opus".into());
        options.effort = Some("high".into());
        options.permission = Some("acceptEdits".into());
        options.images = vec!["/uploads/a.png".into()];
        let args = claude_resident_flags(&paths(), &options);
        assert_eq!(
            args,
            [
                "--model",
                "opus",
                "--effort",
                "high",
                "--permission-mode",
                "acceptEdits",
                "--permission-prompt-tool",
                "stdio",
                "--add-dir",
                "/home/me/.local/share/remote-hand/uploads",
                "--disallowedTools",
                "EnterPlanMode",
                "ExitPlanMode",
            ]
        );
        assert!(!args.iter().any(|arg| arg == "--"), "nothing positional is left to guard");
        assert!(!args.iter().any(|arg| arg.contains("hello")), "the prompt goes over stdin");
        // And the attachment still reaches claude, through the prompt text the
        // driver writes rather than through a flag.
        assert!(with_claude_attachments("hello", &options).starts_with("hello\n\n[1 attached file(s)"));
    }

    #[test]
    fn chat_flags_replace_the_persona_and_disable_every_tool() {
        let mut options = options();
        options.chat = true;
        let args = claude_resident_flags(&paths(), &options);
        assert!(args.contains(&"--disallowedTools".to_string()));
        assert_eq!(args.iter().filter(|a| *a == "*").count(), 1);
        assert!(args.contains(&CHAT_SYSTEM_PROMPT.to_string()));
        // The variadic flags are last, which is what replaces the `--` guard:
        // with no positional argument there is nothing left for them to eat.
        assert_eq!(args.last().expect("a flag"), "*");
        // One `--disallowedTools`, not a second for the plan tools: `*` covers
        // them, and two of a variadic flag is a question for the CLI's parser
        // nobody needs to ask.
        assert_eq!(args.iter().filter(|a| *a == "--disallowedTools").count(), 1);
    }

    /// The prompt tool is what lets claude ask a question at all, and the plan
    /// tools go with it: `EnterPlanMode` runs unasked and only a permission
    /// prompt — which a resident always refuses — leads back out.
    ///
    /// Recorded in the fingerprint, and never the reason for a restart: a
    /// resident started before the flag existed takes a message that wants it,
    /// and picks the flag up the next time it is replaced for a reason of its
    /// own.
    #[test]
    fn every_resident_can_ask_and_none_can_shut_itself_in_plan_mode() {
        for chat in [false, true] {
            let recipe = claude_recipe(&paths(), None, &SendOptions { chat, ..options() }, Env::new(), ClaudeLogin::default());
            assert!(recipe.args.windows(2).any(|pair| pair == ["--permission-prompt-tool", "stdio"]));
            assert!(recipe.fingerprint.questions);
            let disallowed = recipe.args.iter().position(|arg| arg == "--disallowedTools").expect("a deny list");
            let listed = &recipe.args[disallowed + 1..];
            assert!(
                listed.iter().all(|arg| !arg.starts_with("--")),
                "the variadic list is the end of the line: {listed:?}"
            );
            if chat {
                assert_eq!(listed, ["*"]);
            } else {
                assert_eq!(listed, ["EnterPlanMode", "ExitPlanMode"]);
            }
        }
        let wants = claude_recipe(&paths(), None, &options(), Env::new(), ClaudeLogin::default()).fingerprint;
        let older = resident::Fingerprint {
            questions: false,
            ..wants.clone()
        };
        assert!(wants.satisfied_by(&older), "a resident from before the flag is not restarted for it");
    }

    #[test]
    fn uploads_are_a_startup_capability_not_a_per_message_setting() {
        let paths = paths();
        for chat in [false, true] {
            let plain = SendOptions { chat, ..options() };
            let first = claude_recipe(&paths, None, &plain, Env::new(), ClaudeLogin::default());
            assert!(first.fingerprint.uploads);
            assert!(first
                .args
                .windows(2)
                .any(|pair| pair == ["--add-dir", "/home/me/.local/share/remote-hand/uploads"]));
            for attached in [
                SendOptions {
                    images: vec!["/uploads/a.png".into()],
                    ..plain.clone()
                },
                SendOptions {
                    files: vec!["/uploads/a.txt".into()],
                    ..plain.clone()
                },
            ] {
                let next = claude_recipe(&paths, None, &attached, Env::new(), ClaudeLogin::default());
                assert_eq!(first.args, next.args);
                assert_eq!(first.fingerprint, next.fingerprint);
            }
        }
    }

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

    #[test]
    fn the_attachment_suffix_is_what_the_transcript_parser_reads_back() {
        let suffix = attachment_suffix(&["/tmp/a.png".to_string(), "/tmp/b.txt".to_string()]);
        let split = crate::sessions::util::parse_attachment_suffix(&format!("hi{suffix}"));
        assert_eq!(split.text, "hi");
        assert_eq!(split.images, ["/tmp/a.png"]);
        assert_eq!(split.files[0].path, "/tmp/b.txt");
    }

    #[test]
    fn the_three_account_states_are_distinguishable() {
        assert_eq!(AccountChoice::from_request(None), AccountChoice::Unset);
        assert_eq!(AccountChoice::from_request(Some(None)), AccountChoice::Local);
        assert_eq!(AccountChoice::from_request(Some(Some("x".into()))), AccountChoice::Account("x".into()));
        assert_eq!(AccountChoice::Local.named(), None, "null is not an account");
        assert_eq!(
            AccountChoice::from_start_request(Some(Some("x".into())), Some(AccountFrom::Start)),
            AccountChoice::Rule("x".into()),
            "an account the start rule named is the rule's, not a pick"
        );
        assert_eq!(AccountChoice::Rule("x".into()).named(), None, "and nothing binds it as somebody's choice");
        assert_eq!(AccountChoice::from_start_request(Some(Some("x".into())), None), AccountChoice::Account("x".into()));
        assert_eq!(AccountChoice::from_start_request(Some(None), Some(AccountFrom::Start)), AccountChoice::Local);
    }

    #[test]
    fn an_environment_override_only_wins_when_it_is_a_positive_number() {
        assert_eq!(env_ms("DROPPEN_DEFINITELY_UNSET_TIMEOUT", Duration::from_secs(7)), Duration::from_secs(7));
    }

    /// Stop must never signal a pid that has stopped being the run it named.
    ///
    /// The window this closes is real and routine rather than exotic: an
    /// adopted run has no `Child` to be woken by, so [`watch_adopted`] only
    /// learns its CLI has exited on a two-second poll, and the kernel is free
    /// to hand that pid to anything at all in between. Pressing Stop inside
    /// those two seconds used to `SIGTERM` whatever now holds the number.
    ///
    /// The run is put in place through [`InjectionRegistry::resume`] rather
    /// than through a boot, because [`InjectionRegistry::adopt_all`] makes the
    /// same identity check and would never have adopted this record: the state
    /// under test is one that was adopted while its CLI was alive and lost it
    /// afterwards, which is the only way a live entry can point at a stranger.
    #[tokio::test]
    async fn stop_never_signals_a_pid_that_is_no_longer_the_run_it_named() {
        const SESSION: &str = "eeeeeeee-5555-4555-8555-eeeeeeeeeeee";

        let home = tempfile::tempdir().expect("tempdir");
        let paths = Arc::new(Paths::new(home.path().to_path_buf()));
        let dirs = InstanceDirs::new(&paths.data_dir(), crate::instance::DEFAULT_INSTANCE).expect("valid name");
        let registry = InjectionRegistry::new(
            paths.clone(),
            &dirs,
            Arc::new(crate::accounts::store::AccountsStore::new(paths.clone())),
            Arc::new(crate::quota::QuotaCache::new(paths)),
        );

        // Alive, and emphatically not an agent run: the developer's `make`,
        // standing in for whatever the recycled pid actually belongs to.
        let mut stranger = std::process::Command::new("/bin/sh")
            .args(["-c", "sleep 30"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn a stand-in");

        let lock::Outcome::Acquired(claim) = lock::acquire(&registry.lock_dir, Source::Claude, SESSION).expect("claim") else {
            panic!("a free session must be claimable");
        };
        registry.clone().resume(
            runs::RunRecord {
                codex: None,
                codex_account: None,
                hold: None,
                run_id: "run-under-test".to_string(),
                source: Source::Claude,
                session_id: SESSION.to_string(),
                cwd: None,
                cli_pid: stranger.id(),
                // A start time no process alive today can have. This is exactly
                // what the record of a run whose pid has been recycled looks
                // like, and the only part of it that can tell.
                started_at: "2026-01-01T00:00:00.000Z".to_string(),
                pid_token: Some("1.000000".to_string()),
                resident: false,
                shim: None,
                fingerprint: None,
                turn_at: None,
                queued: Vec::new(),
            },
            claim,
        );
        assert!(registry.is_running(Source::Claude, SESSION), "the run is in flight before Stop");

        assert!(
            !registry.interrupt(Source::Claude, SESSION),
            "a pid that fails the identity guard has nothing to interrupt"
        );

        // The assertion the whole fix exists for.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            stranger.try_wait().expect("try_wait").is_none(),
            "a SIGTERM reached a process this server never spawned"
        );

        // And the user is not left with a run that says it is still going and a
        // Stop button that will never end it.
        let seen = registry.injection_for(Source::Claude, SESSION).expect("the run is still reported");
        assert_eq!(seen.state, InjectionRunState::Ok, "a run whose pid is gone has ended, not failed");
        assert!(!seen.interrupted, "nothing was interrupted, so nothing may claim to have been");
        assert!(seen.finished_at.0.is_some(), "and it has an end time");

        let _ = stranger.kill();
        let _ = stranger.wait();
    }

    /// A working directory that has gone missing falls back to somewhere named,
    /// and never to nothing.
    ///
    /// `None` would leave the child inheriting the server's own directory, which
    /// on a developer's machine is the checkout of the server itself. The
    /// fallback has to be a path this code chose.
    #[test]
    fn a_session_whose_directory_is_gone_runs_at_home_rather_than_wherever_the_server_is() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        let real = home.path().join("workspace");
        std::fs::create_dir_all(&real).expect("workspace");

        assert_eq!(
            claude_workdir(&paths, Some(&real.to_string_lossy())),
            real,
            "a directory that exists is used"
        );
        assert_eq!(
            claude_workdir(&paths, Some(&home.path().join("deleted").to_string_lossy())),
            home.path(),
            "one that does not falls back to home"
        );
        assert_eq!(claude_workdir(&paths, None), home.path(), "and so does a session with none recorded");
        // A file is not a directory, and `current_dir` on one fails the spawn.
        let file = home.path().join("not-a-dir");
        std::fs::write(&file, b"x").expect("file");
        assert_eq!(claude_workdir(&paths, Some(&file.to_string_lossy())), home.path());
    }
}
