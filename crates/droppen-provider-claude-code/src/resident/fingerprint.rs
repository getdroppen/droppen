//! Process configuration fingerprint and launch recipe for a Claude resident.

use std::path::PathBuf;

use droppen_base::process_env::Env;
use super::is_false;

// ------------------------------------------------------------ the fingerprint

/// What a resident was started with, and therefore what it can be handed.
///
/// Every field here is on the command line, which for a resident means fixed
/// for the process' whole life: a message that wants a different model cannot
/// simply be written to a process started under the old one. claude does have
/// `control_request` subtypes that change some of these in place, but they are
/// per-setting, versioned, and acknowledged even for values the CLI then
/// ignores — so the honest implementation is to compare and restart, and to
/// leave in-place switching to a phase that can verify it took.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct Fingerprint {
    /// `--model`, as the command line asked for it.
    pub model: Option<String>,
    /// The name the CLI answered under, when the process is the one being
    /// *described* rather than the one being asked for.
    ///
    /// One model, two names: a spawn says `--model fable` and `system:init`
    /// answers `claude-fable-5`. Both name the process that is running, and a
    /// send may carry either — the composer's own choice, or the name it read
    /// back out of the transcript — so both are compared against.
    ///
    /// Keeping only one of them is not a cosmetic loss. The driver used to
    /// replace the alias with the resolved name the moment `init` arrived,
    /// which made every send that still said `fable` look like a settings
    /// change: a flight recording of one real session shows eight turns and
    /// eight respawns, each one killing whatever the resident was minding in
    /// the background, and any message queued behind them failed outright
    /// because a message out of the journal has no environment to start a
    /// process with. Recording both names is what makes a resident a resident.
    ///
    /// Always `None` on a message — a send asks for a model, it cannot know
    /// what the CLI will call it — and ignored on that side by
    /// [`Fingerprint::satisfied_by`]. Absent from every record written before
    /// the second name was kept, which reads as "the CLI has not said yet".
    pub resolved: Option<String>,
    /// `--effort`.
    pub effort: Option<String>,
    /// `--permission-mode`.
    pub permission: Option<String>,
    /// The account whose token is in the environment; `None` is the root's own
    /// login. Not a flag, but the same kind of thing: fixed at spawn, and a
    /// message billed to another account needs its own process.
    pub account: Option<String>,
    /// *Which* credential that account held when this process started.
    ///
    /// The hash the store already keeps — of an OAuth token, or of an env
    /// block — and never a value; see
    /// [`droppen_accounts::store::AccountsStore::credential_hash`].
    ///
    /// The account id is not enough, and the case that proved it is the whole
    /// reason this field exists: a user whose env account pointed at a base URL
    /// with one path segment too many fixed the block, and every *new* session
    /// then worked while the one they were sitting in kept failing. The
    /// resident had been started with the old environment hours earlier, the id
    /// had not changed, so nothing asked it to stop. Re-authorizing an OAuth
    /// account is the same story with a 401 at the end of it.
    ///
    /// Compared exactly, like the account beside it. A record written before
    /// this field existed has `None` here and will not match a message that
    /// knows the hash, which costs one respawn per resident that outlived the
    /// upgrade — once, and never again, because the spawn that replaces it
    /// records the hash.
    ///
    /// `None` for the local login, which has no stored credential at all: that
    /// fingerprint is exactly what it was.
    pub credential: Option<String>,
    /// The instant-chat persona and `--disallowedTools *`.
    pub chat: bool,
    /// A tombstone for the intermediate build of the chat-attachment PR that
    /// ran chats with `--tools Read`.
    ///
    /// Only an intermediate build of the chat-attachment work wrote this, and
    /// only when true. It is kept serialised when true so a journal patch
    /// (which round-trips the typed row) cannot erase it before adoption has
    /// read it: `--tools Read` is bounded by the user's own CLI settings
    /// rather than this server's uploads directory (an allow rule for `Read`,
    /// or `bypassPermissions`, makes it unbounded), so a resident inherited
    /// from that build must be replaced, never kept. A record from main or
    /// from this build has it absent and is accepted; one with `true` is not.
    /// False is not written, so every ordinary row stays the bytes it was.
    #[serde(default, skip_serializing_if = "is_false")]
    pub chat_reads: bool,
    /// Whether `--add-dir <uploads>` is on the command line.
    pub uploads: bool,
    /// Whether `--permission-prompt-tool stdio` and the plan-mode
    /// `--disallowedTools` are: whether this process can ask the user a
    /// question at all. See [`droppen_provider::questions`].
    ///
    /// Written down and never compared. Every spawn this server makes has it,
    /// and a resident started before it existed is not restarted to get it:
    /// throwing away a warm session — and whatever it is minding in the
    /// background — for a capability its next message may never use is the
    /// trade [`Fingerprint::satisfied_by`] exists to refuse. It arrives with the
    /// next replacement the process gets for a reason of its own: a settings
    /// change, an account switch, a crash, an idle retirement. Absent from every
    /// record written before it, which reads as `false` — and is.
    pub questions: bool,
}

