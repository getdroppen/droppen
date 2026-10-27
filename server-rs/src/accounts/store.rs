//! The account store: several Claude logins, kept server-side.
//!
//! Port of the former TypeScript server's `accounts.ts`. A secret never leaves
//! this module: everything the API returns goes through [`Account`], and only
//! [`AccountsStore::account_env`] reads the raw value — to hand it to a spawn as
//! an environment variable. Two narrow accessors publish *about* a credential
//! without publishing it: [`AccountsStore::env_values`], for the named
//! configuration variables of a block, and [`AccountsStore::credential_hash`],
//! for a name saying which credential is stored rather than what it is.
//!
//! # Where the accounts live
//!
//! In [`crate::db`], at `<config>/state.db`. `accounts.json` was a mirror for
//! one version — `0600`, tmp+rename, key names and pretty printing kept
//! compatible with the TypeScript store — so that a build rolled back onto this
//! machine would still find the logins. The rollback floor has since risen to a
//! build that reads the database, and a second copy of every OAuth refresh token
//! is not a thing to keep on disk for a reason that no longer exists. Nothing
//! reads the file any more.
//!
//! [`crate::auth::users`] keeps its file and this one does not, which looks like
//! an inconsistency and is not: that store's copy is a *second opinion*, because
//! an empty users table means authentication is switched off. An empty accounts
//! table means the user has not added a login yet, which is an ordinary answer,
//! so nothing here needs a vote from a file.
//!
//! The split is the interesting part. Accounts are rows in `accounts`, and
//! `sessionAccounts` is a table of its own, because the two have nothing in
//! common but a file. There are two or three accounts and they change when a
//! person types a token; there are thirty-five bindings and there is one more
//! every time a session starts, for ever. In the file, pinning a session to an
//! account re-encoded and re-`fsync`ed **every account's OAuth refresh token**,
//! which is a great deal of writing, and a great deal of exposure, to record
//! that a conversation belongs to a login. It is now one `INSERT` of two short
//! strings, and nothing else on the machine moves.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use gotcha::Schematic;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::limits::LimitClass;
use super::rest::{AccountRest, LimitLog};
use crate::db::{self, Db};
use crate::openapi_ext::{Nullable, Optional};
use crate::sessions::{iso8601, Paths};

/// Where the one setting in this store that is not a row lives.
///
/// `defaultAccountId` is a single machine-wide scalar — "which login do new
/// sessions follow" — and a scalar is what `kv` is for. It keeps its own row so
/// that changing it does not rewrite an account, which is the same reason the
/// bindings got a table.
const SETTINGS_SCOPE: &str = "accounts";
const DEFAULT_KEY: &str = "default-account-id";
/// The same scalar for the other CLI. A second key rather than a second column
/// on one row: "which login do new codex threads follow" is its own question,
/// and answering it must not rewrite the claude answer beside it.
const DEFAULT_CODEX_KEY: &str = "default-codex-account-id";
/// What either default key holds when somebody chose the machine's own login.
///
/// Not the absence of a row: no row is "nobody chose", and a start that nobody
/// pointed anywhere may take a stand-in when the local login is signed out
/// ([`super::rotation::start_choice`]). A chosen local login is never replaced.
/// Never a uuid, so it cannot be mistaken for an account; [`load_file`] drops it
/// from `default_account_id` like any value naming no account.
const LOCAL_DEFAULT: &str = "local";

/// The environment variable a codex account's key is read from.
///
/// Not `OPENAI_API_KEY` and not `CODEX_API_KEY`, because `codex app-server`
/// reads neither: it authenticates from `CODEX_HOME/auth.json` and nothing
/// else. The one lever left that keeps `~/.codex` — and therefore every session
/// this server discovers — exactly where it is, is a *custom model provider*
/// whose key comes from an environment variable of our choosing. This is that
/// variable, and [`codex_provider_args`] is what tells codex to look at it.
///
/// `REMOTE_HAND_CODEX_API_KEY` before the rename to Droppen, and renamed
/// outright, with no fallback: the spawn that sets it is the spawn that names
/// it (`env_key`), so nothing else ever reads it, and a `codex` started under
/// the old name keeps the configuration it was started with.
pub const CODEX_API_KEY_ENV: &str = "DROPPEN_CODEX_API_KEY";
/// The id the injected provider is registered under in codex's config.
pub const CODEX_PROVIDER_ID: &str = "remote_hand";
/// Where a codex account talks when it names no base URL of its own.
pub const CODEX_DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
/// What `ANTHROPIC_BASE_URL` is called, for a claude account that names one.
pub const CLAUDE_BASE_URL_ENV: &str = "ANTHROPIC_BASE_URL";

/// The `-c` overrides that point one `codex` spawn at an account's own key.
///
/// Inline TOML on the command line, which is why [`normalize_base_url`] refuses
/// a quote, a backslash and anything that is not printable: the base URL is
/// interpolated into the table below, and a value that could close the string
/// would be an injection into the CLI's own configuration.
///
/// `wire_api = "responses"` because that is the API the OpenAI-compatible
/// endpoints codex speaks to expose; `name` is only ever shown to a human.
pub fn codex_provider_args(base_url: &str) -> Vec<String> {
    vec![
        "-c".to_string(),
        format!(
            "model_providers.{CODEX_PROVIDER_ID}={{name=\"remote-hand\",base_url=\"{base_url}\",env_key=\"{CODEX_API_KEY_ENV}\",wire_api=\"responses\"}}"
        ),
        "-c".to_string(),
        format!("model_provider={CODEX_PROVIDER_ID}"),
    ]
}

/// Which CLI an account's credential is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "snake_case")]
pub enum AccountProvider {
    /// Claude Code.
    Claude,
    /// Codex.
    Codex,
}

impl Default for AccountProvider {
    /// Every account stored before this field existed is a Claude login.
    fn default() -> Self {
        AccountProvider::Claude
    }
}

impl AccountProvider {
    /// The CLI this provider's sessions belong to.
    pub fn source(self) -> crate::sessions::Source {
        match self {
            AccountProvider::Claude => crate::sessions::Source::Claude,
            AccountProvider::Codex => crate::sessions::Source::Codex,
        }
    }

    /// The provider whose accounts drive this CLI.
    pub fn of(source: crate::sessions::Source) -> Self {
        match source {
            crate::sessions::Source::Claude => AccountProvider::Claude,
            crate::sessions::Source::Codex => AccountProvider::Codex,
        }
    }

    /// What this provider's default account is filed under in `kv`.
    fn default_key(self) -> &'static str {
        match self {
            AccountProvider::Claude => DEFAULT_KEY,
            AccountProvider::Codex => DEFAULT_CODEX_KEY,
        }
    }

    /// The name a refusal calls this CLI by.
    pub fn label(self) -> &'static str {
        match self {
            AccountProvider::Claude => "claude",
            AccountProvider::Codex => "codex",
        }
    }
}

/// How an account authenticates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    /// A `claude setup-token` OAuth token, minted by the wizard.
    OauthToken,
    /// A block of environment variables the user typed, injected as they are.
    ///
    /// One free-form box rather than a field per credential, because the field
    /// per credential was always a guess at somebody's setup. A gateway wants a
    /// token and a base URL; Bedrock wants four variables and no token at all;
    /// a relay wants a key under a name nothing here would have thought of.
    /// What every one of them has in common is that the answer is "these
    /// variables, in the child's environment", so that is what is stored.
    Env,
}

/// The variable a Claude OAuth account's token is handed over in.
pub const CLAUDE_OAUTH_ENV: &str = "CLAUDE_CODE_OAUTH_TOKEN";
/// The variables a codex block may carry its key under, in the order tried.
pub const CODEX_KEY_CANDIDATES: [&str; 2] = ["OPENAI_API_KEY", "CODEX_API_KEY"];
/// The variable a codex block may carry its endpoint under.
pub const CODEX_BASE_URL_VAR: &str = "OPENAI_BASE_URL";

/// One `KEY=VALUE` pair of an env-mode account, in the order it was typed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVar {
    /// The variable's name. Shown back to the user; never a secret.
    pub key: String,
    /// Its value. Part of no list response and never logged; read back only by
    /// the explicit [`AccountsStore::env_block`] route the editor calls.
    pub value: String,
}

/// Variables a block may not set, because this server sets them.
///
/// Not a security boundary — the user already chooses what the CLI runs — but
/// the difference between "my token did not work" and "remote-hand cannot find
/// my sessions any more". `CLAUDE_CONFIG_DIR` and `CODEX_HOME` decide where a
/// transcript lands, which is the whole of how a session is discovered and
/// resumed; `HOME` and `PATH` move everything at once.
const DENIED_ENV_KEYS: [&str; 10] = [
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "HOME",
    "PATH",
    "NODE_OPTIONS",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "DYLD_FRAMEWORK_PATH",
];
/// At most this many variables in one block.
const MAX_ENV_VARS: usize = 64;
/// At most this much text in one block.
const MAX_ENV_BYTES: usize = 8 * 1024;

/// Read a `KEY=VALUE`-per-line block the way a `.env` file is read.
///
/// Deliberately *not* a dotenv implementation. No interpolation, no escapes, no
/// multi-line values: this text is going straight into a child's environment,
/// and every feature that rewrites a value is a way for the thing that ends up
/// there to differ from the thing that was typed. Quotes are stripped because
/// people paste them out of shell snippets, and that is the whole of it.
///
/// Refuses rather than skips, everywhere it can: a line that is not a comment,
/// not blank and not a pair is a typo, and silently dropping it would store a
/// block that is missing the variable the user thinks they set.
pub fn parse_env_block(text: &str) -> Result<Vec<EnvVar>, AccountError> {
    // A block pasted out of a Windows editor may open with a BOM; the browser's
    // `trim()` drops it, ours must too or line 1 is refused for a byte nobody sees.
    let text = text.trim_start_matches('\u{feff}');
    if text.len() > MAX_ENV_BYTES {
        return Err(AccountError::Invalid(format!(
            "the environment block is larger than {} KiB",
            MAX_ENV_BYTES / 1024
        )));
    }
    let mut parsed: Vec<EnvVar> = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let number = index + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // `export FOO=bar` is what a shell snippet looks like, and pasting one
        // in is the most likely way this box gets filled.
        let line = line.strip_prefix("export ").map(str::trim_start).unwrap_or(line);
        // Neither message quotes the text it refused. A `400` body is read by
        // the browser, by every log in front of it and by whatever the client
        // does with an error string — and the likeliest thing on a line that is
        // not a pair is the credential itself, pasted before its name was typed.
        let Some((key, value)) = line.split_once('=') else {
            return Err(AccountError::Invalid(format!("line {number} is not a KEY=VALUE pair")));
        };
        let key = key.trim();
        if !is_env_name(key) {
            return Err(AccountError::Invalid(format!("line {number} does not start with a variable name")));
        }
        if DENIED_ENV_KEYS.contains(&key) {
            return Err(AccountError::Invalid(format!(
                "{key} is set by Droppen itself; setting it here would move where this session's transcript lives"
            )));
        }
        if parsed.iter().any(|var| var.key == key) {
            return Err(AccountError::Invalid(format!("{key} is set twice; keep one of them")));
        }
        if parsed.len() == MAX_ENV_VARS {
            return Err(AccountError::Invalid(format!("more than {MAX_ENV_VARS} variables in one block")));
        }
        parsed.push(EnvVar {
            key: key.to_string(),
            value: unquote(value.trim()).to_string(),
        });
    }
    if parsed.is_empty() {
        return Err(AccountError::Invalid("the environment block is empty".to_string()));
    }
    Ok(parsed)
}

/// One stored variable as a line [`parse_env_block`] reads back unchanged.
///
/// A value with no surrounding whitespace needs no quoting. One that has it
/// does: the parser trims a line's value, so an unquoted ` x ` would come back
/// as `x`. Quoting only applies when both ends agree because that is the only
/// shape [`unquote`] strips.
fn render_env_var(var: &EnvVar) -> String {
    if var.value.trim() != var.value {
        format!("{}=\"{}\"", var.key, var.value)
    } else {
        format!("{}={}", var.key, var.value)
    }
}

/// `"x"` and `'x'` become `x`; anything else is returned as it stands.
///
/// No escape processing inside, on purpose: a backslash in a pasted token is a
/// backslash, and "helpfully" unescaping it would hand the CLI a value nobody
/// typed.
fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote) {
            return &value[1..value.len() - 1];
        }
    }
    value
}

