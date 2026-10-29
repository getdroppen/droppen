//! `droppen-relay [serve]`, `keygen`, `deregister`, `rotate-key`.

use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

/// What the relay reads instead of options, shown under its help.
const CONFIGURATION: &str = "\
Configuration is environment only: DROPPEN_RELAY_HUB, DROPPEN_RELAY_ORIGIN, DROPPEN_RELAY_KEY,
DROPPEN_RELAY_REGISTRATION_TOKEN, PORT, … (see rh_relay::config). The names from before the
rename, RH_RELAY_*, are not read.";

/// The reason `deregister` gives the Hub when the operator names none.
const DEFAULT_REASON: &str = "deregistered by its operator";

fn main() -> ExitCode {
    let command = RelayCli::parse().into_command();
    // The websocket library logs whole frames at trace level: never, here,
    // whatever RUST_LOG says (a relay logs a frame's type, ch and length only).
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info".into())
        .add_directive("tungstenite=info".parse().expect("a directive"))
        .add_directive("tokio_tungstenite=info".parse().expect("a directive"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .with_writer(std::io::stderr)
        .init();
    rh_relay::uplink::tls_provider();
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("droppen-relay: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(command.run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("droppen-relay: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Droppen's relay. Without a command, it serves.
#[derive(Debug, PartialEq, Eq, Parser)]
#[command(name = "droppen-relay", disable_help_subcommand = true, after_help = CONFIGURATION)]
struct RelayCli {
    #[command(subcommand)]
    command: Option<RelayCommand>,
}

impl RelayCli {
    /// The command asked for; none is `serve`.
    fn into_command(self) -> RelayCommand {
        self.command.unwrap_or_else(|| RelayCommand::Serve(ServeArgs::default()))
    }
}

#[derive(Debug, PartialEq, Eq, Subcommand)]
enum RelayCommand {
    /// Run the relay (it registers with the Hub on its first start)
    Serve(ServeArgs),
    /// Print a new key as DROPPEN_RELAY_KEY=… (the public key goes to stderr)
    Keygen(KeygenArgs),
    /// Retire this relay at the Hub (its key is void afterwards)
    #[command(args_override_self = true)]
    Deregister(DeregisterArgs),
    /// Have the Hub pin a new key; prints it as DROPPEN_RELAY_KEY=…
    RotateKey(RotateKeyArgs),
}

impl RelayCommand {
    async fn run(self) -> Result<(), String> {
        match self {
            RelayCommand::Serve(serve) => serve.run().await,
            RelayCommand::Keygen(keygen) => keygen.run(),
            RelayCommand::Deregister(deregister) => deregister.run().await,
            RelayCommand::RotateKey(rotate) => rotate.run().await,
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq, Args)]
struct ServeArgs {}

impl ServeArgs {
    async fn run(self) -> Result<(), String> {
        let config = rh_relay::Config::from_env()?;
        let listener = tokio::net::TcpListener::bind(config.bind)
            .await
            .map_err(|e| format!("bind {}: {e}", config.bind))?;
        tracing::info!(id = %config.id, origin = %config.origin, hub = ?config.hub, addr = %config.bind, "droppen-relay listening");
        let server = rh_relay::Server::new(config);
        rh_relay::run(server, listener, signal()).await
    }
}

#[derive(Debug, Default, PartialEq, Eq, Args)]
struct KeygenArgs {}

impl KeygenArgs {
    fn run(self) -> Result<(), String> {
        let key = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
        eprintln!(
            "public key {} — keep DROPPEN_RELAY_KEY secret (a secret env var, or DROPPEN_RELAY_STATE_DIR on a volume)",
            rh_relay::config::hex(&key.verifying_key().to_bytes())
        );
        println!("DROPPEN_RELAY_KEY={}", rh_relay::config::hex(key.as_bytes()));
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq, Args)]
struct DeregisterArgs {
    /// Why the relay is retired, as the Hub records it
    // The word after `--reason` is its value even when it looks like an option,
    // and `--reason` with nothing after it keeps the default: an operator's
    // script that passes either still retires the relay.
    #[arg(
        long,
        value_name = "TEXT",
        num_args = 0..=1,
        default_value = DEFAULT_REASON,
        default_missing_value = DEFAULT_REASON,
        allow_hyphen_values = true
    )]
    reason: String,
}

impl DeregisterArgs {
    async fn run(self) -> Result<(), String> {
        let config = rh_relay::Config::from_env()?;
        rh_relay::uplink::deregister(&config, &self.reason).await?;
        eprintln!(
            "relay {} is retired: it leaves the directory and every account's set; a running process drains and stops",
            config.id
        );
        Ok(())
    }
}

#[derive(Debug, Default, PartialEq, Eq, Args)]
struct RotateKeyArgs {}

impl RotateKeyArgs {
    async fn run(self) -> Result<(), String> {
        let config = rh_relay::Config::from_env()?;
        let new = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
        rh_relay::uplink::rotate(&config, &new).await?;
        if rh_relay::config::env("DROPPEN_RELAY_KEY").is_none() {
            if let Some(dir) = &config.state_dir {
                rh_relay::config::write_private(&dir.join("relay.key"), &rh_relay::config::hex(new.as_bytes()))?;
                eprintln!("the Hub pinned the new key; {} has it", dir.join("relay.key").display());
                return Ok(());
            }
        }
        eprintln!("the Hub pinned the new key: set DROPPEN_RELAY_KEY to it and restart the relay");
        println!("DROPPEN_RELAY_KEY={}", rh_relay::config::hex(new.as_bytes()));
        Ok(())
    }
}

async fn signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use clap::error::ErrorKind;

    use super::*;

    fn parse(argv: &[&str]) -> Result<RelayCommand, clap::Error> {
        RelayCli::try_parse_from(std::iter::once("droppen-relay").chain(argv.iter().copied())).map(RelayCli::into_command)
    }

    fn deregister(reason: &str) -> RelayCommand {
        RelayCommand::Deregister(DeregisterArgs { reason: reason.to_string() })
    }

    /// The argv other programs start the relay with, and what each must mean:
    /// the image's `CMD` and `cloud-dev.sh` (`serve`), the tests and
    /// `rh-devkit` (no arguments), and the README and `docs/relay-and-plans.md`.
    #[test]
    fn the_callers_argv() {
        assert_eq!(parse(&[]).unwrap(), RelayCommand::Serve(ServeArgs::default()));
        assert_eq!(parse(&["serve"]).unwrap(), RelayCommand::Serve(ServeArgs::default()));
        assert_eq!(parse(&["keygen"]).unwrap(), RelayCommand::Keygen(KeygenArgs::default()));
        assert_eq!(parse(&["deregister"]).unwrap(), deregister(DEFAULT_REASON));
        assert_eq!(parse(&["rotate-key"]).unwrap(), RelayCommand::RotateKey(RotateKeyArgs::default()));
    }

    /// The commands take no words beyond their options; help is clap's.
    #[test]
    fn the_relay_argument_table() {
        assert_eq!(parse(&["deregister", "--reason", "x"]).unwrap(), deregister("x"));
        assert_eq!(parse(&["--"]).unwrap(), RelayCommand::Serve(ServeArgs::default()), "a bare -- is clap's terminator");
        for argv in [&["--help"][..], &["-h"], &["--help", "junk"], &["keygen", "--help"], &["serve", "--help"]] {
            assert_eq!(parse(argv).unwrap_err().kind(), ErrorKind::DisplayHelp, "{argv:?} must answer help");
        }
        for (argv, kind) in [
            (&["bogus"][..], ErrorKind::InvalidSubcommand),
            (&["--", "serve"], ErrorKind::UnknownArgument),
            (&["--version"], ErrorKind::UnknownArgument),
            (&["-V"], ErrorKind::UnknownArgument),
            (&["--help=x"], ErrorKind::TooManyValues),
            (&["serve", "junk", "--x"], ErrorKind::UnknownArgument),
            (&["keygen", "junk"], ErrorKind::UnknownArgument),
            (&["deregister", "junk", "--x"], ErrorKind::UnknownArgument),
            (&["rotate-key", "--x"], ErrorKind::UnknownArgument),
        ] {
            assert_eq!(parse(argv).unwrap_err().kind(), kind, "{argv:?} must be refused");
        }
    }

    #[test]
    fn deregister_reads_its_reason() {
        assert_eq!(parse(&["deregister"]).unwrap(), deregister(DEFAULT_REASON));
        assert_eq!(parse(&["deregister", "--reason"]).unwrap(), deregister(DEFAULT_REASON), "no value keeps the default");
        assert_eq!(parse(&["deregister", "--reason", "x"]).unwrap(), deregister("x"));
        assert_eq!(parse(&["deregister", "--reason=x", "--reason", "y"]).unwrap(), deregister("y"), "the last one wins");
        assert_eq!(parse(&["deregister", "--reason", "--reason"]).unwrap(), deregister("--reason"), "the next token is the value");
        assert_eq!(
            parse(&["deregister", "--reason", "--instance"]).unwrap(),
            deregister("--instance"),
            "even a word that looks like an option is the value"
        );
        for argv in [
            &["deregister", "--reason", "--reason", "x"][..],
            &["deregister", "--reason", "--instance", "x"],
            &["deregister", "junk", "--reason", "x", "junk"],
        ] {
            assert_eq!(parse(argv).unwrap_err().kind(), ErrorKind::UnknownArgument, "{argv:?}");
        }
    }
}
