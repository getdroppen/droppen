//! Entry point: `serve` (the default), `up`, and the tooling subcommands.

use remote_hand_server::cli::{Cli, Command};
use remote_hand_server::program::Name;

fn main() -> std::process::ExitCode {
    let command = Cli::parse_invoked().into_command();
    // `notify` is run by a session's `droppen-notify` script, and an agent
    // reads its errors under that name.
    let prefix = match command {
        Command::Notify(_) => remote_hand_server::notify::cli::NAME,
        _ => Name::invoked().error_prefix(),
    };

    // `up` runs before any runtime exists (`UpArgs::run` says why).
    if let Command::Up(up) = command {
        return match up.run() {
            Ok(code) => std::process::ExitCode::from(code.clamp(0, 255) as u8),
            Err(error) => {
                complain(prefix, &error);
                std::process::ExitCode::FAILURE
            }
        };
    }

    match tokio::runtime::Runtime::new() {
        Ok(runtime) => match runtime.block_on(command.run()) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                complain(prefix, &error.to_string());
                // 1 for everything except `serve` finding its instance already
                // owned, which gets `INSTANCE_HELD_EXIT` (75) so that `up` waits
                // for the holder instead of blaming — and rolling back — the
                // binary. The message above is the same either way.
                std::process::ExitCode::from(remote_hand_server::supervisor::failure_exit_code(&error))
            }
        },
        Err(error) => {
            complain(prefix, &format!("cannot start a runtime: {error}"));
            std::process::ExitCode::FAILURE
        }
    }
}

/// An error on stderr, said in the name this binary was run as
/// (`remote_hand_server::program`): `droppen: …` with the commands it suggests
/// as `droppen …`, or `remote-hand: …` exactly as before.
fn complain(prefix: &str, error: &str) {
    eprintln!("{prefix}: {}", Name::invoked().rewrite(error));
}