/// `^[A-Za-z_][A-Za-z0-9_]*$`, spelled out.
fn is_env_name(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else { return false };
    (first.is_ascii_alphabetic() || first == '_') && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The block as one canonical string, for the duplicate check.
///
/// Sorted, so that the same variables typed in another order are the same
/// credential — which they are, to every CLI that reads them.
fn block_sha256(env: &[EnvVar]) -> String {
    let mut lines: Vec<String> = env.iter().map(|var| format!("{}={}", var.key, var.value)).collect();
    lines.sort();
    sha256_hex(&lines.join("\n"))
}

/// Whether this CLI can be driven by this kind of credential at all.
///
/// Codex is the narrow one, and not by choice: `codex app-server` reads no
/// OAuth token from anywhere but `auth.json`, and there is no codex wizard here
/// to mint one. A key in a block is the only credential remote-hand can inject.
pub fn validate_pairing(provider: AccountProvider, kind: AccountKind) -> Result<(), AccountError> {
    match (provider, kind) {
        (AccountProvider::Claude, _) | (AccountProvider::Codex, AccountKind::Env) => Ok(()),
        (AccountProvider::Codex, AccountKind::OauthToken) => Err(AccountError::Invalid(
            "a codex account is a block of environment variables: codex reads no OAuth token Droppen could inject".to_string(),
        )),
    }
}

/// A base URL fit to be interpolated into codex's inline TOML, or a refusal.
///
/// The rules are narrow on purpose: this string is put inside a quoted TOML
/// value on a command line, so a quote, a backslash or a control character in
/// it is a way to say something to the child that nobody typed.
pub fn normalize_base_url(raw: &str) -> Result<String, AccountError> {
    let url = raw.trim();
    if url.len() > MAX_BASE_URL_LENGTH {
        return Err(AccountError::Invalid(format!("{CODEX_BASE_URL_VAR} is implausibly long")));
    }
    let Some(rest) = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")) else {
        return Err(AccountError::Invalid(format!("{CODEX_BASE_URL_VAR} must start with http:// or https://")));
    };
    // `https:///v1` parses as a URL with no host, and a CLI pointed at one
    // fails somewhere far away from the field that was typed wrong.
    if rest.is_empty() || rest.starts_with('/') {
        return Err(AccountError::Invalid(format!("{CODEX_BASE_URL_VAR} has no host")));
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control() || c == '"' || c == '\\') {
        return Err(AccountError::Invalid(format!(
            "{CODEX_BASE_URL_VAR} cannot contain whitespace, a quote or a backslash"
        )));
    }
    Ok(url.to_string())
}

/// What the API is allowed to see: no secret, and no hash of one either.
///
/// Named `Account` because that is the schema name in `openapi/openapi.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    /// Stable uuid; also the name of the account's data directory.
    pub id: String,
    /// Display name the user gave it.
    pub label: String,
    /// How it authenticates.
    pub kind: AccountKind,
    /// Address of the login, once a usage verification has read one.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub email: Optional<String>,
    /// Whether the quota panel probes this account.
    pub usage_enabled: bool,
    /// When it was stored, ISO-8601.
    pub created_at: String,
    /// When it last changed, ISO-8601.
    pub updated_at: String,
    /// Why the credential was stored without being checked.
    ///
    /// Only ever set on the reply that stored it, and only when the live check
    /// could not run at all — no `claude` on `PATH`, a timeout, a rate limit. It
    /// is not persisted and never appears in `GET /api/accounts`: it exists so
    /// the client can say "saved, but nobody has proved this works" instead of
    /// implying a check that did not happen.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub unverified: Optional<String>,
    /// Which CLI it authenticates.
    pub provider: AccountProvider,
    /// The variable names an env account sets, in the order they were typed.
    ///
    /// Names only: the list a picker, a session row and this page all read has
    /// no value in it. The block itself is returned only by
    /// `GET /api/accounts/{id}/env`, which the settings page calls when it
    /// opens an env account for editing — an explicit read, not one every list
    /// response carries.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub env_keys: Optional<Vec<String>>,
    /// The limits this instance has seen the account hit that have not lifted
    /// yet: one entry per scope, active ones only.
    ///
    /// Recorded, not acted on — nothing switches accounts or holds a queue
    /// because of it. Empty when the account is not
    /// resting; per instance, because the log it comes from is.
    // The description above is part of the published contract and is left as
    // it was; since phase 2 the rotation does read these rests to choose where
    // a session goes next (see `super::rotation`).
    pub rest: Vec<AccountRest>,
    /// Whether the account is in its CLI's rotation pool: a session whose
    /// account hits a limit may be moved onto it and carry on there.
    ///
    /// Defaults to `true` for a Claude OAuth account and `false` for an env or
    /// Codex account.
    pub rotation: bool,
}

/// The account a session runs under, as the session payloads name it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
pub struct AccountRef {
    /// The account's uuid.
    pub id: String,
    /// Its display name.
    pub label: String,
}

/// `GET /api/accounts` and everything that returns the whole list.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AccountList {
    /// Account new claude sessions follow; `null` is none: the local login, or,
    /// while it is signed out and nobody chose it (`local.claude.isDefault`), an
    /// account in the pool standing in for it (`GET /api/accounts/start`).
    pub default_account_id: Nullable<String>,
    /// Every stored account.
    pub accounts: Vec<Account>,
    /// Account new codex sessions follow; `null` is none, as for claude.
    pub default_codex_account_id: Nullable<String>,
    /// The machine's own login for each CLI — the one a session with no account
    /// runs on — and whether it can start a session right now. A picker uses it
    /// to stop offering a signed-out local login as the default.
    #[serde(default)]
    pub local: super::local_login::LocalLogins,
}

/// Everything one account contributes to a spawn.
///
/// Two halves because the two CLIs take a credential differently. Claude reads
/// it out of the environment, so an account is one or two variables and nothing
/// else. Codex reads nothing out of the environment by itself, so the variable
/// has to be *introduced* to it on the command line — hence
/// [`AccountLaunch::codex_args`], which is empty for every claude account and is
/// the whole mechanism for a codex one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountLaunch {
    /// The CLI this launch is for; a spawn refuses the other one's account.
    pub provider: AccountProvider,
    /// Variables to add to the child's environment. Never logged.
    pub env: Vec<(String, String)>,
    /// Extra argv for a `codex` spawn; always empty for claude.
    pub codex_args: Vec<String>,
}

/// The launch one credential produces, without it having to be stored first.
///
/// Shared with [`super::verify`] on purpose: a check that ran with a different
/// environment from the spawn would be a check of something else.
///
/// Fallible, and only ever on the codex side. A claude block is handed over as
/// it stands — a token, a gateway's four variables, a Bedrock setup with no
/// token at all: the CLI is the thing that knows which of those work, and
/// second-guessing it here would refuse configurations that are fine. Codex
/// cannot be handed a block as it stands, because `app-server` reads no key
/// variable of its own, so the block has to be *translated* — and a translation
/// needs something to translate.
pub fn launch_for(provider: AccountProvider, kind: AccountKind, secret: &str, env: &[EnvVar]) -> Result<AccountLaunch, AccountError> {
    let mut vars: Vec<(String, String)> = match kind {
        AccountKind::OauthToken => vec![(CLAUDE_OAUTH_ENV.to_string(), secret.to_string())],
        AccountKind::Env => env.iter().map(|var| (var.key.clone(), var.value.clone())).collect(),
    };
    let mut codex_args = Vec::new();
    if provider == AccountProvider::Codex {
        let key = CODEX_KEY_CANDIDATES
            .iter()
            .find_map(|name| env.iter().find(|var| &var.key == name))
            .ok_or_else(|| {
                AccountError::Invalid(format!(
                    "a codex account needs {} (or {}) in its environment block: `codex app-server` reads neither, so Droppen has to hand the key to a model provider of its own",
                    CODEX_KEY_CANDIDATES[0], CODEX_KEY_CANDIDATES[1]
                ))
            })?;
        // On top of the block rather than instead of it: the variables the user
        // typed still reach the child, and this is the one the injected provider
        // is told to read.
        vars.push((CODEX_API_KEY_ENV.to_string(), key.value.clone()));
        // A blank one is "the vendor default", as it was when the base URL was
        // a field of its own: `OPENAI_BASE_URL=` is how a pasted block says it
        // is not overriding anything, and v0.2.58 read it that way.
        let named = env.iter().find(|var| var.key == CODEX_BASE_URL_VAR && !var.value.trim().is_empty());
        let base_url = match named {
            Some(var) => normalize_base_url(&var.value)?,
            None => CODEX_DEFAULT_BASE_URL.to_string(),
        };
        codex_args = codex_provider_args(&base_url);
    }
    Ok(AccountLaunch {
        provider,
        env: vars,
        codex_args,
    })
}

/// Everything that can go wrong in the store.
#[derive(Debug, Clone, thiserror::Error)]
pub enum AccountError {
    /// The input cannot be stored (empty label, whitespace in the token…).
    #[error("{0}")]
    Invalid(String),
    /// No such account.
    #[error("unknown account: {0}")]
    NotFound(String),
    /// The store itself could not be read or written.
    ///
    /// The TypeScript lets the underlying `fs` error escape as a plain `Error`,
    /// which every route masks behind its own fallback text — so this variant
    /// exists to be masked rather than reported.
    #[error("the account store could not be written: {0}")]
    Io(String),
    /// The account belongs to the other CLI.
    #[error("provider_mismatch")]
    ProviderMismatch {
        /// The CLI the account authenticates.
        account: AccountProvider,
        /// The CLI the caller wanted it for.
        wanted: AccountProvider,
    },
    /// The same token is already stored under another label.
    #[error("this token is already stored as \"{existing_label}\"")]
    Duplicate {
        /// The account that already holds this token.
        existing_id: String,
        /// Its label, which the 409 body reports.
        existing_label: String,
    },
}

/// The on-disk shape. Field order matches the TypeScript's object literals.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredAccount {
    id: String,
    label: String,
    kind: AccountKind,
    /// The OAuth token, for an `oauth_token` account; empty for an env one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    secret: String,
    /// What the duplicate check compares: the token's hash, or the block's.
    secret_sha256: String,
    usage_enabled: bool,
    created_at: String,
    updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    /// Absent on every row written before there was a second CLI to choose.
    #[serde(default)]
    provider: AccountProvider,
    /// The block, for an `env` account; empty for an OAuth one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    env: Vec<EnvVar>,
    /// Whether it is in the rotation pool, when somebody said; absent on every
    /// row written before there was a pool, which reads as [`default_rotation`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rotation: Option<bool>,
}

/// Whether an account nobody has put in or taken out of the rotation pool is in
/// it: a Claude OAuth token is, because every one of them is the same product
/// with its own quota; an env block usually is not — it is a gateway, a key
/// billed differently, often another vendor's models — and codex sessions do
/// not rotate at all.
pub fn default_rotation(provider: AccountProvider, kind: AccountKind) -> bool {
    provider == AccountProvider::Claude && kind == AccountKind::OauthToken
}

impl StoredAccount {
    fn to_public(&self) -> Account {
        Account {
            id: self.id.clone(),
            label: self.label.clone(),
            kind: self.kind,
            email: Optional(self.email.clone()),
            usage_enabled: self.usage_enabled,
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
            // Whether a check ran is not a property of the stored account; the
            // route that stored it is the only thing that knows.
            unverified: Optional(None),
            provider: self.provider,
            env_keys: Optional((self.kind == AccountKind::Env).then(|| self.env.iter().map(|var| var.key.clone()).collect())),
            // Not a property of the stored row either: the rests live in this
            // instance's database, and [`AccountsStore::with_rests`] adds them.
            rest: Vec::new(),
            rotation: self.in_pool(),
        }
    }

    /// Whether this account is in the rotation pool.
    fn in_pool(&self) -> bool {
        self.rotation.unwrap_or_else(|| default_rotation(self.provider, self.kind))
    }

    /// What this account puts into a spawn.
    fn launch(&self) -> Result<AccountLaunch, AccountError> {
        launch_for(self.provider, self.kind, &self.secret, &self.env)
    }

    /// Every value this account could see echoed back at it by a CLI.
    fn secrets(&self) -> impl Iterator<Item = &String> {
        std::iter::once(&self.secret).chain(self.env.iter().map(|var| &var.value))
    }
}

/// The shape the three tables are assembled back into, which is the shape
/// `accounts.json` had.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountsFile {
    version: u8,
    default_account_id: Option<String>,
    accounts: Vec<StoredAccount>,
    /// Session id -> account id, for either CLI.
    session_accounts: BTreeMap<String, String>,
    /// The other CLI's default. Never in the old file — it is younger than that
    /// shape — but this shape is what the three tables are assembled into.
    #[serde(default)]
    default_codex_account_id: Option<String>,
}

impl Default for AccountsFile {
    fn default() -> Self {
        Self {
            version: 1,
            default_account_id: None,
            accounts: Vec::new(),
            session_accounts: BTreeMap::new(),
            default_codex_account_id: None,
        }
    }
}

/// The store, owning its database handle.
///
/// No lock, where the TypeScript keeps one at module scope and this port kept
/// one until the JSON mirror went. Every mutation below does its read, its check
/// and its write inside a single [`Db::write`] transaction, which is atomic
/// against the machine's *other* server as well — something no mutex private to
/// one process ever was. The lock that outlived that only ordered the whole-file
/// rewrites of `accounts.json` against the commits they described, and there is
/// no longer a file to rewrite.
pub struct AccountsStore {
    paths: Arc<Paths>,
    db: Arc<Db>,
}

const MAX_SECRET_LENGTH: usize = 4096;
const MAX_LABEL_LENGTH: usize = 100;
const MAX_BASE_URL_LENGTH: usize = 512;
/// Below this, a stored "secret" is more likely to be a word than a credential,
/// and replacing every occurrence of it would shred the text it appears in.
///
/// Only reachable by a hand-edited row: every credential a person can type in
/// goes through [`normalize_secret`], and the shortest real one is far longer.
const MIN_REDACTABLE_SECRET: usize = 8;

