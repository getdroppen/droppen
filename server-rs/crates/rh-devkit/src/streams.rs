//! Reading a machine's encrypted streams from the hub, as a device does
//! (`rh_client::streams`, with this device's session for the pages).

use std::time::Duration;

use droppen_proto::record::StreamId;

pub use rh_client::streams::{FeedCopy, IndexCopy, StreamCopy, CONTROL_FRAME};

use crate::{Device, Link, Result};

impl Device {
    /// Read `stream` once, whole, verified: `machine_key` is the writer's
    /// Ed25519 key, `mk` its machine key for `epoch`.
    pub async fn read_stream(
        &self,
        link: &Link,
        machine_key: [u8; 32],
        mk: &[u8; 32],
        epoch: u32,
        stream: StreamId,
        timeout: Duration,
    ) -> Result<StreamCopy> {
        self.read_stream_keys(link, machine_key, &[(epoch, *mk)], stream, timeout).await
    }

    /// [`Device::read_stream`] with every machine key epoch the reader holds.
    pub async fn read_stream_keys(
        &self,
        link: &Link,
        machine_key: [u8; 32],
        keys: &[(u32, [u8; 32])],
        stream: StreamId,
        timeout: Duration,
    ) -> Result<StreamCopy> {
        rh_client::streams::read_stream(link, self.pages(), machine_key, keys, stream, timeout).await
    }

    /// A machine's index as a device reading epoch names finds it
    /// ([`rh_client::streams::read_index`]).
    pub async fn read_index(
        &self,
        link: &Link,
        machine_key: [u8; 32],
        keys: &[(u32, [u8; 32])],
        timeout: Duration,
    ) -> Result<(Option<u32>, StreamCopy)> {
        rh_client::streams::read_index(link, self.pages(), machine_key, keys, timeout).await
    }
}

/// Wait (bounded) for `pred` to hold of a fresh read of `stream`.
#[allow(clippy::too_many_arguments)]
pub async fn read_until(
    device: &Device,
    link: &Link,
    machine_key: [u8; 32],
    mk: &[u8; 32],
    epoch: u32,
    stream: StreamId,
    within: Duration,
    pred: impl Fn(&StreamCopy) -> bool,
) -> Result<StreamCopy> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let copy = device.read_stream(link, machine_key, mk, epoch, stream, Duration::from_secs(15)).await;
        match copy {
            Ok(copy) if pred(&copy) => return Ok(copy),
            Ok(_) | Err(_) if tokio::time::Instant::now() < deadline => tokio::time::sleep(Duration::from_millis(300)).await,
            Ok(_) => return Err("the stream never reached the expected state".into()),
            Err(e) => return Err(e),
        }
    }
}
