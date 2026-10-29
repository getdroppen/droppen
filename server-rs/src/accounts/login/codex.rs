//! Codex's part of the accounts: how a credential starts `codex`.

use super::Login;
use crate::accounts::store::{AccountError, AccountKind, AccountLaunch, AccountProvider, EnvVar, CODEX_BASE_URL_VAR, CODEX_KEY_CANDIDATES};

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

const MAX_BASE_URL_LENGTH: usize = 512;

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

/// Codex's [`Login`].
pub struct CodexLogin;

impl Login for CodexLogin {
    /// The block, plus the translation `app-server` needs to read a key at all.
    ///
    /// Fallible, unlike claude's: codex cannot be handed a block as it stands,
    /// because `app-server` reads no key variable of its own, so the block has
    /// to be *translated* — and a translation needs something to translate.
    fn spawn_env(&self, kind: AccountKind, secret: &str, env: &[EnvVar]) -> Result<AccountLaunch, AccountError> {
        let mut vars = kind.vars(secret, env);
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
        let codex_args = codex_provider_args(&base_url);
        Ok(AccountLaunch {
            provider: AccountProvider::Codex,
            env: vars,
            codex_args,
        })
    }
}
