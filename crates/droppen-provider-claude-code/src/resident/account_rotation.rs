//! Rate-limit tracking, account rotation, hold management, and ahead-of-limit switching.

use super::*;

/// A queue held after a limit with nowhere to go.
///
/// The alternative, which is what every release before this one did, fed the
/// rest of the queue to the account that had just refused — one real session
/// shows five rejections inside a minute. While it holds, a further refusal
/// from the same account is the same limit and decides nothing new, and the
/// choice is asked again every watchdog tick and whenever an account frees up
/// ([`Driver::recheck_hold`]). What lets it go:
///
/// * an account to move to appearing — the switch the first decision could
///   not make;
/// * the limit lifting — its rest ending or being cleared — which puts a
///   continuation for the failed message at the head of the queue first;
/// * a new message, which is somebody acting — and which, when it was sent
///   under another account, takes the held messages with it;
/// * a steer, which says "now";
/// * a new process under another account, for whatever reason it started.
///
/// Journalled on the run record, so a server that restarts mid-hold keeps
/// holding — see [`ClaudeRun::hold`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Hold {
    /// The account that refused.
    pub(super) account: String,
    /// Its display name, for the continuation that follows the lift.
    pub(super) label: String,
    /// The limit, as the rest and the decision name it.
    pub(super) class: LimitClass,
    pub(super) scope: String,
    /// When it lifts, epoch milliseconds; `None` until somebody acts or the
    /// rest is cleared.
    pub(super) until_ms: Option<i64>,
    /// How many switches the failed message's chain has had, for the cap.
    pub(super) switches: u8,
    /// Whether the failed message still needs a continuation — cleared once
    /// one is queued, or when somebody's new message takes over.
    pub(super) continue_failed: bool,
}

/// A limit that triggered a rotation, worded once for everything that reports
/// it: the log, the notification and the continuation.
#[derive(Debug, Clone)]
pub(super) struct LimitHit {
    pub(super) class: LimitClass,
    pub(super) scope: String,
    /// When it lifts, epoch milliseconds, when anything said.
    pub(super) until_ms: Option<i64>,
    /// [`rotation::reason`].
    pub(super) reason: String,
    /// [`rotation::until_text`], in this server's zone.
    pub(super) until: Option<String>,
}

impl LimitHit {
    pub(super) fn new(class: LimitClass, scope: &str, until_ms: Option<i64>) -> LimitHit {
        LimitHit {
            class,
            scope: scope.to_string(),
            until_ms,
            reason: rotation::reason(class, scope),
            until: rotation::until_text_local(until_ms),
        }
    }
}

/// [`rotation::Plan`] with the account it names resolved to a name.
pub(super) enum Choice {
    Switch(AccountRef),
    Nowhere(RotationOutcome),
}

impl Driver {
    // ------------------------------------------------------------ limits

    /// Who this resident bills, as the limit log names it, and what it runs.
    ///
    /// The live fingerprint when there is one — the account and the model the
    /// process was *started* with, which are the ones a limit is about. A
    /// resident inherited through an attach and not yet described has none, so
    /// the session's binding stands in for the account and the model is
    /// unknown rather than guessed.
    ///
    /// `None` when the limit is about no login the log names: a session with no
    /// account in this instance's own root runs on the root's own login, and
    /// filing it under the local login would sign the user's `~/.claude` out
    /// for somebody else's failure (see [`rest::claude_limit_account`]).
    pub(super) async fn limit_subject(&self) -> Option<(String, Option<String>)> {
        let (account, model) = match &self.live {
            Some(live) => (live.account.clone(), live.resolved.clone().or_else(|| live.model.clone())),
            None => (
                self.runs.host.accounts.session_account_id_for(&self.id, AccountProvider::Claude).await,
                None,
            ),
        };
        // Which root it runs in matters only when there is no account; read the
        // way `usage_login` reads it, so a reading and a limit agree on whose.
        let root = match (&account, &self.recipe) {
            (Some(_), _) => None,
            (None, Some(recipe)) => Some(recipe_root(&recipe.env)),
            (None, None) => crate::sessions::claude::find_claude_session(&self.runs.host.paths, &self.id)
                .await
                .map(|found| found.root),
        };
        let subject = rest::claude_limit_account(account.as_deref(), root);
        if subject.is_none() {
            tracing::info!(session = %self.id, "a limit on this instance's own root login with no account bound; not filed under the local login");
        }
        subject.map(|account_id| (account_id, model))
    }

    /// Record a `result` that says an account limit was hit, and say which.
    ///
    /// An error result is read in full; one the CLI labelled a success only
    /// when its text is the CLI's own limit sentence (see
    /// [`limits::is_definitive`]), because a successful turn's text is the
    /// model's answer and an answer may well be *about* rate limits.
    ///
    /// A weekly result borrows its reset time from a `rate_limit_event` of
    /// unknown scope seen just before it, when its own words give none — the
    /// event knew when, the result knows whose. See [`limits::with_hint`].
    ///
    /// The verdict comes back for [`Driver::rotate_after_limit`]; `None` when
    /// the result was not a limit.
    pub(super) async fn note_result_limit(&mut self, is_error: bool, detail: Option<&str>, api_status: Option<u16>) -> Option<Verdict> {
        // Taken whatever this result turns out to be: a hint explains the one
        // result after it, and no later one.
        let hint = self.limit_hint.take(Instant::now());
        let text = detail.unwrap_or_default();
        if !is_error && !limits::is_definitive(text) {
            return None;
        }
        limits::classify(text, api_status)?;
        let verdict = limits::read_text(text, api_status)?;
        let verdict = limits::with_hint(verdict, hint.as_ref());
        // The verdict still goes back to the rotation; only the filing needs a
        // login to file it under.
        let Some((account_id, model)) = self.limit_subject().await else {
            return Some(verdict);
        };
        let raw = match api_status {
            Some(status) => format!("{text}\n[api_error_status {status}]"),
            None => text.to_string(),
        };
        self.runs.host.record_limit(Observation {
            account_id,
            session_id: Some(self.id.clone()),
            source: "claude:result",
            model,
            verdict: verdict.clone(),
            raw,
            reason: String::new(),
            may_rest: true,
        });
        Some(verdict)
    }