fn sha256_hex(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Whether `id` is a plain uuid. A directory is only ever named after one.
pub fn is_uuid(id: &str) -> bool {
    let groups = [8usize, 4, 4, 4, 12];
    let mut parts = id.split('-');
    for length in groups {
        let Some(part) = parts.next() else { return false };
        if part.len() != length || !part.bytes().all(|b| b.is_ascii_hexdigit()) {
            return false;
        }
    }
    parts.next().is_none()
}

fn require_uuid(id: &str) -> Result<&str, AccountError> {
    if is_uuid(id) {
        Ok(id)
    } else {
        Err(AccountError::Invalid(format!("invalid account id: {id}")))
    }
}

/// `<data>/accounts/<id>`: machine-level, shared by every instance.
///
/// A free function beside [`AccountsStore::account_dir`] so the quota readings,
/// which live in this directory and are written from places that hold no store
/// (a session's driver), spell the layout the same way the store does.
pub fn account_dir_in(paths: &Paths, id: &str) -> Result<PathBuf, AccountError> {
    Ok(paths.data_dir().join("accounts").join(require_uuid(id)?))
}

/// `<data>/accounts/<id>/claude`: the account's usage login. See
/// [`AccountsStore::usage_config_dir`].
pub fn usage_config_dir_in(paths: &Paths, id: &str) -> Result<PathBuf, AccountError> {
    Ok(account_dir_in(paths, id)?.join("claude"))
}

/// A random v4 uuid, lowercase and hyphenated — the shape `randomUUID()` emits.
fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn normalize_secret(raw: &str) -> Result<String, AccountError> {
    let secret = raw.trim();
    if secret.is_empty() {
        return Err(AccountError::Invalid("the token is empty".into()));
    }
    if secret.len() > MAX_SECRET_LENGTH {
        return Err(AccountError::Invalid("the token is implausibly long".into()));
    }
    if secret.chars().any(char::is_whitespace) {
        return Err(AccountError::Invalid("the token contains whitespace".into()));
    }
    Ok(secret.to_string())
}

fn normalize_label(raw: &str) -> Result<String, AccountError> {
    let label = raw.trim();
    if label.is_empty() {
        return Err(AccountError::Invalid("the label is empty".into()));
    }
    Ok(label.chars().take(MAX_LABEL_LENGTH).collect())
}

/// A missing `kind` or `provider` is a row from before the field existed, and
/// reads as what that build could only have meant: a Claude OAuth token. A
/// field that is *present and unrecognised* is the opposite case and must not
/// take the same fallback — a row this build cannot read would otherwise be
/// handed to a spawn as an OAuth token it is not, which bills the wrong login
/// or 401s under a label that says everything is fine. It is dropped, loudly,
/// exactly as a row with no usable secret is.
///
/// `api_key` and `auth_token` are the exception, because they are not
/// unrecognised: v0.2.58 shipped them, and every one of those rows says exactly
/// what it would have put in the environment. They are converted here, on the
/// way in, and written back in the new shape by the next update that touches
/// the row — see [`legacy_block`].
fn sanitize_account(raw: &serde_json::Value, now: &str) -> Option<StoredAccount> {
    let object = raw.as_object()?;
    let id = object.get("id").and_then(|v| v.as_str()).filter(|id| is_uuid(id))?;
    let provider = match object.get("provider").and_then(|v| v.as_str()).map(str::trim) {
        None | Some("") | Some("claude") => AccountProvider::Claude,
        Some("codex") => AccountProvider::Codex,
        Some(other) => {
            tracing::warn!(account = %id, provider = %other, "an account for a CLI this build does not know is not an account");
            return None;
        }
    };
    let string = |key: &str| object.get(key).and_then(|v| v.as_str()).map(str::to_owned);
    let secret = string("secret").filter(|s| !s.is_empty());
    let raw_kind = object.get("kind").and_then(|v| v.as_str()).map(str::trim);

    // The three-way split: what this build writes, what v0.2.58 wrote, and
    // anything else — which is a row from the future or a hand edit, and is not
    // an account this build may guess at.
    let (kind, env) = match raw_kind {
        None | Some("") | Some("oauth_token") => (AccountKind::OauthToken, Vec::new()),
        Some("env") => (AccountKind::Env, read_env(object.get("env"))),
        Some(legacy @ ("api_key" | "auth_token")) => {
            let Some(secret) = secret.as_deref() else {
                tracing::warn!(account = %id, "a v1 key account with no key is not an account");
                return None;
            };
            (AccountKind::Env, legacy_block(provider, legacy, secret, string("baseUrl").as_deref()))
        }
        Some(other) => {
            tracing::warn!(account = %id, kind = %other, "an account whose kind this build does not know is not an account");
            return None;
        }
    };

    // A credential-shaped row with no credential in it is not an account: it
    // would hand a spawn nothing and let it fall through to whatever login the
    // machine has lying about, under a label that says otherwise.
    let secret = match kind {
        AccountKind::OauthToken => secret?,
        AccountKind::Env => {
            if env.is_empty() {
                tracing::warn!(account = %id, "an env account with no variables is not an account");
                return None;
            }
            String::new()
        }
    };
    if let Err(error) = validate_pairing(provider, kind) {
        tracing::warn!(account = %id, %error, "an account naming a credential its CLI cannot take is not an account");
        return None;
    }
    let hash = match kind {
        AccountKind::OauthToken => string("secretSha256").unwrap_or_else(|| sha256_hex(&secret)),
        // Never the stored hash for an env account: a converted v1 row's hash is
        // of the bare token, and the duplicate check now compares blocks.
        AccountKind::Env => block_sha256(&env),
    };
    Some(StoredAccount {
        id: id.to_string(),
        label: string("label").filter(|l| !l.trim().is_empty()).unwrap_or_else(|| id.to_string()),
        kind,
        secret,
        secret_sha256: hash,
        usage_enabled: object.get("usageEnabled") == Some(&serde_json::Value::Bool(true)),
        created_at: string("createdAt").unwrap_or_else(|| now.to_string()),
        updated_at: string("updatedAt").unwrap_or_else(|| now.to_string()),
        email: string("email").filter(|e| !e.is_empty()),
        provider,
        env,
        rotation: object.get("rotation").and_then(|v| v.as_bool()),
    })
}

/// The `env` array of a row this build wrote, dropping anything unusable.
fn read_env(raw: Option<&serde_json::Value>) -> Vec<EnvVar> {
    let Some(entries) = raw.and_then(|value| value.as_array()) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let object = entry.as_object()?;
            // The denylist again, on the way *in*: the payload column is also
            // hand-editable, and a row that named `CLAUDE_CONFIG_DIR` would
            // move where transcripts land.
            let key = object
                .get("key")
                .and_then(|v| v.as_str())
                .filter(|key| is_env_name(key) && !DENIED_ENV_KEYS.contains(key))?;
            let value = object.get("value").and_then(|v| v.as_str())?;
            Some(EnvVar {
                key: key.to_string(),
                value: value.to_string(),
            })
        })
        .collect()
}

/// A v0.2.58 key account as the block it was always shorthand for.
///
/// Exactly the variables that build injected, under exactly the names it used,
/// so an account that was working before the upgrade is working after it. The
/// codex pair is the one that moves: v1 put the key straight into the variable
/// the injected provider reads, and a block says what the user's key *is*
/// (`OPENAI_API_KEY`) and lets [`launch_for`] do the translating.
fn legacy_block(provider: AccountProvider, kind: &str, secret: &str, base_url: Option<&str>) -> Vec<EnvVar> {
    let (key_var, url_var) = match provider {
        AccountProvider::Claude if kind == "auth_token" => ("ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL"),
        AccountProvider::Claude => ("ANTHROPIC_API_KEY", "ANTHROPIC_BASE_URL"),
        AccountProvider::Codex => (CODEX_KEY_CANDIDATES[0], CODEX_BASE_URL_VAR),
    };
    let mut block = vec![EnvVar {
        key: key_var.to_string(),
        value: secret.to_string(),
    }];
    if let Some(base_url) = base_url.map(str::trim).filter(|url| !url.is_empty()) {
        block.push(EnvVar {
            key: url_var.to_string(),
            value: base_url.to_string(),
        });
    }
    block
}

impl AccountsStore {
    /// A store rooted at the given path set, over the machine-wide database.
    pub fn new(paths: Arc<Paths>) -> Self {
        let db = db::at(paths.config_db());
        Self { paths, db }
    }

    /// Per-account directory; only ever named after a validated uuid.
    pub fn account_dir(&self, id: &str) -> Result<PathBuf, AccountError> {
        account_dir_in(&self.paths, id)
    }

    /// `CLAUDE_CONFIG_DIR` for this account's *usage probe*.
    ///
    /// Deliberately not the managed session root: the probe needs a logged-in
    /// config of its own, while sessions share one root and differ only by the
    /// injected token.
    pub fn usage_config_dir(&self, id: &str) -> Result<PathBuf, AccountError> {
        usage_config_dir_in(&self.paths, id)
    }

    /// An empty working directory, so the probe CLI adopts no project of the user's.
    pub fn usage_cwd(&self, id: &str) -> Result<PathBuf, AccountError> {
        Ok(self.account_dir(id)?.join("cwd"))
    }

    /// The whole store, assembled from its three tables.
    ///
    /// Assembling it is three indexed reads of a few kilobytes; the store it
    /// replaces did a `stat`, a `read` and a parse of the same data and then
    /// filtered it in Rust.
    fn snapshot(&self) -> AccountsFile {
        let now = now_iso();
        self.db.read(|conn| load_file(conn, &now)).unwrap_or_else(|error| {
            tracing::error!(database = %self.db.path().display(), %error, "cannot read the accounts out of the state database");
            AccountsFile::default()
        })
    }

    /// The limit log and the rests, in this *instance's* database.
    ///
    /// Opened on demand rather than held, like every other instance store: the
    /// accounts are machine-wide, but what one instance has seen them do is
    /// its own (see [`super::rest`]).
    pub fn limit_log(&self) -> LimitLog {
        LimitLog::new(db::at(self.paths.instance_db()))
    }

    /// The rotation decisions, in this instance's database beside the rests.
    pub fn rotation_log(&self) -> super::rotation::RotationLog {
        super::rotation::RotationLog::new(db::at(self.paths.instance_db()))
    }

    /// Fill in each account's active rests.
    fn with_rests(&self, mut accounts: Vec<Account>) -> Vec<Account> {
        if accounts.is_empty() {
            return accounts;
        }
        let mut rests = self.limit_log().active_rests(super::rest::now_ms());
        for account in &mut accounts {
            account.rest = rests.remove(&account.id).unwrap_or_default();
        }
        accounts
    }

    /// Every stored account, secrets stripped.
    pub async fn list(&self) -> Vec<Account> {
        self.with_rests(self.snapshot().accounts.iter().map(StoredAccount::to_public).collect())
    }

    /// One account, or `None`.
    pub async fn get(&self, id: &str) -> Option<Account> {
        let account = self.stored(id).as_ref().map(StoredAccount::to_public)?;
        self.with_rests(vec![account]).pop()
    }

    /// The account new sessions of this CLI follow, if one is configured.
    ///
    /// Per CLI because the two are not interchangeable: a codex thread cannot be
    /// started under an Anthropic token, so one machine-wide default would have
    /// to be ignored by whichever CLI it did not name — silently, on the spawn
    /// path, which is the worst place to be surprised.
    pub async fn default_account_id(&self, provider: AccountProvider) -> Option<String> {
        let file = self.snapshot();
        match provider {
            AccountProvider::Claude => file.default_account_id,
            AccountProvider::Codex => file.default_codex_account_id,
        }
    }

    /// Whether somebody chose the machine's own login as this CLI's default,
    /// rather than leaving the default unset (see [`LOCAL_DEFAULT`]).
    pub fn local_is_default(&self, provider: AccountProvider) -> bool {
        self.db
            .read(|conn| {
                conn.query_row(
                    "SELECT value FROM kv WHERE scope = ?1 AND key = ?2",
                    (SETTINGS_SCOPE, provider.default_key()),
                    |row| row.get::<_, String>(0),
                )
                .optional()
            })
            .ok()
            .flatten()
            .is_some_and(|value| value == LOCAL_DEFAULT)
    }

    /// The whole list plus both defaults, as `GET /api/accounts` returns it.
    pub async fn account_list(&self) -> AccountList {
        let file = self.snapshot();
        AccountList {
            default_account_id: Nullable(file.default_account_id.clone()),
            accounts: self.with_rests(file.accounts.iter().map(StoredAccount::to_public).collect()),
            default_codex_account_id: Nullable(file.default_codex_account_id.clone()),
            local: self.local_logins(),
        }
    }

    /// The machine's own logins, from this instance's rests and the machine's
    /// shared reading of the local login. Two small reads, no spawn.
    pub fn local_logins(&self) -> super::local_login::LocalLogins {
        let reading = crate::quota::UsageLogin::Local
            .reading_file(&self.paths)
            .map(|path| crate::quota::readings::load(&path))
            .unwrap_or_default();
        let mut logins = super::local_login::local_logins(&self.limit_log(), &reading, super::rest::now_ms());
        logins.claude.is_default = self.local_is_default(AccountProvider::Claude);
        logins.codex.is_default = self.local_is_default(AccountProvider::Codex);
        logins
    }

    /// One row, as the store understands it, or nothing.
    ///
    /// A row whose payload will not survive [`sanitize_account`] is *not* an
    /// account with some fields missing; it is not an account. That is the same
    /// judgement the file's reader made, and it matters most for `secret`:
    /// keeping the row and treating an unreadable credential as an absent one
    /// would hand a spawn no token and let it fall through to whatever login the
    /// machine has lying about, under a label that says otherwise.
    fn stored(&self, id: &str) -> Option<StoredAccount> {
        let now = now_iso();
        self.db
            .read(|conn| {
                let raw: Option<String> = conn
                    .query_row("SELECT payload FROM accounts WHERE id = ?1", (id,), |row| row.get(0))
                    .optional()?;
                Ok(raw
                    .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                    .and_then(|value| sanitize_account(&value, &now)))
            })
            .unwrap_or(None)
    }

    /// Store a new account.
    ///
    /// Adding one deliberately does not make it the default: that would silently
    /// move every new session off the local login before it has been verified.
    pub async fn create(&self, label: &str, provider: AccountProvider, credential: Credential) -> Result<Account, AccountError> {
        self.create_with(label, provider, credential, None).await
    }

