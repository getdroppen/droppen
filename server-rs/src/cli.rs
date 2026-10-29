//! The command line: one clap tree for every command this binary answers to.
//!
//! `main` parses it once, before any runtime is built. clap answers `--help`
//! and usage errors itself (usage errors exit 2); `--version` is the one answer
//! printed here, bare (see [`Cli::parse_invoked`]).

pub mod agent;

use clap::error::ErrorKind;
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};

use crate::app::RemoteHandApp;
use droppen_base::program::Name;
use crate::{GotchaError, GotchaResult};

/// The top-level command line. No command means `serve`.
#[derive(Debug, PartialEq, Parser)]
#[command(name = "remote-hand-server", version = droppen_http::SERVER_VERSION, disable_help_subcommand = true)]
pub struct Cli {
    /// The command to run.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Every command. `serve`, `up` and `preflight` take a raw tail and have no
/// `--help` of their own: `serve` and `preflight` ignore their arguments, and
/// `up` hands its arguments to the server it starts.
#[derive(Debug, PartialEq, Subcommand)]
pub enum Command {
    /// Run the server (the default with no command).
    #[command(disable_help_flag = true)]
    Serve(RawTail),
    /// Supervise a server: restart it, back off a bad binary, reap orphans.
    #[command(disable_help_flag = true)]
    Up(UpArgs),
    /// Manage the local accounts of one instance.
    User(crate::auth::cli::UserArgs),
    /// Show the one-time code of an instance that has no account yet.
    SetupCode(crate::auth::cli::SetupCodeArgs),
    /// Open an instance's page in this device's browser.
    Open(crate::auth::cli::OpenArgs),
    /// Manage the plugins installed in one instance.
    Plugin(crate::plugins::cli::PluginArgs),
    /// Join, run, inspect or leave the cloud agent.
    Agent(agent::AgentArgs),
    /// Send a push notification from a session.
    #[command(hide = true, override_usage = crate::notify::cli::USAGE_LINE)]
    Notify(crate::notify::cli::NotifyArgs),
    /// Rebuild a recorded session's transcript files under a scratch home.
    Replay(crate::record::replay::ReplayArgs),
    /// Write the served OpenAPI spec to a file.
    ExportOpenapi(crate::export::ExportOpenapiArgs),
    /// Check that this build loads its configuration, then print its version.
    #[command(hide = true, disable_help_flag = true)]
    Preflight(RawTail),
    /// Resident pty owned by a terminal (started by the server).
    #[command(hide = true)]
    PtyShim(droppen_pty::child::PtyShimArgs),
    /// Resident pipe owned by an AI session (started by the server).
    #[command(hide = true)]
    SessionShim(crate::inject::shim::SessionShimArgs),
}

/// Arguments a command takes and ignores.
#[derive(Debug, Default, PartialEq, Eq, Args)]
pub struct RawTail {
    /// The arguments, ignored.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0.., hide = true)]
    pub args: Vec<String>,
}

/// `up`'s arguments: the command line of the server it supervises.
#[derive(Debug, PartialEq, Eq, Args)]
pub struct UpArgs {
    /// The supervised server's command line (`serve` when empty).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0.., hide = true)]
    pub child: Vec<String>,
}

impl UpArgs {
    /// Supervise the server, and answer with the exit status `up` ends with.
    ///
    /// Plain blocking code, run before any tokio runtime exists: a supervisor's
    /// one hard requirement is reaping *every* orphan with `waitpid(-1)`, and
    /// tokio's process driver reaps its own children through the same call —
    /// the two would race over each other's statuses.
    pub fn run(self) -> Result<i32, String> {
        // `child_args` is protocol, not parsing: launchd, systemd, containers
        // and the desktop app start `up`, `up serve` and `up -- serve`, and it
        // says which server command line each of those means.
        crate::supervisor::run(crate::supervisor::child_args(&self.child))
    }
}

impl Cli {
    /// The tree as this binary shows it: under the name it was run as, with
    /// each command's long prose after the options clap lists.
    pub fn command_as(name: Name) -> clap::Command {
        let mut prose = name.rewrite(USAGE).into_owned();
        if name == Name::Droppen {
            // The continuation lines line up under the old name's arguments.
            let shift = Name::Legacy.command().len() - Name::Droppen.command().len();
            let (wide, narrow) = (format!("\n{:30}", ""), format!("\n{:1$}", "", 30 - shift));
            prose = prose.replace(&wide, &narrow);
        }
        let long = |text: &str| name.rewrite(text).into_owned();
        Self::command()
            .bin_name(name.command())
            .about(match name {
                Name::Droppen => "Droppen server: `droppen` (`remote-hand-server` is the same program under its old name).",
                Name::Legacy => "Droppen server: `remote-hand-server` (the same program as `droppen`).",
            })
            .after_long_help(prose)
            .mut_subcommand("user", |command| command.after_long_help(long(crate::auth::cli::USAGE)))
            .mut_subcommand("setup-code", |command| command.after_long_help(long(crate::auth::cli::SETUP_CODE_USAGE)))
            .mut_subcommand("open", |command| command.after_long_help(long(crate::auth::cli::OPEN_USAGE)))
            .mut_subcommand("plugin", |command| command.after_long_help(long(crate::plugins::cli::USAGE)))
            .mut_subcommand("agent", |command| command.after_long_help(long(agent::USAGE)))
            .mut_subcommand("replay", |command| command.after_long_help(long(crate::record::replay::USAGE)))
    }

