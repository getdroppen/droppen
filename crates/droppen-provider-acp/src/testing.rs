//! What a server's test sets up of the ACP agents that production code
//! writes only through the agents' routes: an agent's configuration, which
//! they change under the spawn gate, and its last check. Only built for tests
//! and with the `test-support` feature.

use droppen_instance::paths::Paths;

use crate::agents::{AcpAgent, AcpAgentRegistry, AcpAgentUpdate, AcpCheck};

/// Configure the agent `id` under `paths` as `patch` says, as the routes do
/// under the gate: for a test that needs an agent on file before anything
/// runs.
pub fn configure(paths: &Paths, id: &str, patch: &AcpAgentUpdate) -> Result<AcpAgent, String> {
    AcpAgentRegistry::of(paths).update(id, patch)
}

/// Keep `check` as the agent `id`'s last check under `paths`, as the check
/// route does: for a test that needs one on file. `false` when there is no
/// such agent, or the check was not kept.
pub fn record_check(paths: &Paths, id: &str, check: &AcpCheck) -> bool {
    let agents = AcpAgentRegistry::of(paths);
    agents.get(id).is_some_and(|agent| agent.record_check(&agents, check))
}
