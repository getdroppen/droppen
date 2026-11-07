//! `remote-hand-server notify` — what an injected `droppen-notify` actually
//! runs.
//!
//! # Why this is a subcommand and not a shell script
//!
//! The obvious `droppen-notify` is four lines of `sh` around `curl`, and it is
//! wrong for one reason: the arguments are prose written by a model. They contain
//! quotes, backticks, `$`, newlines, emoji and CJK, and every one of those is a
//! way for a hand-rolled JSON-in-shell encoder to produce a body the server
//! rejects — or, worse, a command substitution. A notification that says
//! "the build failed: `rm -rf $HOME`" must be a notification and not an event.
//!
//! So the only shell involved is the one line [`super::inject`] writes:
//!
//! ```sh
//! exec '<this binary>' notify "$@"
//! ```
//!
//! `"$@"` is the one construct in POSIX shell that passes arguments through
//! without re-splitting or re-expanding them, and everything after it is Rust:
//! real argument parsing, `serde_json` doing the encoding, and `reqwest` doing
//! the request. The binary is already re-executed for `pty-shim` and
//! `session-shim`, so this is the third use of a pattern the server relies on.
//!
//! # What it prints
//!
//! One line, and the audience is a language model reading tool output. A success
//! is short enough to be ignored; a failure says what to do about it, because
//! the alternative is an agent that retries a refusal six times and burns the
//! session's whole rate limit finding out.

use std::time::Duration;

use clap::Args;
use droppen_http::openapi_ext::Optional;

use super::{Level, NotifyBody};

/// Where `POST /api/notify` is, injected at spawn.
///
/// A session outlives the server that started it, and this is read by
/// whichever binary that server's path holds by then — older or newer — so
/// the name is shared by every version.
pub const URL_ENV: &str = droppen_base::process_env::NOTIFY_URL_ENV;
/// The session's capability, injected at spawn under a name as fixed as
/// [`URL_ENV`]'s. See [`super::token`].
pub const TOKEN_ENV: &str = droppen_base::process_env::NOTIFY_TOKEN_ENV;

/// How long the server is given to accept the notification.
///
/// Longer than [`super::bark`]'s own send timeout, so the message an agent gets
/// is the server's account of what Bark said rather than "timed out" — the
/// difference between "your device key is wrong" and no information at all.
const TIMEOUT: Duration = Duration::from_secs(20);

/// One parsed command line.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Options {
    /// The headline.
    pub title: String,
    /// The detail under it.
    pub body: String,
    /// `passive`, `active` or `time-sensitive`; the server defaults it.
    pub level: Option<String>,
    /// How the phone stacks these.
    pub group: Option<String>,
}

/// The command's name on a session's `PATH`.
pub const NAME: &str = "droppen-notify";

/// How to call it, said under the name a session knows it by: clap's errors
/// and help show it as the usage, and a call with no title gets it with
/// [`USAGE`].
pub const USAGE_LINE: &str = "droppen-notify <title> [body] [--level passive|active|time-sensitive] [--group <name>]";

/// What an agent is shown when it gets the call wrong.
pub const USAGE: &str = "\
usage: droppen-notify <title> [body] [--level passive|active|time-sensitive] [--group <name>]

Sends a push notification to the person running this Droppen server.
Use it when something has happened that they would want to be interrupted for.";

/// The `droppen-notify` command line. Positional-first on purpose: the title and
/// body are what a model writes when it has read one example. The flags are for
/// the two things there is no natural position for.
#[derive(Debug, PartialEq, Eq, Args)]
#[command(name = "droppen-notify", disable_help_subcommand = true, args_override_self = true)]
pub struct NotifyArgs {
    /// `passive`, `active` or `time-sensitive`; the server defaults it.
    #[arg(long, short = 'l', allow_hyphen_values = true)]
    pub level: Option<String>,
    /// How the phone stacks these.
    #[arg(long, short = 'g', allow_hyphen_values = true)]
    pub group: Option<String>,
    /// The headline, when it is not the first positional word.
    #[arg(long, short = 't', allow_hyphen_values = true)]
    pub title: Option<String>,
    /// The detail, when it is not the remaining positional words.
    #[arg(long, short = 'b', allow_hyphen_values = true)]
    pub body: Option<String>,
    /// The title and body words.
    #[arg(value_name = "title")]
    pub words: Vec<String>,
}