    /// This process' command line. clap prints help and usage errors and exits
    /// on its own.
    pub fn parse_invoked() -> Self {
        let matches = Self::command_as(Name::invoked()).try_get_matches().unwrap_or_else(|error| {
            if error.kind() == ErrorKind::DisplayVersion {
                // Printed bare, with no logging and no side effects, because
                // `POST /api/server/self-upgrade` runs exactly this on a binary
                // it is deciding whether to install and reads the whole of
                // stdout as the answer; clap's would put the name in front.
                println!("{}", droppen_http::SERVER_VERSION);
                std::process::exit(0);
            }
            error.exit()
        });
        Self::from_arg_matches(&matches).unwrap_or_else(|error| error.exit())
    }

    /// The command to run: `serve` when none was named.
    pub fn into_command(self) -> Command {
        self.command.unwrap_or_else(|| Command::Serve(RawTail::default()))
    }
}

impl Command {
    /// Run every command but `up`, which [`UpArgs::run`] runs outside the
    /// runtime.
    pub async fn run(self) -> GotchaResult<()> {
        match self {
            // `serve` is the default, and its tail has always been ignored.
            Command::Serve(_) => RemoteHandApp::default().run_server().await,
            Command::ExportOpenapi(args) => args.run().await,
            // Local accounts. Outside the server on purpose: the first account has
            // to exist before anyone can log in to create one, and the trust anchor
            // for that is the operating system — whoever can run this binary already
            // has everything the API could hand them. See `crate::auth::cli`.
            Command::User(args) => args.run().await.map_err(GotchaError::message),
            // The one-time code a server in its setup state printed when it
            // started, read back from the file it wrote — for whoever missed the
            // banner, or started the server as a service.
            Command::SetupCode(args) => args.run().await.map_err(GotchaError::message),
            // The page, opened in this device's browser — signed in as the
            // device's owner while the instance has no account. The proof is the
            // operating system's: only this user can reach the running server's
            // control socket (`crate::auth::local_open`).
            Command::Open(args) => args.run().await.map_err(GotchaError::message),
            // The plugins installed in an instance: install, uninstall, switches,
            // and each one's document store. Outside the server for the reason
            // `user` is, and safe beside a running one (`crate::plugins::cli`).
            Command::Plugin(args) => args.run().await.map_err(GotchaError::message),
            // The cloud agent. Outside `serve` entirely, so an instance nobody
            // enrolls is unchanged.
            Command::Agent(args) => args.run().await.map_err(GotchaError::message),
            // The other half of the self-upgrade health check: prove this build
            // can load the configuration it is about to be started with, which
            // `--version` alone says nothing about. Hidden from the usage string
            // for the same reason `pty-shim` is — it exists for one caller, and
            // that caller is us.
            Command::Preflight(_) => {
                let config = droppen_http::config::load(std::env::var("GOTCHA_ACTIVE_PROFILE").ok())?;
                println!("{} ok (instance {})", droppen_http::SERVER_VERSION, config.app.instance());
                Ok(())
            }
            // Hidden on purpose: nothing but the server itself should ever run
            // this, and it is documented where it is spawned rather than in a usage
            // string a user might act on.
            Command::PtyShim(args) => args.run().await.map_err(GotchaError::message),
            // The other half of `crate::record`, and the one a person runs: it
            // rebuilds a recorded session's transcript files under a scratch $HOME,
            // at the pace they were written, so a server pointed at that home
            // renders a real session through the real pipeline. Not hidden — it is
            // for developers to use, which is why it is in the usage string.
            Command::Replay(args) => args.run().await.map_err(GotchaError::message),
            // Hidden for the same reason, and the same shape: the shim that holds a
            // resident claude's pipes so a server upgrade does not kill the session.
            Command::SessionShim(args) => args.run().await.map_err(GotchaError::message),
            // Hidden for the same reason `pty-shim` is, and with the same kind of
            // caller: the `droppen-notify` on a session's PATH is one line of shell
            // that `exec`s this. It is useless without the environment that spawn was
            // given, so a usage string offering it to a person would be offering
            // them a command that always answers "no channel configured".
            Command::Notify(args) => args.run().await.map_err(GotchaError::message),
            Command::Up(_) => unreachable!("`up` never reaches the async dispatcher"),
        }
    }
}

/// Said under the old name; [`Cli::command_as`] shows it as `droppen` when that
/// is the name it was run as, after the command list clap generates.
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
pub(crate) mod tests {
    use clap::error::ErrorKind;

