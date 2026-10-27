//! Entry point: `serve` (the default), `up`, and the tooling subcommands.

use std::path::PathBuf;

use remote_hand_server::app::RemoteHandApp;
use remote_hand_server::program::Name;
use remote_hand_server::GotchaResult;

mod export;

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // `up` is deliberately outside the tokio runtime. It is a supervisor whose
    // one hard requirement is reaping *every* orphan with `waitpid(-1)`, and
    // tokio's process driver reaps its own children through the same call —
    // the two would race over each other's statuses. So the supervisor is plain
    // blocking code that owns the process, and only the paths that actually
    // serve build a runtime.
    if args.first().map(String::as_str) == Some("up") {
        return match remote_hand_server::supervisor::run(remote_hand_server::supervisor::child_args(&args[1..])) {
            Ok(code) => std::process::ExitCode::from(code.clamp(0, 255) as u8),
            Err(error) => {
                complain(&error);
                std::process::ExitCode::FAILURE
            }
        };
    }

    match tokio::runtime::Runtime::new() {
        Ok(runtime) => match runtime.block_on(run(args)) {
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

async fn run(args: Vec<String>) -> GotchaResult<()> {
    match args.first().map(String::as_str) {
        Some("export-openapi") => {
            let out = flag_value(&args, "--out").map(PathBuf::from);
            export::export_openapi(out).await
        }
        // Local accounts. Outside the server on purpose: the first account has
        // to exist before anyone can log in to create one, and the trust anchor
        // for that is the operating system — whoever can run this binary already
        // has everything the API could hand them. See `crate::auth::cli`.
        Some("user") => remote_hand_server::auth::cli::run(&args[1..])
            .await
            .map_err(remote_hand_server::GotchaError::message),
        // The one-time code a server in its setup state printed when it
        // started, read back from the file it wrote — for whoever missed the
        // banner, or started the server as a service.
        Some("setup-code") => remote_hand_server::auth::cli::setup_code(&args[1..])
            .await
            .map_err(remote_hand_server::GotchaError::message),
        // The page, opened in this device's browser — signed in as the
        // device's owner while the instance has no account. The proof is the
        // operating system's: only this user can reach the running server's
        // control socket (`crate::auth::local_open`).
        Some("open") => remote_hand_server::auth::cli::open(&args[1..])
            .await
            .map_err(remote_hand_server::GotchaError::message),
        // The plugins this build bundles, per instance: switches, and each
        // one's document store. Outside the server for the reason `user` is,
        // and safe beside a running one (`crate::plugins::cli`).
        Some("plugin") => remote_hand_server::plugins::cli::run(&args[1..])
            .await
            .map_err(remote_hand_server::GotchaError::message),
        // The cloud agent's skeleton (M0): creates this instance's machine
        // identity and describes what enrolling and running would do. Outside
        // `serve` entirely, so an instance nobody enrolls is unchanged.
        Some("agent") => remote_hand_server::agent::run(&args[1..])
            .await
            .map_err(remote_hand_server::GotchaError::message),
        Some("--help" | "-h") => {
            println!("{}", usage());
            Ok(())
        }
        // Printed bare, with no logging and no side effects, because
        // `POST /api/server/self-upgrade` runs exactly this on a binary it is
        // deciding whether to install and reads the whole of stdout as the
        // answer. Anything else on this stream would become part of the version.
        Some("--version" | "-V") => {
            println!("{}", remote_hand_server::api::SERVER_VERSION);
            Ok(())
        }
        // The other half of that health check: prove this build can load the
        // configuration it is about to be started with, which `--version` alone
        // says nothing about. Hidden from the usage string for the same reason
        // `pty-shim` is — it exists for one caller, and that caller is us.
        Some("preflight") => {
            let config = remote_hand_server::config::load(std::env::var("GOTCHA_ACTIVE_PROFILE").ok())?;
            println!("{} ok (instance {})", remote_hand_server::api::SERVER_VERSION, config.app.instance());
            Ok(())
        }
        // Hidden on purpose: nothing but the server itself should ever run
        // this, and it is documented where it is spawned rather than in a usage
        // string a user might act on.
        Some("pty-shim") => {
            let parsed = remote_hand_server::pty_shim::child::parse(&args[1..]).map_err(remote_hand_server::GotchaError::message)?;
            remote_hand_server::pty_shim::child::run(parsed)
                .await
                .map_err(remote_hand_server::GotchaError::message)
        }
        // The other half of `crate::record`, and the one a person runs: it
        // rebuilds a recorded session's transcript files under a scratch $HOME,
        // at the pace they were written, so a server pointed at that home
        // renders a real session through the real pipeline. Not hidden — it is
        // for developers to use, which is why it is in the usage string.
        Some("replay") => {
            let options = remote_hand_server::record::replay::parse(&args[1..]).map_err(remote_hand_server::GotchaError::message)?;
            remote_hand_server::record::replay::run(options)
                .await
                .map_err(remote_hand_server::GotchaError::message)
        }
        // Hidden for the same reason, and the same shape: the shim that holds a
        // resident claude's pipes so a server upgrade does not kill the session.
        Some("session-shim") => {
            let parsed = remote_hand_server::inject::shim::parse(&args[1..]).map_err(remote_hand_server::GotchaError::message)?;
            remote_hand_server::inject::shim::run(parsed)
                .await
                .map_err(remote_hand_server::GotchaError::message)
        }
        // Hidden for the same reason `pty-shim` is, and with the same kind of
        // caller: the `droppen-notify` on a session's PATH is one line of shell
        // that `exec`s this. It is useless without the environment that spawn was
        // given, so a usage string offering it to a person would be offering
        // them a command that always answers "no channel configured".
        Some("notify") => remote_hand_server::notify::cli::run(&args[1..])
            .await
            .map_err(remote_hand_server::GotchaError::message),
        Some("serve") | None => RemoteHandApp::default().run_server().await,
        Some(other) => {
            eprintln!("unknown command: {other}\n\n{}", usage());
            Err(remote_hand_server::GotchaError::message("unknown command"))
        }
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

/// [`USAGE`], in the name this binary was run as.
fn usage() -> String {
    let name = Name::invoked();
    let header = match name {
        Name::Droppen => "Droppen server: `droppen` (`remote-hand-server` is the same program under its old name).",
        Name::Legacy => "Droppen server: `remote-hand-server` (the same program as `droppen`).",
    };
    let mut body = name.rewrite(USAGE).into_owned();
    if name == Name::Droppen {
        // The continuation lines line up under the old name's arguments.
        let shift = Name::Legacy.command().len() - Name::Droppen.command().len();
        let (wide, narrow) = (format!("\n{:30}", ""), format!("\n{:1$}", "", 30 - shift));
        body = body.replace(&wide, &narrow);
    }
    format!("{header}\n\n{body}")
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let index = args.iter().position(|a| a == flag)?;
    args.get(index + 1).cloned()
}

/// Said under the old name; [`usage`] adds the header and says it as `droppen`
/// when that is the name it was run as.
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
server runs as. An instance created on a machine that already has accounts
inherits them. `APP_AUTH__ENABLED=false` switches authentication off — every
route then answers anyone who can reach the port; for development and tests
only.

`agent` is the cloud agent: `agent enroll` joins this instance to a Droppen
account (it needs a local account first, because a linked device's page reaches
other devices), `agent status` says where it stands, `agent unenroll` leaves.
`remote-hand-server agent` alone prints the details. An instance that is never
enrolled serves exactly as before.

`plugin` manages the plugins this binary bundles, for one instance: whether
each is on (the settings page does the same; `[plugins] enable`/`disable` in the
configuration, or APP_PLUGINS__ENABLE/DISABLE, pin it), and each one's document
store under <data>/instances/<instance>/plugins/<id>/ — export it as JSON,
import such a file back, check it, or move it aside once the plugin is off.
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