    /// File a `rate_limit_event`'s windows under the login this process runs on.
    ///
    /// Every event carries them, `allowed` as much as `rejected`, and they are
    /// the reading the quota panel now lives on: made by the credential actually
    /// being billed, at no cost, and with no second login to keep alive. See
    /// [`droppen_accounts::quota::readings`].
    pub(super) async fn note_usage(&self, info: &Value) {
        let windows = info.windows_from_rate_limit_info();
        if windows.is_empty() {
            return;
        }
        let Some(login) = self.usage_login().await else {
            tracing::debug!(session = %self.id, "a quota reading from a session on the managed root's own login; not filed");
            return;
        };
        if let droppen_accounts::quota::UsageLogin::Account(id) = &login {
            // Only for an account that still exists: a reading must not
            // recreate the directory of one deleted under a running session.
            if self.runs.host.accounts.get(id).await.is_none() {
                return;
            }
        }
        // A reading with room in every window may be the account a held
        // session is waiting for.
        let headroom = windows.iter().all(|window| window.used_percent < rotation::EXHAUSTED_PERCENT);
        self.runs.host.quota.ingest_session(login, windows);
        if headroom {
            self.runs.recheck_holds_after_reading();
        }
    }

    /// Which login this process bills, as the quota readings name it.
    ///
    /// The account the process was started with, or the session's binding when
    /// nothing was written down (see [`Driver::limit_subject`]). With no account,
    /// the root decides: the user's own `~/.claude` is the local login, while
    /// this instance's managed root runs on whatever login that root holds and
    /// is nobody's to file under. The root is read off the recipe's environment
    /// when this server started the process, and off where the transcript lives
    /// when it inherited one.
    async fn usage_login(&self) -> Option<droppen_accounts::quota::UsageLogin> {
        let account = match &self.live {
            Some(live) => live.account.clone(),
            None => self.runs.host.accounts.session_account_id_for(&self.id, AccountProvider::Claude).await,
        };
        let root = match (&account, &self.recipe) {
            (Some(_), _) => None,
            (None, Some(recipe)) => Some(recipe_root(&recipe.env)),
            (None, None) => crate::sessions::claude::find_claude_session(&self.runs.host.paths, &self.id)
                .await
                .map(|found| found.root),
        };
        crate::quota::session_login(account.as_deref(), root)
    }

    /// Record a refused `rate_limit_event`. Recording only.
    ///
    /// A weekly window whose name does not say which models it covers is
    /// logged under scope `unknown` and rests nothing: it is held as a hint for
    /// the `result` that follows, which does say (see [`limits::WindowHint`]).
    pub(super) async fn note_rate_limit(&mut self, info: &Value) {
        let Some(reading) = limits::read_rate_limit_event(info) else { return };
        let window = info.get("rateLimitType").and_then(Value::as_str).unwrap_or("unknown");
        let status = info.get("status").and_then(Value::as_str).unwrap_or("unknown");
        let may_rest = reading.hint.is_none();
        if let Some(hint) = reading.hint {
            self.limit_hint.hold(hint, Instant::now());
        }
        let Some((account_id, model)) = self.limit_subject().await else { return };
        self.runs.host.record_limit(Observation {
            account_id,
            session_id: Some(self.id.clone()),
            source: "claude:rate_limit_event",
            model,
            verdict: reading.verdict,
            // The whole payload, `unifiedWindows` and all: every window's
            // utilization at the moment of refusal is worth more later than
            // the one line that names this one.
            raw: info.to_string(),
            reason: format!("rate_limit_event: {window} window {status}"),
            may_rest,
        });
    }

    /// Record a death whose stderr says the credential was refused.
    ///
    /// Only `auth`: a process that dies of anything else is the recovery
    /// ladder's business and says nothing about the account. Nothing changes
    /// about that ladder either — it still resumes with the same credential.
    pub(super) async fn note_exit_limit(&self) {
        if self.retiring {
            return;
        }
        let reason = self.death_reason();
        if !reason.lines().any(|line| limits::classify(line, None) == Some(LimitClass::Auth)) {
            return;
        }
        let Some((account_id, model)) = self.limit_subject().await else { return };
        self.runs.host.record_limit(Observation {
            account_id,
            session_id: Some(self.id.clone()),
            source: "claude:exit",
            model,
            verdict: Verdict {
                class: LimitClass::Auth,
                scope: limits::SCOPE_ALL.to_string(),
                until_ms: None,
            },
            raw: reason,
            reason: String::new(),
            may_rest: true,
        });
    }