    use super::*;
    use super::agent::{AgentArgs, AgentCommand};
    use crate::auth::cli::{UserArgs, UserCommand};

    /// `argv` (everything after the program name) as this binary parses it.
    pub(crate) fn parse(argv: &[&str]) -> Result<Command, clap::Error> {
        Cli::try_parse_from(std::iter::once("remote-hand-server").chain(argv.iter().copied())).map(Cli::into_command)
    }

    /// `command` followed by `words`, as this binary parses it.
    pub(crate) fn parse_words(command: &str, words: &[String]) -> Result<Command, clap::Error> {
        let argv: Vec<&str> = std::iter::once(command).chain(words.iter().map(String::as_str)).collect();
        parse(&argv)
    }

    /// `--version` and `preflight` print the HTTP kit's version, which has to
    /// be this binary's build and not the kit crate's own `0.1.0`.
    #[test]
    fn the_version_printed_is_the_servers_own() {
        assert_eq!(droppen_http::RUNTIME_VERSION, concat!("rust/", env!("CARGO_PKG_VERSION")));
        let rev = droppen_http::SERVER_VERSION.strip_prefix(env!("CARGO_PKG_VERSION")).expect("the server's version first");
        assert!(rev.is_empty() || rev.starts_with("+g"), "{rev}");
    }

    fn kind(argv: &[&str]) -> ErrorKind {
        match parse(argv) {
            Ok(command) => panic!("{argv:?} must be refused, got {command:?}"),
            Err(error) => error.kind(),
        }
    }

    fn strings(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_string()).collect()
    }

    fn tail(words: &[&str]) -> RawTail {
        RawTail { args: strings(words) }
    }

    /// Every command, with the raw tails exactly as clap leaves them.
    #[test]
    fn the_tree_recognizes_every_command() {
        for (argv, expected) in [
            (vec![], Command::Serve(tail(&[]))),
            (vec!["--"], Command::Serve(tail(&[]))),
            (vec!["serve"], Command::Serve(tail(&[]))),
            (vec!["serve", "--help"], Command::Serve(tail(&["--help"]))),
            (vec!["serve", "junk", "--x"], Command::Serve(tail(&["junk", "--x"]))),
            (vec!["serve", "--version"], Command::Serve(tail(&["--version"]))),
            (vec!["preflight", "--help"], Command::Preflight(tail(&["--help"]))),
            (vec!["preflight", "junk"], Command::Preflight(tail(&["junk"]))),
            (vec!["up", "--help"], Command::Up(UpArgs { child: strings(&["--help"]) })),
            (vec!["up", "--"], Command::Up(UpArgs { child: vec![] })),
            (vec!["up", "--", "serve", "--flag"], Command::Up(UpArgs { child: strings(&["serve", "--flag"]) })),
            (vec!["up", "serve", "--", "x"], Command::Up(UpArgs { child: strings(&["serve", "--", "x"]) })),
        ] {
            assert_eq!(parse(&argv).expect("parses"), expected, "{argv:?}");
        }
        for (argv, name) in [
            (vec!["user", "list"], "user"),
            (vec!["setup-code"], "setup-code"),
            (vec!["open", "--print"], "open"),
            (vec!["plugin", "list"], "plugin"),
            (vec!["agent", "status"], "agent"),
            (vec!["notify", "title"], "notify"),
            (vec!["replay", "--log", "x", "--home", "y"], "replay"),
            (vec!["export-openapi", "--out", "x"], "export-openapi"),
            (vec!["pty-shim", "--socket", "s", "--exit-file", "e", "--", "sh"], "pty-shim"),
            (vec!["session-shim", "--socket", "s", "--stderr-file", "r", "--exit-file", "e", "--", "claude"], "session-shim"),
        ] {
            let parsed = parse(&argv).expect("parses");
            let matched = match (&parsed, name) {
                (Command::User(_), "user")
                | (Command::SetupCode(_), "setup-code")
                | (Command::Open(_), "open")
                | (Command::Plugin(_), "plugin")
                | (Command::Agent(_), "agent")
                | (Command::Notify(_), "notify")
                | (Command::Replay(_), "replay")
                | (Command::ExportOpenapi(_), "export-openapi")
                | (Command::PtyShim(_), "pty-shim")
                | (Command::SessionShim(_), "session-shim") => true,
                _ => false,
            };
            assert!(matched, "{argv:?} parsed as {parsed:?}");
        }
    }

