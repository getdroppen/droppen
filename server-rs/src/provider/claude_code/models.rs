//! Claude Code's model list, its effort ladders and its permission modes.
//!
//! Claude Code publishes no list on disk, but it will *answer* over its control
//! protocol ([`probe`]); a stored account's own block can name its model; and a
//! static list of the tier aliases is the guess when neither can be asked.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::future::BoxFuture;

use droppen_provider::models::{ModelInfo, ModelList, ModelListSource, Models, ModelsQuery};
use droppen_accounts::store::{Account, AccountKind, AccountsStore};
use droppen_http::openapi_ext::Nullable;
use droppen_instance::paths::Paths;

pub mod probe;

pub use probe::ClaudeModelCache;

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
/// Named one by one because that is what [`droppen_accounts::store::AccountsStore::env_values`]
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

/// The directory an account's list can be asked of, when there is one.
///
/// The quota panel's rule, not a new one: only an OAuth account that has opted
/// into probing has a logged-in config root of its own — [`droppen_accounts::quota`] picks
/// the directories it probes on exactly this pair of conditions, and a model
/// list for a root the quota panel will not read would be a second rule.
/// An env account authenticates per request and has no root to ask; an account
/// with probing off has a directory with no login in it, and asking would
/// answer for nobody. Either way the honest answer is the fallback list.
fn probe_dir(accounts: &droppen_accounts::store::AccountsStore, account: &Account) -> Option<PathBuf> {
    if !account.usage_enabled || account.kind != AccountKind::OauthToken {
        return None;
    }
    // Refuses anything that is not a uuid, so a query string can never name a
    // directory of its own.
    accounts.usage_config_dir(&account.id).ok()
}

/// The `--permission-mode` / sandbox values remote-hand will spawn with.
///
/// Codex's are the CLI's own, asked of `droppen_provider_codex::app_server::codex_permission_flags`
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

/// Claude Code's models, for [`Models`]: each login's list, asked of its
/// config root and cached for the process ([`ClaudeModelCache`]).
pub(crate) struct ClaudeModels {
    /// The lists Claude Code published, per config root.
    cache: Arc<ClaudeModelCache>,
    /// The stored accounts, whose roots or env blocks name a login's models.
    accounts: Arc<AccountsStore>,
}

impl ClaudeModels {
    /// The models of the logins in `accounts`, asked of Claude Code under `paths`.
    pub(crate) fn new(paths: Arc<Paths>, accounts: Arc<AccountsStore>) -> Self {
        Self {
            cache: Arc::new(ClaudeModelCache::new(paths)),
            accounts,
        }
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
    async fn login_models(&self, account_id: Option<&str>) -> (Vec<ModelInfo>, ModelListSource) {
        let Some(id) = account_id.map(str::trim).filter(|id| !id.is_empty()) else {
            return self.cache.models(None).await;
        };
        let accounts = &self.accounts;
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
        self.cache.models(Some(&dir)).await
    }
}

impl Models for ClaudeModels {
    // Per *login*: the query names the account whose credentials the run will
    // use; absent is the user's own `~/.claude`.
    fn list<'a>(&'a self, query: &'a ModelsQuery) -> BoxFuture<'a, Result<ModelList, String>> {
        Box::pin(async move {
            let (models, source) = self.login_models(query.account_id.as_deref()).await;
            Ok(ModelList {
                models,
                source,
                hidden: None.into(),
            })
        })
    }

    fn ids<'a>(&'a self, account_id: Option<&'a str>) -> BoxFuture<'a, Vec<String>> {
        Box::pin(async move { self.login_models(account_id).await.0.into_iter().map(|model| model.id).collect() })
    }

    // A ladder per model, derived from the id ([`claude_efforts`]). A named
    // model is checked against its own rungs, so an `--effort max` on a haiku
    // is refused here rather than by the API at three in the morning; an
    // unnamed model gets the full ladder.
    fn accepted_efforts<'a>(&'a self, model: Option<&'a str>) -> BoxFuture<'a, Option<Vec<String>>> {
        Box::pin(std::future::ready(Some(claude_efforts(model.unwrap_or_default()))))
    }

    fn permission_is_known(&self, permission: &str) -> bool {
        CLAUDE_PERMISSIONS.contains(&permission)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_five() -> Nullable<Vec<String>> {
        Nullable(Some(CLAUDE_EFFORTS.iter().map(|e| (*e).to_string()).collect()))
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
        let paths = Arc::new(Paths::new(PathBuf::from("/nonexistent")));
        let models = ClaudeModels::new(paths.clone(), Arc::new(AccountsStore::new(paths, droppen_accounts::login::Logins::stand_ins())));
        for model in ["GLM-5.3-CYBERSECURITY-FP8", "qwen3-max", "glm-5-air"] {
            assert_eq!(
                models.accepted_efforts(Some(model)).await,
                Some(CLAUDE_EFFORTS.iter().map(|effort| (*effort).to_string()).collect::<Vec<_>>()),
                "{model} is not a claude id and must not be given a claude id's ladder"
            );
        }
    }
}