    /// `result`: the turn is over, whatever else the process goes on to do.
    pub(super) async fn on_result(&mut self, is_error: bool, subtype: &str, detail: Option<&str>, api_status: Option<u16>) {
        // Whether or not a turn is in flight here: a `result` is claude saying
        // nothing more is coming, which is what closes the draft — no think is
        // in flight after it. See `drafts`.
        self.draft.end();
        // A question cannot outlive the turn that asked it. One still open here
        // ended without an answer — a stop, most likely, whose cancel this
        // driver did not see — and nobody may answer it into the next turn.
        self.close_questions(QuestionState::Cancelled, "the turn it was asked in ended");
        // Before the turn is taken, and whether or not there is one: a turn
        // claude opened on its own can hit a limit as well as one this server
        // asked for. A turn being stopped from here cannot — its failure is the
        // Stop, not the account.
        let limit = if !self.turn.as_ref().is_some_and(|turn| turn.ladder.is_some()) {
            self.note_result_limit(is_error, detail, api_status).await
        } else {
            None
        };
        let Some(turn) = self.turn.take() else {
            // A result with no turn is a turn that already ended — the ladder
            // killed it, a replay gap swallowed the message that opened it, or
            // a settle judged a steered message absorbed a moment before its
            // own answer arrived. Nothing to close, but it is still proof that
            // claude is between turns, so the queue is pumped: the alternative
            // is a driver holding messages for a process that is sitting idle,
            // which is the one failure mode this whole file exists to refuse.
            tracing::debug!(session = %self.id, "a result arrived with no turn in flight");
            self.pump().await;
            return;
        };
        let (outcome, error) = if turn.ladder.is_some() {
            (InjectionRunState::Failed, Some("interrupted from Droppen".to_string()))
        } else if is_error {
            // `result.result` carries only the first text block of the last
            // message, which makes a poor transcript and a perfectly good
            // explanation of a failure.
            let detail = detail.unwrap_or_default().trim();
            let message = if detail.is_empty() {
                format!("claude ended the turn with {subtype}")
            } else if subtype == "success" {
                // An API error before the stream starts, a limit, a refusal:
                // the subtitle is still `success`, and "ended the turn with
                // success" on top of an error reads as a contradiction. The
                // detail is the whole story here.
                self.runs.host.redact(detail)
            } else {
                format!("claude ended the turn with {subtype}\n{}", self.runs.host.redact(detail))
            };
            (InjectionRunState::Failed, Some(message))
        } else {
            (InjectionRunState::Ok, None)
        };
        let switches = turn.rotation_attempt;
        // Neither refused nor stopped from here, nor failed: the quiet point at
        // which the session may move ahead of a limit, before anything else
        // goes into the process.
        let ended_normally = limit.is_none() && turn.ladder.is_none() && !is_error;
        self.close_turn(&turn, outcome, error);
        // A limit on the session's own turn is where the queue used to be fed,
        // message after message, to the account that had just refused. Now the
        // pump waits for the decision, and the decision may be to move.
        if let Some(verdict) = limit {
            if self.rotate_after_limit(&verdict, switches).await {
                return;
            }
        }
        if ended_normally {
            self.rotate_ahead().await;
        }
        self.pump().await;
    }

    // ------------------------------------------------------------ rotation

    /// A limit ended this session's own turn: move the session, or hold its
    /// queue. Phase 2 of account rotation; see [`rotation`] for the rules.
    ///
    /// `switches` is how many times the chain the failed turn belongs to has
    /// already been moved, which is what the cap counts.
    ///
    /// `true` when the queue has been taken care of — a switch is under way, or
    /// the queue is held — and the caller must not pump. `false` hands the
    /// session back to the ordinary pump, which is right for everything that is
    /// not a new decision: a limit that does not trigger one, a session on no
    /// account or on one outside the pool, a switch already on its way, a
    /// queue already held against this very account.
    pub(super) async fn rotate_after_limit(&mut self, verdict: &Verdict, switches: u8) -> bool {
        let Some(live) = self.live.clone() else { return false };
        let Some(from_id) = live.account.clone() else { return false };
        // Already holding against this account: the same limit again — a
        // steered message, or a turn claude opened by itself, refused by the
        // account the queue is held for. Not a new decision, not a new row,
        // not a second notification; only a later reset time is worth keeping.
        if let Some(hold) = self.hold.as_ref().filter(|hold| hold.account == from_id) {
            if verdict.until_ms.is_some_and(|until| hold.until_ms.is_none_or(|held| until > held)) {
                let mut hold = hold.clone();
                hold.until_ms = verdict.until_ms;
                self.set_hold(Some(hold));
            }
            return false;
        }
        // Already moving: the head of the queue is a continuation for another
        // account, parked behind background work. What failed is claude, still
        // on the old account, answering one of those tasks with the model that
        // ran out — the same limit, not a new decision.
        if self
            .queue
            .front()
            .is_some_and(|next| next.continuation.is_some() && next.fingerprint.account != live.account)
        {
            return false;
        }
        let accounts = self.runs.host.accounts.clone();
        let from = accounts.get(&from_id).await;
        let bound = accounts.session_account_id_for(&self.id, AccountProvider::Claude).await;
        let rebound = bound.as_deref().is_some_and(|bound| bound != from_id);
        if !rotation::triggers(accounts.logins(), verdict.class, from.as_ref(), rebound) {
            return false;
        }
        let Some(from) = from else { return false };
        // The rest phase 1 has just written knows the reset at least as well as
        // this result does, and often better: the refused `rate_limit_event`
        // before it carried the exact instant.
        let until_ms = accounts
            .limit_log()
            .rest_until(&from_id, &verdict.scope, rest::now_ms())
            .flatten()
            .or(verdict.until_ms);
        let hit = LimitHit::new(verdict.class, &verdict.scope, until_ms);
        let from = AccountRef {
            id: from.id,
            label: from.label,
        };
        let outcome = match self.decide(&live, &from.id, switches).await {
            Choice::Switch(to) => match self.switch_account(&live, &from, &to, &hit, switches).await {
                Ok(()) => return true,
                Err(error) => {
                    // Known limitation: a switch that fails part-way — the
                    // new account's recipe or the binding could not be written —
                    // is recorded as `no_candidate`, which is what it leaves the
                    // session as: held, on the account that refused.
                    tracing::warn!(
                        session = %self.id,
                        from = %from.id,
                        to = %to.id,
                        %error,
                        "cannot move this session to the next account; holding its queue instead"
                    );
                    RotationOutcome::NoCandidate
                }
            },
            Choice::Nowhere(outcome) => outcome,
        };
        self.hold_after_limit(from, &hit, switches, outcome).await;
        true
    }

