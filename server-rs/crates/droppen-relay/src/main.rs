//! `droppen-relay [serve]`, `keygen`, `deregister`, `rotate-key`.

use std::process::ExitCode;

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

async fn run(args: Vec<String>) -> Result<(), String> {
    match args.first().map(String::as_str) {
        None | Some("serve") => {
            let config = rh_relay::Config::from_env()?;
            let listener = tokio::net::TcpListener::bind(config.bind)
                .await
                .map_err(|e| format!("bind {}: {e}", config.bind))?;
            tracing::info!(id = %config.id, origin = %config.origin, hub = ?config.hub, addr = %config.bind, "droppen-relay listening");
            let server = rh_relay::Server::new(config);
            rh_relay::run(server, listener, signal()).await
        }
        Some("keygen") => {
            let key = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
            eprintln!(
                "public key {} — keep DROPPEN_RELAY_KEY secret (a secret env var, or DROPPEN_RELAY_STATE_DIR on a volume)",
                rh_relay::config::hex(&key.verifying_key().to_bytes())
            );
            println!("DROPPEN_RELAY_KEY={}", rh_relay::config::hex(key.as_bytes()));
            Ok(())
        }
        Some("deregister") => {
            let config = rh_relay::Config::from_env()?;
            let reason = args
                .iter()
                .position(|a| a == "--reason")
                .and_then(|i| args.get(i + 1))
                .cloned()
                .unwrap_or_else(|| "deregistered by its operator".into());
            rh_relay::uplink::deregister(&config, &reason).await?;
            eprintln!(
                "relay {} is retired: it leaves the directory and every account's set; a running process drains and stops",
                config.id
            );
            Ok(())
        }
        Some("rotate-key") => {
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
        Some("--help" | "-h") => {
            println!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(format!("unknown command {other:?}\n\n{USAGE}")),
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
