//! The models each local CLI can be asked for.
//!
//! Port of the former TypeScript server's `models.ts` plus the `/api/models`
//! route in its `app.ts`, and no longer only a port: the two CLIs are asked in
//! two different ways, because they answer in two different ways.
//!
//! * **codex** publishes a file, `models_cache.json`, and it is simply read.
//! * **claude** publishes nothing durable — the usage history this module used
//!   to mine for concrete ids is a file the current CLI no longer writes — but
//!   it will *answer*, over the control protocol in [`probe`]. That answer is
//!   account-scoped and current, which no file on this machine is.
//!
//! Both degrade to a static list rather than failing, because an empty model
//! picker is worse than a slightly stale one, and both say which of the two the
//! caller is looking at.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use gotcha::{api, Json, Schematic, State};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::accounts::store::{Account, AccountKind};
use crate::api::error::ApiError;
use crate::api::extract::ApiQuery;
use crate::api::{ApiRouter, AppState};
use crate::openapi_ext::{ExtraResponses, Nullable, Optional};
use crate::sessions::Source;

pub mod probe;

pub use probe::ClaudeModelCache;

/// One model the local CLI can be asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    /// The id passed to the CLI.
    pub id: String,
    /// Human-facing name, when the CLI publishes one.
    pub label: Nullable<String>,
    /// Reasoning efforts the model accepts.
    pub efforts: Nullable<Vec<String>>,
    /// The effort used when the caller picks none.
    pub default_effort: Nullable<String>,
}

/// Where a model list came from.
///
/// On the wire so the picker can say so. A list the CLI itself published — read
/// out of its cache, or asked of it directly — describes the models this
/// account may actually run; the fallback is a static guess that exists only so
/// the picker is never empty, and one shown as if it were the other is how a
/// user ends up picking a model their CLI does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "snake_case")]
pub enum ModelListSource {
    /// The CLI's own answer: its cache file, or its control protocol.
    Cache,
    /// The static list, because that cache was missing or unusable.
    Fallback,
    /// The account's own environment block said which model it runs.
    ///
    /// Not a guess, and that is the whole reason it is not [`Self::Fallback`]:
    /// an env account pointed at somebody's own endpoint carries
    /// `ANTHROPIC_MODEL`, and that variable *is* the model every session under
    /// that account will run. Badging it as a fallback would tell the user the
    /// one concrete id on the list is the least trustworthy thing on it.
    AccountEnv,
}

/// The `GET /api/models` response.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct ModelList {
    /// Models currently known to the local CLI, freshest first.
    pub models: Vec<ModelInfo>,
    /// Whether `models` came from the CLI's cache or from the static fallback.
    pub source: ModelListSource,
    /// How many of an ACP agent's models are left out because their ids look
    /// like a credential, so a picker can say why its list is short or empty;
    /// absent when none are.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub hidden: Optional<u32>,
}

/// Query for `GET /api/models`.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct ModelsQuery {
    /// Which CLI to list models for.
    pub source: Source,
    /// Which stored Claude account the list is for; absent or empty is the
    /// user's own login. Ignored by the codex lane, which has one config root.
    pub account_id: Option<String>,
    /// Which ACP agent the list is for, by id: required when `source` is
    /// `acp`, ignored otherwise.
    pub acp_agent: Option<String>,
}

/// Everything `claude --effort` accepts; the CLI presents max as "ultracode".
const CLAUDE_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
/// The ladder before `xhigh` was added between `high` and `max`.
const CLAUDE_EFFORTS_NO_XHIGH: [&str; 4] = ["low", "medium", "high", "max"];
/// The ladder of the first generation to take an effort at all.
const CLAUDE_EFFORTS_EARLY: [&str; 3] = ["low", "medium", "high"];
/// CLI aliases always resolve to the newest model of each tier.
const CLAUDE_ALIASES: [&str; 4] = ["fable", "opus", "sonnet", "haiku"];
/// What the CLI runs at when no effort is named.
const CLAUDE_DEFAULT_EFFORT: &str = "medium";