    /// Where a session limited on `from` goes: [`rotation::choose`] over the
    /// pool as it stands, the session's own binding and the family of the model
    /// the process is running — the name `system:init` gave it first, the one
    /// the spawn asked for second, and unknown after that.
    async fn decide(&self, live: &Fingerprint, from: &str, switches: u8) -> Choice {
        let accounts = &self.runs.host.accounts;
        let bound = accounts.session_account_id_for(&self.id, AccountProvider::Claude).await;
        let pool = rotation::pool_of(accounts, &self.runs.host.quota, AccountProvider::Claude).await;
        let family = ClaudeCodeLogin.model_family(live.resolved.as_deref().or(live.model.as_deref()));
        match rotation::choose(&pool, from, bound.as_deref(), family, switches, rest::now_ms()) {
            Plan::Switch(id) => Choice::Switch(
                pool.iter()
                    .find(|account| account.id == id)
                    .map(|account| AccountRef {
                        id: account.id.clone(),
                        label: account.label.clone(),
                    })
                    .unwrap_or_else(|| AccountRef { label: id.clone(), id }),
            ),
            Plan::NoCandidate => Choice::Nowhere(RotationOutcome::NoCandidate),
            Plan::CapReached => Choice::Nowhere(RotationOutcome::CapReached),
        }
    }

    /// Move this session from `from` to `to` and continue it there.
    ///
    /// In order: a recipe for `to` (the failed process' own, retargeted), the
    /// binding, every queued message pointed at `to`, the decision written down
    /// and announced, and a continuation at the head of the queue. The
    /// continuation needs a process `to` bills, so the fingerprint mismatch
    /// retires the old one and `--resume`s the conversation under `to` —
    /// straight away for a limit that covers every model, or once claude's
    /// background work is done for one that covers a single family.
    ///
    /// Nothing is bound until the recipe exists, so a failure before that leaves
    /// the session where it was.
    async fn switch_account(
        &mut self,
        live: &Fingerprint,
        from: &AccountRef,
        to: &AccountRef,
        hit: &LimitHit,
        switches: u8,
    ) -> Result<(), InjectError> {
        let recipe = match self.recipe.clone() {
            Some(base) => self.runs.retarget_recipe(&self.id, &base, &from.id, &to.id).await?,
            None => {
                let cwd = self.journalled_cwd();
                self.runs.recipe_from_settings(&self.id, live, cwd.as_deref(), &to.id).await?
            }
        };
        self.runs
            .host.accounts
            .bind_session_account(&self.id, Some(&to.id), AccountProvider::Claude)
            .await?;
        self.retarget_queue(&from.id, &recipe);
        let wait = rotation::wait_policy(&hit.scope);
        let interrupted = match wait {
            Wait::Now => self.interrupted_tasks().await,
            Wait::ForBackground => Vec::new(),
        };
        let mut note = Continuation {
            record: None,
            from: from.label.clone(),
            to: to.label.clone(),
            reason: hit.reason.clone(),
            until: hit.until.clone(),
        };
        let text = rotation::continuation_prompt(&note.from, &note.to, &note.reason, note.until.as_deref(), &interrupted);
        let decision = rotation::Decision {
            session_id: self.id.clone(),
            at_ms: rest::now_ms(),
            from: from.clone(),
            to: to.clone(),
            class: hit.class,
            scope: hit.scope.clone(),
            until_ms: hit.until_ms,
            interrupted_tasks: u32::try_from(interrupted.len()).unwrap_or(u32::MAX),
            outcome: RotationOutcome::Switched,
            trigger: RotationTrigger::Limit,
            used_percent: None,
        };
        let notice = rotation::notice(RotationOutcome::Switched, &from.label, &to.label, &hit.reason, hit.until.as_deref());
        note.record = self.runs.record_rotation(&decision, &notice);
        self.recorder.lifecycle(
            Stage::Rotate,
            Some(&format!(
                "{} limit ({}) on {}: moving to {}, {}; attempt {} of {}",
                hit.class.as_str(),
                hit.scope,
                from.id,
                to.id,
                match wait {
                    Wait::Now => format!("now, interrupting {} background task(s)", interrupted.len()),
                    Wait::ForBackground => format!("after {} background task(s)", self.background.len()),
                },
                switches.saturating_add(1),
                rotation::ROTATION_CAP,
            )),
        );
        let continuation = Pending {
            id: new_uuid(),
            text,
            images: Vec::new(),
            files: Vec::new(),
            fingerprint: recipe.fingerprint.clone(),
            recipe: Some(recipe.clone()),
            rotation_attempt: switches.saturating_add(1),
            continuation: Some(note),
        };
        match wait {
            Wait::Now => {
                // Steered messages went into a process that is about to go, so
                // they are undelivered again — and they were sent after the
                // message the continuation stands in for, so they go behind it.
                self.requeue_delivered();
                // Known limitation: the old process still holds these in its stdin
                // until it exits, and claude may open a turn for one (refused by the
                // same limit) before it goes. The resumed process then receives the
                // same user message a second time. Not verified against the CLI's
                // behaviour at stdin EOF.
                if self.respawn_for(continuation, &recipe).await {
                    self.pump().await;
                }
            }
            Wait::ForBackground => {
                // The ordinary settings-change park does the waiting: the head
                // of the queue no longer fits the process, and the pump holds it
                // there while claude minds background work, up to the ceiling.
                //
                // Known limitation: messages already steered into the old
                // process' stdin run there, on the account that refused, while
                // this waits — nothing can take a line back out of a pipe.
                self.queue.push_front(continuation);
                self.publish();
                self.note_progress(None);
                self.pump().await;
            }
        }
        Ok(())
    }