    /// Help and version are clap's; `--version` is printed bare by
    /// [`Cli::parse_invoked`] because self-upgrade reads all of stdout.
    #[test]
    fn help_and_version_are_claps() {
        for argv in [vec!["--help"], vec!["-h"], vec!["--help", "extra"], vec!["-hV"], vec!["-h", "--bogus"], vec!["--help", "--version"]] {
            assert_eq!(kind(&argv), ErrorKind::DisplayHelp, "{argv:?}");
        }
        for argv in [vec!["--version"], vec!["-V"], vec!["--version", "extra"], vec!["-Vh"], vec!["-V", "--bogus"], vec!["--version", "--help"]] {
            assert_eq!(kind(&argv), ErrorKind::DisplayVersion, "{argv:?}");
        }
        for (argv, expected) in [
            (vec!["--help=1"], ErrorKind::TooManyValues),
            (vec!["--version=1"], ErrorKind::TooManyValues),
            (vec!["bogus"], ErrorKind::InvalidSubcommand),
            (vec!["help"], ErrorKind::InvalidSubcommand),
            (vec!["--bogus"], ErrorKind::UnknownArgument),
            (vec!["-x"], ErrorKind::UnknownArgument),
            (vec!["--", "serve"], ErrorKind::UnknownArgument),
        ] {
            assert_eq!(kind(&argv), expected, "{argv:?}");
        }
    }

    /// `export-openapi --out <path>`; without it the spec goes to stdout.
    #[test]
    fn export_openapi_takes_one_out() {
        let out = |path: Option<&str>| Command::ExportOpenapi(crate::export::ExportOpenapiArgs { out: path.map(Into::into) });
        for (argv, expected) in [
            (vec!["export-openapi"], out(None)),
            (vec!["export-openapi", "--out", "a.json"], out(Some("a.json"))),
            (vec!["export-openapi", "--out=a.json"], out(Some("a.json"))),
            (vec!["export-openapi", "--out", "--"], out(Some("--"))),
            (vec!["export-openapi", "--out", "a.json", "--out", "b.json"], out(Some("b.json"))),
        ] {
            assert_eq!(parse(&argv).expect("parses"), expected, "{argv:?}");
        }
        for (argv, expected) in [
            (vec!["export-openapi", "--out"], ErrorKind::InvalidValue),
            (vec!["export-openapi", "junk"], ErrorKind::UnknownArgument),
            (vec!["export-openapi", "--out", "a.json", "junk"], ErrorKind::UnknownArgument),
        ] {
            assert_eq!(kind(&argv), expected, "{argv:?}");
        }
    }

    fn user(app: u8, yes: u8, command: UserCommand) -> Command {
        Command::User(UserArgs {
            instance: strings(&["probe"]),
            app,
            yes,
            command,
        })
    }