/// The variable a claude env block names its one model in.
const CLAUDE_MODEL_VAR: &str = "ANTHROPIC_MODEL";
/// The variable a claude env block names its default effort in.
const CLAUDE_EFFORT_VAR: &str = "CLAUDE_CODE_EFFORT_LEVEL";
/// Which alias each `ANTHROPIC_DEFAULT_*_MODEL` override renames.
///
/// `fable` has no override of its own: the CLI has no
/// `ANTHROPIC_DEFAULT_FABLE_MODEL`, so an account that remaps the other three
/// still sends `--model fable` through untouched.
const CLAUDE_ALIAS_VARS: [(&str, &str); 3] = [
    ("opus", "ANTHROPIC_DEFAULT_OPUS_MODEL"),
    ("sonnet", "ANTHROPIC_DEFAULT_SONNET_MODEL"),
    ("haiku", "ANTHROPIC_DEFAULT_HAIKU_MODEL"),
];

/// Every variable of a claude block this module reads.
///
/// Named one by one because that is what [`crate::accounts::store::AccountsStore::env_values`]
/// takes: the block also holds the credential, and the list of keys is the only
/// thing keeping it there.
const CLAUDE_MODEL_VARS: [&str; 5] = [
    CLAUDE_MODEL_VAR,
    CLAUDE_EFFORT_VAR,
    CLAUDE_ALIAS_VARS[0].1,
    CLAUDE_ALIAS_VARS[1].1,
    CLAUDE_ALIAS_VARS[2].1,
];

/// Which rungs of the effort ladder a Claude model actually accepts.
///
/// Not cosmetic: an effort a model does not take is refused by the API, so a
/// picker offering all five for every id is a way to build a message that
/// cannot be sent. Three shapes matter — the models that take none at all, the
/// generations that predate a rung, and everything else.
///
/// The aliases resolve here too, through the same substring test, because an
/// alias is the newest model of its tier: `haiku` takes no effort for the same
/// reason `claude-haiku-4-5` does not.
///
/// An id this does not recognise gets the full ladder. The set is open — the
/// list is built from ids a CLI has already run — and the failure modes are not
/// symmetric: too many rungs shows one option the API will refuse, while too
/// few silently hides the rung a new model was released for.
pub(crate) fn claude_efforts(model: &str) -> Vec<String> {
    let model = model.to_ascii_lowercase();
    let ladder: &[&str] = if model.contains("haiku") || model.contains("sonnet-4-5") {
        &[]
    } else if model.contains("opus-4-5") {
        &CLAUDE_EFFORTS_EARLY
    } else if model.contains("opus-4-6") || model.contains("sonnet-4-6") {
        &CLAUDE_EFFORTS_NO_XHIGH
    } else {
        &CLAUDE_EFFORTS
    };
    ladder.iter().map(|effort| (*effort).to_string()).collect()
}

/// One Claude model as the picker sees it: its ladder, and the rung it runs at
/// when the caller names none.
///
/// A model that takes no effort has no default either — reporting one would
/// describe a flag that cannot be passed.
fn claude_model(id: String, label: Option<String>) -> ModelInfo {
    let efforts = claude_efforts(&id);
    let default_effort = (!efforts.is_empty()).then(|| CLAUDE_DEFAULT_EFFORT.to_string());
    ModelInfo {
        id,
        label: Nullable(label),
        efforts: Nullable(Some(efforts)),
        default_effort: Nullable(default_effort),
    }
}

/// What the Codex list falls back to when its cache is missing or unusable.
fn codex_fallback() -> Vec<ModelInfo> {
    ["gpt-5.6-sol", "gpt-5.5", "gpt-5.4"]
        .into_iter()
        .map(|id| ModelInfo {
            id: id.to_string(),
            label: Nullable(None),
            efforts: Nullable(None),
            default_effort: Nullable(None),
        })
        .collect()
}

/// Parse a JSON file, or `None` when it is missing or malformed.
async fn read_json(path: &Path) -> Option<Value> {
    let text = tokio::fs::read_to_string(path).await.ok()?;
    serde_json::from_str(&text).ok()
}