    /// Nowhere to move the session: say so, and hold the queue.
    async fn hold_after_limit(&mut self, from: AccountRef, hit: &LimitHit, switches: u8, outcome: RotationOutcome) {
        let notice = rotation::notice(outcome, &from.label, &from.label, &hit.reason, hit.until.as_deref());
        let decision = rotation::Decision {
            session_id: self.id.clone(),
            at_ms: rest::now_ms(),
            from: from.clone(),
            to: from.clone(),
            class: hit.class,
            scope: hit.scope.clone(),
            until_ms: hit.until_ms,
            interrupted_tasks: 0,
            outcome,
            trigger: RotationTrigger::Limit,
            used_percent: None,
        };
        self.runs.record_rotation(&decision, &notice);
        self.recorder.lifecycle(
            Stage::Rotate,
            Some(&format!(
                "{} limit ({}) on {}: {}; holding {} queued message(s)",
                hit.class.as_str(),
                hit.scope,
                from.id,
                outcome.as_str(),
                self.queue.len()
            )),
        );
        self.set_hold(Some(Hold {
            account: from.id,
            label: from.label,
            class: hit.class,
            scope: hit.scope.clone(),
            until_ms: hit.until_ms,
            switches,
            continue_failed: true,
        }));
        // Steered messages are in the old process' stdin and run whatever this
        // driver decides (the known limitation above); the pump opens their turns and then stops
        // at the hold.
        self.pump().await;
    }

    /// Ask a held queue's question again: has its limit lifted, or has an
    /// account appeared that the session can move to?
    ///
    /// Every watchdog tick, and whenever something that could change the answer
    /// happens elsewhere — an account's rest cleared, an account added or
    /// re-authorized, a quota reading with headroom
    /// ([`ClaudeCodeRuns::recheck_holds`]). An answer of "still nowhere"
    /// decides nothing: no row, no notification — the first decision already
    /// said so.
    pub(super) async fn recheck_hold(&mut self) {
        let Some(hold) = self.hold.clone() else { return };
        // From idle only. A turn in flight is claude answering something the
        // old process already had; moving now would cut it off.
        if self.turn.is_some() || self.replaying {
            return;
        }
        let Some(live) = self.live.clone() else { return };
        if live.account.as_deref() != Some(hold.account.as_str()) {
            // The process is no longer the one the queue was held against.
            self.set_hold(None);
            self.pump().await;
            return;
        }
        let now = rest::now_ms();
        let lapsed = hold.until_ms.is_some_and(|until| now >= until);
        let resting = self.runs.host.accounts.limit_log().rest_until(&hold.account, &hold.scope, now).is_some();
        if lapsed || !resting {
            self.lift_hold(hold).await;
            return;
        }
        let Choice::Switch(to) = self.decide(&live, &hold.account, hold.switches).await else {
            return;
        };
        let from = AccountRef {
            id: hold.account.clone(),
            label: hold.label.clone(),
        };
        let hit = LimitHit::new(hold.class, &hold.scope, hold.until_ms);
        self.set_hold(None);
        if let Err(error) = self.switch_account(&live, &from, &to, &hit, hold.switches).await {
            tracing::warn!(session = %self.id, to = %to.id, %error, "cannot move a held session to the account that freed up");
            self.set_hold(Some(hold));
        }
    }

    /// The held account's limit has lifted and nothing took the session over:
    /// continue the failed message there, then let the queue go.
    async fn lift_hold(&mut self, hold: Hold) {
        self.set_hold(None);
        if hold.continue_failed {
            if let Some(live) = self.live.clone() {
                let fingerprint = Fingerprint { resolved: None, ..live };
                let recipe = self.recipe.clone().filter(|recipe| recipe.fingerprint.account == fingerprint.account);
                self.queue.push_front(Pending {
                    id: new_uuid(),
                    text: rotation::resumed_prompt(&hold.label),
                    images: Vec::new(),
                    files: Vec::new(),
                    fingerprint,
                    recipe,
                    rotation_attempt: hold.switches,
                    continuation: None,
                });
            }
        }
        tracing::info!(session = %self.id, account = %hold.account, queued = self.queue.len(), "a held account's limit has lifted; continuing on it");
        self.recorder
            .lifecycle(Stage::Rotate, Some(&format!("{}'s limit has lifted: continuing there", hold.account)));
        self.publish();
        self.note_progress(self.turn.as_ref().map(|turn| turn.at));
        self.pump().await;
    }

    /// Let a held queue go because somebody acted.
    ///
    /// When the thing that released it is a message sent under another account
    /// — the user has moved the session by hand — the held messages follow it
    /// there rather than going back to the account that refused them. No
    /// continuation is queued: the person's own message is what comes next.
    pub(super) fn release_hold(&mut self, hold: Hold, newcomer: Option<&Pending>, why: &str) {
        let elsewhere = newcomer
            .and_then(|pending| pending.recipe.as_ref())
            .filter(|recipe| recipe.fingerprint.account.is_some() && recipe.fingerprint.account.as_deref() != Some(hold.account.as_str()));
        if let Some(recipe) = elsewhere {
            let recipe = recipe.clone();
            self.retarget_queue(&hold.account, &recipe);
        }
        tracing::info!(session = %self.id, account = %hold.account, queued = self.queue.len(), why, "released a queue held after a limit");
        self.recorder.lifecycle(Stage::Rotate, Some(&format!("released the held queue: {why}")));
    }