    /// The argv other programs pass, pinned to what each must mean: the
    /// desktop app's `user` calls (`desktop/core/src/accounts.rs`, always
    /// ending `--instance I`), `scripts/install.sh`'s `user add`, `agent
    /// status` and `agent enroll -`, and the supervisor's, the service files'
    /// and self-upgrade's `up`, `serve`, `--version` and `preflight`.
    #[test]
    fn the_argv_other_programs_pass_is_pinned() {
        for (argv, expected) in [
            (vec!["user", "list", "--instance", "probe"], user(0, 0, UserCommand::List)),
            (vec!["user", "list", "--app", "--instance", "probe"], user(1, 0, UserCommand::List)),
            (
                vec!["agent", "status", "--instance", "probe"],
                Command::Agent(AgentArgs {
                    hub: None,
                    instance: strings(&["probe"]),
                    command: AgentCommand::Status,
                }),
            ),
            (
                vec!["agent", "enroll", "-", "--hub", "ws://127.0.0.1:1", "--instance", "probe"],
                Command::Agent(AgentArgs {
                    hub: Some("ws://127.0.0.1:1".to_string()),
                    instance: strings(&["probe"]),
                    command: AgentCommand::Enroll { code: "-".to_string() },
                }),
            ),
            (vec!["up", "--", "serve"], Command::Up(UpArgs { child: strings(&["serve"]) })),
            (vec!["up"], Command::Up(UpArgs { child: vec![] })),
            (vec!["up", "serve"], Command::Up(UpArgs { child: strings(&["serve"]) })),
            (vec!["serve"], Command::Serve(tail(&[]))),
            (vec![], Command::Serve(tail(&[]))),
            (vec!["preflight"], Command::Preflight(tail(&[]))),
        ] {
            let parsed = parse(&argv).expect("parses");
            if let Command::User(args) = &parsed {
                assert!(args.flags_fit(), "{argv:?} runs");
            }
            if let Command::Up(up) = &parsed {
                assert_eq!(crate::supervisor::child_args(&up.child), strings(&["serve"]), "{argv:?} supervises `serve`");
            }
            assert_eq!(parsed, expected, "{argv:?}");
        }
        assert_eq!(kind(&["--version"]), ErrorKind::DisplayVersion);

        // A user name may be anything `^[a-z0-9._-]{1,32}$` allows, so in the
        // name position a hyphen-led word is a name, never a flag.
        for name in ["droppen-app", "ada", "me", "--help", "-h", "-x", "-", "--x", "-hh", "--version", "-V"] {
            let named = |command: fn(String) -> UserCommand| command(name.to_string());
            for (argv, expected) in [
                (vec!["user", "add", name, "--app", "--instance", "probe"], user(1, 0, named(|name| UserCommand::Add { name }))),
                (vec!["user", "passwd", name, "--instance", "probe"], user(0, 0, named(|name| UserCommand::Passwd { name }))),
                (vec!["user", "mark-app", name, "--instance", "probe"], user(0, 0, named(|name| UserCommand::MarkApp { name }))),
                (vec!["user", "rm", name, "--yes", "--instance", "probe"], user(0, 1, named(|name| UserCommand::Rm { name }))),
                (vec!["user", "add", name, "--instance", "probe"], user(0, 0, named(|name| UserCommand::Add { name }))),
            ] {
                let parsed = parse(&argv).expect("parses");
                let Command::User(args) = &parsed else { panic!("{argv:?}: {parsed:?}") };
                assert!(args.flags_fit(), "{argv:?} runs");
                assert_eq!(parsed, expected, "{argv:?}");
            }
        }

        // The one name clap cannot take: `--` ends the options, so a name of
        // exactly `--` is refused in these forms. Such an account is still
        // managed in the web settings.
        for (argv, expected) in [
            (vec!["user", "add", "--", "--app", "--instance", "probe"], ErrorKind::UnknownArgument),
            (vec!["user", "passwd", "--", "--instance", "probe"], ErrorKind::UnknownArgument),
            (vec!["user", "mark-app", "--", "--instance", "probe"], ErrorKind::UnknownArgument),
            (vec!["user", "rm", "--", "--yes", "--instance", "probe"], ErrorKind::UnknownArgument),
            (vec!["user", "add", "--", "--instance", "probe"], ErrorKind::UnknownArgument),
            (vec!["user", "add", "--instance", "probe", "--"], ErrorKind::MissingRequiredArgument),
        ] {
            assert_eq!(kind(&argv), expected, "{argv:?}");
        }
    }