    /// [`AccountsStore::create`], putting the account in or out of the rotation
    /// pool when `rotation` says; `None` leaves it at [`default_rotation`].
    pub async fn create_with(
        &self,
        label: &str,
        provider: AccountProvider,
        credential: Credential,
        rotation: Option<bool>,
    ) -> Result<Account, AccountError> {
        let kind = credential.kind();
        validate_pairing(provider, kind)?;
        let label = normalize_label(label)?;
        let (secret, env) = match credential {
            Credential::Oauth(secret) => (normalize_secret(&secret)?, Vec::new()),
            Credential::Env(env) => (String::new(), env),
        };
        // Proved to be spawnable before it is stored: a codex block with no key
        // variable in it is a row nothing could ever start.
        launch_for(provider, kind, &secret, &env)?;
        let hash = match kind {
            AccountKind::OauthToken => sha256_hex(&secret),
            AccountKind::Env => block_sha256(&env),
        };

        let now = now_iso();
        let account = StoredAccount {
            id: new_uuid(),
            label,
            kind,
            secret,
            secret_sha256: hash.clone(),
            // Only the verify step (a real `claude auth status`) may turn this on.
            usage_enabled: false,
            created_at: now.clone(),
            updated_at: now,
            email: None,
            provider,
            env,
            rotation,
        };
        let public = account.to_public();
        self.db
            .write(|tx| {
                // Inside the transaction, so that the machine's other server
                // storing the same token at the same moment produces a duplicate
                // rather than two accounts sharing one credential.
                let now = now_iso();
                // Scoped to the provider, because one string can legitimately be
                // two credentials: a relay that fronts both vendors issues a
                // single key, and storing it once for claude must not make it
                // unstorable for codex. Within one CLI it is still a merge and
                // still refused.
                if let Some(clash) = load_accounts(tx, &now)?
                    .into_iter()
                    .find(|a| a.secret_sha256 == hash && a.provider == provider)
                {
                    return Ok(Err(AccountError::Duplicate {
                        existing_id: clash.id,
                        existing_label: clash.label,
                    }));
                }
                tx.execute("INSERT INTO accounts (id, payload) VALUES (?1, ?2)", (&account.id, payload_of(&account)))?;
                Ok(Ok(()))
            })
            .map_err(store_db)??;
        Ok(public)
    }

    /// Patch an account's label, email, usage flag or credential. `None` leaves
    /// a field alone.
    ///
    /// Replacing the credential is what re-authorization is: the id survives, and
    /// with it every `sessionAccounts` binding, so a session mid-conversation
    /// carries on under the same account with a token that works again.
    pub async fn update(&self, id: &str, patch: AccountPatch) -> Result<Option<Account>, AccountError> {
        let new_credential = patch.credential.is_some();
        let updated = self
            .db
            .write(|tx| {
                let now = now_iso();
                let accounts = load_accounts(tx, &now)?;
                let Some(mut account) = accounts.iter().find(|a| a.id == id).cloned() else {
                    return Ok(Ok(None));
                };
                // Normalized and checked for a clash before anything is written:
                // a credential another account already holds would make two
                // accounts one, and re-authorizing into the same token this
                // account already has is a no-op rather than a clash.
                if let Some(credential) = patch.credential {
                    if credential.kind() != account.kind {
                        return Ok(Err(AccountError::Invalid(match account.kind {
                            AccountKind::OauthToken => {
                                "this is an OAuth account: re-authorize it or paste a new token, not an environment block".to_string()
                            }
                            AccountKind::Env => "this is an env account: replace its environment block, not a token".to_string(),
                        })));
                    }
                    let (secret, env) = match credential {
                        Credential::Oauth(secret) => match normalize_secret(&secret) {
                            Ok(secret) => (secret, Vec::new()),
                            Err(error) => return Ok(Err(error)),
                        },
                        Credential::Env(env) => (String::new(), env),
                    };
                    if let Err(error) = launch_for(account.provider, account.kind, &secret, &env) {
                        return Ok(Err(error));
                    }
                    let hash = match account.kind {
                        AccountKind::OauthToken => sha256_hex(&secret),
                        AccountKind::Env => block_sha256(&env),
                    };
                    if let Some(clash) = accounts
                        .iter()
                        .find(|a| a.secret_sha256 == hash && a.id != id && a.provider == account.provider)
                    {
                        return Ok(Err(AccountError::Duplicate {
                            existing_id: clash.id.clone(),
                            existing_label: clash.label.clone(),
                        }));
                    }
                    account.secret = secret;
                    account.env = env;
                    account.secret_sha256 = hash;
                }
                if let Some(label) = patch.label {
                    account.label = match normalize_label(&label) {
                        Ok(label) => label,
                        Err(error) => return Ok(Err(error)),
                    };
                }
                if let Some(email) = patch.email {
                    let email = email.trim().to_string();
                    account.email = (!email.is_empty()).then_some(email);
                }
                if let Some(usage_enabled) = patch.usage_enabled {
                    account.usage_enabled = usage_enabled;
                }
                if let Some(rotation) = patch.rotation {
                    account.rotation = Some(rotation);
                }
                account.updated_at = now;
                tx.execute("UPDATE accounts SET payload = ?2 WHERE id = ?1", (id, payload_of(&account)))?;
                Ok(Ok(Some(account.to_public())))
            })
            .map_err(store_db)??;
        let Some(account) = updated else { return Ok(None) };
        // A refused credential rests the account until somebody changes it, and
        // this is somebody changing it. Only the `auth` rest goes: a new token
        // does not refill a spent quota.
        if new_credential {
            if let Err(error) = self.limit_log().clear_class(id, LimitClass::Auth) {
                tracing::warn!(account = %id, %error, "cannot clear the auth rest of a re-authorized account");
            }
        }
        Ok(self.with_rests(vec![account]).pop())
    }

    /// Remove an account, its bindings, its default status and its probe login.
    ///
    /// `Ok(false)` means there was no such account; a failed write is an error,
    /// not a "not found".
    pub async fn delete(&self, id: &str) -> Result<bool, AccountError> {
        // Three tables, one transaction: an account cannot survive as the thing
        // a session is still bound to, and a binding cannot outlive the
        // credential it names, however the process dies in between.
        let removed = self
            .db
            .write(|tx| {
                if tx.execute("DELETE FROM accounts WHERE id = ?1", (id,))? == 0 {
                    return Ok(false);
                }
                tx.execute("DELETE FROM session_accounts WHERE account_id = ?1", (id,))?;
                tx.execute(
                    "DELETE FROM kv WHERE scope = ?1 AND key IN (?2, ?3) AND value = ?4",
                    (SETTINGS_SCOPE, DEFAULT_KEY, DEFAULT_CODEX_KEY, id),
                )?;
                Ok(true)
            })
            .map_err(store_db)?;
        if !removed {
            return Ok(false);
        }
        // Its rests name an account that no longer exists. The log keeps its
        // events: they are history, and history does not stop being true.
        if let Err(error) = self.limit_log().clear(id, None) {
            tracing::warn!(account = %id, %error, "cannot clear the rests of a deleted account");
        }
        // The probe login lives outside the store; drop it with the account.
        if let Ok(dir) = self.account_dir(id) {
            let _ = tokio::fs::remove_dir_all(dir).await;
        }
        Ok(true)
    }

