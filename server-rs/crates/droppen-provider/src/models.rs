//! The models each local CLI can be asked for.
//!
//! Every CLI is asked in its own way, because each answers in its own way: Codex
//! publishes a cache file (`codex::models`), Claude Code answers over its control
//! protocol (`claude_code::models`), and an ACP agent publishes a `model` option
//! (`acp::models`). So each provider answers [`Models`] for its own CLI, and the
//! route asks the provider of the source it is asked about.
//!
//! Claude's and Codex's lists degrade to a static guess rather than failing,
//! because an empty model picker is worse than a slightly stale one, and say
//! which of the two the caller is looking at.

use std::sync::Arc;

use futures_util::future::BoxFuture;
use gotcha::axum::extract::FromRef;
use gotcha::{api, GotchaRouter, Json, Schematic, State};
use serde::{Deserialize, Serialize};

use droppen_http::error::ApiError;
use droppen_http::extract::ApiQuery;
use droppen_http::openapi_ext::{ExtraResponses, Nullable, Optional};
use droppen_sessions::Source;


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

/// What a model id spelled as a name may look like ([`ModelRule::NAMED`]).
///
/// A word-ish name, optionally followed by one bracketed variant tag — the
/// shape of `claude-opus-4-8[1m]`, which is how the CLI is asked for the
/// long-context variant of a model that has one. The tag is spelled out rather
/// than folded into the character class on purpose: the value is handed to a
/// CLI as a flag value, and a class permissive enough to hold a bracket
/// anywhere would also hold an unbalanced one. Alphanumerics only inside it, so
/// nothing a shell would read as special can ever get in even if a caller one
/// day builds a command line out of this instead of an argv.
///
/// The name itself is `[[:word:]]`, `.`, `:` and `-`. That POSIX class is the
/// ASCII `\w` the original pattern was reaching for by writing `(?-u:\w)`
/// *inside* a character class — where it is not a flag group at all: the `?-u`
/// in the middle of it reads as a **range**, `?`(0x3F) through `u`(0x75), and
/// that range quietly admitted `\`, `` ` ``, `^`, `@`, `(`, `)` and both
/// brackets. So brackets were already getting through, by accident and
/// unbalanced; this says what is meant instead.
pub const MODEL_PATTERN: &str = r"^[A-Za-z0-9][[:word:].:-]*(?:\[[A-Za-z0-9]+\])?$";

/// How a CLI's model ids are spelled, which every model id is held to before
/// it reaches the CLI: on the send routes, in the composer's choices and in a
/// scheduled task. The value becomes a CLI argument, so an unbounded `model`
/// could read as a flag rather than as a model name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelRule {
    /// The pattern a model id matches.
    pattern: &'static str,
    /// The longest a model id may be, in characters.
    max_len: usize,
}

impl ModelRule {
    /// A model id spelled as a name ([`MODEL_PATTERN`]), at most 100
    /// characters: the rule of a CLI that does not spell its ids its own way.
    pub const NAMED: ModelRule = ModelRule::new(MODEL_PATTERN, 100);

    /// The rule of a source no provider runs, which admits no model id.
    pub const NONE: ModelRule = ModelRule::new("", 0);

    /// Ids matching `pattern`, at most `max_len` characters long.
    pub const fn new(pattern: &'static str, max_len: usize) -> Self {
        Self { pattern, max_len }
    }

    /// Whether `model` is spelled the way this rule allows.
    pub fn admits(&self, model: &str) -> bool {
        self.max_len > 0 && model.chars().count() <= self.max_len && regex::Regex::new(self.pattern).expect("valid model pattern").is_match(model)
    }
}

/// What one CLI says about its models: the list a picker offers, the ids a
/// start is chosen against, and which efforts and permission modes it takes.
/// Each provider answers for its own CLI ([`crate::Provider::models`]).
pub trait Models: Send + Sync {
    /// The list `GET /api/models` serves for `query`, or why the query is
    /// refused, in the words of the route's `400`.
    fn list<'a>(&'a self, query: &'a ModelsQuery) -> BoxFuture<'a, Result<ModelList, String>>;

    /// The ids [`Models::list`] gives for one login, in its order: the same
    /// list a composer resolves its model against, so a model chosen from it on
    /// the server is one the composer shows (`GET /api/accounts/start`).
    /// `account_id` absent is the user's own login.
    fn ids<'a>(&'a self, account_id: Option<&'a str>) -> BoxFuture<'a, Vec<String>>;

    /// The efforts a CLI would actually accept, or `None` when nothing on this
    /// machine can say.
    ///
    /// Only `schedule` asks. The composer does not need it — a person
    /// picking from a list cannot pick something that is not in it — but a task that
    /// fires at three in the morning needs its flags checked at the moment somebody
    /// is *there to fix them*, and this is the only place that knows what each CLI
    /// takes.
    ///
    /// A CLI that cannot say answers `None`, and the caller accepts whatever it
    /// was given. That is the rule this module states about its lists: too many
    /// rungs offers one option the API will refuse, while too few silently hides
    /// the rung a new model was released for, and refusing to schedule a valid
    /// effort because this machine has never run a CLI would be the second kind
    /// of wrong.
    fn accepted_efforts<'a>(&'a self, model: Option<&'a str>) -> BoxFuture<'a, Option<Vec<String>>>;

    /// Whether this CLI would take `permission` as the permission mode it is
    /// spawned with.
    fn permission_is_known(&self, permission: &str) -> bool;

    /// How this CLI's model ids are spelled.
    fn model_rule(&self) -> ModelRule {
        ModelRule::NAMED
    }
}

/// What the model routes use of the server's state.
#[derive(Clone)]
pub struct ModelsState {
    /// Every CLI this server drives, each of which lists its own models.
    pub providers: Arc<crate::ProviderRegistry>,
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
    // A source no provider runs has no list to offer.
    let models = state
        .providers
        .models(query.source)
        .ok_or_else(|| ApiError::bad_request(format!("no provider runs {} sessions", query.source)))?;
    models.list(&query).await.map(Json).map_err(ApiError::bad_request)
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
