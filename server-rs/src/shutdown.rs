//! Graceful shutdown.
//!
//! gotcha has no shutdown story: [`GotchaApp::run`](gotcha::GotchaApp::run) calls
//! `axum::serve(listener, router).await` bare (`gotcha-0.4.1/src/lib.rs:230`),
//! with no `with_graceful_shutdown`, no signal handling, and no lifecycle hook
//! to attach one to. So the application overrides `run` and serves the router
//! itself — [`GotchaApp::build_router`](gotcha::GotchaApp::build_router) is a
//! public trait method that hands back the fully assembled `axum::Router`, which
//! is what makes that possible without reaching into the crate-private
//! `into_axum_router`.
//!
//! A [`CancellationToken`] lives in the application state, so a live websocket
//! sees the shutdown at the same moment the listener stops accepting.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::feed::global::GlobalFeed;
use crate::feed::FeedRegistry;

/// Resolves when the process is asked to stop.
///
/// SIGTERM matters as much as SIGINT here: a container runtime sends it, and
/// without it the supervisor would only work when a human pressed Ctrl-C.
pub async fn signalled() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(error) => {
                tracing::warn!(%error, "cannot listen for SIGTERM; only SIGINT will stop the server");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = interrupt => tracing::info!("received SIGINT"),
        _ = terminate => tracing::info!("received SIGTERM"),
    }
}

/// Cancel the token on the first signal, then run the registered cleanups.
///
/// Kept separate from the serve loop so the ordering is explicit: cancel first
/// (so sockets start closing and `axum::serve` stops accepting), then tear down
/// the background watchers that would otherwise keep the runtime alive.
pub async fn supervise(cancel: CancellationToken, cleanups: Cleanups) {
    tokio::select! {
        () = signalled() => tracing::info!("shutting down"),
        // A handler can ask for the same thing the signal does. `self-upgrade`
        // is the one that does: having installed a new binary it stops this
        // process so the supervisor starts the new one, and it wants the *whole*
        // shutdown — the cleanups that detach terminals and apps rather than
        // kill them — not a bare `exit`.
        () = cancel.cancelled() => tracing::info!("shutting down on request"),
    }
    cancel.cancel();
    cleanups.run();
}

/// Everything that must be torn down before the process exits.
///
/// A struct rather than a bare call so a later cleanup has an obvious place to
/// go without touching the serve loop.
///
/// A CLI remote-hand injected into — a resident `claude`, a `codex exec` run —
/// is deliberately **not** on this list. Both are headless: they finish on
/// their own and append to the session transcript, which the next boot reads
/// back in full. Killing them would throw
/// that turn away for nothing — the user asked for it, and no client is waiting
/// on the socket. A resident makes the point harder rather than softer, because
/// it is the *session*, not one turn: ending it at shutdown would take the
/// background work of every session on the machine with it.
#[derive(Clone)]
pub struct Cleanups {
    /// The live session feeds.
    pub feeds: Arc<FeedRegistry>,
    /// The live web terminals. Detached rather than killed on the way out —
    /// see [`crate::terminals::Registry::shutdown_all`].
    pub terminals: Arc<crate::terminals::Registry>,
    /// The running embedded apps. Detached rather than killed on the way out,
    /// for the same reason the terminals are — see
    /// [`crate::apps::Registry::shutdown_all`].
    pub apps: Arc<crate::apps::Registry>,
    /// The shared session-list watcher.
    pub global: Arc<GlobalFeed>,
}

impl Cleanups {
    /// Tear everything down, in registration order.
    pub fn run(&self) {
        let feeds = self.feeds.shutdown_all();
        self.global.shutdown();
        tracing::info!(feeds, "closed session feed watchers");
        // Web terminals are *detached*, not killed. Each one's pty belongs to a
        // `pty-shim` process that is meant to outlive this one; killing it here
        // would throw away exactly what a restart exists to keep. The next boot
        // finds the shims in the instance directory and reattaches.
        let terminals = self.terminals.shutdown_all();
        tracing::info!(terminals, "detached web terminals; their shims keep running");
        // Same reasoning one level up: a `code-server` is spawned detached, so
        // it outlives this process with the user's unsaved buffers in it. The
        // next boot finds it in the instance directory's `apps.json` and adopts
        // it back — see [`crate::apps::Registry::shutdown_all`].
        let apps = self.apps.shutdown_all();
        tracing::info!(apps, "detached embedded apps; they keep running");
    }
}
