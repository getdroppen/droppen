//! Entry point: `serve` (the default), `up`, and the tooling subcommands.

use std::path::PathBuf;

use clap::{Args, CommandFactory, Parser, Subcommand};
use remote_hand_server::app::RemoteHandApp;
use remote_hand_server::program::Name;
use remote_hand_server::GotchaResult;

mod export;

/// The top-level commands. `serve`, `up`, `preflight` and `export-openapi`
/// carry their arguments untouched: their tails are ignored, forwarded verbatim
/// or scanned for one exact `--out` (see `run`), none of which is a clap
/// grammar. The other commands' own parsers receive the original tail too, so
/// each sees exactly the bytes it saw before clap.
#[derive(Debug, Subcommand)]
enum Command {
    /// Run the server (the default with no command).
    #[command(disable_help_flag = true, disable_version_flag = true)]
    Serve(RawTail),
    /// Supervise a server: restart it, back off a bad binary, reap orphans.
    #[command(disable_help_flag = true, disable_version_flag = true)]
    Up(RawTail),
    /// Manage the local accounts of one instance.
    #[command(disable_help_flag = true, disable_version_flag = true)]
    User(RawTail),
    /// Show the one-time code of an instance that has no account yet.
    #[command(disable_help_flag = true, disable_version_flag = true)]
    SetupCode(RawTail),
    /// Open an instance's page in this device's browser.
    #[command(disable_help_flag = true, disable_version_flag = true)]
    Open(RawTail),
    /// Manage the plugins installed in one instance.
    #[command(disable_help_flag = true, disable_version_flag = true)]
    Plugin(RawTail),
    /// Join, run, inspect or leave the cloud agent.
    #[command(disable_help_flag = true, disable_version_flag = true)]
    Agent(RawTail),
    /// Send a push notification from a session.
    #[command(hide = true, disable_help_flag = true, disable_version_flag = true)]
    Notify(RawTail),
    /// Rebuild a recorded session's transcript files under a scratch home.
    #[command(disable_help_flag = true, disable_version_flag = true)]
    Replay(RawTail),
    /// Write the served OpenAPI spec to a file.
    #[command(disable_help_flag = true, disable_version_flag = true)]
    ExportOpenapi(RawTail),
    /// Check that this build loads its configuration, then print its version.
    #[command(hide = true, disable_help_flag = true, disable_version_flag = true)]
    Preflight(RawTail),
    /// Resident pty owned by a terminal (started by the server).
    #[command(hide = true, disable_help_flag = true, disable_version_flag = true)]
    PtyShim(RawTail),
    /// Resident pipe owned by an AI session (started by the server).
    #[command(hide = true, disable_help_flag = true, disable_version_flag = true)]
    SessionShim(RawTail),
}

/// Arguments a command takes untouched. Declared so clap accepts any tail at
/// all for it; the dispatch passes the original argv tail rather than this
/// copy, because clap reserves `--` as an option terminator and these commands
/// never did.
#[derive(Debug, Args)]
struct RawTail {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0.., hide = true)]
    args: Vec<String>,
}

/// The command tree, with the name and prose of the name this binary was run
/// as (`crate::program`).
#[derive(Debug, Parser)]
#[command(
    name = "remote-hand-server",
    disable_help_flag = true,
    disable_version_flag = true,
    disable_help_subcommand = true,
    args_override_self = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

/// What to do after parsing. Each command's own arguments are the original
/// tail; this only says which one to dispatch to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Invocation {
    Serve,
    Up,
    User,
    SetupCode,
    Open,
    Plugin,
    Agent,
    Notify,
    Replay,
    ExportOpenapi,
    Preflight,
    PtyShim,
    SessionShim,
}

/// A successful parse: a command to run, or the help/version answer.
#[derive(Debug)]
enum Parsed {
    Help(String),
    Version,
    Run(Invocation),
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let invocation = match parse(&args) {
        Ok(Parsed::Help(text)) => {
            print!("{text}");
            return std::process::ExitCode::SUCCESS;
        }
        // Printed bare, with no logging and no side effects, because
        // `POST /api/server/self-upgrade` runs exactly this on a binary it is
        // deciding whether to install and reads the whole of stdout as the
        // answer. Anything else on this stream would become part of the version.
        Ok(Parsed::Version) => {
            println!("{}", remote_hand_server::api::SERVER_VERSION);
            return std::process::ExitCode::SUCCESS;
        }
        Ok(Parsed::Run(invocation)) => invocation,
        Err(error) => {
            // Every command-line refusal exits 1, never clap's own 2.
            complain(&error);
            return std::process::ExitCode::FAILURE;
        }
    };

