//! `droppen a2a-proxy` (`docs/a2a.md` §7.4): an a2a device that takes
//! standard A2A requests over HTTP — from a client on a private network or
//! this computer, dont-do on Railway say — and carries each over an
//! end-to-end channel, through a relay that sees only ciphertext, to the
//! device that runs the agent:
//!
//! ```text
//! A2A client ──A2A over HTTP──▶ droppen a2a-proxy ──Noise channel, via a relay──▶ device: bridge → /a2a/*
//! ```
//!
//! The proxy is a standalone device of the account (`droppen-client`'s
//! [`droppen_client::device`]): its own identity and verified chain under
//! `DROPPEN_DATA_DIR`, its own session at the hub, its own key in every
//! handshake. The hub and the relays route; what the proxy may reach, and
//! what a refusal means, is its own chain's to say ([`upstream`]).
//!
//! - [`config`]: the environment;
//! - [`route`]: `/a2a/<device>/agents/<entry>/…` and which binding answers;
//! - [`server`]: the listener — the token, the pipe, the streamed answer;
//! - [`card`]: the Agent Card's addresses, made the proxy's own;
//! - [`errors`]: the proxy's own answers, in each binding's form;
//! - [`upstream`]: where requests go, and how failures are judged;
//! - [`enroll`]: joining an account — the seam for what comes next.

pub mod card;
pub mod config;
pub mod enroll;
pub mod errors;
pub mod route;
pub mod server;
pub mod upstream;

use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use droppen_client::device::{DeviceOptions, StandaloneDevice};

pub use config::Config;
use server::Proxy;
use upstream::DeviceUpstream;

/// How often the proxy syncs its chain on its own (a removal, a change of
/// whom a device accepts, reach it this soon at the latest).
const SYNC_EVERY: Duration = Duration::from_secs(300);

/// Run the proxy with `config` until a stop signal (`SIGTERM`, `SIGINT`).
/// `version` is the program's, for the hello's client info.
pub async fn run(config: Config, version: &str) -> Result<(), String> {
    init_logging();
    let device = Arc::new(open(&config, version).await?);
    let identity = device.identity();
    tracing::info!(
        device = %identity.device,
        account = %identity.account,
        fingerprint = %identity.fingerprint(),
        hub = %config.hub,
        "the a2a proxy's device"
    );
    match device.sync_chain().await {
        Ok(_) => tracing::info!(seq = ?device.chain_seq(), "device chain synced"),
        Err(error) => tracing::warn!(%error, "could not sync the device chain; going on with the one kept"),
    }
    {
        let device = device.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(SYNC_EVERY);
            tick.tick().await;
            loop {
                tick.tick().await;
                if let Err(error) = device.sync_chain().await {
                    tracing::warn!(%error, "could not sync the device chain");
                }
            }
        });
    }
    let proxy = Arc::new(Proxy::new(
        Arc::new(DeviceUpstream::new(device)),
        &config.token,
        config.public_base.clone(),
        config.aliases.clone(),
    ));
    let listener = match tokio::net::TcpListener::bind(config.bind).await {
        Ok(listener) => listener,
        // No IPv6 here: every IPv4 address instead.
        Err(_) if config.bind.is_ipv6() && config.bind.ip().is_unspecified() => {
            tokio::net::TcpListener::bind(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), config.bind.port()))
                .await
                .map_err(|e| format!("listen on port {}: {e}", config.bind.port()))?
        }
        Err(e) => return Err(format!("listen on {}: {e}", config.bind)),
    };
    tracing::info!(addr = %listener.local_addr().map_err(|e| e.to_string())?, "the a2a proxy is listening");
    serve(listener, proxy, stop_signal()).await
}

/// Serve `proxy` on `listener` until `stop` resolves.
pub async fn serve(listener: tokio::net::TcpListener, proxy: Arc<Proxy>, stop: impl Future<Output = ()> + Send + 'static) -> Result<(), String> {
    axum::serve(listener, server::router(proxy))
        .with_graceful_shutdown(stop)
        .await
        .map_err(|e| format!("serve: {e}"))
}

/// The proxy's device, from its data directory; joining the account first
/// when it never did ([`enroll`]).
async fn open(config: &Config, version: &str) -> Result<StandaloneDevice, String> {
    let options = DeviceOptions {
        client: format!("droppen-a2a-proxy/{version}"),
        ..DeviceOptions::default()
    };
    let device = match StandaloneDevice::open_dir(&config.data_dir, options.clone())? {
        Some(device) => {
            if config.enroll_code.is_some() {
                tracing::warn!("DROPPEN_ENROLL_CODE is set but the proxy has joined its account already: remove it from the environment");
            }
            device
        }
        None => {
            enroll::enroll(config).await?;
            StandaloneDevice::open_dir(&config.data_dir, options)?.ok_or("the proxy did not join an account")?
        }
    };
    let joined = droppen_client::HubUrl::parse(&device.identity().hub)?.http;
    if joined != config.hub {
        return Err(format!(
            "the proxy joined its account at {joined}, not at DROPPEN_HUB ({}): set DROPPEN_HUB to {joined}, or start from an empty DROPPEN_DATA_DIR",
            config.hub
        ));
    }
    Ok(device)
}

/// `RUST_LOG` (default `info`) to stderr; nothing if a logger is set already.
fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).try_init();
}

/// `SIGTERM` or `SIGINT`.
async fn stop_signal() {
    #[cfg(unix)]
    {
        let mut term = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(term) => term,
            Err(_) => return std::future::pending().await,
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    tracing::info!("stopping");
}