impl Fingerprint {
    /// The name the process runs its model under: the one the CLI answered
    /// with, else the one it was asked for. `deepseek-v4.1-flash[1m]` either
    /// way, which is the point: the transcript names that process' model
    /// without the marker.
    pub(crate) fn running_name(&self) -> Option<&str> {
        self.resolved.as_deref().or(self.model.as_deref())
    }

    /// Whether a message wanting `self` can go to a resident started with `live`.
    ///
    /// Every field is an equality test except the uploads directory, which is
    /// one-way on purpose. `--add-dir` only ever *widens* what claude may read
    /// without asking, so a resident that has it can take a message that does
    /// not need it; treating that as a mismatch would restart the process every
    /// time a conversation alternated between messages with and without an
    /// attachment — the common case, and the worst possible reason to throw a
    /// warm session away.
    ///
    /// `None` on the message side is *no opinion*, not "the default": a send
    /// that named no model, effort or permission is asking for the session as
    /// it stands, and reading it as a demand for factory settings would
    /// restart — or park behind background work — a process nobody asked to
    /// touch. The account is the exception, because `None` there is a real
    /// value (the local login, no token) and treating it as a wildcard would
    /// quietly keep billing an account the sender chose to drop.
    pub(crate) fn satisfied_by(&self, live: &Fingerprint) -> bool {
        fn wants(asked: &Option<String>, running: &Option<String>) -> bool {
            asked.is_none() || asked == running
        }
        // The model is the one setting with two right answers, because the CLI
        // renames it: see [`Fingerprint::resolved`]. Asking for either name is
        // asking for the process that is running.
        (wants(&self.model, &live.model) || wants(&self.model, &live.resolved))
            && wants(&self.effort, &live.effort)
            && wants(&self.permission, &live.permission)
            && self.account == live.account
            && self.credential == live.credential
            && self.chat == live.chat
            // A chat never runs with a permission mode: the server ignores one
            // in the body, and a process started with one by an earlier build
            // is replaced rather than used.
            && (!self.chat || live.permission.is_none())
            // See the field: a chat from the intermediate build has Read.
            && !live.chat_reads
            && (!self.uploads || live.uploads)
        // `questions` deliberately absent: see the field.
    }

    /// The same question, asked of a process whose settings may not be recorded.
    ///
    /// `None` is *unknown*, and unknown accepts. It has to: the alternative is
    /// what shipped, which read a record written before the fingerprint field
    /// existed as "a process started with no flags at all" — a description
    /// nothing but a flagless message satisfies, because the account is compared
    /// exactly and the wildcards are all on the message's side. A resident that
    /// outlived the upgrade therefore refused every message it was holding
    /// ("the settings this message needs were lost when the server restarted")
    /// and was then thrown away by the first fresh one, background work and all.
    ///
    /// Accepting is not a hole in the check, because unknown does not last: the
    /// first message that brings a recipe says what the process is, and
    /// [`super::Driver::take_settings`] writes that back to the journal. What is
    /// deliberately not done is *guessing* — a guess that happens to be wrong is
    /// the failure this replaces, in the other direction.
    pub(super) fn accepted_by(&self, live: Option<&Fingerprint>) -> bool {
        match live {
            Some(live) => self.satisfied_by(live),
            None => true,
        }
    }