impl NotifyArgs {
    /// The options this command line asks for, or the refusal.
    fn options(&self) -> Result<Options, String> {
        let mut options = Options {
            title: self.title.clone().unwrap_or_default(),
            body: self.body.clone().unwrap_or_default(),
            level: self.level.clone(),
            group: self.group.clone(),
        };
        // The flags win over the positions, so `--title` can override the first
        // word of a shell-mangled command line rather than fighting with it.
        if options.title.is_empty() {
            options.title = self.words.first().cloned().unwrap_or_default();
        }
        if options.body.is_empty() && self.words.len() > 1 {
            options.body = self.words[1..].join(" ");
        }
        if options.title.trim().is_empty() {
            return Err(format!("a notification needs a title\n\n{USAGE}"));
        }
        Ok(options)
    }

    /// Send one notification, and say in one line what happened.
    ///
    /// `Err` is the process' exit code as well as its message: an agent that is
    /// blocked and could not say so needs to know that, because the alternative
    /// is waiting forever for an answer to a question nobody was asked.
    pub async fn run(self) -> Result<(), String> {
        let options = self.options()?;
        let payload = payload(&options)?;
        let (Some(url), Some(token)) = (droppen_base::env::var(URL_ENV), droppen_base::env::var(TOKEN_ENV)) else {
            return Err("this session cannot send notifications: no channel was configured when it started. \
                 Turn one on in Droppen's settings, under Notifications."
                .to_string());
        };
        let client = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .build()
            .map_err(|error| format!("cannot build an HTTP client: {error}"))?;
        let response = client
            .post(&url)
            .bearer_auth(token)
            .json(&payload)
            .send()
            .await
            .map_err(|error| format!("cannot reach the Droppen server: {error}"))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if status.is_success() {
            println!("notification sent: {}", options.title);
            return Ok(());
        }
        // The API answers a refusal with `{"error": …}`; that message was written
        // for this reader, so it is what gets printed rather than the status.
        let detail = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|value| value.get("error").and_then(|error| error.as_str()).map(str::to_string))
            .unwrap_or_else(|| text.trim().chars().take(300).collect());
        Err(if detail.is_empty() {
            format!("the notification was refused (http {status})")
        } else {
            format!("the notification was refused: {detail}")
        })
    }
}

/// What the server's tests of this command line reach (`cli::tests::notify`):
/// the options a parsed command line asks for.
#[cfg(any(test, feature = "test-support"))]
impl crate::testing::NotifyArgsTesting for NotifyArgs {
    fn options(&self) -> Result<Options, String> {
        NotifyArgs::options(self)
    }
}

/// The level names this accepts, mapped onto the ones the API takes.
///
/// `time-sensitive` as well as `timeSensitive` because a shell argument written
/// by hand is kebab-case and one copied off the API is camelCase, and being
/// refused over a hyphen is a bad way to find out a build has broken.
fn level(raw: &str) -> Result<Level, String> {
    match raw.trim().to_ascii_lowercase().replace(['-', '_'], "").as_str() {
        "passive" | "low" => Ok(Level::Passive),
        "active" | "normal" | "default" => Ok(Level::Active),
        "timesensitive" | "urgent" | "high" => Ok(Level::TimeSensitive),
        other => Err(format!("unknown level {other:?}; use passive, active or time-sensitive")),
    }
}

