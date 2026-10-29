//! The models each local CLI can be asked for.
//!
//! Port of the former TypeScript server's `models.ts` plus the `/api/models`
//! route in its `app.ts`, and no longer only a port: the two CLIs are asked in
//! two different ways, because they answer in two different ways.
//!
//! * **codex** publishes a file, `models_cache.json`, and it is simply read.
//! * **claude** publishes nothing durable — the usage history this module used
//!   to mine for concrete ids is a file the current CLI no longer writes — but
//!   it will *answer*, over the control protocol in [`claude::probe`]. That answer is
//!   account-scoped and current, which no file on this machine is.
//!
//! Both degrade to a static list rather than failing, because an empty model
//! picker is worse than a slightly stale one, and both say which of the two the
//! caller is looking at.

use std::sync::Arc;

use gotcha::axum::extract::FromRef;
use gotcha::{api, GotchaRouter, Json, Schematic, State};
use serde::{Deserialize, Serialize};

use crate::accounts::store::AccountsStore;
use droppen_http::error::ApiError;
use droppen_http::extract::ApiQuery;
use droppen_http::openapi_ext::{ExtraResponses, Nullable, Optional};
use crate::sessions::Source;

pub mod acp;
pub mod claude;
pub mod codex;

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

/// What the model routes use of the server's state.
#[derive(Clone)]
pub struct ModelsState {
    /// The instance's paths: codex's model cache and the ACP agents' configs.
    pub paths: Arc<droppen_instance::paths::Paths>,
    /// The lists Claude Code published, per config root.
    pub models: Arc<claude::ClaudeModelCache>,
    /// The live runs, which ask an ACP agent for its options.
    pub injections: Arc<crate::inject::InjectionRegistry>,
    /// The stored accounts, whose roots or env blocks name a login's models.
    pub accounts: Arc<AccountsStore>,
}
droppen_http::substate!(ModelsState);

/// The models this machine's CLI publishes for `source`.
///
/// Claude's list is per *login*, so the caller names the account whose
/// credentials the run will use and gets that account's entitlements;
/// `accountId` absent is the user's own `~/.claude`. Each root's list is cached
/// for the process — asking costs a CLI startup — so only the first request
/// after a cold start or an expired reading waits for one.
#[api(id = "list_models", group = "models", summary = "List models")]
pub async fn list_models(State(state): State<ModelsState>, ApiQuery(query): ApiQuery<ModelsQuery>) -> Result<Json<ModelList>, ApiError> {
    let paths = state.paths.as_ref();
    let mut hidden = 0;
    let (models, source) = match query.source {
        Source::Codex => codex::codex_models(&crate::sessions::codex::CodexHome::of(&paths).models_cache()).await,
        Source::Claude => claude::claude_models(&state, query.account_id.as_deref()).await,
        Source::Acp => {
            // Every agent has its own list: one served for another is a picker
            // offering models the session cannot run.
            let agent = query
                .acp_agent
                .as_deref()
                .filter(|id| crate::sessions::acp_transcript::valid_agent_id(id))
                .ok_or_else(|| ApiError::bad_request("acpAgent names the ACP agent to list models for"))?;
            let (models, source, left_out) = acp::acp_models(&state, agent).await;
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
pub(crate) async fn model_ids(state: &ModelsState, source: Source, account_id: Option<&str>) -> Vec<String> {
    let (models, _) = match source {
        Source::Codex => codex::codex_models(&crate::sessions::codex::CodexHome::of(&state.paths).models_cache()).await,
        Source::Claude => claude::claude_models(state, account_id).await,
        // An ACP agent has no logins; its list is asked for by agent.
        Source::Acp => (Vec::new(), ModelListSource::Fallback),
    };
    models.into_iter().map(|model| model.id).collect()
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
pub async fn accepted_efforts(paths: &droppen_instance::paths::Paths, source: Source, model: Option<&str>) -> Option<Vec<String>> {
    match source {
        // The agent checks an effort against the model it runs when it is set.
        Source::Acp => None,
        Source::Claude => Some(claude::claude_efforts(model.unwrap_or_default())),
        Source::Codex => {
            let (models, list) = codex::codex_models(&crate::sessions::codex::CodexHome::of(&paths).models_cache()).await;
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

/// Whether `permission` is one `source` would accept.
pub fn permission_is_known(source: Source, permission: &str) -> bool {
    match source {
        Source::Claude => claude::CLAUDE_PERMISSIONS.contains(&permission),
        Source::Codex => crate::inject::codex_permission_flags(permission).is_ok(),
        // An ACP agent has no permission modes to choose between.
        Source::Acp => false,
    }
}

impl ModelsState {
    /// Register this subsystem's routes.
    pub fn routes<S>(router: GotchaRouter<S>) -> GotchaRouter<S>
    where
        S: Clone + Send + Sync + 'static,
        ModelsState: FromRef<S>,
    {
        router.get("/api/models", list_models)
    }
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
