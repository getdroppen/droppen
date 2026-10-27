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

use clap::{CommandFactory, FromArgMatches, Parser};
use serde_json::json;

/// Where `POST /api/notify` is, injected at spawn.
///
/// Injected under its old name too, `RH_NOTIFY_URL` (and read under both,
/// [`crate::env`]): a session outlives the server that started it, and this is
/// run by whichever binary that server's path holds by then — older or newer.
pub const URL_ENV: &str = "DROPPEN_NOTIFY_URL";
/// The session's capability, injected at spawn (and as `RH_NOTIFY_TOKEN`, for
/// the reason [`URL_ENV`] is). See [`super::token`].
pub const TOKEN_ENV: &str = "DROPPEN_NOTIFY_TOKEN";

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

/// The usage an agent is shown when it gets the call wrong, under `name`.
pub fn usage(name: &str) -> String {
    format!(
        "usage: {name} <title> [body] [--level passive|active|time-sensitive] [--group <name>]

Sends a push notification to the person running this Droppen server.
Use it when something has happened that they would want to be interrupted for."
    )
}

/// Parse `droppen-notify`'s arguments.
pub fn parse(args: &[String]) -> Result<Options, String> {
    parse_as(args, NAME)
}

/// Parse `droppen-notify`'s arguments; `name` is what the usage calls it.
///
/// Positional-first on purpose: `droppen-notify \"Blocked on a decision\"
/// \"Postgres or SQLite?\"` is what a model writes when it has read one example,
/// and a tool whose happy path needs two flags is a tool that gets called wrong.
/// The flags exist for the two things there is no natural position for.
pub fn parse_as(args: &[String], name: &str) -> Result<Options, String> {
    let help = || usage(name);
    scan_reserved(args, name)?;
    let cli = notify_command(name)
        .try_get_matches_from(std::iter::once("droppen-notify".to_string()).chain(args.iter().cloned()))
        .map_err(|error| format!("{error}\n\n{}", help()))?;
    let cli = NotifyCli::from_arg_matches(&cli).map_err(|error| format!("{error}\n\n{}", help()))?;
    let mut options = Options {
        title: cli.title.unwrap_or_default(),
        body: cli.body.unwrap_or_default(),
        level: cli.level,
        group: cli.group,
    };
    // The flags win over the positions, so `--title` can override the first
    // word of a shell-mangled command line rather than fighting with it.
    if options.title.is_empty() {
        options.title = cli.words.first().cloned().unwrap_or_default();
    }
    if options.body.is_empty() && cli.words.len() > 1 {
        options.body = cli.words[1..].join(" ");
    }
    if options.title.trim().is_empty() {
        return Err(format!("a notification needs a title\n\n{}", help()));
    }
    Ok(options)
}

/// The spellings this command refuses before clap sees them, and the help
/// tokens it answers itself.
///
/// The hand parser accepted an option only as a flag and a separate value word,
/// so `--level=active` and `-lpassive` were unknown options (clap takes both),
/// and `--help` was this command's usage as an error (clap would call it an
/// unknown flag). A value-taking flag's next token is skipped: the old parser
/// consumed it as the value, whatever it looked like, so `--title --help` is a
/// title and `--group --level=x` is a group.
fn scan_reserved(args: &[String], name: &str) -> Result<(), String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--level" | "-l" | "--group" | "-g" | "--title" | "-t" | "--body" | "-b" => {
                if iter.next().is_none() {
                    return Err(format!("{arg} needs a value\n\n{}", usage(name)));
                }
            }
            "--help" | "-h" => return Err(usage(name)),
            // Everything after `--` is text, however much it looks like a flag:
            // a title that starts with a dash is a title.
            "--" => return Ok(()),
            other if is_attached_option(other) => return Err(format!("unknown option {other}\n\n{}", usage(name))),
            other if other.starts_with('-') && other.len() > 1 => {
                return Err(format!("unknown option {other}\n\n{}", usage(name)));
            }
            _ => {}
        }
    }
    Ok(())
}

/// `--level=active` or `-lpassive`: an option and its value in one word.
fn is_attached_option(arg: &str) -> bool {
    ["--level=", "--group=", "--title=", "--body="].iter().any(|prefix| arg.starts_with(prefix))
        || ["-l", "-g", "-t", "-b"].iter().any(|short| arg.len() > short.len() && arg.starts_with(short))
}

