//! The A2A server's [`DeviceGate`] (`docs/a2a.md` §7.3): which A2A proxies
//! this device takes A2A calls from, answered from the running Droppen
//! link's verified device chain ([`droppen_cloud::control::Control`]). No
//! running link, no proxy: a device that is not linked, or whose link
//! stopped, admits none.

use std::sync::Arc;

use droppen_a2a::devices::{A2aDevice, DeviceGate};
use droppen_cloud::control::Control;
use droppen_http::openapi_ext::Nullable;

/// The gate over this server's link control.
pub struct LinkDevices {
    control: Arc<Control>,
}

impl LinkDevices {
    /// The gate over `control`.
    pub fn new(control: Arc<Control>) -> Arc<Self> {
        Arc::new(Self { control })
    }
}

impl DeviceGate for LinkDevices {
    fn admits(&self, device: &str) -> bool {
        self.control.takes_a2a_calls_from(device)
    }

    fn admitted(&self) -> Vec<A2aDevice> {
        // Neither the chain nor the other devices' copies name an A2A proxy
        // (it publishes no status of its own): its id and fingerprint are
        // what there is.
        self.control
            .a2a_callers()
            .into_iter()
            .map(|(id, fingerprint)| A2aDevice {
                id,
                fingerprint,
                name: Nullable(None),
            })
            .collect()
    }

    fn changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.control.a2a_changes()
    }
}
