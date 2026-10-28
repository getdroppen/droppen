//! `droppen-relay [serve]`, `keygen`, `deregister`, `rotate-key`.

use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

const USAGE: &str = "\
USAGE:
    droppen-relay [serve]               relay (registers with the Hub on its first start)
    droppen-relay keygen                print a new key as DROPPEN_RELAY_KEY=… (the public key goes to stderr)
    droppen-relay deregister [--reason TEXT]
                                        retire this relay at the Hub (its key is void afterwards)
    droppen-relay rotate-key            have the Hub pin a new key; prints it as DROPPEN_RELAY_KEY=…

Configuration is environment only: DROPPEN_RELAY_HUB, DROPPEN_RELAY_ORIGIN, DROPPEN_RELAY_KEY,
DROPPEN_RELAY_REGISTRATION_TOKEN, PORT, … (see rh_relay::config). The old names,
RH_RELAY_*, are deprecated and stop working in v0.2.135: set the DROPPEN_RELAY_* name instead
(each old name used is logged once).";

fn main() -> ExitCode {
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
    let args: Vec<String> = std::env::args().skip(1).collect();
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("droppen-relay: {e}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(args)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("droppen-relay: {e}");
            ExitCode::FAILURE
        }
    }
}

/// What the command line asked for.
#[derive(Debug, PartialEq, Eq)]
enum Invocation {
    Help,
    Serve,
    Keygen,
    Deregister,
    RotateKey,
}

async fn run(args: Vec<String>) -> Result<(), String> {
    match parse(&args)? {
        Invocation::Help => {
            println!("{USAGE}");
            Ok(())
        }
        Invocation::Serve => {
            let config = rh_relay::Config::from_env()?;
            let listener = tokio::net::TcpListener::bind(config.bind)
                .await
                .map_err(|e| format!("bind {}: {e}", config.bind))?;
            tracing::info!(id = %config.id, origin = %config.origin, hub = ?config.hub, addr = %config.bind, "droppen-relay listening");
            let server = rh_relay::Server::new(config);
            rh_relay::run(server, listener, signal()).await
        }
        Invocation::Keygen => {
            let key = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
            eprintln!(
                "public key {} — keep DROPPEN_RELAY_KEY secret (a secret env var, or DROPPEN_RELAY_STATE_DIR on a volume)",
                rh_relay::config::hex(&key.verifying_key().to_bytes())
            );
            println!("DROPPEN_RELAY_KEY={}", rh_relay::config::hex(key.as_bytes()));
            Ok(())
        }
        Invocation::Deregister => {
            let config = rh_relay::Config::from_env()?;
            let reason = reason(&args);
            rh_relay::uplink::deregister(&config, &reason).await?;
            eprintln!(
                "relay {} is retired: it leaves the directory and every account's set; a running process drains and stops",
                config.id
            );
            Ok(())
        }
        Invocation::RotateKey => {
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
}

/// The command, or the usage. Help is the exact first argument, as it always
/// was; every command ignores a tail it does not read, which is what deployed
/// relays start with.
fn parse(args: &[String]) -> Result<Invocation, String> {
    if let Some(first) = args.first() {
        match first.as_str() {
            "--help" | "-h" => return Ok(Invocation::Help),
            // clap reads a leading `--` as its terminator, leaving no command
            // and so meaning `serve`; this binary has always called it unknown.
            "--" => return Err(format!("unknown command {first:?}\n\n{USAGE}")),
            _ => {}
        }
    }
    let cli = RelayCli::try_parse_from(std::iter::once("droppen-relay".to_string()).chain(args.iter().cloned()))
        .map_err(|error| format!("{error}\n\n{USAGE}"))?;
    Ok(match cli.command {
        None | Some(RelayCommand::Serve(_)) => Invocation::Serve,
        Some(RelayCommand::Keygen(_)) => Invocation::Keygen,
        Some(RelayCommand::Deregister(_)) => Invocation::Deregister,
        Some(RelayCommand::RotateKey(_)) => Invocation::RotateKey,
    })
}

/// `droppen-relay`'s command tree. Every command carries a hidden, ignored
/// tail: a relay's start command is fixed and deployed, so an argument a
/// command never read must keep being accepted and ignored.
#[derive(Debug, Parser)]
#[command(
    name = "droppen-relay",
    disable_help_flag = true,
    disable_version_flag = true,
    disable_help_subcommand = true,
    args_override_self = true
)]
struct RelayCli {
    #[command(subcommand)]
    command: Option<RelayCommand>,
}

