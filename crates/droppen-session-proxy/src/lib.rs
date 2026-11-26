//! `droppen session-proxy`: a device of the account that takes a small
//! session HTTP surface from an external backend — a service in the cloud,
//! such as dont-do on Railway — and carries each request over an end-to-end
//! channel, through a relay that sees only ciphertext, to a computer's
//! `/api/sessions/*`:
//!
//! ```text
//! backend ──HTTP──▶ droppen session-proxy ──Noise channel, via a relay──▶ computer: bridge → /api/*
//! ```
//!
//! Unlike the A2A proxy the session proxy is a **normal device**: it enrolls
//! with an `RH1-` code (`kind: "machine"`), the page adds it as a device that
//! runs AI, and it is a full controller — `/api/*`, sessions and all — its own
//! chain says what a refusal means ([`upstream`]).
//!
//! The proxy is a standalone device of the account (`droppen-client`'s
//! [`droppen_client::device`]): its own identity and verified chain under
//! `DROPPEN_DATA_DIR`, its own session at the hub, its own key in every
//! handshake. The hub and the relays route; what the proxy may reach is its
//! own chain's to say.
//!
//! - [`config`]: the environment;
//! - [`route`]: the session → device registry, and what a `/rpc` path may be;
//! - [`server`]: the listener — the token, the routes, the streamed answers;
//! - [`errors`]: the proxy's own answers, and the status each refusal has;
//! - [`upstream`]: where requests go, and how failures are judged;
//! - [`enroll`]: joining an account with an `RH1` code, on the first start.

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

use droppen_client::device::DeviceOptions;
use droppen_client::relays::RelayOptions;

pub use config::Config;
use server::Proxy;
use upstream::DeviceUpstream;

/// How often the proxy syncs its chain on its own (a removal, a change of
/// whom a device accepts, reach it this soon at the latest).
const SYNC_EVERY: Duration = Duration::from_secs(300);

/// Run the proxy with `config` until a stop signal (`SIGTERM`, `SIGINT`).
/// `version` is the program's, for the hello's client info.
///
/// It listens first — its health answers while it joins its account, which
/// can take as long as the user takes on the page — then opens its device,
/// joining the account on its first start ([`enroll::open_or_join`]), and
/// from then on carries requests.
pub async fn run(config: Config, version: &str) -> Result<(), String> {
    init_logging();
    let proxy = Arc::new(Proxy::waiting(&config.token));
    let listener = bind(config.bind).await?;
    tracing::info!(addr = %listener.local_addr().map_err(|e| e.to_string())?, "the session proxy is listening");
    let mut serving = tokio::spawn(serve(listener, proxy.clone(), stop_signal()));
    let options = device_options(version, &config);
    let device = tokio::select! {
        device = enroll::open_or_join(&config, options) => Arc::new(device?),
        served = &mut serving => return served.map_err(|e| format!("serve: {e}"))?,
    };
    let identity = device.identity();
    tracing::info!(
        device = %identity.device,
        account = %identity.account,
        fingerprint = %identity.fingerprint(),
        hub = %config.hub,
        "the session proxy's device"
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
    proxy.ready(Arc::new(DeviceUpstream::new(device)));
    tracing::info!("the session proxy carries requests");
    serving.await.map_err(|e| format!("serve: {e}"))?
}

/// How the proxy's stand-alone device runs: its hello's client info, and
/// whether it may dial a relay on loopback over `ws://`
/// (`DROPPEN_SESSION_PROXY_RELAY_INSECURE`; a self-run relay, local testing
/// only).
fn device_options(version: &str, config: &Config) -> DeviceOptions {
    DeviceOptions {
        client: format!("droppen-session-proxy/{version}"),
        relays: RelayOptions {
            allow_loopback: config.relay_insecure,
            ..RelayOptions::default()
        },
        ..DeviceOptions::default()
    }
}

/// Listen on `addr`; on every IPv4 address instead when it is `[::]` and the
/// host has no IPv6.
async fn bind(addr: SocketAddr) -> Result<tokio::net::TcpListener, String> {
    match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => Ok(listener),
        Err(_) if addr.is_ipv6() && addr.ip().is_unspecified() => {
            tokio::net::TcpListener::bind(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), addr.port()))
                .await
                .map_err(|e| format!("listen on port {}: {e}", addr.port()))
        }
        Err(e) => Err(format!("listen on {addr}: {e}")),
    }
}

/// Serve `proxy` on `listener` until `stop` resolves.
pub async fn serve(listener: tokio::net::TcpListener, proxy: Arc<Proxy>, stop: impl Future<Output = ()> + Send + 'static) -> Result<(), String> {
    axum::serve(listener, server::router(proxy))
        .with_graceful_shutdown(stop)
        .await
        .map_err(|e| format!("serve: {e}"))
}

/// The container's health check (`droppen session-proxy --health`): `GET
/// /healthz` on this computer's `port`, answered 200 within 5 s. Plain
/// HTTP/1.0 over a socket: the check runs where there is no HTTP client.
pub fn check_health(port: u16) -> Result<(), String> {
    use std::io::{Read, Write};
    let addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port);
    let timeout = Duration::from_secs(5);
    let mut socket = std::net::TcpStream::connect_timeout(&addr, timeout).map_err(|e| format!("{addr}: {e}"))?;
    socket.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    socket.set_write_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    socket
        .write_all(b"GET /healthz HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .map_err(|e| format!("{addr}: {e}"))?;
    let mut answer = Vec::new();
    socket.take(4096).read_to_end(&mut answer).map_err(|e| format!("{addr}: {e}"))?;
    let status = String::from_utf8_lossy(&answer).lines().next().unwrap_or_default().to_string();
    match status.split_whitespace().nth(1) {
        Some("200") => Ok(()),
        _ => Err(format!("{addr}/healthz answered {status:?}")),
    }
}

/// `RUST_LOG` (default `info`) to stderr; nothing if a logger is set already.
fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    // Colours only for a person at a terminal: a container's log is plain.
    let ansi = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(ansi)
        .try_init();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn config(insecure: bool) -> Config {
        let values: Vec<(&str, String)> = vec![
            ("DROPPEN_DATA_DIR", "/data".to_string()),
            ("DROPPEN_SESSION_PROXY_TOKEN", "0123456789abcdef0123456789abcdef".to_string()), // gitleaks:allow
            ("DROPPEN_SESSION_PROXY_RELAY_INSECURE", if insecure { "1" } else { "" }.to_string()),
        ];
        Config::from_vars(|name| values.iter().find(|(k, _)| *k == name).map(|(_, v)| v.clone())).unwrap()
    }

    #[test]
    fn only_the_insecure_flag_opens_loopback_relays() {
        assert!(!device_options("v", &config(false)).relays.allow_loopback);
        assert!(device_options("v", &config(true)).relays.allow_loopback);
        assert_eq!(device_options("1.2.3", &config(false)).client, "droppen-session-proxy/1.2.3");
    }
}