    /// The shims' argv as the server spawns it (`terminals::spawn_shim`,
    /// `inject::shim::spawn_via_shim`): flags, `--`, then the command whole.
    #[test]
    fn the_shim_spawns_are_pinned() {
        use crate::inject::shim::SessionShimArgs;
        use droppen_pty::child::PtyShimArgs;
        let pty = |cols: Option<&str>, rows: Option<&str>, command: &[&str]| {
            Command::PtyShim(PtyShimArgs {
                socket: "/run/t.sock".into(),
                exit_file: "/run/t.exit".into(),
                cols: cols.map(Into::into),
                rows: rows.map(Into::into),
                command: strings(command),
            })
        };
        let session = |buffer_bytes: Option<&str>, command: &[&str]| {
            Command::SessionShim(SessionShimArgs {
                socket: "/run/s.sock".into(),
                stderr_file: "/run/s.err".into(),
                exit_file: "/run/s.exit".into(),
                buffer_bytes: buffer_bytes.map(Into::into),
                command: strings(command),
            })
        };
        for (argv, expected) in [
            (
                vec!["pty-shim", "--socket", "/run/t.sock", "--exit-file", "/run/t.exit", "--", "/bin/zsh", "-l"],
                pty(None, None, &["/bin/zsh", "-l"]),
            ),
            (
                vec!["pty-shim", "--socket", "/run/t.sock", "--exit-file", "/run/t.exit", "--cols", "120", "--rows", "40", "--", "claude", "--help", "--", "-x"],
                pty(Some("120"), Some("40"), &["claude", "--help", "--", "-x"]),
            ),
            (
                vec!["session-shim", "--socket", "/run/s.sock", "--stderr-file", "/run/s.err", "--exit-file", "/run/s.exit", "--", "claude", "-p", "--verbose"],
                session(None, &["claude", "-p", "--verbose"]),
            ),
            (
                vec![
                    "session-shim", "--socket", "/run/s.sock", "--stderr-file", "/run/s.err", "--exit-file", "/run/s.exit", "--buffer-bytes", "1048576", "--", "claude", "--",
                ],
                session(Some("1048576"), &["claude", "--"]),
            ),
        ] {
            assert_eq!(parse(&argv).expect("parses"), expected, "{argv:?}");
        }
    }

    /// The `pty-shim` command line as this binary parses it.
    mod pty_shim {
        use std::path::PathBuf;

        use clap::error::ErrorKind;

        use super::super::Command;
        use droppen_pty::child::PtyShimArgs;

        fn parse(words: &[String]) -> Result<PtyShimArgs, clap::Error> {
            match super::parse_words("pty-shim", words)? {
                Command::PtyShim(parsed) => Ok(parsed),
                other => panic!("{words:?} parsed as {other:?}"),
            }
        }

        #[test]
        fn the_command_line_needs_both_paths_and_a_separator() {
            let argv: Vec<String> = ["--socket", "/tmp/a.sock", "--exit-file", "/tmp/a.exit", "--", "/bin/sh", "-l"]
                .iter()
                .map(|s| s.to_string())
                .collect();
            let args = parse(&argv).expect("parse");
            assert_eq!(args.socket, PathBuf::from("/tmp/a.sock"));
            assert_eq!(args.exit_file, PathBuf::from("/tmp/a.exit"));
            assert_eq!(args.command, vec!["/bin/sh".to_string(), "-l".to_string()]);

            let missing_separator: Vec<String> = ["--socket", "/tmp/a.sock", "--exit-file", "/tmp/a.exit"]
                .iter()
                .map(|s| s.to_string())
                .collect();
            assert_eq!(
                parse(&missing_separator).unwrap_err().kind(),
                ErrorKind::MissingRequiredArgument,
                "the command must follow `--`"
            );

            let empty_command: Vec<String> = ["--socket", "/s", "--exit-file", "/e", "--"].iter().map(|s| s.to_string()).collect();
            assert_eq!(parse(&empty_command).unwrap_err().kind(), ErrorKind::MissingRequiredArgument);

            let no_socket: Vec<String> = ["--exit-file", "/e", "--", "/bin/sh"].iter().map(|s| s.to_string()).collect();
            assert_eq!(parse(&no_socket).unwrap_err().kind(), ErrorKind::MissingRequiredArgument);

            // An argument that looks like a flag is only a flag *before* the
            // separator; after it, it belongs to the shell.
            let after: Vec<String> = ["--socket", "/s", "--exit-file", "/e", "--", "/bin/sh", "--socket", "-c", "echo"]
                .iter()
                .map(|s| s.to_string())
                .collect();
            let parsed = parse(&after).expect("parse");
            assert_eq!(&parsed.command[1..], ["--socket", "-c", "echo"]);
        }
    }
}