/// The listable entries of Codex's `models_cache.json`.
///
/// `visibility` gates entries the backend offers but does not want in a picker,
/// so anything other than `"list"` is dropped.
fn parse_codex_cache(raw: &Value) -> Vec<ModelInfo> {
    let Some(models) = raw.get("models").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in models {
        let Some(entry) = entry.as_object() else { continue };
        let Some(slug) = entry.get("slug").and_then(Value::as_str) else {
            continue;
        };
        if entry.get("visibility").and_then(Value::as_str) != Some("list") {
            continue;
        }
        // An absent key means "unknown", which is null; a present but junk-filled
        // array just yields the levels that did parse.
        let efforts = entry.get("supported_reasoning_levels").and_then(Value::as_array).map(|levels| {
            levels
                .iter()
                .filter_map(|level| level.as_object()?.get("effort")?.as_str().map(str::to_owned))
                .collect()
        });
        out.push(ModelInfo {
            id: slug.to_string(),
            label: Nullable(entry.get("display_name").and_then(Value::as_str).map(str::to_owned)),
            efforts: Nullable(efforts),
            default_effort: Nullable(entry.get("default_reasoning_level").and_then(Value::as_str).map(str::to_owned)),
        });
    }
    out
}

/// Codex's own cache of what its backend offers, or the static fallback.
pub async fn codex_models(cache: &Path) -> (Vec<ModelInfo>, ModelListSource) {
    let models = read_json(cache).await.map(|raw| parse_codex_cache(&raw)).unwrap_or_default();
    if models.is_empty() {
        (codex_fallback(), ModelListSource::Fallback)
    } else {
        (models, ModelListSource::Cache)
    }
}

/// What the Claude list falls back to when the CLI cannot be asked.
///
/// The four tier aliases, labelled as the guess they are. Every one of them is
/// real — an alias always resolves to the newest model of its tier — but which
/// model that is today is exactly what this list cannot say, and the `(latest)`
/// is there so nobody reads it as a claim about a concrete id.
fn claude_fallback() -> Vec<ModelInfo> {
    CLAUDE_ALIASES
        .iter()
        .map(|id| claude_model((*id).to_string(), Some(format!("{id} (latest)"))))
        .collect()
}

/// What an env account's own block says it runs, when it says anything.
///
/// An account whose block sets `ANTHROPIC_BASE_URL` is not talking to
/// Anthropic, and nothing this machine can ask will list that endpoint's
/// models: there is no cache to read and no CLI login to probe. The block
/// itself is the only authority there is, and it is a good one — `ANTHROPIC_MODEL`
/// is not a hint about what is available, it is the model every session under
/// this account will run.
///
/// So the named model goes first, and the four aliases follow it. The aliases
/// stay because they still work: `--model sonnet` under a block that remaps
/// sonnet reaches whatever the block remapped it to, which is exactly what the
/// label says. One remapped alias is a different way of asking for the same
/// endpoint, not a second model.
///
/// The whole ladder, whatever the id looks like. [`claude_efforts`] reads
/// Anthropic's own naming — a `-4-5` predates a rung, a haiku takes none — and
/// that reading says nothing about a model somebody else named: a gateway is
/// free to call its model `glm-haiku-fast`, and silently hiding every effort
/// from it would be the module's own stated worse mistake. The account's
/// `CLAUDE_CODE_EFFORT_LEVEL` is the default when it names a rung that exists,
/// because that is the rung this account's sessions run at.
fn claude_env_models(vars: &BTreeMap<String, String>) -> Option<Vec<ModelInfo>> {
    let named = vars.get(CLAUDE_MODEL_VAR).map(|model| model.trim()).filter(|model| !model.is_empty())?;
    let effort = vars
        .get(CLAUDE_EFFORT_VAR)
        .map(|effort| effort.trim().to_ascii_lowercase())
        .filter(|effort| CLAUDE_EFFORTS.contains(&effort.as_str()))
        .unwrap_or_else(|| CLAUDE_DEFAULT_EFFORT.to_string());
    let mut models = vec![ModelInfo {
        id: named.to_string(),
        label: Nullable(Some(named.to_string())),
        efforts: Nullable(Some(CLAUDE_EFFORTS.iter().map(|effort| (*effort).to_string()).collect())),
        default_effort: Nullable(Some(effort)),
    }];
    models.extend(claude_fallback().into_iter().map(|mut alias| {
        let mapped = CLAUDE_ALIAS_VARS
            .iter()
            .find(|(name, _)| *name == alias.id)
            .and_then(|(_, var)| vars.get(*var))
            .map(|model| model.trim())
            .filter(|model| !model.is_empty());
        if let Some(mapped) = mapped {
            // The arrow rather than `(latest)`: this alias is not a guess at
            // Anthropic's newest model of a tier, it is a name the block has
            // already answered.
            alias.label = Nullable(Some(format!("{} → {mapped}", alias.id)));
        }
        alias
    }));
    Some(models)
}