    /// Whether a message wanting `self` would fit a process running `live` if
    /// only the login were set aside — the shape of a message an account
    /// rotation, or a binding changed by hand, pointed at another account
    /// while the process it was queued behind kept running on the old one.
    pub(super) fn fits_but_for_login(&self, live: &Fingerprint) -> bool {
        let unlogged = |fingerprint: &Fingerprint| Fingerprint {
            account: None,
            credential: None,
            ..fingerprint.clone()
        };
        self.account.is_some() && unlogged(self).satisfied_by(&unlogged(live))
    }

    /// Why a message wanting `self` cannot go to a process running `live`, in
    /// words the person who sent it will recognise.
    ///
    /// For the report and nothing else — [`Fingerprint::satisfied_by`] is the
    /// decision — which is why it can afford to be approximate at the edges: a
    /// caller only reaches here having already been told the two differ.
    pub(super) fn differences(&self, live: &Fingerprint) -> String {
        fn named(value: &Option<String>) -> &str {
            value.as_deref().unwrap_or("none")
        }
        fn login(value: &Option<String>) -> String {
            match value {
                Some(account) => format!("the {account} account"),
                // Not an account id: the root's own login, which is a real
                // choice and not the absence of one.
                None => "the local login".to_string(),
            }
        }
        /// Both of a running process' names, when the CLI gave it two.
        fn running_model(live: &Fingerprint) -> String {
            match (&live.model, &live.resolved) {
                (Some(asked), Some(answered)) if asked != answered => format!("{asked} (answering as {answered})"),
                (Some(name), _) | (None, Some(name)) => name.clone(),
                (None, None) => "none".to_string(),
            }
        }
        let mut parts = Vec::new();
        if self.model.is_some() && self.model != live.model && self.model != live.resolved {
            parts.push(format!("model {} rather than {}", named(&self.model), running_model(live)));
        }
        for (setting, asked, running) in [
            ("effort", &self.effort, &live.effort),
            ("permission mode", &self.permission, &live.permission),
        ] {
            if asked.is_some() && asked != running {
                parts.push(format!("{setting} {} rather than {}", named(asked), named(running)));
            }
        }
        if self.account != live.account {
            parts.push(format!("{} rather than {}", login(&self.account), login(&live.account)));
        } else if self.credential != live.credential {
            // Same login, different credential: the account was re-authorized
            // or its environment block edited after this process started.
            parts.push(format!(
                "{}'s current credentials, which this process did not start with",
                login(&self.account)
            ));
        }
        if self.chat != live.chat {
            parts.push(if self.chat { "the chat persona" } else { "no chat persona" }.to_string());
        }
        if live.chat_reads {
            parts.push("a chat with no tools rather than one with Read".to_string());
        }
        if self.chat && live.permission.is_some() {
            parts.push("a chat with no permission mode".to_string());
        }
        if self.uploads && !live.uploads {
            parts.push("access to the uploads directory".to_string());
        }
        if parts.is_empty() {
            // Not reachable from a real mismatch, and not worth an assertion:
            // an empty reason in a failure report reads as a bug in the report.
            return "different settings".to_string();
        }
        parts.join(", ")
    }
}

/// Everything needed to start a resident, resolved where it can be.
///
/// Built on the send path because that is the only place with the account
/// store, the uploads directory and the session's working directory in hand;
/// carried by the message so that a queued message which does not fit the
/// running process still knows what process it does fit.
#[derive(Debug, Clone)]
pub(crate) struct Recipe {
    /// Everything after the session selector: model, effort, permission,
    /// `--add-dir`, the chat persona. The selector itself is the driver's,
    /// because a respawn resumes an id the first spawn may not have known.
    pub args: Vec<String>,
    /// The whole environment, applied after an `env_clear`.
    pub env: Env,
    /// Where claude runs.
    pub workdir: Option<PathBuf>,
    /// What [`Fingerprint::satisfied_by`] compares.
    pub fingerprint: Fingerprint,
}
