//! What the proxy reaches devices through ([`Upstream`]): the proxy's own
//! standalone device ([`DeviceUpstream`], `droppen-client`), or a stand-in in
//! tests. And how a failure is judged — by the proxy's own verified chain,
//! never by the far end's word.
//!
//! The proxy is a full device, so control is [`ChainState::may_control`]'s to
//! say: a current device to a current host, never itself, and only where the
//! host's accept policy lets it. The far ends are ambiguous (4401 and 4403
//! both mean removed and not accepted to a relay, 412 `lapsed` both a stale
//! view and a device that may no longer call), so a chain that has this
//! device removed or not accepted is asked first; only past that does the
//! failure's kind decide.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use droppen_client::device::StandaloneDevice;
use droppen_client::failure::Failure;
use droppen_client::rpc::RpcRequest;
use droppen_proto::chain::ChainState;
use futures_util::future::BoxFuture;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;

use crate::errors::Refusal;

/// An answer from a device: its head, and its body as it comes.
pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: BoxStream<'static, Result<Vec<u8>, Failure>>,
}

/// One message down a mux socket: a frame, or the socket's clean end.
pub enum MuxItem {
    Frame(Vec<u8>),
    Closed,
}

/// Where the proxy's requests go.
pub trait Upstream: Send + Sync + 'static {
    /// `request` to `host`'s loopback server.
    fn call<'a>(&'a self, host: &'a str, request: RpcRequest) -> BoxFuture<'a, Result<Answer, Failure>>;
    /// A `/api/events?v=2` socket to `host`, with `sub` (the subscription
    /// frame) already sent: the frames as they come.
    fn events<'a>(&'a self, host: &'a str, sub: String) -> BoxFuture<'a, Result<BoxStream<'static, Result<MuxItem, Failure>>, Failure>>;
    /// Whether this device may call `host` at all, by its own chain (synced
    /// first when the answer would be no): `Err` is answered at once, with
    /// nothing sent to the host.
    fn admits<'a>(&'a self, host: &'a str) -> BoxFuture<'a, Result<(), Refusal>>;
    /// What `failure` of a call to `host` means for the client.
    fn judge<'a>(&'a self, host: &'a str, failure: &'a Failure) -> BoxFuture<'a, Refusal>;
}

/// Where device `me` stands for `host` on `chain`: removed (no current device
/// of the account), or not accepted (the chain says it may not control it),
/// or fine.
pub fn standing(chain: &ChainState, me: &str, host: &str) -> Result<(), Refusal> {
    if chain.device(me).is_none() {
        return Err(Refusal::DeviceRemoved);
    }
    chain.may_control(me, host).map_err(|why| Refusal::HostNotAccepting { why: why.to_string() })
}

/// What `failure` of a call from `me` to `host` means, `chain` being `me`'s
/// verified view: whatever the far end said, a chain that has `me` removed or
/// not accepted says so; past that, a refusal the chain does not explain is
/// the host not accepting, and a `lapsed` one the same (its view is not this
/// one's yet). A fork or a protocol error is a channel failure, never a 500.
pub fn classify(chain: Option<&ChainState>, me: &str, host: &str, failure: &Failure) -> Refusal {
    if let Some(chain) = chain {
        if let Err(refusal) = standing(chain, me, host) {
            return refusal;
        }
    }
    match failure {
        Failure::Offline(_) => Refusal::HostOffline,
        Failure::Stale(_) => Refusal::HostStale,
        Failure::Lapsed(why) => Refusal::HostNotAccepting { why: why.clone() },
        Failure::Refused { why, .. } => Refusal::HostNotAccepting { why: why.clone() },
        Failure::Removed(_) => Refusal::DeviceRemoved,
        Failure::Chain(_) | Failure::Other(_) => Refusal::ChannelFailed,
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

    fn standing(&self, host: &str) -> Option<Result<(), Refusal>> {
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

    fn events<'a>(&'a self, host: &'a str, sub: String) -> BoxFuture<'a, Result<BoxStream<'static, Result<MuxItem, Failure>>, Failure>> {
        Box::pin(async move {
            let pipe = self.device.mux(host, "/api/events?v=2").await?;
            pipe.send_text(&sub).await?;
            // The pipe rides in the stream's state; finishing or failing lets
            // it go, which closes the socket.
            Ok(futures_util::stream::unfold(Some(pipe), |state| async move {
                let mut pipe = state?;
                match pipe.next().await {
                    Some(Ok(bytes)) => Some((Ok(MuxItem::Frame(bytes)), Some(pipe))),
                    Some(Err(failure)) => Some((Err(failure), None)),
                    None => Some((Ok(MuxItem::Closed), None)),
                }
            })
            .boxed())
        })
    }

    fn admits<'a>(&'a self, host: &'a str) -> BoxFuture<'a, Result<(), Refusal>> {
        Box::pin(async move {
            if let Some(Ok(())) = self.standing(host) {
                return Ok(());
            }
            self.sync().await;
            match self.standing(host) {
                Some(standing) => standing,
                // No verified chain yet: nothing to call anyone by.
                None => Err(Refusal::HostOffline),
            }
        })
    }

    fn judge<'a>(&'a self, host: &'a str, failure: &'a Failure) -> BoxFuture<'a, Refusal> {
        Box::pin(async move {
            if matches!(failure, Failure::Refused { .. } | Failure::Lapsed(_) | Failure::Removed(_)) {
                self.sync().await;
            }
            self.device.with_chain(|c| classify(c, self.me(), host, failure))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use droppen_client::failure::Refuser;

    #[test]
    fn each_failure_has_its_refusal() {
        for (failure, code, status) in [
            (Failure::Offline("x".into()), "host_offline", 503),
            (Failure::Stale("x".into()), "host_stale", 503),
            (Failure::Lapsed("x".into()), "host_not_accepting", 403),
            (
                Failure::Refused {
                    by: Refuser::Agent,
                    code: 4401,
                    why: "x".into(),
                },
                "host_not_accepting",
                403,
            ),
            (
                Failure::Refused {
                    by: Refuser::Relay,
                    code: 4403,
                    why: "x".into(),
                },
                "host_not_accepting",
                403,
            ),
            (Failure::Removed("x".into()), "device_removed", 410),
            (Failure::Chain("x".into()), "channel_failed", 502),
            (Failure::Other("x".into()), "channel_failed", 502),
        ] {
            let refusal = classify(None, "me", "host", &failure);
            assert_eq!((refusal.code(), refusal.status().as_u16()), (code, status), "{failure:?}");
        }
    }
}