#[derive(Debug, Subcommand)]
enum RelayCommand {
    #[command(disable_help_flag = true, disable_version_flag = true)]
    Serve(RelayTail),
    #[command(disable_help_flag = true, disable_version_flag = true)]
    Keygen(RelayTail),
    #[command(disable_help_flag = true, disable_version_flag = true)]
    Deregister(RelayTail),
    #[command(disable_help_flag = true, disable_version_flag = true)]
    RotateKey(RelayTail),
}

/// The arguments the command does not read. Declared so clap accepts them;
/// `deregister` scans the original tail for `--reason`.
#[derive(Debug, Args)]
struct RelayTail {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0.., hide = true)]
    args: Vec<String>,
}

/// The first exact `--reason` anywhere and the token after it, as the hand
/// parser read it; every other argument is ignored. `--reason=…` is not that
/// word, and `--reason` with nothing after it keeps the default.
fn reason(args: &[String]) -> String {
    args.iter()
        .position(|a| a == "--reason")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "deregistered by its operator".into())
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
    use super::*;

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_string()).collect()
    }

    /// Every command keeps accepting the tail a deployed relay may carry; help is the first token only.
    #[test]
    fn the_relay_argument_table() {
        assert_eq!(parse(&args(&[])).unwrap(), Invocation::Serve);
        assert_eq!(parse(&args(&["serve"])).unwrap(), Invocation::Serve);
        assert_eq!(parse(&args(&["serve", "junk", "--x"])).unwrap(), Invocation::Serve);
        assert_eq!(parse(&args(&["keygen", "--help"])).unwrap(), Invocation::Keygen);
        assert_eq!(parse(&args(&["deregister", "--reason", "x"])).unwrap(), Invocation::Deregister);
        assert_eq!(parse(&args(&["deregister", "junk", "--x"])).unwrap(), Invocation::Deregister);
        assert_eq!(parse(&args(&["rotate-key", "--x"])).unwrap(), Invocation::RotateKey);
        assert_eq!(parse(&args(&["--help"])).unwrap(), Invocation::Help);
        assert_eq!(parse(&args(&["-h", "junk"])).unwrap(), Invocation::Help, "the first token only");
        for argv in [
            vec!["bogus"],
            vec!["--"],
            vec!["--", "serve"],
            vec!["--version"],
            vec!["-V"],
            vec!["--help=x"],
        ] {
            assert!(parse(&args(&argv)).is_err(), "{argv:?} must be refused");
        }
    }

    #[test]
    fn deregister_reads_the_first_exact_reason() {
        assert_eq!(reason(&args(&["deregister"])), "deregistered by its operator");
        assert_eq!(reason(&args(&["deregister", "--reason"])), "deregistered by its operator");
        assert_eq!(reason(&args(&["deregister", "--reason", "x"])), "x");
        assert_eq!(reason(&args(&["deregister", "--reason=x", "--reason", "y"])), "y", "the exact word only");
        assert_eq!(reason(&args(&["deregister", "--reason", "--reason", "x"])), "--reason", "the next token is the value");
        assert_eq!(
            reason(&args(&["deregister", "--reason", "--instance", "x"])),
            "--instance",
            "even a word that looks like an option is the value"
        );
        assert_eq!(reason(&args(&["deregister", "junk", "--reason", "x", "junk"])), "x");
    }
}
