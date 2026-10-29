//! An ACP agent's model list: the values of the `model` option it publishes.

use super::{ModelInfo, ModelListSource, ModelsState};
use droppen_http::openapi_ext::Nullable;

/// An ACP agent's list: the values of its `model` option, each with the
/// ladder of its `thought_level` option, as the agent itself publishes them
/// ([`crate::inject::InjectionRegistry::acp_options`]).
///
/// The agent's list is the whole list: there is no static guess to fall back
/// on, so an agent that cannot be asked — or is not configured — yields no
/// models, which hides the pickers, and a `fallback` source, which says why.
///
/// With the models, how many were left out: see [`ModelList::hidden`].
pub(super) async fn acp_models(state: &ModelsState, agent: &str) -> (Vec<ModelInfo>, ModelListSource, u32) {
    use crate::acp::{category, option_for};
    let Some(agent) = crate::acp_agent::AcpAgentRegistry::of(&state.paths).get(agent) else {
        return (Vec::new(), ModelListSource::Fallback, 0);
    };
    let options = match state.injections.acp_options(&agent).await {
        Ok(options) => options,
        Err(error) => {
            tracing::info!(agent = %agent.id, %error, "cannot read the agent's models");
            return (Vec::new(), ModelListSource::Fallback, 0);
        }
    };
    // The agent's words, shown to whoever can start a session: a label is
    // redacted as its errors are, and no longer than a name. An id goes back
    // to the agent as it is when picked, so one that looks like a credential,
    // or is too long to show whole, is left out rather than shown altered
    // ([`crate::inject::InjectionRegistry::acp_id_shown`]).
    let injections = &state.injections;
    let shown = |text: &str| droppen_base::text::clip(&injections.redact_acp(&agent, text), crate::acp_agent::MAX_AGENT_WORD);
    let usable = |id: &str| injections.acp_id_shown(id);
    let effort = option_for(&options, category::THOUGHT_LEVEL);
    let efforts = effort.map(|effort| {
        effort
            .values()
            .iter()
            .map(|value| value.value.clone())
            .filter(|value| usable(value))
            .collect::<Vec<_>>()
    });
    let default_effort = effort.and_then(|effort| effort.current_value.clone()).filter(|value| usable(value));
    let offered = option_for(&options, category::MODEL).map(|model| model.values()).unwrap_or_default();
    let models: Vec<ModelInfo> = offered
        .iter()
        .filter(|value| usable(&value.value))
        .map(|value| ModelInfo {
            id: value.value.clone(),
            label: Nullable((!value.name.is_empty() && value.name != value.value).then(|| shown(&value.name))),
            efforts: Nullable(efforts.clone()),
            default_effort: Nullable(default_effort.clone()),
        })
        .collect();
    let hidden = u32::try_from(offered.len() - models.len()).unwrap_or(u32::MAX);
    (models, ModelListSource::Cache, hidden)
}