/// `droppen-notify`'s command tree, shown under the name the agent ran it as.
fn notify_command(name: &str) -> clap::Command {
    NotifyCli::command().bin_name(name).after_long_help(usage(name))
}

/// `droppen-notify`'s command line. Positional-first on purpose: the title and
/// body are what a model writes when it has read one example. The flags are for
/// the two things there is no natural position for.
#[derive(Debug, Parser)]
#[command(
    name = "droppen-notify",
    disable_help_flag = true,
    disable_version_flag = true,
    disable_help_subcommand = true,
    args_override_self = true
)]
struct NotifyCli {
    #[arg(long, short = 'l', allow_hyphen_values = true)]
    level: Option<String>,
    #[arg(long, short = 'g', allow_hyphen_values = true)]
    group: Option<String>,
    #[arg(long, short = 't', allow_hyphen_values = true)]
    title: Option<String>,
    #[arg(long, short = 'b', allow_hyphen_values = true)]
    body: Option<String>,
    #[arg(value_name = "title")]
    words: Vec<String>,
}

/// The level names this accepts, mapped onto the ones the API takes.
///
/// `time-sensitive` as well as `timeSensitive` because a shell argument written
/// by hand is kebab-case and one copied off the API is camelCase, and being
/// refused over a hyphen is a bad way to find out a build has broken.
fn level(raw: &str) -> Result<&'static str, String> {
    match raw.trim().to_ascii_lowercase().replace(['-', '_'], "").as_str() {
        "passive" | "low" => Ok("passive"),
        "active" | "normal" | "default" => Ok("active"),
        "timesensitive" | "urgent" | "high" => Ok("timeSensitive"),
        other => Err(format!("unknown level {other:?}; use passive, active or time-sensitive")),
    }
}

/// The JSON body one invocation sends.
///
/// Separate from [`run`] so the encoding can be asserted against the prose a
/// model actually writes — quotes, newlines, `$`, CJK — without a server.
pub fn payload(options: &Options) -> Result<serde_json::Value, String> {
    let mut body = json!({ "title": options.title, "body": options.body });
    let map = body.as_object_mut().expect("a json object");
    if let Some(raw) = options.level.as_deref() {
        map.insert("level".into(), json!(level(raw)?));
    }
    if let Some(group) = options.group.as_deref().filter(|group| !group.trim().is_empty()) {
        map.insert("group".into(), json!(group));
    }
    Ok(body)
}