/// The models this machine's CLI publishes for `source`.
///
/// Claude's list is per *login*, so the caller names the account whose
/// credentials the run will use and gets that account's entitlements;
/// `accountId` absent is the user's own `~/.claude`. Each root's list is cached
/// for the process — asking costs a CLI startup — so only the first request
/// after a cold start or an expired reading waits for one.
#[api(id = "list_models", group = "models", summary = "List models")]
pub async fn list_models(State(state): State<AppState>, ApiQuery(query): ApiQuery<ModelsQuery>) -> Result<Json<ModelList>, ApiError> {
    let paths = state.paths.as_ref();
    let mut hidden = 0;
    let (models, source) = match query.source {
        Source::Codex => codex_models(&paths.codex_models_cache()).await,
        Source::Claude => claude_models(&state, query.account_id.as_deref()).await,
        Source::Acp => {
            // Every agent has its own list: one served for another is a picker
            // offering models the session cannot run.
            let agent = query
                .acp_agent
                .as_deref()
                .filter(|id| crate::acp_agent::valid_id(id))
                .ok_or_else(|| ApiError::bad_request("acpAgent names the ACP agent to list models for"))?;
            let (models, source, left_out) = acp_models(&state, agent).await;
            hidden = left_out;
            (models, source)
        }
    };
    Ok(Json(ModelList {
        models,
        source,
        hidden: (hidden > 0).then_some(hidden).into(),
    }))
}

/// The ids `GET /api/models` lists for one login of `source`, in its order — the
/// same list a composer resolves its model against, so a model chosen from it
/// on the server is one the composer shows (`GET /api/accounts/start`).
pub(crate) async fn model_ids(state: &AppState, source: Source, account_id: Option<&str>) -> Vec<String> {
    let (models, _) = match source {
        Source::Codex => codex_models(&state.paths.codex_models_cache()).await,
        Source::Claude => claude_models(state, account_id).await,
        // An ACP agent has no logins; its list is asked for by agent.
        Source::Acp => (Vec::new(), ModelListSource::Fallback),
    };
    models.into_iter().map(|model| model.id).collect()
}