    /// Change the hold, and the journal's copy of it.
    pub(super) fn set_hold(&mut self, hold: Option<Hold>) {
        self.hold = hold;
        self.runs.host.journal.mark_hold(&self.record, self.hold.clone());
    }

    /// Take the hold out, if there is one, clearing the journal's copy too.
    pub(super) fn take_hold(&mut self) -> Option<Hold> {
        let hold = self.hold.take();
        if hold.is_some() {
            self.runs.host.journal.mark_hold(&self.record, None);
        }
        hold
    }

    // ------------------------------------------------------ ahead of a limit

    /// Move this session off its account before a window of it runs out.
    ///
    /// Asked only at a quiet point — right after a turn ended normally, and
    /// before the pump starts the next one — and never while claude minds
    /// background work: the point of moving early is that nothing is cut off,
    /// so a subagent still running puts the question off to the next quiet
    /// point. The rules themselves are [`rotation::plan_ahead`]'s.
    ///
    /// Only the binding and the queue move here. The process is replaced the
    /// next time the pump has something for it: the head of the queue then
    /// names the new account, and the ordinary settings-change path
    /// `--resume`s the conversation under it. No continuation prompt — nothing
    /// failed, so there is nothing to carry on from. `true` when it moved.
    pub(super) async fn rotate_ahead(&mut self) -> bool {
        let Some(threshold) = self.runs.rotate_ahead else { return false };
        if self.turn.is_some()
            || self.replaying
            || self.hold.is_some()
            || self.awaiting_background
            || !self.delivered_ahead.is_empty()
            || self.awaiting_user()
        {
            return false;
        }
        let Some(live) = self.live.clone() else { return false };
        let Some(from_id) = live.account.clone() else { return false };
        // Already on its way somewhere: a waiting message names another
        // account — a rotation's continuation, or somebody's own choice.
        if self.queue.iter().any(|pending| pending.fingerprint.account != live.account) {
            return false;
        }
        let family = ClaudeCodeLogin.model_family(live.resolved.as_deref().or(live.model.as_deref()));
        let now = rest::now_ms();
        // The cheap question first, at every quiet point: this account's own
        // reading. The binding, the log, the background work on disk and the
        // whole pool only when it is near.
        let own: Vec<rotation::Window> = self
            .runs
            .host.quota
            .cached(&droppen_accounts::quota::UsageLogin::Account(from_id.clone()))
            .windows
            .iter()
            .map(rotation::Window::from_quota)
            .collect();
        if rotation::near_window(&own, family, threshold, now).is_none() {
            return false;
        }
        let accounts = self.runs.host.accounts.clone();
        // A binding that names another account already is where the next
        // message goes; there is nothing to decide.
        let bound = accounts.session_account_id_for(&self.id, AccountProvider::Claude).await;
        if bound.as_deref() != Some(from_id.as_str()) {
            return false;
        }
        let log = accounts.rotation_log();
        let guards = AheadGuards {
            background: self.minding_background().await,
            last_ahead_ms: log.last_ahead_ms(&self.id),
            bound_by_hand_ms: log.bound_by_hand_ms(&self.id),
        };
        let pool = rotation::pool_of(&accounts, &self.runs.host.quota, AccountProvider::Claude).await;
        match rotation::plan_ahead(&pool, &from_id, family, threshold, guards, now) {
            AheadPlan::Stay => false,
            AheadPlan::Defer(near, why) => {
                tracing::debug!(
                    session = %self.id,
                    account = %from_id,
                    window = %near.label,
                    used = near.used_percent,
                    ?why,
                    "a window of this session's account is about to run out; not moving it yet"
                );
                false
            }
            AheadPlan::NoCandidate(near) => {
                // Once per crossing: the same window of the same account,
                // until it resets, says the same thing at every quiet point.
                let crossing = format!("{from_id} {} {}", near.label, near.resets_at_ms.unwrap_or_default());
                if self.ahead_noted.as_deref() != Some(crossing.as_str()) {
                    tracing::info!(
                        session = %self.id,
                        account = %from_id,
                        window = %near.label,
                        used = near.used_percent,
                        threshold,
                        "a window of this session's account is about to run out and no pool account is known to have room; staying"
                    );
                    self.ahead_noted = Some(crossing);
                }
                false
            }
            AheadPlan::Switch(to, near) => {
                let named = |id: &str| AccountRef {
                    id: id.to_string(),
                    label: pool
                        .iter()
                        .find(|account| account.id == id)
                        .map(|account| account.label.clone())
                        .unwrap_or_else(|| id.to_string()),
                };
                let (from, to) = (named(&from_id), named(&to));
                match self.switch_ahead(&live, &from, &to, &near).await {
                    Ok(()) => true,
                    Err(error) => {
                        tracing::warn!(
                            session = %self.id,
                            from = %from.id,
                            to = %to.id,
                            %error,
                            "cannot move this session ahead of its account's limit; staying"
                        );
                        false
                    }
                }
            }
        }
    }