    // `up` is deliberately outside the tokio runtime. It is a supervisor whose
    // one hard requirement is reaping *every* orphan with `waitpid(-1)`, and
    // tokio's process driver reaps its own children through the same call —
    // the two would race over each other's statuses. So the supervisor is plain
    // blocking code that owns the process, and only the paths that actually
    // serve build a runtime.
    if invocation == Invocation::Up {
        let child_args = remote_hand_server::supervisor::child_args(args.get(1..).unwrap_or_default());
        return match remote_hand_server::supervisor::run(child_args) {
            Ok(code) => std::process::ExitCode::from(code.clamp(0, 255) as u8),
            Err(error) => {
                complain(&error);
                std::process::ExitCode::FAILURE
            }
        };
    }

    match tokio::runtime::Runtime::new() {
        Ok(runtime) => match runtime.block_on(run(invocation, args)) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                complain(&error.to_string());
                // 1 for everything except `serve` finding its instance already
                // owned, which gets `INSTANCE_HELD_EXIT` (75) so that `up` waits
                // for the holder instead of blaming — and rolling back — the
                // binary. The message above is the same either way.
                std::process::ExitCode::from(remote_hand_server::supervisor::failure_exit_code(&error))
            }
        },
        Err(error) => {
            complain(&format!("cannot start a runtime: {error}"));
            std::process::ExitCode::FAILURE
        }
    }
}

/// Parse `args` (everything after argv[0]).
///
/// Help and version are recognized only as the exact first argument and take
/// no notice of what follows, as this binary always has. clap's own help and
/// version flags are disabled for the same reason: they would also answer
/// `-hV`, `-Vh` and `--help=…`, which are unknown commands today.
fn parse(args: &[String]) -> Result<Parsed, String> {
    if let Some(first) = args.first() {
        match first.as_str() {
            "--help" | "-h" => return Ok(Parsed::Help(help_text())),
            "--version" | "-V" => return Ok(Parsed::Version),
            // clap reads a leading `--` as its option terminator, leaving no
            // subcommand and so meaning `serve`; this binary has always called
            // it an unknown command.
            "--" => return Err(format!("unknown command: {first}\n\n{}", Name::invoked().rewrite(USAGE))),
            _ => {}
        }
    }

    let matches = cli_command()
        .try_get_matches_from(std::iter::once("remote-hand-server".to_string()).chain(args.iter().cloned()))
        .map_err(|error| error.to_string())?;
    let cli = <Cli as clap::FromArgMatches>::from_arg_matches(&matches).map_err(|error| error.to_string())?;
    let invocation = match cli.command {
        None | Some(Command::Serve(_)) => Invocation::Serve,
        Some(Command::Up(_)) => Invocation::Up,
        Some(Command::User(_)) => Invocation::User,
        Some(Command::SetupCode(_)) => Invocation::SetupCode,
        Some(Command::Open(_)) => Invocation::Open,
        Some(Command::Plugin(_)) => Invocation::Plugin,
        Some(Command::Agent(_)) => Invocation::Agent,
        Some(Command::Notify(_)) => Invocation::Notify,
        Some(Command::Replay(_)) => Invocation::Replay,
        Some(Command::ExportOpenapi(_)) => Invocation::ExportOpenapi,
        Some(Command::Preflight(_)) => Invocation::Preflight,
        Some(Command::PtyShim(_)) => Invocation::PtyShim,
        Some(Command::SessionShim(_)) => Invocation::SessionShim,
    };
    Ok(Parsed::Run(invocation))
}

/// [`USAGE`], under the invoked name, as clap's long help.
fn help_text() -> String {
    cli_command().render_long_help().to_string()
}

/// The top-level tree as clap's help should show it: the name this binary was
/// run as, the same header `usage()` always printed, and the whole old usage
/// text after the generated command list so `--help` still says everything.
fn cli_command() -> clap::Command {
    let name = Name::invoked();
    let mut prose = name.rewrite(USAGE).into_owned();
    if name == Name::Droppen {
        // The continuation lines line up under the old name's arguments.
        let shift = Name::Legacy.command().len() - Name::Droppen.command().len();
        let (wide, narrow) = (format!("\n{:30}", ""), format!("\n{:1$}", "", 30 - shift));
        prose = prose.replace(&wide, &narrow);
    }
    Cli::command()
        .bin_name(name.command())
        .about(match name {
            Name::Droppen => "Droppen server: `droppen` (`remote-hand-server` is the same program under its old name).",
            Name::Legacy => "Droppen server: `remote-hand-server` (the same program as `droppen`).",
        })
        .after_long_help(prose)
}

