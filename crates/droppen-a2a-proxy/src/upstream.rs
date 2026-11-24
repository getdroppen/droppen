//! What the proxy reaches devices through ([`Upstream`]): the proxy's own
//! standalone device ([`DeviceUpstream`], `droppen-client`), or a stand-in in
//! tests. And how a failure is judged — by the proxy's own verified chain,
//! never by the far end's word (`docs/a2a.md` §7.4): the agent closes a hello
//! it will not take with 4401 whether this device was removed or is not
//! accepted, a relay refuses with 4403 for both, the bridge answers 412
//! `lapsed` for a stale view and for a device that may no longer call.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use droppen_client::device::StandaloneDevice;
use droppen_client::failure::Failure;
use droppen_client::rpc::RpcRequest;
use droppen_proto::chain::ChainState;
use futures_util::future::BoxFuture;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;

use crate::errors::Code;

/// An answer from a device: its head, and its body as it comes.
pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: BoxStream<'static, Result<Vec<u8>, Failure>>,
}

/// Where the proxy's requests go.
pub trait Upstream: Send + Sync + 'static {
    /// `request` to `host`'s loopback server.
    fn call<'a>(&'a self, host: &'a str, request: RpcRequest) -> BoxFuture<'a, Result<Answer, Failure>>;
    /// Whether this device may call `host` at all, by its own chain (synced
    /// first when the answer would be no): `Err(code)` is answered at once,
    /// with nothing sent to the host.
    fn admits<'a>(&'a self, host: &'a str) -> BoxFuture<'a, Result<(), Code>>;
    /// What `failure` of a call to `host` means for the client.
    fn judge<'a>(&'a self, host: &'a str, failure: &'a Failure) -> BoxFuture<'a, Option<Code>>;
}

/// Whether device `me` may call `host`'s A2A server, by `chain`.
///
/// Chain v3's a2a devices and `SetA2aAccept` are another lane's (M2a:
/// `ChainState::may_call_a2a`); until they land this asks `may_control` —
/// what admits a device that is no admin to a host today, through the host's
/// `SetAccept`. Switch here, and only here, once `may_call_a2a` exists: an
/// a2a device is never one `may_control` admits.
pub fn may_call(chain: &ChainState, me: &str, host: &str) -> bool {
    chain.may_control(me, host).is_ok()
}

/// Where device `me` stands for `host` on `chain`: removed (no current
/// device of the account), not accepted, or fine.
pub fn standing(chain: &ChainState, me: &str, host: &str) -> Result<(), Code> {
    if chain.device(me).is_none() {
        return Err(Code::DeviceRemoved);
    }
    if !may_call(chain, me, host) {
        return Err(Code::NotAccepted);
    }
    Ok(())
}

/// What `failure` of a call from `me` to `host` means, `chain` being `me`'s
/// verified view: whatever the far end said, a chain that has `me` removed
/// or not accepted says so; past that, a refusal the chain does not explain
/// is the host not accepting (its view, or the hub's, is not this one yet),
/// and a `lapsed` one a view nobody vouched for. `None`: no code fits (a fork,
/// a protocol error), an internal error.
pub fn classify(chain: Option<&ChainState>, me: &str, host: &str, failure: &Failure) -> Option<Code> {
    if let Some(Err(code)) = chain.map(|c| standing(c, me, host)) {
        return Some(code);
    }
    match failure {
        Failure::Offline(_) => Some(Code::DeviceOffline),
        Failure::Stale(_) | Failure::Lapsed(_) => Some(Code::AttestationStale),
        Failure::Refused { .. } => Some(Code::NotAccepted),
        Failure::Removed(_) => Some(Code::DeviceRemoved),
        Failure::Chain(_) | Failure::Other(_) => None,
    }
}

/// The least time between two syncs a request asks for.
const SYNC_EVERY: Duration = Duration::from_secs(5);
/// How long a request waits for one.
const SYNC_WAIT: Duration = Duration::from_secs(10);

/// The proxy's own device.
pub struct DeviceUpstream {
    device: Arc<StandaloneDevice>,
    synced: Mutex<Option<Instant>>,
}

impl DeviceUpstream {
    pub fn new(device: Arc<StandaloneDevice>) -> Self {
        Self {
            device,
            synced: Mutex::new(None),
        }
    }

    /// Sync the chain, at most every [`SYNC_EVERY`] for requests, waiting at
    /// most [`SYNC_WAIT`]; a failure leaves the view as it is.
    async fn sync(&self) {
        {
            let mut synced = self.synced.lock().expect("sync clock");
            if synced.is_some_and(|at| at.elapsed() < SYNC_EVERY) {
                return;
            }
            *synced = Some(Instant::now());
        }
        match tokio::time::timeout(SYNC_WAIT, self.device.sync_chain()).await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => tracing::warn!(%error, "could not sync the device chain"),
            Err(_) => tracing::warn!("the device chain did not sync in time"),
        }
    }

    fn me(&self) -> &str {
        &self.device.identity().device
    }

    fn standing(&self, host: &str) -> Option<Result<(), Code>> {
        self.device.with_chain(|c| c.map(|c| standing(c, self.me(), host)))
    }
}

impl Upstream for DeviceUpstream {
    fn call<'a>(&'a self, host: &'a str, request: RpcRequest) -> BoxFuture<'a, Result<Answer, Failure>> {
        Box::pin(async move {
            let response = self.device.rpc(host, request).await?;
            Ok(Answer {
                status: response.status,
                headers: response.headers,
                body: response.body.into_stream().boxed(),
            })
        })
    }

    fn admits<'a>(&'a self, host: &'a str) -> BoxFuture<'a, Result<(), Code>> {
        Box::pin(async move {
            if let Some(Ok(())) = self.standing(host) {
                return Ok(());
            }
            self.sync().await;
            match self.standing(host) {
                Some(standing) => standing,
                // No verified chain yet: nothing to call anyone by.
                None => Err(Code::DeviceOffline),
            }
        })
    }

    fn judge<'a>(&'a self, host: &'a str, failure: &'a Failure) -> BoxFuture<'a, Option<Code>> {
        Box::pin(async move {
            if matches!(failure, Failure::Refused { .. } | Failure::Lapsed(_) | Failure::Removed(_)) {
                self.sync().await;
            }
            self.device.with_chain(|c| classify(c, self.me(), host, failure))
        })
    }
}