    /// Move this session from `from` to `to` ahead of `near` running out: a
    /// recipe for `to`, the binding, every queued message pointed at `to`, and
    /// the decision written down and announced.
    ///
    /// The recipe is built first, as [`Driver::switch_account`] builds it, so a
    /// failure leaves the session bound where it was.
    async fn switch_ahead(&mut self, live: &Fingerprint, from: &AccountRef, to: &AccountRef, near: &rotation::Near) -> Result<(), InjectError> {
        let recipe = match self.recipe.clone() {
            Some(base) => self.runs.retarget_recipe(&self.id, &base, &from.id, &to.id).await?,
            None => {
                let cwd = self.journalled_cwd();
                self.runs.recipe_from_settings(&self.id, live, cwd.as_deref(), &to.id).await?
            }
        };
        self.runs
            .host.accounts
            .bind_session_account(&self.id, Some(&to.id), AccountProvider::Claude)
            .await?;
        self.retarget_queue(&from.id, &recipe);
        let until = rotation::until_text_local(near.resets_at_ms);
        let decision = rotation::Decision {
            session_id: self.id.clone(),
            at_ms: rest::now_ms(),
            from: from.clone(),
            to: to.clone(),
            class: near.class,
            scope: near.scope.clone(),
            until_ms: near.resets_at_ms,
            interrupted_tasks: 0,
            outcome: RotationOutcome::Switched,
            trigger: RotationTrigger::Ahead,
            used_percent: Some(near.used_percent),
        };
        let notice = rotation::ahead_notice(&from.label, &to.label, near, until.as_deref());
        self.runs.record_rotation(&decision, &notice);
        self.ahead_noted = None;
        self.recorder.lifecycle(
            Stage::Rotate,
            Some(&format!(
                "{} window at {}% on {}: moving to {} ahead of the limit; {} queued message(s) follow, and the next one resumes there",
                near.label,
                rotation::percent_text(near.used_percent),
                from.id,
                to.id,
                self.queue.len()
            )),
        );
        self.publish();
        Ok(())
    }

    /// Where the journal says this process was started, for a recipe built
    /// from nothing.
    fn journalled_cwd(&self) -> Option<String> {
        self.runs
            .host.journal
            .read()
            .into_iter()
            .find(|record| record.run_id == self.record)
            .and_then(|record| record.cwd)
    }

    /// A recipe for a message read back out of the journal that names another
    /// account than the running process — a rotation's continuation and the
    /// messages it retargeted, parked when the server restarted — when that
    /// account is the one the session is bound to now.
    ///
    /// Built on the running process' recipe with the account swapped when this
    /// driver has one, and from the message's own settings when it does not
    /// (every re-attached driver, until a message brings one). Any other
    /// mismatch is not rebuilt, and the pump fails the message as before.
    pub(super) async fn recipe_for_binding(&self, next: &Pending) -> Option<Recipe> {
        let live = self.live.as_ref()?;
        let account = next.fingerprint.account.as_deref()?;
        if !next.fingerprint.fits_but_for_login(live) {
            return None;
        }
        let bound = self.runs.host.accounts.session_account_id_for(&self.id, AccountProvider::Claude).await?;
        if bound != account {
            return None;
        }
        let built = match (&self.recipe, live.account.as_deref()) {
            (Some(base), Some(from)) => self.runs.retarget_recipe(&self.id, base, from, account).await,
            _ => {
                let cwd = self.journalled_cwd();
                self.runs
                    .recipe_from_settings(&self.id, &next.fingerprint, cwd.as_deref(), account)
                    .await
            }
        };
        match built {
            Ok(recipe) => {
                tracing::info!(session = %self.id, account, "rebuilt a journalled message's recipe for the account the session is bound to");
                Some(recipe)
            }
            Err(error) => {
                tracing::warn!(session = %self.id, account, %error, "cannot rebuild a journalled message for the session's account");
                None
            }
        }
    }

    /// Point every waiting message that was going to `from` at the account
    /// `template` bills instead.
    ///
    /// The environment is copied whole: it is the session's, not the
    /// message's — the Claude root, the account, the `rh-notify` token minted
    /// for this session — so every message for this session under one account
    /// has the same one. A message out of the journal has no recipe to rewrite,
    /// and only its fingerprint moves: it was accepted by the process that just
    /// failed, and the replacement is that process under another account.
    pub(super) fn retarget_queue(&mut self, from: &str, template: &Recipe) {
        let mut moved = 0usize;
        for pending in self.delivered_ahead.iter_mut().chain(self.queue.iter_mut()) {
            if pending.fingerprint.account.as_deref() != Some(from) {
                continue;
            }
            pending.fingerprint.account = template.fingerprint.account.clone();
            pending.fingerprint.credential = template.fingerprint.credential.clone();
            if let Some(recipe) = pending.recipe.as_mut() {
                recipe.env = template.env.clone();
                recipe.fingerprint.account = template.fingerprint.account.clone();
                recipe.fingerprint.credential = template.fingerprint.credential.clone();
            }
            moved += 1;
        }
        if moved > 0 {
            tracing::debug!(session = %self.id, from, moved, "queued messages follow the session to its new account");
            self.note_progress(self.turn.as_ref().map(|turn| turn.at));
        }
    }

    /// The background tasks a respawn now would cut short, as the
    /// continuation lists them: what each was asked to do, and its output file
    /// when claude has written one.
    async fn interrupted_tasks(&self) -> Vec<rotation::Interrupted> {
        if self.background.is_empty() {
            return Vec::new();
        }
        let dir = self.task_output_dir().await;
        let mut tasks = Vec::with_capacity(self.background.len());
        for task in &self.background.entries {
            let output = match &dir {
                Some(dir) => {
                    let path = dir.join(format!("{}.output", task.id));
                    tokio::fs::metadata(&path).await.is_ok().then(|| path.display().to_string())
                }
                None => None,
            };
            tasks.push(rotation::Interrupted {
                description: task
                    .description
                    .0
                    .clone()
                    .filter(|description| !description.trim().is_empty())
                    .unwrap_or_else(|| format!("后台任务 {}", task.id)),
                output,
            });
        }
        tasks
    }