/// Send one notification, and say in one line what happened.
///
/// `Err` is the process' exit code as well as its message: an agent that is
/// blocked and could not say so needs to know that, because the alternative is
/// waiting forever for an answer to a question nobody was asked.
pub async fn run(args: &[String]) -> Result<(), String> {
    let options = parse(args)?;
    let payload = payload(&options)?;
    let (Some(url), Some(token)) = (crate::env::var(URL_ENV), crate::env::var(TOKEN_ENV)) else {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn the_shape_a_model_writes_first_is_the_one_that_works() {
        let parsed = parse(&args(&["Blocked on a decision", "Postgres or SQLite?"])).expect("parsed");
        assert_eq!(parsed.title, "Blocked on a decision");
        assert_eq!(parsed.body, "Postgres or SQLite?");
        assert_eq!(parsed.level, None);
        assert_eq!(parsed.group, None);

        // Everything after the title is the body, so an unquoted sentence still
        // arrives whole rather than as a title and four ignored words.
        let parsed = parse(&args(&["Done", "all", "41", "tests", "pass"])).expect("parsed");
        assert_eq!(parsed.body, "all 41 tests pass");
    }

    #[test]
    fn the_flags_are_there_for_the_two_things_with_no_natural_position() {
        let parsed = parse(&args(&["Blocked", "Need a decision", "--level", "time-sensitive", "--group", "deploy"])).expect("parsed");
        assert_eq!(parsed.level.as_deref(), Some("time-sensitive"));
        assert_eq!(parsed.group.as_deref(), Some("deploy"));

        let parsed = parse(&args(&["--title", "T", "--body", "B"])).expect("parsed");
        assert_eq!((parsed.title.as_str(), parsed.body.as_str()), ("T", "B"));

        // A title that starts with a dash is a title.
        let parsed = parse(&args(&["--", "--not-a-flag", "body"])).expect("parsed");
        assert_eq!(parsed.title, "--not-a-flag");
        assert_eq!(parsed.body, "body");
    }

    #[test]
    fn a_call_that_cannot_work_says_so_before_the_network() {
        assert!(parse(&args(&[])).expect_err("no title").contains("needs a title"));
        assert!(parse(&args(&["  "])).expect_err("blank title").contains("needs a title"));
        assert!(parse(&args(&["T", "--level"])).expect_err("dangling").contains("needs a value"));
        assert!(parse(&args(&["T", "--nope"])).expect_err("unknown").contains("unknown option"));
        assert!(parse(&args(&["--help"])).expect_err("help").contains("usage:"));
    }

    #[test]
    fn the_usage_names_the_command() {
        let error = parse(&args(&[])).expect_err("no title");
        assert!(error.contains("usage: droppen-notify <title>"), "{error}");
        let error = parse_as(&args(&["--help"]), NAME).expect_err("help");
        assert!(error.starts_with("usage: droppen-notify <title>"), "{error}");
    }

    #[test]
    fn the_level_spelling_a_shell_user_reaches_for_is_accepted() {
        assert_eq!(level("time-sensitive"), Ok("timeSensitive"));
        assert_eq!(level("timeSensitive"), Ok("timeSensitive"));
        assert_eq!(level("TIME_SENSITIVE"), Ok("timeSensitive"));
        assert_eq!(level("urgent"), Ok("timeSensitive"));
        assert_eq!(level(" active "), Ok("active"));
        assert_eq!(level("passive"), Ok("passive"));
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
        let payload = payload(&options).expect("payload");
        assert_eq!(payload["title"], hostile, "every byte survives the round trip");
        assert_eq!(payload["body"], hostile);
        assert_eq!(payload["level"], "timeSensitive");
        assert_eq!(payload["group"], "deploy");
        // And the encoded form really is one JSON string, not a shell command.
        let encoded = serde_json::to_string(&payload).expect("encode");
        assert!(!encoded.contains("\n"), "a raw newline would end the JSON string: {encoded}");
    }

    #[test]
    fn an_absent_flag_is_absent_from_the_body() {
        let payload = payload(&Options {
            title: "T".into(),
            body: String::new(),
            level: None,
            group: Some("   ".into()),
        })
        .expect("payload");
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
    #[test]
    fn the_notify_argument_table() {
        let words = |line: &[&str]| -> Vec<String> { line.iter().map(|word| (*word).to_string()).collect() };
        let accepted: Vec<Vec<String>> = [
            &["T"][..],
            &["T", "B"],
            &["T", "B", "C"],
            &["--title", "T", "--body", "B"],
            &["T", "B", "--level", "active"],
            &["T", "--level", "passive", "--group", "deploy"],
            &["T", "--", "--not-a-flag"],
            &["--", "--not-a-flag", "body"],
            &["-"],
            &["T", "-"],
            &["T", "--level", "--help"],
            &["T", "--group", "--level=x"],
            &["--title", "-h"],
            &["T", "--title", "U"],
            &["--level", "active", "T", "B"],
        ]
        .into_iter()
        .map(words)
        .collect();
        for argv in &accepted {
            assert!(parse(argv).is_ok(), "{argv:?} must parse: {:?}", parse(argv).err());
        }

        let refused: Vec<Vec<String>> = [
            &[][..],
            &["  "],
            &["T", "--level"],
            &["T", "--nope"],
            &["T", "--level=active"],
            &["T", "--group=deploy"],
            &["T", "--title=U"],
            &["T", "--body=B"],
            &["T", "-lactive"],
            &["T", "-gdeploy"],
            &["T", "-tU"],
            &["T", "-bB"],
            &["--help"],
            &["-h"],
            &["T", "--help"],
            &["T", "-h"],
            &["-x"],
        ]
        .into_iter()
        .map(words)
        .collect();
        for argv in &refused {
            assert!(parse(argv).is_err(), "{argv:?} must be refused");
        }
    }

}