/// An ACP agent's list: the values of its `model` option, each with the
/// ladder of its `thought_level` option, as the agent itself publishes them
/// ([`crate::inject::InjectionRegistry::acp_options`]).
///
/// The agent's list is the whole list: there is no static guess to fall back
/// on, so an agent that cannot be asked — or is not configured — yields no
/// models, which hides the pickers, and a `fallback` source, which says why.
///
/// With the models, how many were left out: see [`ModelList::hidden`].
async fn acp_models(state: &AppState, agent: &str) -> (Vec<ModelInfo>, ModelListSource, u32) {
    use crate::acp::{category, option_for};
    let Some(agent) = state.paths.acp_agent(agent) else {
        return (Vec::new(), ModelListSource::Fallback, 0);
    };
    let options = match state.drive.injections.acp_options(&agent).await {
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
    let injections = &state.drive.injections;
    let shown = |text: &str| crate::accounts::limits::clip(&injections.redact_acp(&agent, text), crate::acp_agent::MAX_AGENT_WORD);
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

/// The Claude list for one login: the local one, or a stored account's.
///
/// Three lanes, because the three kinds of login can be asked three different
/// things. An OAuth account has a config root of its own and is *asked*. An env
/// account has no root and usually no Anthropic behind it either, but it does
/// have a block, and a block that names a model has answered the question — see
/// [`claude_env_models`]. Everything else falls back.
///
/// An account that cannot be asked is not an error — an unknown id, a deleted
/// account, one with no `/usage` login of its own — because the caller is a
/// picker and a picker must always have something to show. It gets the same
/// alias list and the same [`ModelListSource::Fallback`] as any other probe
/// failure, which the composer already marks as a guess.
async fn claude_models(state: &AppState, account_id: Option<&str>) -> (Vec<ModelInfo>, ModelListSource) {
    let Some(id) = account_id.map(str::trim).filter(|id| !id.is_empty()) else {
        return state.drive.models.models(None).await;
    };
    let accounts = &state.drive.accounts;
    let Some(account) = accounts.get(id).await else {
        return (claude_fallback(), ModelListSource::Fallback);
    };
    if account.kind == AccountKind::Env {
        // A block that names no model says nothing, and gets the guess it is.
        return match claude_env_models(&accounts.env_values(id, &CLAUDE_MODEL_VARS).await) {
            Some(models) => (models, ModelListSource::AccountEnv),
            None => (claude_fallback(), ModelListSource::Fallback),
        };
    }
    let Some(dir) = probe_dir(accounts, &account) else {
        return (claude_fallback(), ModelListSource::Fallback);
    };
    state.drive.models.models(Some(&dir)).await
}

/// The directory an account's list can be asked of, when there is one.
///
/// The quota panel's rule, not a new one: only an OAuth account that has opted
/// into probing has a logged-in config root of its own — [`crate::quota`] picks
/// the directories it probes on exactly this pair of conditions, and a model
/// list for a root the quota panel will not read would be a second rule.
/// An env account authenticates per request and has no root to ask; an account
/// with probing off has a directory with no login in it, and asking would
/// answer for nobody. Either way the honest answer is the fallback list.
fn probe_dir(accounts: &crate::accounts::store::AccountsStore, account: &Account) -> Option<PathBuf> {
    if !account.usage_enabled || account.kind != AccountKind::OauthToken {
        return None;
    }
    // Refuses anything that is not a uuid, so a query string can never name a
    // directory of its own.
    accounts.usage_config_dir(&account.id).ok()
}

/// The efforts a CLI would actually accept, or `None` when nothing on this
/// machine can say.
///
/// Only [`crate::schedule`] asks. The composer does not need it — a person
/// picking from a list cannot pick something that is not in it — but a task that
/// fires at three in the morning needs its flags checked at the moment somebody
/// is *there to fix them*, and this is the only place that knows what each CLI
/// takes.
///
/// The two sources answer differently, and the difference is theirs rather than
/// ours:
///
/// * **claude** publishes a ladder per model, in this file, derived from the id.
///   A named model is checked against its own rungs, so an `--effort max` on a
///   haiku is refused here rather than by the API at three in the morning; an
///   unnamed model gets the full ladder.
/// * **codex** publishes nothing static. Its rungs come out of the cache the CLI
///   itself writes, which is the *only* authority — so when that cache is
///   missing, this answers `None` and the caller accepts whatever it was given.
///
/// That asymmetry is deliberate and is the rule this module already states about
/// its lists: too many rungs offers one option the API will refuse, while too
/// few silently hides the rung a new model was released for. Refusing to
/// schedule a valid effort because this machine has never run codex would be the
/// second kind of wrong.
pub async fn accepted_efforts(paths: &crate::sessions::Paths, source: Source, model: Option<&str>) -> Option<Vec<String>> {
    match source {
        // The agent checks an effort against the model it runs when it is set.
        Source::Acp => None,
        Source::Claude => Some(claude_efforts(model.unwrap_or_default())),
        Source::Codex => {
            let (models, list) = codex_models(&paths.codex_models_cache()).await;
            if list == ModelListSource::Fallback {
                return None;
            }
            // A named model answers for itself; without one, anything any listed
            // model accepts is accepted, because the model may be chosen later
            // (or left to the CLI's own default).
            let named = model.and_then(|wanted| models.iter().find(|entry| entry.id == wanted));
            let efforts: Vec<String> = match named {
                Some(entry) => entry.efforts.as_ref()?.clone(),
                None => {
                    let mut all: Vec<String> = models.iter().filter_map(|entry| entry.efforts.as_ref()).flatten().cloned().collect();
                    all.sort();
                    all.dedup();
                    all
                }
            };
            (!efforts.is_empty()).then_some(efforts)
        }
    }
}

/// The `--permission-mode` / sandbox values remote-hand will spawn with.
///
/// Codex's are the CLI's own, asked of [`crate::inject::codex_permission_flags`]
/// so the two cannot drift: that function is what turns the value into `argv`,
/// and it already refuses what it does not know.
///
/// Claude's are a list, because there is no equivalent function to ask — the
/// server passes `--permission-mode` through and the CLI is the checker. The
/// cost of naming them here is that a mode a newer claude adds cannot be
/// scheduled until this list catches up; the benefit is that a typo in an
/// unattended task is caught by the person who typed it rather than by a CLI at
/// three in the morning. The same five the composer offers.
const CLAUDE_PERMISSIONS: [&str; 5] = ["acceptEdits", "auto", "plan", "dontAsk", "bypassPermissions"];

/// Whether `permission` is one `source` would accept.
pub fn permission_is_known(source: Source, permission: &str) -> bool {
    match source {
        Source::Claude => CLAUDE_PERMISSIONS.contains(&permission),
        Source::Codex => crate::inject::codex_permission_flags(permission).is_ok(),
        // An ACP agent has no permission modes to choose between.
        Source::Acp => false,
    }
}

/// Register this subsystem's routes.
pub fn routes(router: ApiRouter) -> ApiRouter {
    router.get("/api/models", list_models)
}

/// Everything gotcha cannot express about those routes.
pub fn describe(reg: &mut ExtraResponses) {
    reg.describe(
        "GET",
        "/api/models",
        200,
        "Models the named login's CLI publishes, in the order it publishes them",
    )
    .error("GET", "/api/models", 400, "An ACP list that names no usable agent");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_codex_cache_keeps_only_listable_entries() {
        let models = parse_codex_cache(&json!({
            "models": [
                {
                    "slug": "gpt-5.6-sol",
                    "display_name": "GPT-5.6 Sol",
                    "visibility": "list",
                    "supported_reasoning_levels": [{ "effort": "low" }, { "effort": "high" }, { "note": "no effort here" }],
                    "default_reasoning_level": "high"
                },
                { "slug": "gpt-internal", "visibility": "hidden" },
                { "slug": "gpt-5.5", "visibility": "list" },
                { "visibility": "list" },
                "not an object"
            ]
        }));

        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gpt-5.6-sol");
        assert_eq!(models[0].label, Nullable(Some("GPT-5.6 Sol".to_string())));
        assert_eq!(models[0].efforts, Nullable(Some(vec!["low".to_string(), "high".to_string()])));
        assert_eq!(models[0].default_effort, Nullable(Some("high".to_string())));

        assert_eq!(models[1].id, "gpt-5.5");
        assert_eq!(models[1].label, Nullable(None), "an absent display_name is null, not missing");
        assert_eq!(models[1].efforts, Nullable(None), "an absent levels key is null, not an empty list");
    }

    #[test]
    fn a_cache_with_nothing_listable_parses_to_nothing() {
        assert!(parse_codex_cache(&json!({ "models": [] })).is_empty());
        assert!(parse_codex_cache(&json!({})).is_empty(), "a cache with no models key is empty");
    }

    fn all_five() -> Nullable<Vec<String>> {
        Nullable(Some(CLAUDE_EFFORTS.iter().map(|e| (*e).to_string()).collect()))
    }

    #[tokio::test]
    async fn a_missing_codex_cache_falls_back_to_the_static_list() {
        let (models, source) = codex_models(Path::new("/nonexistent/models_cache.json")).await;
        assert_eq!(models, codex_fallback());
        assert_eq!(models[0].id, "gpt-5.6-sol");
        assert_eq!(source, ModelListSource::Fallback, "and says so");
    }

    /// The static guess, for a machine whose CLI cannot be asked.
    #[test]
    fn the_claude_fallback_is_the_four_tier_aliases() {
        let models = claude_fallback();
        let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(ids, vec!["fable", "opus", "sonnet", "haiku"]);
        assert_eq!(models[0].label, Nullable(Some("fable (latest)".to_string())));
        assert_eq!(models[0].default_effort, Nullable(Some("medium".to_string())));
        assert_eq!(models[0].efforts, all_five());
    }

    /// The alias list is what a picker offers before anything can be asked, so
    /// the tier that takes no effort has to say so there too.
    #[test]
    fn the_haiku_alias_offers_no_effort() {
        let models = claude_fallback();
        let haiku = models.iter().find(|model| model.id == "haiku").expect("the haiku alias");
        assert_eq!(haiku.efforts, Nullable(Some(Vec::new())), "an empty ladder, not an unknown one");
        assert_eq!(haiku.default_effort, Nullable(None), "and so no rung to default to");
    }

    #[test]
    fn each_generation_gets_the_ladder_it_actually_accepts() {
        for id in [
            "claude-fable-5",
            "claude-mythos-5",
            "claude-opus-5",
            "claude-opus-4-8",
            "claude-sonnet-5",
            "opus",
        ] {
            assert_eq!(claude_efforts(id), CLAUDE_EFFORTS, "{id} takes the whole ladder");
        }
        for id in ["claude-opus-4-6", "claude-sonnet-4-6"] {
            assert_eq!(claude_efforts(id), CLAUDE_EFFORTS_NO_XHIGH, "{id} predates xhigh");
        }
        assert_eq!(claude_efforts("claude-opus-4-5"), CLAUDE_EFFORTS_EARLY);
        for id in ["claude-haiku-4-5", "haiku", "claude-sonnet-4-5"] {
            assert!(claude_efforts(id).is_empty(), "{id} refuses an effort outright");
        }
        assert_eq!(
            claude_efforts("claude-something-7"),
            CLAUDE_EFFORTS,
            "an unknown id gets the whole ladder: the set is open, and a missing rung hides a real one"
        );
    }

    /// A block, as the store hands one over: the named keys and nothing else.
    fn block(vars: &[(&str, &str)]) -> BTreeMap<String, String> {
        vars.iter().map(|(key, value)| ((*key).to_string(), (*value).to_string())).collect()
    }

    /// The account this was built for: a self-hosted model behind an
    /// Anthropic-compatible endpoint, which no cache on this machine and no
    /// probe of any login will ever list.
    #[test]
    fn a_block_that_names_a_model_puts_it_at_the_top_of_the_list() {
        let models = claude_env_models(&block(&[
            ("ANTHROPIC_MODEL", "GLM-5.3-CYBERSECURITY-FP8"),
            ("CLAUDE_CODE_EFFORT_LEVEL", "max"),
        ]))
        .expect("a block that names a model");

        assert_eq!(models[0].id, "GLM-5.3-CYBERSECURITY-FP8");
        assert_eq!(models[0].label, Nullable(Some("GLM-5.3-CYBERSECURITY-FP8".to_string())));
        assert_eq!(models[0].efforts, all_five(), "somebody else's model keeps the whole ladder");
        assert_eq!(models[0].default_effort, Nullable(Some("max".to_string())), "the rung the block runs at");

        // The aliases still work under a block, so they are still offered.
        let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();
        assert_eq!(ids, vec!["GLM-5.3-CYBERSECURITY-FP8", "fable", "opus", "sonnet", "haiku"]);
        assert_eq!(
            models[1].label,
            Nullable(Some("fable (latest)".to_string())),
            "an un-remapped alias is unchanged"
        );
    }

    /// `CLAUDE_CODE_EFFORT_LEVEL` is free-form text in a box somebody typed.
    #[test]
    fn an_effort_level_the_cli_would_refuse_is_not_offered_as_the_default() {
        for typed in ["ultracode", "", "  ", "MAXIMUM"] {
            let models = claude_env_models(&block(&[("ANTHROPIC_MODEL", "glm-5"), ("CLAUDE_CODE_EFFORT_LEVEL", typed)])).expect("a model");
            assert_eq!(
                models[0].default_effort,
                Nullable(Some("medium".to_string())),
                "{typed:?} is not a rung, so the CLI's own default stands"
            );
        }
        // Case and padding are the user's typing, not a different rung.
        let models = claude_env_models(&block(&[("ANTHROPIC_MODEL", "glm-5"), ("CLAUDE_CODE_EFFORT_LEVEL", " XHigh ")])).expect("a model");
        assert_eq!(models[0].default_effort, Nullable(Some("xhigh".to_string())));
    }

    /// A block with no model in it has said nothing, and the caller falls back.
    #[test]
    fn a_block_that_names_no_model_answers_nothing() {
        assert!(claude_env_models(&block(&[("CLAUDE_CODE_EFFORT_LEVEL", "max")])).is_none());
        assert!(
            claude_env_models(&block(&[("ANTHROPIC_MODEL", "   ")])).is_none(),
            "a blank is not a model"
        );
        assert!(
            claude_env_models(&BTreeMap::new()).is_none(),
            "an OAuth account, which has no block at all"
        );
    }

    /// An alias the block remaps is still the alias to send — it is the *name*
    /// that is no longer a guess about Anthropic's newest model of a tier.
    #[test]
    fn a_remapped_alias_is_labelled_with_what_it_reaches() {
        let models = claude_env_models(&block(&[
            ("ANTHROPIC_MODEL", "glm-5"),
            ("ANTHROPIC_DEFAULT_OPUS_MODEL", "glm-5-max"),
            ("ANTHROPIC_DEFAULT_SONNET_MODEL", "glm-5"),
            ("ANTHROPIC_DEFAULT_HAIKU_MODEL", "glm-5-air"),
        ]))
        .expect("a model");
        let label = |id: &str| {
            models
                .iter()
                .find(|model| model.id == id)
                .and_then(|model| model.label.0.clone())
                .unwrap_or_default()
        };
        assert_eq!(label("opus"), "opus \u{2192} glm-5-max");
        assert_eq!(label("sonnet"), "sonnet \u{2192} glm-5");
        assert_eq!(label("haiku"), "haiku \u{2192} glm-5-air");
        assert_eq!(label("fable"), "fable (latest)", "the CLI has no ANTHROPIC_DEFAULT_FABLE_MODEL to remap");
        // Remapping a tier does not change what that alias accepts: haiku is
        // still the tier the CLI refuses an --effort for.
        let haiku = models.iter().find(|model| model.id == "haiku").expect("haiku");
        assert_eq!(haiku.efforts, Nullable(Some(Vec::new())));
    }

    /// The schedule validator's side of the same open set: a model id nothing
    /// on this machine has heard of takes every rung, because hiding one would
    /// refuse a task the CLI would have run.
    #[tokio::test]
    async fn a_model_this_build_has_never_heard_of_accepts_the_whole_ladder() {
        let paths = crate::sessions::Paths::new(PathBuf::from("/nonexistent"));
        for model in ["GLM-5.3-CYBERSECURITY-FP8", "qwen3-max", "glm-5-air"] {
            assert_eq!(
                accepted_efforts(&paths, Source::Claude, Some(model)).await,
                Some(CLAUDE_EFFORTS.iter().map(|effort| (*effort).to_string()).collect::<Vec<_>>()),
                "{model} is not a claude id and must not be given a claude id's ladder"
            );
        }
    }
}
