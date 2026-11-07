//! An ACP agent's model list: the values of the `model` option it publishes.

use std::sync::Arc;

use futures_util::future::BoxFuture;

use droppen_provider::models::{ModelInfo, ModelList, ModelListSource, ModelRule, Models, ModelsQuery};
use crate::AcpRuns;
use droppen_http::openapi_ext::Nullable;
use droppen_instance::paths::Paths;

/// A model id as an ACP agent spells it: `provider:model`, where the model
/// half is often a path (`openrouter:vendor/name`). Still nothing that could
/// read as a flag or hold a space; the agent checks it against its own list.
pub const ACP_MODEL_PATTERN: &str = r"^[A-Za-z0-9][[:word:].:/@+-]*$";

/// The ACP agents' models, for [`Models`]: each agent's own list, asked of the
/// agent ([`AcpRuns::acp_options`]).
pub(crate) struct AcpModels {
    /// The instance's paths, which hold the agents' configurations.
    paths: Arc<Paths>,
    /// The live runs, which ask an ACP agent for its options.
    runs: Arc<AcpRuns>,
}

impl AcpModels {
    /// The models of the agents configured under `paths`, asked through
    /// `runs`.
    pub(crate) fn new(paths: Arc<Paths>, runs: Arc<AcpRuns>) -> Self {
        Self { paths, runs }
    }

    /// An ACP agent's list: the values of its `model` option, each with the
    /// ladder of its `thought_level` option, as the agent itself publishes them
    /// ([`AcpRuns::acp_options`]).
    ///
    /// The agent's list is the whole list: there is no static guess to fall back
    /// on, so an agent that cannot be asked — or is not configured — yields no
    /// models, which hides the pickers, and a `fallback` source, which says why.
    ///
    /// With the models, how many were left out: see [`ModelList::hidden`].
    async fn agent_models(&self, agent: &str) -> (Vec<ModelInfo>, ModelListSource, u32) {
        use crate::protocol::{category, option_for};
        let Some(agent) = crate::agents::AcpAgentRegistry::of(&self.paths).get(agent) else {
            return (Vec::new(), ModelListSource::Fallback, 0);
        };
        let options = match self.runs.acp_options(&agent).await {
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
        // ([`AcpRuns::acp_id_shown`]).
        let runs = &self.runs;
        let shown = |text: &str| droppen_base::text::clip(&runs.redact_acp(&agent, text), crate::agents::MAX_AGENT_WORD);
        let usable = |id: &str| runs.acp_id_shown(id);
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
}

impl Models for AcpModels {
    // The agent id is checked before it is used: it is the key the agent's
    // configuration is looked up by, and later a path segment (the agent's
    // transcript directory, `AcpTranscripts::agent_dir`).
    fn list<'a>(&'a self, query: &'a ModelsQuery) -> BoxFuture<'a, Result<ModelList, String>> {
        Box::pin(async move {
            // Every agent has its own list: one served for another is a picker
            // offering models the session cannot run.
            let agent = query
                .acp_agent
                .as_deref()
                .filter(|id| crate::sessions::valid_agent_id(id))
                .ok_or_else(|| "acpAgent names the ACP agent to list models for".to_string())?;
            let (models, source, hidden) = self.agent_models(agent).await;
            Ok(ModelList {
                models,
                source,
                hidden: (hidden > 0).then_some(hidden).into(),
            })
        })
    }

    // An ACP agent has no logins; its list is asked for by agent.
    fn ids<'a>(&'a self, _account_id: Option<&'a str>) -> BoxFuture<'a, Vec<String>> {
        Box::pin(std::future::ready(Vec::new()))
    }

    // The agent checks an effort against the model it runs when it is set.
    fn accepted_efforts<'a>(&'a self, _model: Option<&'a str>) -> BoxFuture<'a, Option<Vec<String>>> {
        Box::pin(std::future::ready(None))
    }

    // An ACP agent has no permission modes to choose between.
    fn permission_is_known(&self, _permission: &str) -> bool {
        false
    }

    // An ACP agent has no accounts: its model is the agent's to name.
    fn account_model<'a>(&'a self, _account_id: &'a str) -> BoxFuture<'a, Option<String>> {
        Box::pin(std::future::ready(None))
    }

    fn model_rule(&self) -> ModelRule {
        ModelRule::new(ACP_MODEL_PATTERN, 200)
    }
}