async fn run(invocation: Invocation, args: Vec<String>) -> GotchaResult<()> {
    let tail = args.get(1..).unwrap_or_default();
    match invocation {
        // `serve` is the default, and its tail has always been ignored.
        Invocation::Serve => RemoteHandApp::default().run_server().await,
        Invocation::ExportOpenapi => {
            // The grammar is "the first exact --out anywhere, take the next
            // token (whatever it is); every other argument is ignored", which
            // clap has no way to say: a declared --out would float and
            // `--out=value`, which is ignored here, would be accepted.
            let out = flag_value(tail, "--out").map(PathBuf::from);
            export::export_openapi(out).await
        }
        // Local accounts. Outside the server on purpose: the first account has
        // to exist before anyone can log in to create one, and the trust anchor
        // for that is the operating system — whoever can run this binary already
        // has everything the API could hand them. See `crate::auth::cli`.
        Invocation::User => remote_hand_server::auth::cli::run(tail)
            .await
            .map_err(remote_hand_server::GotchaError::message),
        // The one-time code a server in its setup state printed when it
        // started, read back from the file it wrote — for whoever missed the
        // banner, or started the server as a service.
        Invocation::SetupCode => remote_hand_server::auth::cli::setup_code(tail)
            .await
            .map_err(remote_hand_server::GotchaError::message),
        // The page, opened in this device's browser — signed in as the
        // device's owner while the instance has no account. The proof is the
        // operating system's: only this user can reach the running server's
        // control socket (`crate::auth::local_open`).
        Invocation::Open => remote_hand_server::auth::cli::open(tail)
            .await
            .map_err(remote_hand_server::GotchaError::message),
        // The plugins installed in an instance: install, uninstall, switches,
        // and each one's document store. Outside the server for the reason
        // `user` is, and safe beside a running one (`crate::plugins::cli`).
        Invocation::Plugin => remote_hand_server::plugins::cli::run(tail)
            .await
            .map_err(remote_hand_server::GotchaError::message),
        // The cloud agent's skeleton (M0): creates this instance's machine
        // identity and describes what enrolling and running would do. Outside
        // `serve` entirely, so an instance nobody enrolls is unchanged.
        Invocation::Agent => remote_hand_server::agent::run(tail)
            .await
            .map_err(remote_hand_server::GotchaError::message),
        // The other half of that health check: prove this build can load the
        // configuration it is about to be started with, which `--version` alone
        // says nothing about. Hidden from the usage string for the same reason
        // `pty-shim` is — it exists for one caller, and that caller is us.
        Invocation::Preflight => {
            let config = remote_hand_server::config::load(std::env::var("GOTCHA_ACTIVE_PROFILE").ok())?;
            println!("{} ok (instance {})", remote_hand_server::api::SERVER_VERSION, config.app.instance());
            Ok(())
        }
        // Hidden on purpose: nothing but the server itself should ever run
        // this, and it is documented where it is spawned rather than in a usage
        // string a user might act on.
        Invocation::PtyShim => {
            let parsed = remote_hand_server::pty_shim::child::parse(tail).map_err(remote_hand_server::GotchaError::message)?;
            remote_hand_server::pty_shim::child::run(parsed)
                .await
                .map_err(remote_hand_server::GotchaError::message)
        }
        // The other half of `crate::record`, and the one a person runs: it
        // rebuilds a recorded session's transcript files under a scratch $HOME,
        // at the pace they were written, so a server pointed at that home
        // renders a real session through the real pipeline. Not hidden — it is
        // for developers to use, which is why it is in the usage string.
        Invocation::Replay => {
            let options = remote_hand_server::record::replay::parse(tail).map_err(remote_hand_server::GotchaError::message)?;
            remote_hand_server::record::replay::run(options)
                .await
                .map_err(remote_hand_server::GotchaError::message)
        }
        // Hidden for the same reason, and the same shape: the shim that holds a
        // resident claude's pipes so a server upgrade does not kill the session.
        Invocation::SessionShim => {
            let parsed = remote_hand_server::inject::shim::parse(tail).map_err(remote_hand_server::GotchaError::message)?;
            remote_hand_server::inject::shim::run(parsed)
                .await
                .map_err(remote_hand_server::GotchaError::message)
        }
        // Hidden for the same reason `pty-shim` is, and with the same kind of
        // caller: the `droppen-notify` on a session's PATH is one line of shell
        // that `exec`s this. It is useless without the environment that spawn was
        // given, so a usage string offering it to a person would be offering
        // them a command that always answers "no channel configured".
        Invocation::Notify => remote_hand_server::notify::cli::run(tail)
            .await
            .map_err(remote_hand_server::GotchaError::message),
        // `up` is handled before the runtime is built.
        Invocation::Up => unreachable!("`up` never reaches the async dispatcher"),
    }
}