    /// A continuation about to restart the process, after waiting for
    /// background work: if the wait ran out with tasks still going, say which
    /// in its text and in the log.
    pub(super) async fn account_for_interruptions(&mut self, mut next: Pending) -> Pending {
        if self.background.is_empty() {
            return next;
        }
        let Some(note) = next.continuation.clone() else { return next };
        let interrupted = self.interrupted_tasks().await;
        next.text = rotation::continuation_prompt(&note.from, &note.to, &note.reason, note.until.as_deref(), &interrupted);
        if let Some(record) = note.record {
            let count = u32::try_from(interrupted.len()).unwrap_or(u32::MAX);
            if let Err(error) = self.runs.host.accounts.rotation_log().set_interrupted(record, count) {
                tracing::warn!(session = %self.id, %error, "cannot record what an account switch interrupted");
            }
            self.runs.host.announce(Source::Claude, &self.id);
        }
        next
    }

    /// The main thread wrote a line with no turn in flight: claude opened one.
    ///
    /// # The turn nobody asked for
    ///
    /// A turn normally starts because this server wrote a prompt into stdin,
    /// which is why [`Driver::start_turn`] was for a long time the only place
    /// one could begin. claude 2.1.231 has a second way in, and it is not rare:
    /// a background task that finishes while the session is between turns wakes
    /// the main agent with its `task_notification`, and the CLI then runs a
    /// whole turn on its own — `system:init`, assistant messages, tool results,
    /// `result` — with nothing on stdin to have started it.
    ///
    /// Read off a real session (`3c002577`, 2026-08-30): three `result` lines
    /// went out where this server had opened two turns, and the turn it had not
    /// opened ran for two and a half minutes. Two user-visible failures came out
    /// of that gap, and both are this function:
    ///
    /// * the session read **idle** while its main thread was working, because
    ///   the phase only ever moved when a prompt was written;
    /// * a message sent during such a turn was judged to be arriving *between*
    ///   turns, so [`Driver::pump`] wrote it straight into stdin — where the CLI
    ///   folded it into the turn already running, answered it inside somebody
    ///   else's reply, and left no `user` entry in the transcript at all. "My
    ///   message disappeared", exactly.
    ///
    /// The rule fixes both at once, and the second one without a line of new
    /// queue code: an implicit turn is a turn, so `pump` holds the next message
    /// and the machinery that already exists journals it and delivers it when
    /// the `result` lands.
    ///
    /// # What may not open one
    ///
    /// Everything hangs off the discriminator in [`classify`], and the
    /// exclusions are the load-bearing part:
    ///
    /// * a **forwarded child event** — a background subagent's stream,
    ///   republished on the parent's stdout — is that subagent working, not this
    ///   thread. It is the ordinary state of an idle session that dispatched
    ///   background work, and a turn opened on one would never end;
    /// * a **`system` line**, `task_progress` and `task_updated` included, for
    ///   the same reason;
    /// * **`init`**, pointedly. Every process writes one at start-up, so a turn
    ///   opened on an init would be opened by a session that has done nothing —
    ///   and the notification-driven turn's own init arrives *before* its first
    ///   assistant line anyway, so nothing is lost by waiting for the latter;
    /// * a **`result`**, stale or duplicated: results only ever close, and a
    ///   result with no turn is handled where it always was, in
    ///   [`Driver::on_result`].
    ///
    /// A replay never reaches here at all — [`Driver::replay`] reconstructs
    /// rather than opens — so a re-attach mid-implicit-turn is decided by the
    /// same evidence and the same `replay_open` policy as any other turn.
    pub(super) fn on_speech(&mut self) {
        if self.turn.is_some() {
            return;
        }
        // The idle shutdown has already closed this process' stdin and is
        // waiting for it to go. Whatever it writes on the way out belongs to the
        // ending rather than to a turn, and opening one here would have
        // [`Driver::finish`] report a shutdown this server asked for as a failed
        // injection, with a notification to match.
        if self.retiring {
            return;
        }
        tracing::info!(session = %self.id, "claude opened a turn of its own; following it");
        // Marked in the recording, because the forensic question a flight log is
        // read for — why does this session say it is working — has a different
        // answer here than two lines above: there is no `stdin` line before this
        // `turn-open`, and there never will be.
        self.recorder
            .lifecycle(Stage::TurnOpen, Some("claude opened a turn of its own; nothing was written for it"));
        let turn = Turn {
            // The helper the replay's reconstruction uses, and for the same
            // reason: a run this session has already published is the one the
            // composer is looking at, and opening a second would leave the first
            // running for ever.
            run: self.launching_turn(),
            at: self.link.at(),
            seq: self.seq,
            ladder: None,
            // Real by observation. `provisional` is for a turn opened on a
            // *guess* — a steered message the CLI may have folded into somebody
            // else's turn — and the settle that judges it would close this one
            // in the middle of a long tool call. What is in flight here is not
            // in doubt: claude is talking.
            provisional: false,
            rotation_attempt: 0,
        };
        // Deliberately no journal mark. `start_turn` writes one because the
        // prompt it wrote is the only evidence its turn exists; this turn's
        // evidence is claude's own stdout, which a re-attach replays from byte
        // zero and reads as a turn in flight without any help — see
        // [`Driver::replay`]. The one case where a mark earns its keep here is a
        // message queued behind this turn, and `Cmd::Deliver` records the
        // offset when that happens.
        self.set_phase(Phase::Working);
        self.turn = Some(turn);
    }
}