    /// Point new sessions of one CLI at an account, or back at the local login
    /// with `None`.
    ///
    /// `None` is written down as a choice ([`LOCAL_DEFAULT`]), not as the
    /// absence of one: somebody who picked their own login keeps it even when
    /// it is signed out, where an unset default would start on a stand-in.
    ///
    /// `Ok(false)` means there was no such account; an account belonging to the
    /// *other* CLI is refused rather than stored, because a default nothing can
    /// spawn under is a setting that only fails later.
    pub async fn set_default(&self, id: Option<&str>, provider: AccountProvider) -> Result<bool, AccountError> {
        let key = provider.default_key();
        let known = self
            .db
            .write(|tx| match id {
                None => {
                    tx.execute(
                        "INSERT INTO kv (scope, key, value, updated_at) VALUES (?1, ?2, ?3, unixepoch())
                         ON CONFLICT (scope, key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                        (SETTINGS_SCOPE, key, LOCAL_DEFAULT),
                    )?;
                    Ok(Ok(true))
                }
                Some(id) => {
                    let now = now_iso();
                    let Some(account) = load_accounts(tx, &now)?.into_iter().find(|account| account.id == id) else {
                        return Ok(Ok(false));
                    };
                    if account.provider != provider {
                        return Ok(Err(AccountError::Invalid(format!(
                            "\"{}\" is a {} account, so it cannot be the default for {} sessions",
                            account.label,
                            account.provider.label(),
                            provider.label()
                        ))));
                    }
                    tx.execute(
                        "INSERT INTO kv (scope, key, value, updated_at) VALUES (?1, ?2, ?3, unixepoch())
                         ON CONFLICT (scope, key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                        (SETTINGS_SCOPE, key, id),
                    )?;
                    Ok(Ok(true))
                }
            })
            .map_err(store_db)??;
        Ok(known)
    }

    /// Leave one CLI with no default at all — neither an account nor a chosen
    /// local login — as a fresh install has: new sessions then start on the
    /// local login, or on a stand-in while it is signed out
    /// ([`super::rotation::start_choice`]).
    pub async fn clear_default(&self, provider: AccountProvider) -> Result<(), AccountError> {
        self.db
            .write(|tx| tx.execute("DELETE FROM kv WHERE scope = ?1 AND key = ?2", (SETTINGS_SCOPE, provider.default_key())))
            .map_err(store_db)?;
        Ok(())
    }

    /// The account a claude session is pinned to.
    ///
    /// The join is the rule the file's reader applied by hand: a binding that
    /// names an account nobody has any more is not a binding.
    pub async fn session_account_id(&self, session_id: &str) -> Option<String> {
        self.db
            .read(|conn| {
                conn.query_row(
                    "SELECT b.account_id
                       FROM session_accounts b
                       JOIN accounts a ON a.id = b.account_id
                      WHERE b.session_id = ?1",
                    (session_id,),
                    |row| row.get::<_, String>(0),
                )
                .optional()
            })
            .unwrap_or(None)
    }

    /// The account a session is pinned to, when it belongs to this CLI.
    ///
    /// One binding table serves both CLIs, so "there is a row for this id" is
    /// not on its own evidence that *this* CLI knows the session: ids are
    /// uuids, and a codex thread id posted to the claude route names a row
    /// that has nothing to do with claude. Every caller that treats a binding
    /// as proof of ownership has to ask this question instead of the one
    /// above.
    pub async fn session_account_id_for(&self, session_id: &str, provider: AccountProvider) -> Option<String> {
        let bound = self.session_account_id(session_id).await?;
        let account = self.stored(&bound)?;
        (account.provider == provider).then_some(bound)
    }

    /// That binding resolved to a name, as the session payloads carry it.
    pub async fn session_account_ref(&self, session_id: &str) -> Option<AccountRef> {
        let account = self.stored(&self.session_account_id(session_id).await?)?;
        Some(AccountRef {
            id: account.id,
            label: account.label,
        })
    }

    /// Every claude session id that is pinned to an account.
    ///
    /// The join rather than a whole snapshot: the session feed asks for this
    /// every couple of seconds per open session, and it has no use for the
    /// payloads — which is exactly the sort of question the file could not be
    /// asked without reading all of it.
    pub async fn session_account_map(&self) -> BTreeMap<String, String> {
        self.db
            .read(|conn| {
                let mut statement = conn.prepare(
                    "SELECT b.session_id, b.account_id
                       FROM session_accounts b
                       JOIN accounts a ON a.id = b.account_id",
                )?;
                let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
                Ok(rows.filter_map(Result::ok).collect())
            })
            .unwrap_or_default()
    }

    /// Pin a session to an account, or unpin it with `None`.
    ///
    /// One row of two short strings. This is the write that used to re-encode
    /// and re-`fsync` every account's OAuth token, and it happened every time a
    /// session was started under an account.
    pub async fn bind_session_account(&self, session_id: &str, account_id: Option<&str>, expect: AccountProvider) -> Result<(), AccountError> {
        self.db
            .write(|tx| {
                let Some(account_id) = account_id else {
                    tx.execute("DELETE FROM session_accounts WHERE session_id = ?1", (session_id,))?;
                    return Ok(Ok(()));
                };
                let now = now_iso();
                let Some(account) = load_accounts(tx, &now)?.into_iter().find(|account| account.id == account_id) else {
                    return Ok(Err(AccountError::NotFound(account_id.to_string())));
                };
                // One binding table for both CLIs, so the pairing is checked
                // here rather than implied by which route wrote the row.
                if account.provider != expect {
                    return Ok(Err(AccountError::ProviderMismatch {
                        account: account.provider,
                        wanted: expect,
                    }));
                }
                tx.execute(
                    "INSERT INTO session_accounts (session_id, account_id) VALUES (?1, ?2)
                     ON CONFLICT (session_id) DO UPDATE SET account_id = excluded.account_id",
                    (session_id, account_id),
                )?;
                Ok(Ok(()))
            })
            .map_err(store_db)??;
        Ok(())
    }

    /// Take every stored credential out of a piece of text.
    ///
    /// The other half of [`crate::process_env::redact_secrets`], and the half
    /// that does not have to guess. A pattern can only catch credentials that
    /// *look* like credentials, and two of the three kinds this store now holds
    /// look like nothing at all: an `auth_token` for somebody's gateway and a
    /// relay's codex key are arbitrary strings. A CLI handed one and refused
    /// quotes it back, and that line travels through `injection.error` into
    /// `GET /api/sessions/{source}/{id}` — so the exact values are matched here,
    /// where they are known, and the pattern stays as the second layer for the
    /// credentials this server never stored (the user's own `~/.claude` login,
    /// a key in a prompt).
    ///
    /// Longest first, so a credential that contains another is not left as a
    /// readable tail. One database read, on a path that only runs when a CLI
    /// has already failed.
    pub fn redact_known_secrets(&self, text: &str) -> String {
        if text.is_empty() {
            return String::new();
        }
        // Every value of every block, not just the one field that used to hold
        // a token: a block is a credential in as many pieces as the user typed,
        // and a CLI refused by one of them quotes that one back. Keys are left
        // alone — a variable *name* is not a secret, and blanking it would make
        // the complaint unreadable.
        let mut secrets: Vec<String> = self
            .snapshot()
            .accounts
            .iter()
            .flat_map(StoredAccount::secrets)
            .filter(|secret| secret.len() >= MIN_REDACTABLE_SECRET)
            .cloned()
            .collect();
        secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
        let mut redacted = text.to_string();
        for secret in secrets {
            if redacted.contains(&secret) {
                redacted = redacted.replace(&secret, "[redacted]");
            }
        }
        redacted
    }

    /// The one place a stored secret is read for a spawn.
    ///
    /// Everything the API lists goes through [`Account`], which has no field
    /// for it. The one other caller that reads the block itself is
    /// [`Self::env_block`], which hands it back to the person editing it.
    pub async fn account_env(&self, id: &str) -> Result<AccountLaunch, AccountError> {
        self.stored(id).ok_or_else(|| AccountError::NotFound(id.to_string()))?.launch()
    }

    /// An env account's whole block, as the text it was typed as.
    ///
    /// The account list names only the variables ([`Account::env_keys`]); this
    /// is how the settings page opens its editor on what is stored instead of
    /// on an empty box. It is deliberately a route of its own rather than a
    /// field on [`Account`]: reading a credential back is an act, and only the
    /// editor asks for it.
    ///
    /// Values are rendered as they were stored, with quotes around the ones
    /// whose leading or trailing spaces would otherwise be trimmed on the way
    /// back in, so this text parses to the same block.
    pub async fn env_block(&self, id: &str) -> Result<String, AccountError> {
        let account = self.stored(id).ok_or_else(|| AccountError::NotFound(id.to_string()))?;
        if account.kind != AccountKind::Env {
            return Err(AccountError::Invalid("this account has no environment block".to_string()));
        }
        Ok(account.env.iter().map(render_env_var).collect::<Vec<_>>().join("\n"))
    }

    /// The values of a few *named* variables of an env account's block.
    ///
    /// Deliberately not "the block": the caller says which keys it wants and
    /// gets nothing else. What this exists for is the handful of variables
    /// that are configuration rather than credential — `ANTHROPIC_MODEL` and
    /// the alias overrides beside it, which decide what the model picker may
    /// honestly offer for this account.
    ///
    /// Whether a named key is a secret is the caller's business and not
    /// knowable here: a block is free-form, and `ANTHROPIC_MODEL` is a model
    /// name for everyone who sets it. The rule for a caller is the obvious one
    /// — ask for keys whose values may be shown, and nothing else.
    ///
    /// An OAuth account has no block, and answers with nothing.
    pub(crate) async fn env_values(&self, id: &str, wanted: &[&str]) -> BTreeMap<String, String> {
        let Some(account) = self.stored(id).filter(|account| account.kind == AccountKind::Env) else {
            return BTreeMap::new();
        };
        account
            .env
            .into_iter()
            .filter(|var| wanted.contains(&var.key.as_str()))
            .map(|var| (var.key, var.value))
            .collect()
    }

    /// A name for *which* credential this account currently holds.
    ///
    /// The hash the duplicate check already compares — of the token, or of the
    /// canonical block — and not a secret: it is what the store publishes about
    /// a credential when the credential itself must stay here.
    ///
    /// It exists because an account id says who is billed and nothing about
    /// what is in the environment. A resident claude process is started once
    /// and then fed messages for hours; re-authorizing its account, or fixing a
    /// wrong `ANTHROPIC_BASE_URL` in its block, changes nothing about the
    /// process already running. Carrying this in the resident's fingerprint is
    /// what makes the next message retire it.
    pub(crate) async fn credential_hash(&self, id: &str) -> Option<String> {
        self.stored(id).map(|account| account.secret_sha256)
    }
}

/// The whole store, as its three tables have it.
fn load_file(conn: &Connection, now: &str) -> rusqlite::Result<AccountsFile> {
    let accounts = load_accounts(conn, now)?;
    let ids: HashSet<&str> = accounts.iter().map(|account| account.id.as_str()).collect();
    // A default naming nothing is not a default, and a binding to nothing is
    // dropped: a row that disagrees with the accounts table is not a truth.
    let default_of = |key: &str| -> rusqlite::Result<Option<String>> {
        Ok(conn
            .query_row("SELECT value FROM kv WHERE scope = ?1 AND key = ?2", (SETTINGS_SCOPE, key), |row| {
                row.get::<_, String>(0)
            })
            .optional()?
            .filter(|id| ids.contains(id.as_str())))
    };
    let default_account_id = default_of(DEFAULT_KEY)?;
    let default_codex_account_id = default_of(DEFAULT_CODEX_KEY)?;
    let mut statement = conn.prepare("SELECT session_id, account_id FROM session_accounts")?;
    let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
    let session_accounts = rows
        .filter_map(Result::ok)
        .filter(|(_, account_id)| ids.contains(account_id.as_str()))
        .collect();
    Ok(AccountsFile {
        version: 1,
        default_account_id,
        accounts,
        session_accounts,
        default_codex_account_id,
    })
}

/// Every account row, in the order they were stored.
///
/// `ORDER BY rowid` because that is the order the file's array had — the order
/// they were created in, which the list route hands to the client — and an
/// `UPDATE` keeps its rowid, so re-authorizing an
/// account leaves it where it was in the list. Each payload goes back through
/// [`sanitize_account`], so a row a person has edited by hand degrades in
/// exactly the way an entry in the file did.
fn load_accounts(conn: &Connection, now: &str) -> rusqlite::Result<Vec<StoredAccount>> {
    let mut statement = conn.prepare("SELECT payload FROM accounts ORDER BY rowid")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    Ok(rows
        .filter_map(Result::ok)
        .filter_map(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .filter_map(|value| sanitize_account(&value, now))
        .collect())
}

/// One account as the row holds it: the same JSON the file held, minus the array
/// around it.
fn payload_of(account: &StoredAccount) -> String {
    serde_json::to_string(account).unwrap_or_default()
}

/// The fields [`AccountsStore::update`] may change; `None` means "leave it".
#[derive(Debug, Clone, Default)]
pub struct AccountPatch {
    /// New display name.
    pub label: Option<String>,
    /// New address; an empty string clears it.
    pub email: Option<String>,
    /// Whether the quota panel should probe this account.
    pub usage_enabled: Option<bool>,
    /// Put the account in or take it out of the rotation pool.
    pub rotation: Option<bool>,
    /// A replacement credential, from a re-authorization or a re-typed block.
    ///
    /// Has to match the account's own kind: an env block is not a re-authorized
    /// OAuth token and swapping one for the other silently would change what
    /// the account *is* under a label that says otherwise.
    pub credential: Option<Credential>,
}

/// A credential as it arrives, before it belongs to an account.
///
/// One type for the two shapes so that "which kind is this" is answered once,
/// by the thing holding the value, rather than by every caller comparing a
/// `kind` against which of two `Option`s happens to be set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credential {
    /// A `claude setup-token` token.
    Oauth(String),
    /// A parsed `KEY=VALUE` block.
    Env(Vec<EnvVar>),
}

impl Credential {
    /// The kind of account this credential makes.
    pub fn kind(&self) -> AccountKind {
        match self {
            Credential::Oauth(_) => AccountKind::OauthToken,
            Credential::Env(_) => AccountKind::Env,
        }
    }

    /// Every value a CLI could echo back, for the probe's own redaction.
    pub fn values(&self) -> Vec<&str> {
        match self {
            Credential::Oauth(secret) => vec![secret.as_str()],
            Credential::Env(env) => env.iter().map(|var| var.value.as_str()).collect(),
        }
    }
}

fn store_db(error: rusqlite::Error) -> AccountError {
    AccountError::Io(error.to_string())
}

fn now_iso() -> String {
    iso8601(std::time::SystemTime::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two credential shapes, as short as the tests need them.
    fn oauth(secret: &str) -> Credential {
        Credential::Oauth(secret.to_string())
    }

    fn env(block: &str) -> Credential {
        Credential::Env(parse_env_block(block).expect("a block the tests wrote"))
    }

    fn store() -> (tempfile::TempDir, AccountsStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Arc::new(Paths::new(dir.path().to_path_buf()));
        (dir, AccountsStore::new(paths))
    }

    /// Choosing the local login is a setting of its own, not the absence of a
    /// default: written down per CLI, reported as `local.<cli>.isDefault`, and
    /// replaced by an account picked later. A store that never had a default,
    /// and one whose default account was deleted, have no choice at all.
    #[tokio::test]
    async fn the_local_login_chosen_as_the_default_is_a_choice_not_a_blank() {
        let (_dir, store) = store();
        let work = store
            .create("Work", AccountProvider::Claude, oauth("sk-ant-token-work"))
            .await
            .expect("work");
        assert!(!store.local_is_default(AccountProvider::Claude), "nobody chose anything yet");
        assert!(!store.account_list().await.local.claude.is_default);

        assert!(store.set_default(None, AccountProvider::Claude).await.expect("local"));
        assert!(store.local_is_default(AccountProvider::Claude));
        assert!(!store.local_is_default(AccountProvider::Codex), "the other CLI's choice is its own");
        let list = store.account_list().await;
        assert_eq!(list.default_account_id.0, None, "still no account: the wire shape is unchanged");
        assert!(list.local.claude.is_default && !list.local.codex.is_default);
        assert_eq!(store.default_account_id(AccountProvider::Claude).await, None);

        store.clear_default(AccountProvider::Claude).await.expect("automatic");
        assert!(!store.local_is_default(AccountProvider::Claude), "cleared: no choice at all, as on a fresh install");
        assert!(!store.account_list().await.local.claude.is_default);
        assert!(store.set_default(None, AccountProvider::Claude).await.expect("local again"));
        assert!(store.local_is_default(AccountProvider::Claude));

        assert!(store.set_default(Some(&work.id), AccountProvider::Claude).await.expect("work"));
        assert!(!store.local_is_default(AccountProvider::Claude), "an account picked later replaces it");
        assert!(store.delete(&work.id).await.expect("delete"));
        assert!(
            !store.local_is_default(AccountProvider::Claude),
            "a deleted default leaves no choice behind, not a choice of the local login"
        );
    }

    /// `Account.rest` is the active rests of this instance's log; a new
    /// credential ends the `auth` one and leaves a spent quota alone; deleting
    /// the account ends them all.
    #[tokio::test]
    async fn an_account_carries_its_active_rests_and_reauthorizing_ends_the_auth_one() {
        use super::super::limits::{LimitClass, Verdict};
        use super::super::rest::{now_ms, Observation};

        let (_dir, store) = store();
        let account = store
            .create("Work", AccountProvider::Claude, oauth("sk-ant-token-one"))
            .await
            .expect("create");
        assert!(account.rest.is_empty());
        let seen = |class: LimitClass, scope: &str| Observation {
            account_id: account.id.clone(),
            session_id: Some("session-1".into()),
            source: "claude:result",
            model: Some("claude-fable-5-1".into()),
            verdict: Verdict {
                class,
                scope: scope.into(),
                until_ms: None,
            },
            raw: "401 OAuth access token is invalid.".into(),
            reason: "401 OAuth access token is invalid.".into(),
            may_rest: true,
        };
        let log = store.limit_log();
        log.record(&seen(LimitClass::Auth, "all"), now_ms()).expect("auth");
        log.record(&seen(LimitClass::Weekly, "opus"), now_ms()).expect("weekly");
        // An expired rest is not reported.
        log.record(
            &Observation {
                account_id: account.id.clone(),
                verdict: Verdict {
                    class: LimitClass::Rate,
                    scope: "sonnet".into(),
                    until_ms: Some(now_ms() - 1),
                },
                ..seen(LimitClass::Rate, "sonnet")
            },
            now_ms() - 60_000,
        )
        .expect("expired");

        let listed = store.account_list().await;
        let classes: Vec<_> = listed.accounts[0].rest.iter().map(|rest| (rest.class, rest.scope.clone())).collect();
        assert_eq!(
            classes,
            vec![(LimitClass::Auth, "all".to_string()), (LimitClass::Weekly, "opus".to_string())]
        );
        assert_eq!(store.get(&account.id).await.expect("get").rest.len(), 2);
        let wire = serde_json::to_value(&listed.accounts[0]).expect("json");
        assert_eq!(wire["rest"][0]["class"], "auth");
        assert!(wire["rest"][0]["untilAt"].is_null(), "an auth rest has no end: {wire}");
        assert_eq!(wire["rest"][1]["scope"], "opus");

        // A rename is not a new credential.
        let renamed = store
            .update(
                &account.id,
                AccountPatch {
                    label: Some("Renamed".into()),
                    ..Default::default()
                },
            )
            .await
            .expect("rename")
            .expect("found");
        assert_eq!(renamed.rest.len(), 2);

        let reauthorized = store
            .update(
                &account.id,
                AccountPatch {
                    credential: Some(oauth("sk-ant-token-two")),
                    ..Default::default()
                },
            )
            .await
            .expect("reauthorize")
            .expect("found");
        assert_eq!(
            reauthorized.rest.iter().map(|rest| rest.class).collect::<Vec<_>>(),
            vec![LimitClass::Weekly],
            "the auth rest went with the old credential; the spent quota did not"
        );

        assert!(store.delete(&account.id).await.expect("delete"));
        assert!(store.limit_log().active_rests(now_ms()).is_empty(), "no rest outlives its account");
        assert_eq!(store.limit_log().events(Some(&account.id), 10).len(), 3, "the history does");
    }

    /// The rotation pool, for rows that predate it and rows that say.
    ///
    /// A row written before the field existed reads as its default — an OAuth
    /// claude account in, an env account and every codex account out — and an
    /// explicit answer, from creation or a patch, survives a round trip.
    #[tokio::test]
    async fn the_pool_defaults_by_provider_and_kind_and_keeps_what_it_was_told() {
        let now = now_iso();
        let legacy = |provider: &str, kind: &str, extra: serde_json::Value| {
            let mut row = serde_json::json!({
                "id": "11111111-2222-3333-4444-555555555555",
                "label": "old",
                "kind": kind,
                "provider": provider,
                "secret": "sk-ant-legacy-token",
                "secretSha256": "x",
                "usageEnabled": false,
                "env": [{"key": "OPENAI_API_KEY", "value": "sk-legacy"}, {"key": "ANTHROPIC_API_KEY", "value": "sk-ant-legacy"}],
            });
            row.as_object_mut()
                .expect("object")
                .extend(extra.as_object().cloned().unwrap_or_default());
            sanitize_account(&row, &now).expect("a readable row").to_public().rotation
        };
        assert!(
            legacy("claude", "oauth_token", serde_json::json!({})),
            "an OAuth claude account is in by default"
        );
        assert!(!legacy("claude", "env", serde_json::json!({})), "an env account is out by default");
        assert!(!legacy("codex", "env", serde_json::json!({})), "and so is every codex account");
        assert!(
            legacy("", "", serde_json::json!({})),
            "a row older than both fields is an OAuth claude account, and in"
        );
        assert!(
            !legacy("claude", "oauth_token", serde_json::json!({"rotation": false})),
            "an explicit no stands"
        );
        assert!(
            legacy("claude", "env", serde_json::json!({"rotation": true})),
            "and so does an explicit yes"
        );

        let (_dir, store) = store();
        let token = store
            .create("Work", AccountProvider::Claude, oauth("sk-ant-token-one"))
            .await
            .expect("create");
        assert!(token.rotation);
        let block = store
            .create_with(
                "Relay",
                AccountProvider::Claude,
                env("ANTHROPIC_AUTH_TOKEN=t\nANTHROPIC_BASE_URL=https://relay.example"),
                Some(true),
            )
            .await
            .expect("create");
        assert!(block.rotation, "put in the pool when it was created");
        let out = store
            .update(
                &token.id,
                AccountPatch {
                    rotation: Some(false),
                    ..Default::default()
                },
            )
            .await
            .expect("update")
            .expect("found");
        assert!(!out.rotation);
        let listed = store.list().await;
        assert_eq!(
            listed.iter().map(|account| account.rotation).collect::<Vec<_>>(),
            vec![false, true],
            "and it is stored"
        );
        let wire = serde_json::to_value(&listed[0]).expect("json");
        assert_eq!(wire["rotation"], false, "always on the wire: {wire}");
    }

    #[test]
    fn only_a_uuid_names_a_directory() {
        assert!(is_uuid("11111111-2222-3333-4444-555555555555"));
        assert!(!is_uuid("../../etc"));
        assert!(!is_uuid("11111111-2222-3333-4444-55555555555"), "one digit short");
        assert!(!is_uuid("11111111-2222-3333-4444-555555555555-x"));
    }

    #[test]
    fn a_secret_must_be_a_single_unpadded_token() {
        assert!(matches!(normalize_secret("  "), Err(AccountError::Invalid(_))));
        assert!(matches!(normalize_secret("a b"), Err(AccountError::Invalid(_))));
        assert!(matches!(normalize_secret(&"x".repeat(5000)), Err(AccountError::Invalid(_))));
        assert_eq!(normalize_secret("  sk-ant-x  ").expect("trimmed"), "sk-ant-x");
    }

    #[tokio::test]
    async fn the_same_token_twice_is_a_duplicate_naming_the_first_label() {
        let (_dir, store) = store();
        let first = store
            .create("Work", AccountProvider::Claude, oauth("sk-ant-token"))
            .await
            .expect("create");
        let clash = store.create("Home", AccountProvider::Claude, oauth(" sk-ant-token ")).await;
        match clash {
            Err(AccountError::Duplicate { existing_id, existing_label }) => {
                assert_eq!(existing_id, first.id);
                assert_eq!(existing_label, "Work");
            }
            other => panic!("expected a duplicate, got {other:?}"),
        }
    }

    /// The inverse of the test that used to pin the mirror's bytes, its `0600`
    /// mode and its tmp+rename: there is no second copy of anyone's OAuth
    /// refresh token on disk to get any of those wrong. The one copy left is the
    /// database, which is `0600` for the reason the file was.
    #[tokio::test]
    async fn creating_an_account_writes_no_file() {
        let (_dir, store) = store();
        store
            .create("Work", AccountProvider::Claude, oauth("sk-ant-token"))
            .await
            .expect("create");

        let file = store.paths.config_dir().join("accounts.json");
        assert!(!file.exists(), "no file is created where there was none");
        assert!(!std::fs::exists(file.with_extension("json.tmp")).unwrap_or(false));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(store.paths.config_db()).expect("stat").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the database holds the secrets now, so it is as private as the file was");
        }
    }

    /// The model picker needs to know which model a block runs, and must not
    /// be handed the token sitting in the same block.
    #[tokio::test]
    async fn only_the_named_variables_of_a_block_come_back_out_of_it() {
        let (_dir, store) = store();
        let account = store
            .create(
                "Gateway",
                AccountProvider::Claude,
                env("ANTHROPIC_AUTH_TOKEN=sk-ant-secret-value\nANTHROPIC_BASE_URL=https://gw.example.com\nANTHROPIC_MODEL=GLM-5.3\nCLAUDE_CODE_EFFORT_LEVEL=max"),
            )
            .await
            .expect("create");

        let values = store.env_values(&account.id, &["ANTHROPIC_MODEL", "CLAUDE_CODE_EFFORT_LEVEL"]).await;
        assert_eq!(values.get("ANTHROPIC_MODEL").map(String::as_str), Some("GLM-5.3"));
        assert_eq!(values.get("CLAUDE_CODE_EFFORT_LEVEL").map(String::as_str), Some("max"));
        assert_eq!(values.len(), 2, "the keys that were asked for, and not one more: {values:?}");

        assert!(store.env_values(&account.id, &[]).await.is_empty(), "asking for nothing reads nothing");
        assert!(
            store
                .env_values("22222222-2222-2222-2222-222222222222", &["ANTHROPIC_MODEL"])
                .await
                .is_empty(),
            "an account nobody has is a block nobody reads"
        );

        // An OAuth account has no block, and the accessor is not a way to look.
        let oauth = store
            .create("Login", AccountProvider::Claude, oauth("sk-ant-oat01-whatever"))
            .await
            .expect("create");
        assert!(store.env_values(&oauth.id, &["ANTHROPIC_MODEL"]).await.is_empty());
    }

    /// What a resident is fingerprinted by, so that editing a block retires the
    /// process already running under it.
    #[tokio::test]
    async fn the_credential_hash_changes_when_the_credential_does() {
        let (_dir, store) = store();
        let account = store
            .create(
                "Gateway",
                AccountProvider::Claude,
                env("ANTHROPIC_AUTH_TOKEN=sk-ant-value\nANTHROPIC_BASE_URL=https://gw.example.com/v1"),
            )
            .await
            .expect("create");
        let before = store.credential_hash(&account.id).await.expect("a hash");
        assert!(!before.is_empty());
        assert!(!before.contains("sk-ant-value"), "a name for the credential, not the credential");
        assert_eq!(
            store.credential_hash(&account.id).await.as_deref(),
            Some(before.as_str()),
            "stable while nothing changes"
        );

        // The same edit the picker exists to make painless: one variable fixed.
        store
            .update(
                &account.id,
                AccountPatch {
                    credential: Some(env("ANTHROPIC_AUTH_TOKEN=sk-ant-value\nANTHROPIC_BASE_URL=https://gw.example.com")),
                    ..Default::default()
                },
            )
            .await
            .expect("update");
        let after = store.credential_hash(&account.id).await.expect("a hash");
        assert_ne!(before, after, "the block is a different credential, and the running process is not on it");
        assert_eq!(store.credential_hash("22222222-2222-2222-2222-222222222222").await, None);
    }

    #[tokio::test]
    async fn a_public_account_never_carries_the_secret() {
        let (_dir, store) = store();
        let account = store
            .create("Work", AccountProvider::Claude, env("ANTHROPIC_API_KEY=sk-ant-secret-value"))
            .await
            .expect("create");
        let json = serde_json::to_string(&account).expect("serialize");
        assert!(!json.contains("sk-ant-secret-value"), "the API shape has no field for it: {json}");
        assert!(!json.contains("secretSha256"), "not even the hash: {json}");
        assert_eq!(
            store.account_env(&account.id).await.expect("env").env,
            vec![("ANTHROPIC_API_KEY".to_string(), "sk-ant-secret-value".to_string())]
        );
    }

    #[tokio::test]
    async fn deleting_an_account_takes_its_default_status_and_bindings_with_it() {
        let (_dir, store) = store();
        let account = store.create("Work", AccountProvider::Claude, oauth("sk-ant-one")).await.expect("create");
        assert!(store.set_default(Some(&account.id), AccountProvider::Claude).await.expect("set default"));
        store
            .bind_session_account("session-a", Some(&account.id), AccountProvider::Claude)
            .await
            .expect("bind");
        let probe_dir = store.usage_config_dir(&account.id).expect("dir");
        std::fs::create_dir_all(&probe_dir).expect("probe dir");

        assert!(store.delete(&account.id).await.expect("delete"));
        assert_eq!(store.default_account_id(AccountProvider::Claude).await, None);
        assert_eq!(store.session_account_id("session-a").await, None);
        assert!(!probe_dir.exists(), "the probe login goes with the account");
        assert!(!store.delete(&account.id).await.expect("delete"), "a second delete is a no-op");
    }

    #[tokio::test]
    async fn binding_to_an_unknown_account_is_refused() {
        let (_dir, store) = store();
        let error = store
            .bind_session_account("session-a", Some("11111111-2222-3333-4444-555555555555"), AccountProvider::Claude)
            .await
            .expect_err("unknown account");
        assert!(matches!(error, AccountError::NotFound(_)));
        // Unbinding is always allowed, even for a session that was never bound.
        store
            .bind_session_account("session-a", None, AccountProvider::Claude)
            .await
            .expect("unbind");
    }

    /// A deletion has one place to reach, and the credential is gone from the
    /// machine with it — there is no copy beside the table for it to survive in.
    #[tokio::test]
    async fn a_deletion_takes_the_credential_with_it() {
        let (_dir, store) = store();
        let account = store.create("Work", AccountProvider::Claude, oauth("sk-ant-one")).await.expect("create");
        assert_eq!(store.list().await.len(), 1);

        store.delete(&account.id).await.expect("delete");
        assert_eq!(store.list().await.len(), 0);
        assert!(payloads(&store).is_empty(), "and the credential is gone from the row as well");
    }

    /// Every account row's payload, exactly as the table holds it.
    fn payloads(store: &AccountsStore) -> Vec<String> {
        store
            .db
            .read(|conn| {
                let mut statement = conn.prepare("SELECT payload FROM accounts ORDER BY rowid")?;
                let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
                Ok(rows.filter_map(Result::ok).collect())
            })
            .expect("payloads")
    }

    /// The bindings are why this store was rewritten whole so often: there is
    /// one more of them every time a session starts under an account, and each
    /// one used to re-encode and re-`fsync` every account's OAuth token.
    #[tokio::test]
    async fn binding_a_session_writes_one_row_and_leaves_the_accounts_untouched() {
        let (_dir, store) = store();
        let work = store.create("Work", AccountProvider::Claude, oauth("sk-ant-one")).await.expect("create");
        store
            .create("Home", AccountProvider::Claude, env("ANTHROPIC_API_KEY=sk-ant-two"))
            .await
            .expect("create");
        let before = payloads(&store);
        let changes = |store: &AccountsStore| store.db.read(|conn| Ok(conn.total_changes())).expect("counter");
        let counted = changes(&store);

        store
            .bind_session_account("session-a", Some(&work.id), AccountProvider::Claude)
            .await
            .expect("bind");

        assert_eq!(changes(&store) - counted, 1, "one row changed: the binding, and nothing else");
        assert_eq!(payloads(&store), before, "the secrets were not so much as re-encoded");
        assert_eq!(store.session_account_id("session-a").await.as_deref(), Some(work.id.as_str()));

        // Unbinding is the same shape, and neither one writes anything else.
        store
            .bind_session_account("session-a", None, AccountProvider::Claude)
            .await
            .expect("unbind");
        assert_eq!(payloads(&store), before);
        assert_eq!(store.session_account_id("session-a").await, None);
    }

    /// A row is a row on disk, not a copy in this process.
    #[tokio::test]
    async fn an_account_survives_a_reopen_of_the_database() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Arc::new(Paths::new(dir.path().to_path_buf()));
        let store = AccountsStore::new(paths.clone());
        let account = store.create("Work", AccountProvider::Claude, oauth("sk-ant-one")).await.expect("create");
        store
            .bind_session_account("session-a", Some(&account.id), AccountProvider::Claude)
            .await
            .expect("bind");

        // A second connection to the same file, as another process would open it.
        let reopened = crate::db::Db::open(paths.config_db());
        let bound: String = reopened
            .read(|conn| {
                conn.query_row("SELECT account_id FROM session_accounts WHERE session_id = 'session-a'", [], |row| {
                    row.get(0)
                })
            })
            .expect("the binding is on disk");
        assert_eq!(bound, account.id);
        let payload: String = reopened
            .read(|conn| conn.query_row("SELECT payload FROM accounts WHERE id = ?1", (&account.id,), |row| row.get(0)))
            .expect("the account is on disk");
        assert!(payload.contains("sk-ant-one"));
    }

    #[tokio::test]
    async fn updating_email_and_usage_round_trips_through_disk() {
        let (_dir, store) = store();
        let account = store.create("Work", AccountProvider::Claude, oauth("sk-ant-one")).await.expect("create");
        let patched = store
            .update(
                &account.id,
                AccountPatch {
                    email: Some("me@example.com".into()),
                    usage_enabled: Some(true),
                    ..Default::default()
                },
            )
            .await
            .expect("update")
            .expect("found");
        assert_eq!(patched.email.as_deref(), Some("me@example.com"));
        assert!(patched.usage_enabled);

        // A second store proves the values are in the database rather than in
        // this store's head.
        let reopened = AccountsStore::new(store.paths.clone());
        let reloaded = reopened.get(&account.id).await.expect("still there");
        assert_eq!(reloaded.email.as_deref(), Some("me@example.com"));

        let cleared = store
            .update(
                &account.id,
                AccountPatch {
                    email: Some(String::new()),
                    ..Default::default()
                },
            )
            .await
            .expect("update")
            .expect("found");
        assert_eq!(cleared.email.as_deref(), None, "an empty email removes the field");
        assert!(store
            .update("11111111-2222-3333-4444-555555555555", AccountPatch::default())
            .await
            .expect("ok")
            .is_none());
    }

    /// The whole of the box's grammar, in one place.
    #[test]
    fn a_block_is_read_the_way_a_person_pastes_one() {
        let parsed = parse_env_block(
            "# a comment\n\
             \n\
             ANTHROPIC_API_KEY=sk-ant-plain\n\
             export ANTHROPIC_BASE_URL=https://gw.example.com/v1\n\
               ANTHROPIC_AUTH_TOKEN = \"quoted value\"  \n\
             SINGLE='single quoted'\n\
             EMPTY=\n\
             WITH_EQUALS=a=b=c\n",
        )
        .expect("a block a person would actually type");
        assert_eq!(
            parsed,
            vec![
                EnvVar {
                    key: "ANTHROPIC_API_KEY".into(),
                    value: "sk-ant-plain".into()
                },
                EnvVar {
                    key: "ANTHROPIC_BASE_URL".into(),
                    value: "https://gw.example.com/v1".into()
                },
                EnvVar {
                    key: "ANTHROPIC_AUTH_TOKEN".into(),
                    value: "quoted value".into()
                },
                EnvVar {
                    key: "SINGLE".into(),
                    value: "single quoted".into()
                },
                EnvVar {
                    key: "EMPTY".into(),
                    value: String::new()
                },
                EnvVar {
                    key: "WITH_EQUALS".into(),
                    value: "a=b=c".into()
                },
            ],
            "order is kept, `export` and quotes come off, the first `=` splits"
        );
        // No escape processing: a backslash in a token is a backslash.
        assert_eq!(parse_env_block("K=a\\nb").expect("literal")[0].value, "a\\nb");
        assert_eq!(parse_env_block("_UNDERSCORE1=x").expect("a legal name")[0].key, "_UNDERSCORE1");
    }

    /// The editor opens on what is stored, so reading a block back must produce
    /// text that parses to the same variables — an edit may change one line
    /// without the others silently changing with it.
    #[tokio::test]
    async fn an_env_accounts_block_reads_back_as_text_that_parses_to_itself() {
        let (_dir, store) = store();
        let block = "ANTHROPIC_AUTH_TOKEN=sk-ant-one\n\
                     ANTHROPIC_BASE_URL=https://gw.example.com\n\
                     PADDED=\"  spaced  \"\n\
                     EMPTY=\n\
                     WITH_EQUALS=a=b=c";
        let account = store.create("Gateway", AccountProvider::Claude, env(block)).await.expect("create");
        let text = store.env_block(&account.id).await.expect("read back");
        assert_eq!(
            parse_env_block(&text).expect("the text the editor shows is a block"),
            parse_env_block(block).expect("the block that was stored"),
            "leading and trailing spaces survive inside quotes"
        );

        let token_account = store.create("Work", AccountProvider::Claude, oauth("sk-ant-two")).await.expect("create");
        assert!(
            matches!(store.env_block(&token_account.id).await, Err(AccountError::Invalid(_))),
            "an OAuth account has no environment block to read"
        );
        assert!(matches!(
            store.env_block("11111111-2222-3333-4444-555555555555").await,
            Err(AccountError::NotFound(_))
        ));
    }

    /// A line that is not a pair is a typo, and dropping it silently would
    /// store a block that is missing the variable the user thinks they set.
    #[test]
    fn a_block_refuses_rather_than_skips_what_it_cannot_read() {
        let refused = |block: &str| match parse_env_block(block) {
            Err(AccountError::Invalid(message)) => message,
            other => panic!("expected a refusal for {block:?}, got {other:?}"),
        };

        assert!(refused("A=1\njust some words\n").contains("line 2"), "the refusal names the line");
        assert!(refused("A=1\n1BAD=2").contains("line 2"));
        assert!(refused("no-equals-at-all").contains("line 1"));
        // And never the text it refused: the likeliest thing on a line that is
        // not a pair is the credential, pasted before its name was typed, and
        // this message is a `400` body.
        let pasted = refused("ANTHROPIC_AUTH_TOKEN sk-ant-oat01-PastedBeforeItsName");
        assert!(!pasted.contains("sk-ant"), "a refusal must not quote the line: {pasted}");
        let odd = refused("sk-ant-oat01-PastedBeforeItsName=");
        assert!(!odd.contains("sk-ant"), "nor the half of it before the `=`: {odd}");
        assert!(refused("A=1\nA=2").contains("A is set twice"));
        assert!(refused("").contains("empty"), "an empty box is not a credential");
        assert!(refused("# only a comment\n\n").contains("empty"));

        for denied in DENIED_ENV_KEYS {
            let message = refused(&format!("OPENAI_API_KEY=k\n{denied}=/somewhere"));
            assert!(message.contains(denied), "the refusal names the key: {message}");
        }

        let many = (0..=MAX_ENV_VARS).map(|i| format!("K{i}=v")).collect::<Vec<_>>().join("\n");
        assert!(refused(&many).contains("more than"));
        assert!(refused(&format!("K=\"{}\"", "x".repeat(MAX_ENV_BYTES))).contains("KiB"));
    }

    /// v0.2.58 shipped four key-account shapes. Every one of them has to come
    /// back as the block it was always shorthand for, or an account that worked
    /// before the upgrade stops working after it.
    #[test]
    fn every_v1_key_account_is_read_as_the_block_it_was_shorthand_for() {
        let now = "2024-01-01T00:00:00.000Z";
        let id = "11111111-2222-3333-4444-555555555555";
        let row = |kind: &str, provider: &str, base_url: Option<&str>| {
            let mut object = serde_json::json!({
                "id": id, "label": "From v1", "kind": kind, "provider": provider,
                "secret": "the-key", "secretSha256": "stale-hash-of-the-bare-token"
            });
            if let Some(base_url) = base_url {
                object["baseUrl"] = serde_json::json!(base_url);
            }
            sanitize_account(&object, now).expect("a v1 row is still an account")
        };
        let pairs = |account: &StoredAccount| account.env.iter().map(|var| (var.key.clone(), var.value.clone())).collect::<Vec<_>>();

        let api = row("api_key", "claude", None);
        assert_eq!(api.kind, AccountKind::Env);
        assert_eq!(api.secret, "", "the bare token field is emptied on the way in");
        assert_eq!(pairs(&api), vec![("ANTHROPIC_API_KEY".into(), "the-key".into())]);

        let auth = row("auth_token", "claude", Some("https://gw/v1"));
        assert_eq!(
            pairs(&auth),
            vec![
                ("ANTHROPIC_AUTH_TOKEN".to_string(), "the-key".to_string()),
                ("ANTHROPIC_BASE_URL".to_string(), "https://gw/v1".to_string()),
            ]
        );
        assert_eq!(pairs(&row("api_key", "claude", Some("https://gw/v1")))[1].0, "ANTHROPIC_BASE_URL");

        // The codex pair is the one that moves: v1 stored the key under the
        // variable the *injected provider* reads, and a block says what the
        // user's key is and lets `launch_for` do the translating.
        let codex = row("api_key", "codex", Some("https://relay/openai"));
        assert_eq!(
            pairs(&codex),
            vec![
                ("OPENAI_API_KEY".to_string(), "the-key".to_string()),
                ("OPENAI_BASE_URL".to_string(), "https://relay/openai".to_string()),
            ]
        );
        assert_eq!(
            codex.secret_sha256,
            block_sha256(&codex.env),
            "and the stale hash of the bare token is replaced, so duplicates compare blocks"
        );
        // A v1 row with nothing to convert is not an account.
        assert!(sanitize_account(&serde_json::json!({"id": id, "kind": "api_key"}), now).is_none());
    }

    /// A row written before any of this, and a row this build cannot read.
    #[test]
    fn an_unreadable_row_is_dropped_where_an_old_one_is_read_as_claude() {
        let now = "2024-01-01T00:00:00.000Z";
        let id = "11111111-2222-3333-4444-555555555555";
        let parse = |raw: serde_json::Value| sanitize_account(&raw, now);

        let legacy = parse(serde_json::json!({"id": id, "secret": "sk-ant-x"})).expect("a row with no kind at all");
        assert_eq!(legacy.provider, AccountProvider::Claude);
        assert_eq!(legacy.kind, AccountKind::OauthToken);

        let block = parse(serde_json::json!({
            "id": id, "kind": "env", "provider": "codex",
            "env": [{"key": "OPENAI_API_KEY", "value": "k"}, {"key": "not a name", "value": "x"}]
        }))
        .expect("a row this build wrote");
        assert_eq!(block.env.len(), 1, "an unusable pair is dropped, the row survives");

        assert!(
            parse(serde_json::json!({"id": id, "secret": "x", "kind": "device_code"})).is_none(),
            "unknown kind"
        );
        assert!(
            parse(serde_json::json!({"id": id, "secret": "x", "provider": "gemini"})).is_none(),
            "unknown CLI"
        );
        assert!(
            parse(serde_json::json!({"id": id, "secret": "x", "kind": "oauth_token", "provider": "codex"})).is_none(),
            "a pairing no spawn could use"
        );
        assert!(
            parse(serde_json::json!({"id": id, "kind": "env", "provider": "claude", "env": []})).is_none(),
            "an env account with no variables is not an account"
        );
    }

    /// What each shape actually puts into a child process.
    #[test]
    fn a_launch_hands_claude_the_block_and_translates_it_for_codex() {
        fn pairs(launch: &AccountLaunch) -> Vec<(&str, &str)> {
            launch.env.iter().map(|(key, value)| (key.as_str(), value.as_str())).collect()
        }
        let block = |text: &str| parse_env_block(text).expect("block");

        let oauth = launch_for(AccountProvider::Claude, AccountKind::OauthToken, "tok", &[]).expect("oauth");
        assert_eq!(pairs(&oauth), vec![("CLAUDE_CODE_OAUTH_TOKEN", "tok")]);
        assert!(oauth.codex_args.is_empty());

        // Claude takes the block exactly as typed — including a setup with no
        // recognisable credential variable in it at all, which is what Bedrock
        // and a helper script look like. The CLI decides, not this.
        let claude = launch_for(
            AccountProvider::Claude,
            AccountKind::Env,
            "",
            &block("ANTHROPIC_AUTH_TOKEN=bearer\nANTHROPIC_BASE_URL=https://gw/v1"),
        )
        .expect("claude env");
        assert_eq!(
            pairs(&claude),
            vec![("ANTHROPIC_AUTH_TOKEN", "bearer"), ("ANTHROPIC_BASE_URL", "https://gw/v1")]
        );
        assert!(claude.codex_args.is_empty());
        assert!(launch_for(
            AccountProvider::Claude,
            AccountKind::Env,
            "",
            &block("CLAUDE_CODE_USE_BEDROCK=1\nAWS_REGION=us-east-1")
        )
        .is_ok());

        // Codex gets the block *and* the translation on top of it.
        let codex = launch_for(AccountProvider::Codex, AccountKind::Env, "", &block("OPENAI_API_KEY=sk-codex")).expect("codex");
        assert_eq!(
            pairs(&codex),
            vec![("OPENAI_API_KEY", "sk-codex"), ("DROPPEN_CODEX_API_KEY", "sk-codex")]
        );
        assert_eq!(
            codex.codex_args,
            vec![
                "-c".to_string(),
                "model_providers.remote_hand={name=\"remote-hand\",base_url=\"https://api.openai.com/v1\",env_key=\"DROPPEN_CODEX_API_KEY\",wire_api=\"responses\"}".to_string(),
                "-c".to_string(),
                "model_provider=remote_hand".to_string(),
            ]
        );
        assert!(
            !codex.codex_args.iter().any(|arg| arg.contains("sk-codex")),
            "a command line is world-readable in `ps`"
        );

        // A blank base URL is not a broken one: it is the vendor default, as it
        // was when the URL was a field that could be cleared.
        let blank = launch_for(
            AccountProvider::Codex,
            AccountKind::Env,
            "",
            &block("OPENAI_API_KEY=sk-1\nOPENAI_BASE_URL=  "),
        )
        .expect("a block that overrides nothing");
        assert!(
            blank.codex_args[1].contains(&format!("base_url=\"{CODEX_DEFAULT_BASE_URL}\"")),
            "{:?}",
            blank.codex_args
        );

        // `CODEX_API_KEY` is the fallback, and `OPENAI_BASE_URL` moves the provider.
        let relay = launch_for(
            AccountProvider::Codex,
            AccountKind::Env,
            "",
            &block("CODEX_API_KEY=sk-relay\nOPENAI_BASE_URL=https://relay.example.com/v1"),
        )
        .expect("codex relay");
        assert_eq!(
            relay.env.last().expect("translated"),
            &("DROPPEN_CODEX_API_KEY".to_string(), "sk-relay".to_string())
        );
        assert!(
            relay.codex_args[1].contains("base_url=\"https://relay.example.com/v1\""),
            "{:?}",
            relay.codex_args
        );
        // `OPENAI_API_KEY` wins when both are present.
        let both = launch_for(
            AccountProvider::Codex,
            AccountKind::Env,
            "",
            &block("CODEX_API_KEY=second\nOPENAI_API_KEY=first"),
        )
        .expect("both");
        assert_eq!(both.env.last().expect("translated").1, "first");

        // Nothing to translate, and nothing a CLI could be pointed at.
        let missing = launch_for(AccountProvider::Codex, AccountKind::Env, "", &block("SOME_OTHER_VAR=x")).expect_err("no key");
        assert!(missing.to_string().contains("OPENAI_API_KEY"), "{missing}");
        for bad in [
            "OPENAI_BASE_URL=relay.example.com",
            "OPENAI_BASE_URL=https:///v1",
            "OPENAI_BASE_URL=https://a\"b",
        ] {
            let refused = launch_for(AccountProvider::Codex, AccountKind::Env, "", &block(&format!("OPENAI_API_KEY=k\n{bad}")));
            assert!(matches!(refused, Err(AccountError::Invalid(_))), "{bad} must be refused");
        }
    }

    /// Duplicates compare the whole block, order-independently, per CLI.
    #[tokio::test]
    async fn a_block_is_the_same_credential_however_its_lines_are_ordered() {
        let (_dir, store) = store();
        assert_eq!(
            block_sha256(&parse_env_block("A=1\nB=2").expect("block")),
            block_sha256(&parse_env_block("B=2\nA=1").expect("block")),
            "the same variables are the same credential to every CLI that reads them"
        );
        assert_ne!(
            block_sha256(&parse_env_block("A=1\nB=2").expect("block")),
            block_sha256(&parse_env_block("A=1\nB=3").expect("block"))
        );

        store
            .create(
                "Gateway",
                AccountProvider::Claude,
                env("ANTHROPIC_API_KEY=k\nANTHROPIC_BASE_URL=https://gw/v1"),
            )
            .await
            .expect("create");
        let clash = store
            .create(
                "Same, reordered",
                AccountProvider::Claude,
                env("ANTHROPIC_BASE_URL=https://gw/v1\nANTHROPIC_API_KEY=k"),
            )
            .await;
        assert!(
            matches!(clash, Err(AccountError::Duplicate { ref existing_label, .. }) if existing_label == "Gateway"),
            "{clash:?}"
        );
        // One string, two credentials: the same key at a relay that fronts both
        // vendors is two accounts, not a merge.
        store
            .create(
                "Relay (codex)",
                AccountProvider::Codex,
                env("OPENAI_API_KEY=k\nANTHROPIC_BASE_URL=https://gw/v1"),
            )
            .await
            .expect("the other CLI is not a duplicate");
        assert_eq!(store.list().await.len(), 2);
    }

    /// A block is a credential in as many pieces as the user typed, and a CLI
    /// refused by one of them quotes that one back into `injection.error`.
    #[tokio::test]
    async fn every_value_of_a_block_is_taken_out_of_a_clis_complaint_by_value() {
        let (_dir, store) = store();
        store
            .create(
                "Gateway",
                AccountProvider::Claude,
                env("ANTHROPIC_AUTH_TOKEN=hunter2-gateway-token\nANTHROPIC_BASE_URL=https://gw.example.com/v1\nSHORT=xy"),
            )
            .await
            .expect("create");

        let redacted = store.redact_known_secrets("401 for hunter2-gateway-token at https://gw.example.com/v1 (SHORT=xy)");
        assert!(!redacted.contains("hunter2"), "{redacted}");
        assert!(
            !redacted.contains("gw.example.com"),
            "a base URL is a value of the block like any other: {redacted}"
        );
        assert!(
            redacted.contains("SHORT=xy"),
            "a two-character value is a word, not a credential: {redacted}"
        );
        assert!(redacted.contains("401 for [redacted]"), "{redacted}");
        assert_eq!(store.redact_known_secrets("nothing to do here"), "nothing to do here");
        assert_eq!(store.redact_known_secrets(""), "");
    }

    /// An env account for the wrong CLI, and a credential of the wrong shape.
    #[tokio::test]
    async fn a_row_nothing_could_spawn_is_refused_before_it_is_stored() {
        let (_dir, store) = store();
        assert!(matches!(
            store.create("Nope", AccountProvider::Codex, oauth("tok")).await,
            Err(AccountError::Invalid(_))
        ));
        // A codex block with no key variable never reaches a spawn, so it never
        // reaches the table either.
        assert!(matches!(
            store.create("Nope", AccountProvider::Codex, env("SOMETHING_ELSE=x")).await,
            Err(AccountError::Invalid(_))
        ));
        assert!(store.list().await.is_empty(), "neither refusal left a row behind");

        let account = store
            .create("Gateway", AccountProvider::Claude, env("ANTHROPIC_AUTH_TOKEN=bearer"))
            .await
            .expect("create");
        assert_eq!(account.kind, AccountKind::Env);
        assert_eq!(
            account.env_keys.as_ref().map(Vec::as_slice),
            Some(["ANTHROPIC_AUTH_TOKEN".to_string()].as_slice())
        );
        let json = serde_json::to_string(&account).expect("serialize");
        assert!(!json.contains("bearer"), "a value is shown back to nobody: {json}");

        // A patch has to match the account's own kind in both directions.
        assert!(matches!(
            store
                .update(
                    &account.id,
                    AccountPatch {
                        credential: Some(oauth("tok")),
                        ..Default::default()
                    }
                )
                .await,
            Err(AccountError::Invalid(_))
        ));
        let replaced = store
            .update(
                &account.id,
                AccountPatch {
                    credential: Some(env("ANTHROPIC_AUTH_TOKEN=bearer2\nANTHROPIC_BASE_URL=https://gw/v1")),
                    ..Default::default()
                },
            )
            .await
            .expect("update")
            .expect("found");
        assert_eq!(
            replaced.env_keys.as_ref().map(Vec::as_slice),
            Some(["ANTHROPIC_AUTH_TOKEN".to_string(), "ANTHROPIC_BASE_URL".to_string()].as_slice()),
            "the block is replaced whole: what is not in the text is not set"
        );
    }

    /// Two CLIs, two defaults, one delete.
    #[tokio::test]
    async fn each_cli_keeps_its_own_default_and_a_deletion_clears_only_its_own() {
        let (_dir, store) = store();
        let claude = store.create("Work", AccountProvider::Claude, oauth("sk-ant-one")).await.expect("create");
        let codex = store
            .create("Codex", AccountProvider::Codex, env("OPENAI_API_KEY=sk-codex-one"))
            .await
            .expect("create");

        assert!(store
            .set_default(Some(&claude.id), AccountProvider::Claude)
            .await
            .expect("claude default"));
        assert!(store.set_default(Some(&codex.id), AccountProvider::Codex).await.expect("codex default"));
        assert_eq!(
            store.default_account_id(AccountProvider::Claude).await.as_deref(),
            Some(claude.id.as_str())
        );
        assert_eq!(store.default_account_id(AccountProvider::Codex).await.as_deref(), Some(codex.id.as_str()));
        let list = store.account_list().await;
        assert_eq!(list.default_account_id.0.as_deref(), Some(claude.id.as_str()));
        assert_eq!(list.default_codex_account_id.0.as_deref(), Some(codex.id.as_str()));

        // A default nothing can spawn under is refused rather than stored.
        assert!(matches!(
            store.set_default(Some(&codex.id), AccountProvider::Claude).await,
            Err(AccountError::Invalid(_))
        ));
        assert_eq!(
            store.default_account_id(AccountProvider::Claude).await.as_deref(),
            Some(claude.id.as_str())
        );

        assert!(store.delete(&codex.id).await.expect("delete"));
        assert_eq!(store.default_account_id(AccountProvider::Codex).await, None);
        assert_eq!(
            store.default_account_id(AccountProvider::Claude).await.as_deref(),
            Some(claude.id.as_str()),
            "the other CLI's default is untouched"
        );
    }

    /// One binding table for both CLIs, so the pairing is checked on the write.
    #[tokio::test]
    async fn a_session_cannot_be_bound_to_the_other_clis_account() {
        let (_dir, store) = store();
        let codex = store
            .create("Codex", AccountProvider::Codex, env("OPENAI_API_KEY=sk-codex-one"))
            .await
            .expect("create");

        let error = store
            .bind_session_account("thread-a", Some(&codex.id), AccountProvider::Claude)
            .await
            .expect_err("a codex key cannot run a claude session");
        assert!(
            matches!(error, AccountError::ProviderMismatch { account, wanted }
                if account == AccountProvider::Codex && wanted == AccountProvider::Claude),
            "{error:?}"
        );
        assert_eq!(error.to_string(), "provider_mismatch", "the client keys on the token, not the sentence");
        assert_eq!(store.session_account_id("thread-a").await, None, "and nothing was written");

        store
            .bind_session_account("thread-a", Some(&codex.id), AccountProvider::Codex)
            .await
            .expect("its own CLI");
        assert_eq!(store.session_account_id("thread-a").await.as_deref(), Some(codex.id.as_str()));
        assert_eq!(store.account_env(&codex.id).await.expect("env").provider, AccountProvider::Codex);
    }

    /// One binding table, two CLIs, uuids for ids: "a row exists" is not the
    /// same question as "this CLI knows this session", and every ownership
    /// shortcut in the routes turns on the second one.
    #[tokio::test]
    async fn a_binding_only_answers_for_the_cli_whose_account_it_names() {
        let (_dir, store) = store();
        let claude = store.create("Work", AccountProvider::Claude, oauth("sk-ant-one")).await.expect("create");
        let codex = store
            .create("Codex", AccountProvider::Codex, env("OPENAI_API_KEY=sk-codex-one"))
            .await
            .expect("create");

        // The same id under both routes: a codex thread bound to a codex key.
        store
            .bind_session_account("shared-id", Some(&codex.id), AccountProvider::Codex)
            .await
            .expect("bind");
        assert_eq!(
            store.session_account_id_for("shared-id", AccountProvider::Codex).await.as_deref(),
            Some(codex.id.as_str())
        );
        assert_eq!(
            store.session_account_id_for("shared-id", AccountProvider::Claude).await,
            None,
            "the claude route must not read a codex binding as a session it owns"
        );

        // And the mirror image.
        store
            .bind_session_account("shared-id", Some(&claude.id), AccountProvider::Claude)
            .await
            .expect("rebind");
        assert_eq!(
            store.session_account_id_for("shared-id", AccountProvider::Claude).await.as_deref(),
            Some(claude.id.as_str())
        );
        assert_eq!(store.session_account_id_for("shared-id", AccountProvider::Codex).await, None);

        // An id nobody has bound answers for neither.
        assert_eq!(store.session_account_id_for("nothing", AccountProvider::Claude).await, None);
        assert_eq!(store.session_account_id_for("nothing", AccountProvider::Codex).await, None);
    }

    /// Re-authorization is a *replacement*, not a new account: the id is what
    /// `sessionAccounts` binds sessions to, so a session mid-conversation has to
    /// come out the other side still bound.
    #[tokio::test]
    async fn re_authorizing_swaps_the_credential_and_keeps_the_id() {
        let (_dir, store) = store();
        let account = store.create("Work", AccountProvider::Claude, oauth("sk-ant-old")).await.expect("create");
        store
            .bind_session_account("session-a", Some(&account.id), AccountProvider::Claude)
            .await
            .expect("bind");

        let patch = |secret: &str| AccountPatch {
            credential: Some(oauth(secret)),
            ..Default::default()
        };
        let updated = store.update(&account.id, patch("  sk-ant-new  ")).await.expect("update").expect("found");
        assert_eq!(updated.id, account.id, "the id survives, and with it the bindings");
        assert_eq!(updated.label, "Work");
        assert_eq!(store.session_account_id("session-a").await.as_deref(), Some(account.id.as_str()));

        // A fresh store proves the swap reached disk, hash and all.
        let reopened = AccountsStore::new(store.paths.clone());
        assert_eq!(reopened.account_env(&account.id).await.expect("env").env[0].1, "sk-ant-new");
        // The old token is gone, so it is free to be stored again elsewhere.
        reopened
            .create("Home", AccountProvider::Claude, oauth("sk-ant-old"))
            .await
            .expect("no clash");

        // Re-authorizing into the token this account already holds is a no-op…
        store.update(&account.id, patch("sk-ant-new")).await.expect("same token").expect("found");
        // …but taking another account's is a clash, not a merge.
        let clash = store.update(&account.id, patch("sk-ant-old")).await;
        assert!(
            matches!(clash, Err(AccountError::Duplicate { ref existing_label, .. }) if existing_label == "Home"),
            "{clash:?}"
        );
        assert!(matches!(store.update(&account.id, patch("  ")).await, Err(AccountError::Invalid(_))));
    }
}