/// An error on stderr, said in the name this binary was run as
/// (`remote_hand_server::program`): `droppen: …` with the commands it suggests
/// as `droppen …`, or `remote-hand: …` exactly as before.
///
/// `notify` is the exception: it is run by a session's `droppen-notify`
/// script, and an agent reads its errors under that name.
fn complain(error: &str) {
    let name = Name::invoked();
    let notify = std::env::args().nth(1).as_deref() == Some("notify");
    let prefix = if notify {
        remote_hand_server::notify::cli::NAME
    } else {
        name.error_prefix()
    };
    eprintln!("{prefix}: {}", name.rewrite(error));
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let index = args.iter().position(|a| a == flag)?;
    args.get(index + 1).cloned()
}

/// Said under the old name; [`cli_command`] shows it as `droppen` when that is
/// the name it was run as, after the command list clap generates.
const USAGE: &str = "\
USAGE:
    remote-hand-server [serve]
    remote-hand-server up [--] [serve ...]
    remote-hand-server user add|list|passwd|rm [<name>] [--instance <name>]
    remote-hand-server open [--print] [--instance <name>]
    remote-hand-server setup-code [--instance <name>]
    remote-hand-server agent enroll <code>|- --hub <url> | run | status [--instance <name>]
    remote-hand-server plugin list | enable <id> | disable <id> | verify <id>
                              | export <id> [--out <file>]
                              | import <id> [<file>] [--replace]
                              | purge <id> --yes          [--instance <name>]
    remote-hand-server export-openapi [--out <path>]
    remote-hand-server replay --log <recording> --home <scratch-home>
                              [--instance <name>] [--speed <n>]
                              [--max-gap <seconds>] [--instant]
    remote-hand-server --version

`user` manages the local accounts of **one instance**, in
~/.local/share/remote-hand/instances/<instance>/users.json — the instance
$APP_INSTANCE names, or the one --instance says. Every request to a server needs
a session; nobody is let in for connecting from this device. Once an instance
has an account, that means logging in (POST /api/auth/login). **An instance with
no account is in its setup state**: `open` opens its page in this device's
browser signed in as the device's owner (the running server hands this
operating-system user — and nobody else — a one-time link), and from anywhere
else the first account is made on the page with the one-time setup code the
server keeps in <data>/instances/<instance>/setup-code (`setup-code` shows it;
it is printed only to a terminal). `user add` makes it here, with no code:
whoever can run this binary already has the operating-system account the
server runs as. Accounts belong to one instance: another instance on the same
machine has its own. `APP_AUTH__ENABLED=false` switches authentication off — every
route then answers anyone who can reach the port; for development and tests
only.

`agent` is the cloud agent: `agent enroll` joins this instance to a Droppen
account (it needs a local account first, because a linked device's page reaches
other devices), `agent status` says where it stands, `agent unenroll` leaves.
`remote-hand-server agent` alone prints the details. An instance that is never
enrolled serves exactly as before.

`plugin` manages the plugins installed in one instance: `install <file.tgz>`
unpacks and checks a package under <data>/instances/<instance>/plugin-packages/
and switches it on, `uninstall` removes the code (documents and the switch
stay), `list` shows each one's build, and `enable`/`disable` switch one (the
settings page does the same; `[plugins] enable`/`disable` in the configuration,
or APP_PLUGINS__ENABLE/DISABLE, pin it). Each plugin's document store lives
under <data>/instances/<instance>/plugins/<id>/ — export it as JSON, import such
a file back, check it, or move it aside once the plugin is off.
`remote-hand-server plugin` alone prints the details.

`up` supervises a server: it restarts one on a clean exit (which is how
`POST /api/server/self-upgrade` restarts into a new binary), backs off and rolls
back a binary that will not stay up, and reaps orphaned processes — so it is what
should be pid 1 in a container. A server that exits 75 (EX_TEMPFAIL) because
another one already owns its instance is not a bad binary: `up` keeps retrying
until that one is gone, and rolls nothing back.

`replay` rebuilds the transcript files of a recorded session under a scratch
$HOME, at the pace they were originally written. Recordings are made for every
claude session this server drives and live in
<data>/instances/<name>/session-logs/<session-id>.ndjson (DROPPEN_SESSION_LOG=0
turns them off, DROPPEN_SESSION_LOG_MAX_MB moves the per-session ceiling). Then serve that
home — APP_HOME_DIR=<scratch-home> remote-hand-server serve — to watch the
session happen again through the real rendering pipeline.

