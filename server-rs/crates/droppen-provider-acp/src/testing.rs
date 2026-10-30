//! What a server's test sets up of the ACP agents that production code does
//! only under the spawn gate. Only built for tests and with the
//! `test-support` feature.

use droppen_instance::paths::Paths;

use crate::agents::{AcpAgent, AcpAgentRegistry, AcpAgentUpdate};

/// Configure the agent `id` under `paths` as `patch` says, as the routes do
/// under the gate: for a test that needs an agent on file before anything
/// runs.
pub fn configure(paths: &Paths, id: &str, patch: &AcpAgentUpdate) -> Result<AcpAgent, String> {
    AcpAgentRegistry::of(paths).update(id, patch)
}
