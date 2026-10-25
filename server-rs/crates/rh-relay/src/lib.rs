//! Droppen's relay (`docs/relay-and-plans.md` §2), `droppen-relay`: the one
//! place a device's end-to-end channel meets a machine's. A relay pairs an
//! `OPEN` from a device with the machine's connection, copies `DATA` between
//! the two without parsing it, and enforces the credit windows and the limits
//! that bound what any one peer can make it hold. It has no database and no
//! keys to anything: it checks the Hub's tickets offline, holds nothing a
//! restart would miss, and only ever sees ciphertext.
//!
//! - [`route`] — the routing table and credit, sans-IO;
//! - [`server`] — who is signed in on which ticket, revocations, throttles;
//! - [`client`] — a device's or an agent's connection (`/v1/connect`);
//! - [`http`] — `/healthz`, `/v1/ping`, `/v1/relay-id`, and the per-address limits;
//! - [`uplink`] — the control link to the Hub (`/v1/relay`): registration,
//!   heartbeats, usage, revocations;
//! - [`meter`] — transfer per account;
//! - [`conn`] — a connection's budgeted sending half and its writer task;
//! - [`limit`] — token buckets;
//! - [`config`] — the environment.

pub mod client;
pub mod config;
pub mod conn;
pub mod http;
pub mod limit;
pub mod meter;
pub mod route;
pub mod server;
pub mod traffic;
pub mod uplink;

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub use config::Config;
pub use server::Server;

/// Serve on `listener` until `stop` resolves or the Hub refuses this relay
/// for good; then `goaway` everyone, wait out the drain and close (1012).
/// `Err` carries a fatal refusal's reason.
pub async fn run(server: Arc<Server>, listener: tokio::net::TcpListener, stop: impl Future<Output = ()> + Send + 'static) -> Result<(), String> {
    server.load_saved_keys();
    let app = http::router(server.clone());
    let http_stop = tokio_util::sync::CancellationToken::new();
    let serving = {
        let stopped = http_stop.clone();
        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
                .with_graceful_shutdown(async move { stopped.cancelled().await })
                .await
        })
    };
    // Usage windows close on time, whether or not the Hub is reachable.
    let cutter = server.clone();
    server.tasks.spawn(async move {
        let mut tick = tokio::time::interval(cutter.config.timing.usage_every);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => cutter.meter.cut(server::now_ms()),
                () = cutter.shutdown.cancelled() => break,
            }
        }
    });
    // The per-address limits forget each address once it is done counting,
    // a day after its last request at the latest (droppen.app/privacy).
    let forgetter = server.clone();
    server.tasks.spawn(async move {
        let mut tick = tokio::time::interval(limit::SWEEP_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    forgetter.unauth.forget_old(Instant::now());
                }
                () = forgetter.shutdown.cancelled() => break,
            }
        }
    });
    let fatal = async {
        if server.config.hub.is_some() {
            uplink::run(server.clone()).await
        } else {
            std::future::pending().await
        }
    };
    let result = tokio::select! {
        () = stop => Ok(()),
        reason = fatal => Err(reason),
    };
    drain(&server, &result).await;
    http_stop.cancel();
    server.shutdown.cancel();
    server.tasks.close();
    if tokio::time::timeout(Duration::from_secs(5), server.tasks.wait()).await.is_err() {
        tracing::warn!("some connections did not close in time");
    }
    let _ = tokio::time::timeout(Duration::from_secs(5), serving).await;
    result
}

/// `goaway`, then wait (at most the drain time) for clients to move on.
async fn drain(server: &Arc<Server>, why: &Result<(), String>) {
    let drain = server.config.drain;
    let reason = match why {
        Ok(()) => "restart",
        Err(_) => "stopping",
    };
    tracing::info!(drain_s = drain.as_secs(), reason, "draining: goaway to every connection");
    server.goaway(drain.as_millis() as u64, reason);
    server.tell_hub(&serde_json::json!({"t": "draining", "until_ms": server::now_ms() + drain.as_millis() as u64}));
    let until = Instant::now() + drain;
    while Instant::now() < until && server.counts().0 > 0 {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