Configuration comes from configurations/application.toml and the APP_ prefix,
e.g. APP_SERVER__PORT=9000, APP_HOME_DIR=/tmp/fake-home.";


#[cfg(test)]
mod tests {
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_string()).collect()
    }

    fn invocation(argv: &[&str]) -> Invocation {
        match parse(&args(argv)) {
            Ok(Parsed::Run(invocation)) => invocation,
            other => panic!("{argv:?} should parse to a command, got {other:?}"),
        }
    }

    /// Every command, including the raw tails that go straight to their own parser (with a `--help`
    /// or a `--` that parser must see itself).
    #[test]
    fn the_tree_recognizes_every_command() {
        for (argv, expected) in [
            (vec![], Invocation::Serve),
            (vec!["serve"], Invocation::Serve),
            (vec!["serve", "--help"], Invocation::Serve),
            (vec!["serve", "junk", "--x"], Invocation::Serve),
            (vec!["up"], Invocation::Up),
            (vec!["up", "--", "serve"], Invocation::Up),
            (vec!["user", "--help"], Invocation::User),
            (vec!["user", "--", "list"], Invocation::User),
            (vec!["setup-code"], Invocation::SetupCode),
            (vec!["open", "--print"], Invocation::Open),
            (vec!["plugin", "list"], Invocation::Plugin),
            (vec!["agent", "status"], Invocation::Agent),
            (vec!["notify", "--help"], Invocation::Notify),
            (vec!["replay", "--log", "x", "--home", "y"], Invocation::Replay),
            (vec!["export-openapi", "--out", "x"], Invocation::ExportOpenapi),
            (vec!["preflight", "junk"], Invocation::Preflight),
            (vec!["pty-shim", "--socket", "s"], Invocation::PtyShim),
            (vec!["session-shim", "--socket", "s"], Invocation::SessionShim),
        ] {
            assert_eq!(invocation(&argv), expected, "{argv:?}");
        }
    }

    /// Help and version are the exact first token and nothing else; any other spelling is an unknown
    /// command, as before clap (which would also answer `-hV`, `-Vh` and `--help=…`).
    #[test]
    fn help_and_version_are_the_first_token_only() {
        for argv in [vec!["--help"], vec!["-h"]] {
            match parse(&args(&argv)) {
                Ok(Parsed::Help(text)) => assert!(text.contains("USAGE:"), "{argv:?}: {text}"),
                other => panic!("{argv:?}: {other:?}"),
            }
        }
        for argv in [vec!["--version"], vec!["-V"]] {
            assert!(matches!(parse(&args(&argv)), Ok(Parsed::Version)), "{argv:?}");
        }
        for argv in [
            vec!["-hV"],
            vec!["-Vh"],
            vec!["--help=1"],
            vec!["--version=1"],
            vec!["--"],
            vec!["--", "serve"],
            vec!["bogus"],
            vec!["help"],
        ] {
            assert!(parse(&args(&argv)).is_err(), "{argv:?} must be refused");
        }
    }
    /// `export-openapi` takes the first exact `--out` and its next token, and
    /// ignores every other argument; `--out=…` is not that word.
    #[test]
    fn export_openapi_scans_for_the_first_exact_out() {
        assert_eq!(flag_value(&args(&["--out", "a.json"]), "--out"), Some("a.json".to_string()));
        assert_eq!(
            flag_value(&args(&["junk", "--out", "a.json", "--out", "b.json"]), "--out"),
            Some("a.json".to_string()),
            "the first one"
        );
        assert_eq!(flag_value(&args(&["--out=--"]), "--out"), None);
        assert_eq!(flag_value(&args(&["--out"]), "--out"), None, "no value means stdout");
        assert_eq!(flag_value(&args(&["--out", "--", "x"]), "--out"), Some("--".to_string()));
        assert_eq!(flag_value(&args(&[]), "--out"), None);
    }

    /// The commands with no clap grammar still take any tail; the dispatch
    /// ignores, forwards or scans it.
    #[test]
    fn the_raw_tail_commands_take_any_arguments() {
        for argv in [
            vec!["serve", "--help"],
            vec!["serve", "junk", "--x"],
            vec!["preflight", "--help"],
            vec!["preflight", "junk"],
            vec!["export-openapi", "--out=x"],
            vec!["export-openapi", "--out", "x", "junk"],
            vec!["up", "--"],
            vec!["up", "--", "serve", "--flag"],
            vec!["up", "--help"],
        ] {
            assert!(matches!(parse(&args(&argv)), Ok(Parsed::Run(_))), "{argv:?}");
        }
    }

}
