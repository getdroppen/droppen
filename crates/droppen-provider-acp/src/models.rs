//! An ACP agent's model list: the values of the `model` option it publishes.

use std::sync::Arc;

use futures_util::future::BoxFuture;

use droppen_provider::models::{ModelInfo, ModelList, ModelListSource, ModelRule, Models, ModelsQuery, PermissionModeInfo};
use crate::AcpRuns;
use droppen_http::openapi_ext::{Nullable, Optional};
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
    /// ladder of its `thought_level` option, and its permission modes, as the
    /// agent itself publishes them ([`AcpRuns::acp_options`]).
    ///
    /// The agent's list is the whole list: there is no static guess to fall back
    /// on, so an agent that cannot be asked — or is not configured — yields no
    /// models, which hides the pickers, and a `fallback` source, which says why.
    ///
    /// With the models, how many were left out: see [`ModelList::hidden`].
    async fn agent_models(&self, agent: &str) -> ModelList {
        let fallback = || ModelList {
            models: Vec::new(),
            source: ModelListSource::Fallback,
            hidden: None.into(),
            permissions: None.into(),
            default_permission: None.into(),
        };
        let Some(agent) = crate::agents::AcpAgentRegistry::of(&self.paths).get(agent) else {
            return fallback();
        };
        let options = match self.runs.acp_options(&agent).await {
            Ok(options) => options,
            Err(error) => {
                tracing::info!(agent = %agent.id, %error, "cannot read the agent's models");
                return fallback();
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
        let (models, hidden) = build_models(&options.options, &options.available_models, shown, usable);
        let (permissions, default_permission) = build_permissions(&options.options, options.modes.as_ref(), shown, usable);
        ModelList {
            models,
            source: ModelListSource::Cache,
            hidden: (hidden > 0).then_some(hidden).into(),
            permissions: (!permissions.is_empty()).then_some(permissions).into(),
            default_permission: default_permission.into(),
        }
    }
}

/// Turn an agent's fresh session options (and optional `models.availableModels`
/// ids) into the composer's [`ModelInfo`] list and hidden count.
///
/// When an agent publishes both a session-level `thought_level` option and
/// `models.availableModels` containing `{modelId}-{effort}` variants (as
/// Antigravity does), each model gets its own exact effort ladder instead of
/// inheriting the default model's ladder: e.g. `claude-opus-5-5` keeps
/// `low..high` without `xhigh`, and `gpt-oss-120b-medium` gets `[]`.
///
/// When an agent publishes no `thought_level` option at all, every model gets
/// `Some(vec![])` (`[]`) rather than `None`, so the composer knows the model
/// has no effort ladder and hides the picker instead of inventing a fallback.
fn build_models(
    options: &[crate::protocol::ConfigOption],
    available_models: &[String],
    shown: impl Fn(&str) -> String,
    usable: impl Fn(&str) -> bool,
) -> (Vec<ModelInfo>, u32) {
    use std::collections::HashSet;
    use crate::protocol::{category, option_for};

    let effort = option_for(options, category::THOUGHT_LEVEL);
    let base_efforts: Vec<String> = effort
        .map(|effort| {
            effort
                .values()
                .iter()
                .map(|value| value.value.clone())
                .filter(|value| usable(value))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let base_default = effort.and_then(|effort| effort.current_value.clone()).filter(|value| usable(value));
    let offered = option_for(options, category::MODEL).map(|model| model.values()).unwrap_or_default();

    // Activate per-model derivation only when `available_models` actually uses
    // the `{model_id}-{effort}` variant convention for at least one listed
    // model and effort, so agents with plain `availableModels` lists are
    // unaffected.
    let available_set: HashSet<&str> = available_models.iter().map(String::as_str).collect();
    let has_per_model_variants = !base_efforts.is_empty()
        && !available_set.is_empty()
        && offered
            .iter()
            .any(|m| base_efforts.iter().any(|e| available_set.contains(format!("{}-{e}", m.value).as_str())));

    let models: Vec<ModelInfo> = offered
        .iter()
        .filter(|value| usable(&value.value))
        .map(|value| {
            let (efforts, default_effort) = if base_efforts.is_empty() {
                (Vec::new(), None)
            } else if has_per_model_variants {
                let matched: Vec<String> = base_efforts
                    .iter()
                    .filter(|e| available_set.contains(format!("{}-{e}", value.value).as_str()))
                    .cloned()
                    .collect();
                if !matched.is_empty() {
                    let def = base_default.as_ref().filter(|d| matched.contains(d)).cloned();
                    (matched, def)
                } else if available_set.contains(value.value.as_str()) {
                    (Vec::new(), None)
                } else {
                    (base_efforts.clone(), base_default.clone())
                }
            } else {
                (base_efforts.clone(), base_default.clone())
            };
            ModelInfo {
                id: value.value.clone(),
                label: Nullable((!value.name.is_empty() && value.name != value.value).then(|| shown(&value.name))),
                efforts: Nullable(Some(efforts)),
                default_effort: Nullable(default_effort),
            }
        })
        .collect();
    let hidden = u32::try_from(offered.len() - models.len()).unwrap_or(u32::MAX);
    (models, hidden)
}

/// Turn an agent's `mode` configuration option — or, when absent, its legacy
/// top-level `modes` block — into the composer's [`PermissionModeInfo`] list
/// and default mode id.
fn build_permissions(
    options: &[crate::protocol::ConfigOption],
    modes: Option<&crate::protocol::SessionModes>,
    shown: impl Fn(&str) -> String,
    usable: impl Fn(&str) -> bool,
) -> (Vec<PermissionModeInfo>, Option<String>) {
    use crate::protocol::{category, option_for};

    let (raw_items, raw_current): (Vec<(&str, &str)>, Option<&str>) = if let Some(opt) = option_for(options, category::MODE) {
        (
            opt.values().into_iter().map(|v| (v.value.as_str(), v.name.as_str())).collect(),
            opt.current_value.as_deref(),
        )
    } else if let Some(modes) = modes {
        (
            modes.available_modes.iter().map(|m| (m.id.as_str(), m.name.as_str())).collect(),
            (!modes.current_mode_id.is_empty()).then_some(modes.current_mode_id.as_str()),
        )
    } else {
        return (Vec::new(), None);
    };

    let mut seen = std::collections::HashSet::new();
    let permissions: Vec<PermissionModeInfo> = raw_items
        .into_iter()
        .filter(|(id, _)| usable(id) && droppen_provider::models::valid_permission(id) && seen.insert(*id))
        .map(|(id, name)| {
            let label = (!name.is_empty() && name != id)
                .then(|| shown(name))
                .filter(|label| !label.is_empty() && label != id);
            PermissionModeInfo {
                id: id.to_string(),
                label: Optional(label),
            }
        })
        .collect();
    let default_permission = raw_current
        .filter(|current| permissions.iter().any(|p| p.id == *current))
        .map(str::to_owned)
        .or_else(|| permissions.first().map(|p| p.id.clone()));
    (permissions, default_permission)
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
            Ok(self.agent_models(agent).await)
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

    // Scheduled tasks never run on ACP, and ACP modes are per-agent rather than
    // a fixed CLI table; per-session mode checks use the agent's own options.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{available_model_ids, config_options};
    use serde_json::json;

    #[test]
    fn per_model_efforts_are_derived_from_available_model_variants() {
        let answer = json!({
            "configOptions": [
                {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "gemini-4-neon",
                 "options": [
                     {"value": "gemini-4-neon", "name": "Gemini 4 Neon"},
                     {"value": "claude-opus-5-5", "name": "Claude Opus 5.5"},
                     {"value": "gpt-oss-120b-medium", "name": "GPT-OSS 120B (Medium)"},
                 ]},
                {"id": "thought_level", "name": "Thinking Level", "category": "thought_level", "type": "select", "currentValue": "high",
                 "options": [
                     {"value": "low", "name": "Low"},
                     {"value": "medium", "name": "Medium"},
                     {"value": "high", "name": "High"},
                     {"value": "xhigh", "name": "Extra"},
                 ]},
            ],
            "models": {
                "availableModels": [
                    {"modelId": "gemini-4-neon-low"},
                    {"modelId": "gemini-4-neon-medium"},
                    {"modelId": "gemini-4-neon-high"},
                    {"modelId": "gemini-4-neon-xhigh"},
                    {"modelId": "claude-opus-5-5-low"},
                    {"modelId": "claude-opus-5-5-medium"},
                    {"modelId": "claude-opus-5-5-high"},
                    {"modelId": "gpt-oss-120b-medium"},
                ]
            }
        });
        let options = config_options(&answer);
        let available = available_model_ids(&answer);
        let (models, hidden) = build_models(&options, &available, str::to_owned, |_| true);
        assert_eq!(hidden, 0);
        assert_eq!(models[0].efforts.0.as_deref(), Some(["low", "medium", "high", "xhigh"].map(str::to_owned).as_slice()));
        assert_eq!(models[0].default_effort.0.as_deref(), Some("high"));
        assert_eq!(models[1].efforts.0.as_deref(), Some(["low", "medium", "high"].map(str::to_owned).as_slice()));
        assert_eq!(models[1].default_effort.0.as_deref(), Some("high"));
        assert_eq!(models[2].efforts.0.as_deref(), Some([].as_slice()));
        assert_eq!(models[2].default_effort.0, None);
    }

    #[test]
    fn agent_without_thought_level_publishes_empty_effort_ladder() {
        let answer = json!({
            "configOptions": [
                {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "m1",
                 "options": [{"value": "m1", "name": "M1"}]}
            ]
        });
        let options = config_options(&answer);
        let (models, hidden) = build_models(&options, &[], str::to_owned, |_| true);
        assert_eq!(hidden, 0);
        assert_eq!(models[0].efforts.0.as_deref(), Some([].as_slice()), "empty vec, not None");
        assert_eq!(models[0].default_effort.0, None);
    }

    #[test]
    fn permission_modes_prefer_config_options_and_fall_back_to_legacy_modes() {
        use crate::protocol::session_modes;

        let answer = json!({
            "configOptions": [
                {"id": "mode", "name": "Mode", "category": "mode", "type": "select", "currentValue": "accept-edits",
                 "options": [
                     {"value": "default", "name": "Default"},
                     {"value": "accept-edits", "name": "Accept edits"},
                     {"value": "accept-edits", "name": "Duplicate"},
                     {"value": "auto", "name": "auto"},
                     {"value": "-bad", "name": "Flag-like"},
                     {"value": "secret-mode", "name": "Secret"},
                 ]}
            ],
            "modes": {
                "currentModeId": "plan",
                "availableModes": [{"id": "plan", "name": "Plan"}]
            }
        });
        let options = config_options(&answer);
        let modes = session_modes(&answer);
        let (perms, def) = build_permissions(&options, modes.as_ref(), str::to_owned, |id| id != "secret-mode");
        assert_eq!(
            perms,
            vec![
                PermissionModeInfo { id: "default".into(), label: Optional(Some("Default".into())) },
                PermissionModeInfo { id: "accept-edits".into(), label: Optional(Some("Accept edits".into())) },
                PermissionModeInfo { id: "auto".into(), label: Optional(None) },
            ]
        );
        assert_eq!(def.as_deref(), Some("accept-edits"));

        // Without configOptions mode, falls back to legacy `modes`; invalid current falls back to first.
        let legacy = json!({
            "modes": {
                "currentModeId": "-invalid",
                "availableModes": [
                    {"id": "default", "name": ""},
                    {"id": "plan", "name": "Plan Mode"},
                ]
            }
        });
        let legacy_modes = session_modes(&legacy);
        let (perms, def) = build_permissions(&[], legacy_modes.as_ref(), str::to_owned, |_| true);
        assert_eq!(
            perms,
            vec![
                PermissionModeInfo { id: "default".into(), label: Optional(None) },
                PermissionModeInfo { id: "plan".into(), label: Optional(Some("Plan Mode".into())) },
            ]
        );
        assert_eq!(def.as_deref(), Some("default"));

        // Neither published -> empty list and no default.
        let (perms, def) = build_permissions(&[], None, str::to_owned, |_| true);
        assert!(perms.is_empty());
        assert_eq!(def, None);
    }
}