/// The JSON body one invocation sends.
///
/// Separate from [`NotifyArgs::run`] so the encoding can be asserted against the
/// prose a model actually writes — quotes, newlines, `$`, CJK — without a server.
pub fn payload(options: &Options) -> Result<NotifyBody, String> {
    Ok(NotifyBody {
        title: options.title.clone(),
        body: options.body.clone(),
        level: Optional(options.level.as_deref().map(level).transpose()?),
        group: Optional(options.group.clone().filter(|group| !group.trim().is_empty())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_level_spelling_a_shell_user_reaches_for_is_accepted() {
        assert_eq!(level("time-sensitive"), Ok(Level::TimeSensitive));
        assert_eq!(level("timeSensitive"), Ok(Level::TimeSensitive));
        assert_eq!(level("TIME_SENSITIVE"), Ok(Level::TimeSensitive));
        assert_eq!(level("urgent"), Ok(Level::TimeSensitive));
        assert_eq!(level(" active "), Ok(Level::Active));
        assert_eq!(level("passive"), Ok(Level::Passive));
        assert!(level("critical").is_err(), "the one level this server does not offer");
        assert!(level("shout").is_err());
    }

    /// The whole reason this is a Rust subcommand rather than `curl` in `sh`.
    #[test]
    fn prose_written_by_a_model_survives_being_encoded() {
        let hostile = r#"build failed: `rm -rf $HOME` "quoted" \backslash
        newline · 中文 · 🙂 · 100% of $(nothing)"#;
        let options = Options {
            title: hostile.to_string(),
            body: hostile.to_string(),
            level: Some("urgent".into()),
            group: Some("deploy".into()),
        };
        let payload = serde_json::to_value(payload(&options).expect("payload")).unwrap();
        assert_eq!(payload["title"], hostile, "every byte survives the round trip");
        assert_eq!(payload["body"], hostile);
        assert_eq!(payload["level"], "timeSensitive");
        assert_eq!(payload["group"], "deploy");
        // And the encoded form really is one JSON string, not a shell command.
        let encoded = serde_json::to_string(&payload).expect("encode");
        assert!(!encoded.contains("\n"), "a raw newline would end the JSON string: {encoded}");
    }

    /// The body `rh-notify` posts, byte for byte.
    #[test]
    fn the_payload_bytes() {
        let bytes = |options: &Options| serde_json::to_string(&payload(options).expect("payload")).unwrap();
        let full = Options {
            title: "Blocked".into(),
            body: "line one\nline \"two\"".into(),
            level: Some("time-sensitive".into()),
            group: Some("deploy".into()),
        };
        assert_eq!(bytes(&full), r#"{"title":"Blocked","body":"line one\nline \"two\"","level":"timeSensitive","group":"deploy"}"#);
        let bare = Options {
            title: "T".into(),
            body: String::new(),
            level: None,
            group: Some(" ".into()),
        };
        assert_eq!(bytes(&bare), r#"{"title":"T","body":""}"#);
        let quiet = Options {
            level: Some("low".into()),
            group: None,
            ..bare
        };
        assert_eq!(bytes(&quiet), r#"{"title":"T","body":"","level":"passive"}"#);
    }

    #[test]
    fn an_absent_flag_is_absent_from_the_body() {
        let payload = serde_json::to_value(
            payload(&Options {
                title: "T".into(),
                body: String::new(),
                level: None,
                group: Some("   ".into()),
            })
            .expect("payload"),
        )
        .unwrap();
        let map = payload.as_object().expect("object");
        assert!(!map.contains_key("level"), "the server owns the default");
        assert!(!map.contains_key("group"), "a blank group is no group");
        assert_eq!(payload["body"], "");
    }

    #[test]
    fn an_unusable_level_is_refused_before_anything_is_sent() {
        let error = payload(&Options {
            title: "T".into(),
            body: String::new(),
            level: Some("critical".into()),
            group: None,
        })
        .expect_err("refused");
        assert!(error.contains("passive, active or time-sensitive"), "{error}");
    }
}
