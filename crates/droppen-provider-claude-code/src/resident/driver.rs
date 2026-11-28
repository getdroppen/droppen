//! Core resident driver event loop, stdout dispatch, watchdog, recovery, and bookkeeping.

use super::*;

impl Driver {
    pub(super) async fn run(mut self) {
        let mut watchdog = tokio::time::interval(self.runs.host.watchdog_interval);
        watchdog.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        watchdog.tick().await; // the first tick is immediate

        // The message a fresh spawn was born holding goes in before anything is
        // heard: claude says nothing — not even `init` — until it has read a
        // first message (see `pump`), so an event-driven first delivery would
        // wait for ever on the event this delivery unlocks. A no-op for an
        // attached driver, whose replay gate holds until history has caught up.
        self.pump().await;
        // Steered messages inherited from the last server are watched for
        // folds like any others, from here on.
        self.arm_absorb();

        loop {
            // The two receivers are borrowed by hand: `select!` needs disjoint
            // borrows, and a method on `self` would take the whole of it.
            let wake = {
                let Driver { link, rx, .. } = &mut self;
                tokio::select! {
                    output = link.recv() => Wake::Output(output),
                    command = rx.recv() => Wake::Command(command),
                    _ = watchdog.tick() => Wake::Tick,
                }
            };
            match wake {
                Wake::Output(Some(Output::Line(line))) => {
                    // Recorded before it is acted on, and whether or not it is
                    // history: a recording is what claude *said*, in order, and
                    // the replay flag is the one thing a reader cannot work out
                    // for itself afterwards.
                    self.recorder.stdout(&line, self.replaying);
                    if self.replaying {
                        self.replay(&line).await;
                        self.settle_replay().await;
                    } else {
                        self.on_line(&line).await;
                    }
                }
                Wake::Output(Some(Output::Gap { from, to })) => {
                    // Lost output is not lost work: the transcript is the record
                    // and the feed reads it directly. What it does mean is that
                    // a `result` may have been in the gap, so the turn is left
                    // for the process' own exit or the user's Stop to end.
                    tracing::warn!(session = %self.id, from, to, "the session shim's replay buffer overflowed");
                    self.recorder.gap(from, to);
                    self.touch();
                    if self.replaying {
                        // A gap in the *history* is the one thing that can leave
                        // the reconstruction genuinely uncertain, so it is
                        // resolved the safe way: something was lost, and what it
                        // was may have been a turn. The journal's mark is taken
                        // rather than left pending, because whatever it said is
                        // already covered by that, and leaving it would let it
                        // re-open a turn the lines after the gap had ended.
                        self.cross_turn_mark();
                        self.replay_open = true;
                        self.replay_gapped = true;
                        self.settle_replay().await;
                    }
                }
                Wake::Output(None) => {
                    // A credential refused hard enough to end the process is
                    // written down first; the ladder below is unchanged by it.
                    self.note_exit_limit().await;
                    // The link is gone. Either a replacement takes its place and
                    // the session goes on, or there is nothing left to run.
                    if self.recover().await {
                        continue;
                    }
                    break;
                }
                Wake::Command(Some(command)) => self.on_command(command).await,
                // Every sender is gone, which cannot happen while the map holds
                // one; treat it as a shutdown rather than spinning on it.
                Wake::Command(None) => break,
                Wake::Tick => self.watchdog().await,
            }
        }
        self.finish().await;
    }

    // -------------------------------------------------------------- stdout

    /// Dispatch one line of claude's stdout.
    ///
    /// Tolerant by policy: an unparseable line is logged and stepped over,
    /// never treated as an error and never allowed to end a turn. The stream is
    /// versioned by a CLI that adds types and fields between patch releases,
    /// and a parser that refused what it did not recognise would turn each of
    /// those into a session that silently stops answering.
    pub(super) async fn on_line(&mut self, line: &str) {
        self.touch();
        // Parsed once for both of its readers. The draft keeps the count of a
        // think in flight, which the transcript will never say, and `classify`
        // decides what the driver does about the line; neither is a reason to
        // read a large tool result twice.
        let value = serde_json::from_str::<Value>(line).ok();
        if let Some(value) = &value {
            self.draft.observe(value);
        }
        match value.as_ref().map_or(Event::Unparseable, classify_value) {
            Event::Init { session, model } => {
                self.canonicalize_model(model);
                self.on_init(session.as_deref()).await
            }
            Event::Result {
                is_error,
                subtype,
                detail,
                api_status,
            } => self.on_result(is_error, &subtype, detail.as_deref(), api_status).await,
            // Recorded and nothing else: the turn it precedes runs, or fails,
            // exactly as it would have. The refusal's windows are a reading
            // like any other.
            Event::RateLimit { info } => {
                self.note_usage(&info).await;
                self.note_rate_limit(&info).await
            }
            Event::UsageReading { info } => self.note_usage(&info).await,
            // Acknowledged but not trusted: the CLI answers `success` to
            // requests it then ignores, so the ladder is timed from when the
            // request was *sent* and the turn's own `result` is the only proof
            // an interrupt landed.
            Event::ControlResponse => {
                tracing::debug!(session = %self.id, response = %clip(line), "claude answered a control request")
            }
            Event::BackgroundTasks { tasks } => {
                let changed = self.background.snapshot(tasks);
                self.after_background(changed).await;
            }
            Event::TaskStarted { task } => {
                let changed = self.background.observe(task);
                self.after_background(changed).await;
            }
            // Straight to the mirror rather than through `after_background`: the
            // list has not moved, so there is nothing to log and nothing parked
            // on it to wake — and these arrive every few seconds per task, which
            // is a rate worth keeping off both. `publish` writes the shared
            // struct readers already poll, so the cost is one lock.
            Event::TaskProgress { progress } => {
                if self.background.progress(progress) {
                    self.publish();
                }
            }
            Event::TaskFinished { id } => {
                let changed = self.background.remove(&id);
                self.after_background(changed).await;
            }
            Event::Speech => {
                // The result claude built from an answer comes back as a `user`
                // line: the question or permission it answers is over, by whatever path.
                if self.awaiting_user() {
                    let answered = answered_in(line);
                    let settled: Vec<String> = self
                        .asked
                        .iter()
                        .filter(|question| question.tool_use_id.as_ref().is_some_and(|id| answered.contains(id)))
                        .map(|question| question.request_id.clone())
                        .collect();
                    for request_id in settled {
                        self.withdraw(&request_id, QuestionState::Cancelled, "claude has a result for it");
                    }
                    let settled_perms: Vec<String> = self
                        .pending_permissions
                        .iter()
                        .filter(|perm| perm.tool_use_id.as_ref().is_some_and(|id| answered.contains(id)))
                        .map(|perm| perm.request_id.clone())
                        .collect();
                    for request_id in settled_perms {
                        self.withdraw_permission(&request_id, "claude has a result for it");
                    }
                }
                self.on_speech()
            }
            Event::Permission(request) => {
                if request.is_question() {
                    self.ask(&request);
                } else {
                    self.ask_permission(&request);
                }
            }
            Event::ControlRequest { request_id } => self.decline_request(request_id).await,
            Event::ControlCancel { request_id } => {
                self.withdraw(&request_id, QuestionState::Cancelled, "claude withdrew it");
                self.withdraw_permission(&request_id, "claude withdrew it");
            }
            Event::Unparseable => tracing::debug!(session = %self.id, line = %clip(line), "claude wrote a line that is not JSON"),
            // Everything else — a subagent's forwarded stream, stream_event, the
            // other system subtypes, and whatever the next CLI release invents —
            // is in the transcript the feed renders, or will be. A think the
            // session's own thread is in the middle of went to the draft above;
            // the only job left here is the `touch`, which is what the stall
            // watchdog reads.
            Event::Housekeeping | Event::Other => {}
        }
    }

    /// What claude is minding in the background has changed; say so.
    ///
    /// The only thing that *acts* on it is a settings change that has been parked
    /// waiting for the tasks to end, which is why the pump is only woken when the
    /// map empties. Everything else about a background task is claude's business:
    /// remote-hand neither starts nor ends them, it just declines to pull the
    /// process out from under one — and now describes them while they run.
    ///
    /// `changed` covers a description arriving for a task already known as well
    /// as the list itself moving, because both change what a row should say; a
    /// snapshot that repeats what was already published publishes nothing.
    pub(super) async fn after_background(&mut self, changed: bool) {
        if !changed {
            return;
        }
        tracing::debug!(session = %self.id, tasks = self.background.len(), "claude's background task list changed");
        self.publish();
        if self.background.is_empty() && self.awaiting_background {
            self.pump().await;
        }
    }

    /// `system:init`: the process is listening, and it has named the session.
    ///
    /// The id in this line is authoritative. remote-hand asks for one with
    /// `--resume` or `--session-id`, but which id the CLI actually writes under
    /// is its decision — a resume it cannot satisfy in place is answered by
    /// forking into a new one — and following the wrong file means the user
    /// watches a session that has quietly stopped replying. Note that `init` is
    /// *not* the first line: hook events precede it, so it is matched on rather
    /// than waited for positionally.
    /// Learn the model name `init` reports, *beside* the one the spawn asked for.
    ///
    /// The spawn may have said `haiku` and the process answers as
    /// `claude-haiku-4-5-…`: same model, two names, and a send may carry either
    /// — the composer's own choice, or the name it read back out of the
    /// transcript. So both are kept and [`Fingerprint::satisfied_by`] compares
    /// against both.
    ///
    /// It used to *replace* the alias, which quietly made a resident stop being
    /// one: every send still saying `fable` then read as a settings change, so
    /// the process was restarted — taking its background work with it — and
    /// anything queued behind the message that caused it was failed outright,
    /// having no environment of its own to be respawned for. Eight turns and
    /// eight respawns in the flight recording that found it.
    ///
    /// Written back to the journal as well as held here, so the next server
    /// starts out knowing both names rather than re-learning the second one at
    /// the cost of a restart. One rewrite, once per process — unless the model
    /// is switched inside the process.
    ///
    /// `init` opens every turn and names the model in force for it (claude
    /// 2.1.312), so a `/model` sent as a message is answered here at the next
    /// turn. That includes a switch back to the name the spawn asked for: the
    /// answer replaces the one before, or the process would go on being
    /// described by a model it no longer runs.
    ///
    /// A model name is not on its own a fingerprint, so an *unknown* live is
    /// left unknown: filling in the one field this line carries and defaulting
    /// the other five — the account above all — is precisely the guess that
    /// [`Fingerprint::accepted_by`] exists to refuse.
    pub(super) fn canonicalize_model(&mut self, model: Option<String>) {
        let Some(model) = model else { return };
        let Some(live) = self.live.as_mut() else { return };
        if live.running_name() == Some(model.as_str()) {
            return;
        }
        tracing::debug!(session = %self.id, asked = ?live.model, answered = %model, "claude named the model it is running");
        live.resolved = Some(model);
        // On the mirror too, and now: a process asked for no model (an
        // account's `ANTHROPIC_MODEL` chose it) is named here and nowhere
        // else, and the feed reads the session's window off this name.
        self.publish();
        self.note_fingerprint();
    }

    /// Act on a `system:init`: follow the id it names, and unblock the pump.
    ///
    /// # An `init` in the middle of a session's life
    ///
    /// Not an anomaly, and not only a background task's doing: a message
    /// buffered in claude's stdin — every steered one — is answered with a
    /// fresh `system:init` of its own before its assistant lines, verified
    /// against claude 2.1.231. So this runs again, mid-life, once per steered
    /// message, and must do nothing: the id is the same, so `adopt` is not
    /// reached; the model is the same, so `canonicalize_model` is a no-op; and
    /// `ready` is already true, so the pump below — which would open a *second*
    /// turn for a message this driver has already opened one for — is skipped.
    /// That gate is load-bearing, not defensive.
    pub(super) async fn on_init(&mut self, session: Option<&str>) {
        if let Some(id) = session {
            if id != self.id {
                self.adopt(id).await;
            }
        }
        // …with the one exception that makes steering safe: an `init` while a
        // *provisional* turn is open is claude saying it started a turn of its
        // own for the message it was handed, which is the proof that turn was
        // waiting for. Nothing else in the stream can say it — background
        // chatter and tool progress go on whether a turn is running or not, so
        // "claude said something" is not evidence and this deliberately is.
        if let Some(turn) = self.turn.as_mut().filter(|turn| turn.provisional) {
            turn.provisional = false;
            let at = turn.at;
            tracing::debug!(session = %self.id, "claude opened a turn of its own for the steered message");
            self.recorder
                .lifecycle(Stage::Absorb, Some("claude opened a turn of its own for the steered message"));
            // Only now is there a turn a restart could reconstruct: see
            // [`Driver::start_delivered_turn`] for why a provisional one is
            // kept out of the journal.
            self.note_progress(Some(at));
        }
        if !self.ready {
            self.ready = true;
            // A process that got this far started successfully, whatever it took
            // to get here: the recovery ladder is about resumes that *cannot*
            // start, so reaching `init` puts it back on its first rung and ends
            // the episode. The breaker's window is not reset — a session that
            // dies after init four times in five minutes is still a session in
            // a crash loop.
            self.rung = 0;
            self.recovering = false;
            self.pump().await;
        }
    }

    /// Follow claude to the session id it chose instead.
    ///
    /// Everything keyed by the id moves together, and the claim is re-taken
    /// before the old one is dropped so the session is never momentarily
    /// unowned. The account binding is deliberately *not* moved: it lives in a
    /// store this task holds no handle on, and a resume that forked has a
    /// bigger problem than its billing record — the recovery ladder that owns
    /// that case is the next phase's.
    pub(super) async fn adopt(&mut self, id: &str) {
        tracing::warn!(session = %self.id, adopted = %id, "claude answered under a different session id; following it");
        // The recording keeps the name it was opened under — one file per
        // session, whatever the CLI renames it to — so this line is the only
        // thing that says the transcript being followed from here on is a
        // different file.
        self.recorder
            .lifecycle(Stage::Adopt, Some(&format!("claude answered as {id} instead of {}", self.id)));
        self.recorder.follow(id);
        let claim = match self.lease.claim_other(Source::Claude, id) {
            Ok(claim) => Some(claim),
            Err(error) => {
                // Losing the claim degrades a cross-instance guarantee for a
                // process that is already running, which is not worth killing a
                // live session over.
                let busy = matches!(error, ClaimError::Busy { .. });
                tracing::warn!(session = %id, busy = ?busy, "cannot claim the session claude adopted");
                None
            }
        };
        {
            let mut residents = self.runs.residents.lock().expect("residents poisoned");
            if let Some(handle) = residents.remove(&self.id) {
                residents.insert(id.to_string(), handle);
            }
        }
        self.runs.host.rename_run(Source::Claude, &self.id, id);
        self.runs.host.journal.repoint(&self.record, id);
        // The page following the old id would otherwise show a think in
        // flight for ever; the one following the new id sees the rest of it.
        self.draft.retarget(self.runs.draft_channel(id));
        if let Err(error) = crate::sessions::claude::ClaudeCodeSessions.record_owned(&self.runs.host.paths, id).await {
            tracing::error!(session = %id, %error, "cannot record the adopted claude session as ours; it will list as external");
        }
        // Carry the account binding over to the adopted/forked session id so
        // subsequent turns and restarts keep the same billing account.
        if let Some(account) = self.live.as_ref().and_then(|fp| fp.account.as_deref()) {
            if let Err(error) = self
                .runs
                .host
                .accounts
                .bind_session_account(id, Some(account), AccountProvider::Claude)
                .await
            {
                tracing::warn!(session = %id, account, %error, "cannot migrate account binding to adopted claude session");
            }
        }
        #[cfg(test)]
        let old = self.id.clone();
        self.id = id.to_string();
        self.lease.replace(claim);
        #[cfg(test)]
        {
            self.runs.host.step("adopted: the new id", Source::Claude, Some(id));
            self.runs.host.step("adopted: the old id", Source::Claude, Some(&old));
        }
    }

    // ------------------------------------------------------------ watchdog

    /// Notice a turn that has gone quiet, and do nothing about it.
    ///
    /// The watchdog this replaces ended a run that made no progress for twenty
    /// minutes, which against a resident would mean killing every session that
    /// is merely between messages. It was also wrong for the case it was
    /// written for: a build, a test suite or a background agent can legitimately
    /// produce nothing for an hour, and a watchdog that kills them destroys work
    /// nobody can get back.
    ///
    /// So this marks and never kills, and only while a turn is in flight — a
    /// process that has not reached `system:init` counts, because a launch that
    /// never finishes is exactly as stuck. The mark is what a Stop button is
    /// offered beside; the decision stays the user's.
    pub(super) async fn watchdog(&mut self) {
        // A queue held after a limit asks its question again every tick: has
        // the limit lifted, or has an account freed up?
        if self.hold.is_some() {
            self.recheck_hold().await;
        }
        // Waiting on the user is neither a stall nor a ghost, and certainly not
        // idle. claude says nothing at all while a question is open — that is
        // what blocked means — and the transcript, asked whether a turn is in
        // flight, reads a tail ending on `AskUserQuestion` as "waiting for
        // input". Both are right, and either one acted on would throw away the
        // turn the question belongs to and push the queue into the middle of
        // it. The user takes as long as they take.
        if self.awaiting_user() {
            self.idle_since = None;
            return;
        }
        if self.turn.is_none() && self.ready {
            // Idle: no stall clock runs at all. Two other things do, and both
            // are about a process with nothing to do rather than a turn with
            // nothing to show.
            if self.awaiting_background {
                // The parked settings change. Woken by an event when there is
                // one, but the mtime signal in `minding_background` has no event
                // behind it, so something has to come back and ask again.
                self.pump().await;
            }
            self.idle_shutdown().await;
            return;
        }
        self.idle_since = None;
        if self.heard.elapsed() < self.runs.stall_after {
            return;
        }
        // The transcript is the other half of "quiet": a turn writing tool
        // results is making progress even while the stream-json side is silent.
        if let Some(progress) = self.runs.last_write(&self.id).await {
            if progress > self.heard {
                self.heard = progress;
            }
        }
        if self.heard.elapsed() < self.runs.stall_after {
            return;
        }
        // Quiet on both channels for long enough to be worth a second opinion,
        // and the transcript is the one witness that can give it. A turn it says
        // is not happening is a turn this driver reconstructed from something
        // that was not work — a `system` line after the last `result`, a replay
        // gap, a stale turn mark — and such a turn never ends on its own: the
        // `result` that would close it was written before this server attached.
        // The queue behind it would wait for ever, which is the failure this is
        // here to stop; getting it wrong the other way costs one turn re-read as
        // idle after ten minutes of total silence.
        if self.turn.is_some() && self.transcript_is_between_turns().await {
            self.discard_ghost_turn().await;
            return;
        }
        let mut shared = self.shared.lock().expect("resident mirror poisoned");
        if !shared.stalled {
            shared.stalled = true;
            drop(shared);
            self.recorder
                .lifecycle(Stage::Stalled, Some(&format!("nothing heard for {:?}", self.heard.elapsed())));
            tracing::warn!(
                session = %self.id,
                quiet_for = ?self.heard.elapsed(),
                "claude has produced nothing for a long time; marking the turn stalled and leaving it alone"
            );
        }
    }

    /// Retire a resident that has had nothing to do for long enough.
    ///
    /// Off by default, and deliberately so: a resident is a whole `claude`, and
    /// a whole `claude` is three or four hundred megabytes of resident memory
    /// per session that is doing nothing at all. On a machine with a handful of
    /// sessions that is the price of the thing working — the next message goes
    /// into a process that is already warm, and nothing in the background is
    /// ever cut off. On a machine with fifty it is not, and `DROPPEN_RESIDENT_IDLE_MS`
    /// is how that machine says so.
    ///
    /// What makes it safe is that nothing is lost by it. `close_stdin` is the
    /// polite end — claude writes its last line and exits zero — the claim and
    /// the journal record go with the driver, and the next message spawns a
    /// `--resume` that picks the conversation up where it was. What it must not
    /// do is end a process that is minding background work, which is the same
    /// guard the settings respawn takes.
    pub(super) async fn idle_shutdown(&mut self) {
        let Some(after) = self.runs.resident_idle else { return };
        // A steered message is outstanding work even though no turn is open for
        // it: ending the process would take it with it. So is a held queue's
        // promise to continue the failed message.
        if !self.queue.is_empty() || !self.delivered_ahead.is_empty() || self.hold.is_some() || self.awaiting_user() {
            self.idle_since = None;
            return;
        }
        let since = *self.idle_since.get_or_insert_with(Instant::now);
        if since.elapsed() < after {
            return;
        }
        if self.minding_background().await {
            // Not idle at all: the turn ended, the work did not. The clock is
            // restarted rather than merely skipped, so a session is given the
            // full quiet period *after* its background work finishes.
            self.idle_since = None;
            return;
        }
        tracing::info!(session = %self.id, idle_for = ?since.elapsed(), "retiring an idle resident claude");
        // Marked before the stdin goes, because the exit it causes arrives on
        // the same loop that decides whether an exit is a crash.
        self.retiring = true;
        if self.link.close_stdin().await.is_err() {
            self.link.kill_claude(libc::SIGTERM);
        }
    }

    // ------------------------------------------------------------ recovery

    /// The process died. Bring it back, or say why it is not coming back.
    ///
    /// `true` means a replacement is behind the link and the driver's loop
    /// should carry on; `false` means the session is over and [`Driver::finish`]
    /// takes it from here.
    ///
    /// Only *unexpected* deaths are recovered, and everything else in this file
    /// exists to make that distinction cheap:
    ///
    /// * an **idle** resident that went away has lost nothing — that is the
    ///   whole difference between a process ending and a turn ending — so it is
    ///   cleaned up in silence and the next message starts a fresh one;
    /// * a resident this server **stopped on purpose** — the interrupt ladder's
    ///   last rung, the idle shutdown — is not a crash, and restarting it would
    ///   undo the thing that was asked for;
    /// * a **settings respawn** that could not start its replacement has already
    ///   said so in `fatal`, and trying the same spawn again would only fail
    ///   again.
    pub(super) async fn recover(&mut self) -> bool {
        // A question is a promise from *this* process: the turn it blocks is
        // the only place an answer could go, and `--resume` closes the call as
        // interrupted and never asks it again. So it expires the moment the
        // process does, whatever happens next — and stays answerable, as an
        // ordinary message to whatever comes back.
        self.close_questions(QuestionState::Expired, "the process that asked it is gone");
        // Its draft goes with it: a process that is gone is not thinking.
        self.draft.end();
        // Before anything decides whether there is work left: a message steered
        // into the process that just died went into a pipe nothing will ever
        // read, so it is undelivered again — and it has to be back in the queue
        // *now*, both because the emptiness test below is the test for "is
        // there anything to recover for", and because everything downstream
        // (the relink's journal entry, the failure report if the breaker trips)
        // reads the queue and not this.
        self.requeue_delivered();
        let in_flight = self.turn.as_ref().is_some_and(|turn| turn.ladder.is_none());
        // `recovering` keeps an episode alive between rungs: a replacement that
        // dies *before saying it is listening* has no turn and usually no queue,
        // and without the flag it would read as an idle death — ending the
        // episode one rung in, with the fork never tried.
        if self.retiring || self.fatal.is_some() || (!in_flight && self.queue.is_empty() && !self.recovering) {
            return false;
        }
        let Some(recipe) = self.recipe.clone().or_else(|| self.queue.front().and_then(|next| next.recipe.clone())) else {
            // A resident inherited through an attach, with nothing queued behind
            // it: the journal knows what settings it ran under but not the
            // environment it ran with, and inventing one would start claude with
            // no credentials against somebody's repository.
            tracing::warn!(session = %self.id, "cannot recover a re-attached resident: nothing has described how to start one");
            self.fatal = Some("claude died and this server does not know how to start it again".to_string());
            return false;
        };

        // The breaker counts deaths, not attempts, and over a window rather than
        // in a row: a session that dies four times in five minutes is not going
        // to be fixed by a fifth restart, and one that dies once an hour is a
        // session that has been running for hours.
        let now = Instant::now();
        self.deaths.retain(|at| now.duration_since(*at) < RECOVERY_WINDOW);
        self.deaths.push_back(now);
        if self.deaths.len() >= RECOVERY_STRIKES {
            tracing::error!(session = %self.id, deaths = self.deaths.len(), "a resident claude is in a crash loop; giving up");
            self.fatal = Some(format!(
                "claude died {} times in {} minutes and could not be recovered (tried --resume, then --resume --fork-session)",
                self.deaths.len(),
                RECOVERY_WINDOW.as_secs() / 60,
            ));
            return false;
        }
        let selector = match self.rung {
            0 => Selector::Resume,
            1 => Selector::Fork,
            _ => {
                tracing::error!(session = %self.id, "a resident claude could not be resumed or forked; giving up");
                self.fatal = Some("claude died and neither --resume nor --resume --fork-session could bring the session back".to_string());
                return false;
            }
        };
        // The rung only advances for a process that died *before* saying it was
        // listening, because that is what makes the resume itself the suspect. A
        // process that reached `init` and then fell over has a healthy resume
        // path and a different problem, and forking its session would answer the
        // wrong question — see [`Driver::on_init`], which resets this.
        if !self.ready {
            self.rung = self.rung.saturating_add(1);
        }
        let ladder = &self.runs.recovery_backoff;
        let backoff = ladder[(self.deaths.len() - 1).min(ladder.len() - 1)];
        tracing::warn!(
            session = %self.id,
            ?selector,
            ?backoff,
            deaths = self.deaths.len(),
            "a resident claude died unexpectedly; recovering"
        );
        // The death's own reason, which is what makes a recording of a crash
        // loop readable: three `recover` lines all saying "exited with code 91"
        // is a different bug from three saying different things.
        self.recorder
            .lifecycle_with(Stage::Recover, None, Some(selector.name()), Some(&self.death_reason()));
        tokio::time::sleep(backoff).await;

        // The turn that was in flight is failed either way. Its work is in the
        // transcript up to the point claude stopped, and re-sending the prompt
        // into the resumed session would run the half of it that already
        // happened a second time — writing the same files, sending the same
        // mail. The queue is different: those messages were never delivered at
        // all, and now there is a session to deliver them to.
        if let Some(turn) = self.turn.take() {
            let reason = self.death_reason();
            self.close_turn(
                &turn,
                InjectionRunState::Failed,
                Some(format!("{reason}\nthe session was recovered; this turn was not")),
            );
        }
        if let Err(error) = self.relink(&recipe, selector).await {
            tracing::error!(session = %self.id, %error, ?selector, "cannot start a replacement for a resident that died");
            self.fatal = Some(format!("claude died and could not be restarted: {error}"));
            return false;
        }
        self.recovering = true;
        self.publish();
        // The replacement says nothing until it reads something (see `pump`),
        // so the queue this recovery exists to deliver is pumped at it now
        // rather than waited on.
        self.pump().await;
        true
    }

    /// Put steered messages back in the queue, for a process that cannot run them.
    ///
    /// The one thing that undoes a steer, and the only safe one: the write went
    /// into a pipe whose reader is gone, so the message provably never reached
    /// claude and re-delivering it runs it for the first time. Back at the
    /// *head*, in order, because that is where they were — ahead of everything
    /// still queued.
    ///
    /// Note what this is not: a message whose turn had already opened is not
    /// here to be requeued. It left `delivered_ahead` when its turn started,
    /// and a turn that died is failed rather than retried — see
    /// [`Driver::recover`].
    pub(super) fn requeue_delivered(&mut self) {
        if self.delivered_ahead.is_empty() {
            return;
        }
        tracing::warn!(
            session = %self.id,
            steered = self.delivered_ahead.len(),
            "messages steered into a process that died were never read; they go back to the queue"
        );
        while let Some(pending) = self.delivered_ahead.pop_back() {
            self.queue.push_front(pending);
        }
        self.publish();
    }

    /// What the tombstone and the stderr tail say about the exit just seen.
    pub(super) fn death_reason(&self) -> String {
        let reason = match self.sidecars.read_tombstone() {
            Some(shim::Exit { signal: Some(signal), .. }) => format!("claude was {}", droppen_provider::signal_name(signal).to_lowercase()),
            Some(shim::Exit { status: Some(code), .. }) => format!("claude exited with code {code}"),
            _ => "claude exited for an unknown reason".to_string(),
        };
        let tail = self
            .sidecars
            .read_stderr()
            .map(|stderr| self.runs.host.redact(&last_lines(&stderr, STDERR_TAIL_LINES)))
            .unwrap_or_default();
        if tail.is_empty() {
            reason
        } else {
            format!("{reason}\n{tail}")
        }
    }

    // --------------------------------------------------------------- exits

    /// The process is gone; say what that means for the turn it was running.
    ///
    /// The distinction here is the point of the whole module: a resident that
    /// dies between messages costs nothing and is cleaned up in silence, while
    /// one that dies *during* a turn has lost work somebody is waiting for and
    /// owes them the reason. The reason outlives the process because the shim
    /// wrote it down first — the tombstone and the stderr tail are already on
    /// disk by the time `recv` returns `None`.
    pub(super) async fn finish(mut self) {
        // Belt and braces: every path here but one comes through
        // [`Driver::recover`], which has already done this. The exception is a
        // driver whose inbox closed, and a message reported as undelivered is
        // the right answer for that too. The same for the questions.
        self.requeue_delivered();
        self.close_questions(QuestionState::Expired, "the process that asked it is gone");
        self.draft.end();
        let gave_up = self.fatal.is_some();
        let detailed = match self.fatal.take() {
            Some(fatal) => fatal,
            None => self.death_reason(),
        };
        let reason = detailed.lines().next().unwrap_or_default().to_string();
        // The tombstone and the stderr tail, whole: this is the last line of the
        // recording and the only one that can say why a session stopped. The
        // recorder itself lives until the end of this function, so the transcript
        // tailer gets a final sweep after it.
        self.recorder.lifecycle(Stage::Exit, Some(&detailed));

        if let Some(turn) = self.turn.take() {
            let interrupted = turn.ladder.is_some();
            tracing::warn!(session = %self.id, %reason, interrupted, "a resident claude died with a turn in flight");
            let error = if interrupted {
                "interrupted from Droppen".to_string()
            } else {
                detailed.clone()
            };
            self.close_turn(&turn, InjectionRunState::Failed, Some(error));
        } else {
            tracing::info!(session = %self.id, %reason, queued = self.queue.len(), "a resident claude ended");
            // A resident that died before its replay could say what it was in
            // the middle of leaves the run adoption published and no turn to
            // close it with. It is closed here instead, and only when nothing is
            // queued: the block below opens a run of its own for those, and two
            // failures for one death is one notification too many.
            if self.queue.is_empty() {
                if let Some(run) = self.published_run() {
                    let turn = Turn {
                        run,
                        at: 0,
                        seq: self.seq,
                        ladder: None,
                        provisional: false,
                        rotation_attempt: 0,
                    };
                    self.close_turn(&turn, InjectionRunState::Failed, Some(detailed.clone()));
                } else if gave_up {
                    // A verdict like the breaker's arrives with no turn to carry
                    // it — the turn died rungs ago — and it must not evaporate:
                    // this is the one line telling the user their session is not
                    // coming back on its own. A run is opened for the purpose.
                    let turn = Turn {
                        run: self.open_turn(),
                        at: self.link.at(),
                        seq: self.seq,
                        ladder: None,
                        provisional: false,
                        rotation_attempt: 0,
                    };
                    self.close_turn(&turn, InjectionRunState::Failed, Some(detailed.clone()));
                }
            }
        }

        // The queue only reaches here once [`Driver::recover`] has run out of
        // ways to give it a session to arrive in. Saying so is the last thing
        // this driver can usefully do: the messages are gone, and the person who
        // sent them has to hear it from somewhere.
        if !self.queue.is_empty() {
            let dropped = self.queue.len();
            for pending in &self.queue {
                self.receipt(pending, ReceiptStatus::Failed);
            }
            let turn = Turn {
                run: self.open_turn(),
                at: self.link.at(),
                seq: self.seq,
                ladder: None,
                provisional: false,
                rotation_attempt: 0,
            };
            self.close_turn(
                &turn,
                InjectionRunState::Failed,
                Some(format!("{detailed}\n{dropped} queued message(s) were not delivered")),
            );
        }

        // Its own entry, and only if it is still its own: `adopt` may have moved
        // the key, and a racing send may already have put a replacement there.
        {
            let mut residents = self.runs.residents.lock().expect("residents poisoned");
            if residents.get(&self.id).is_some_and(|handle| handle.tx.same_channel(&self.tx)) {
                residents.remove(&self.id);
            }
        }
        self.runs.host.journal.remove(&self.record);
        // The claim goes last, and outside every other lock: dropping it
        // unlinks a file, and until it is gone this session is still ours.
        self.lease.release();
        #[cfg(test)]
        self.runs.host.step("resident ended", Source::Claude, Some(&self.id));
        let _ = std::fs::remove_file(self.sidecars.exit());
        let _ = std::fs::remove_file(self.sidecars.stderr());
    }

    // --------------------------------------------------------- bookkeeping

    /// Put a `Running` injection in the registry for the turn about to start.
    pub(super) fn open_turn(&mut self) -> Arc<RunEntry> {
        self.seq += 1;
        let run = running_turn(&self.id);
        self.runs.host.publish_run(Source::Claude, &self.id, run.clone());
        run
    }

    /// The run to fail when a launch is stopped before it ever started a turn.
    ///
    /// [`ClaudeCodeRuns::start_resident`] put one there the moment the send
    /// was accepted, so that a session waiting on a cold CLI does not read as
    /// idle. That is the run whoever pressed Stop is looking at, so it is the
    /// one that has to be closed — opening a second would leave the first
    /// running for ever.
    pub(super) fn launching_turn(&mut self) -> Arc<RunEntry> {
        match self.published_run() {
            Some(run) => {
                self.seq += 1;
                run
            }
            None => self.open_turn(),
        }
    }

    /// The `Running` injection this session already has, if it has one.
    ///
    /// Always one this driver is responsible for: a session with a resident is
    /// claimed, and nothing else may put a run against a claimed session. It is
    /// either the one the send path published while the CLI was starting, or the
    /// one adoption published while the replay was still arriving.
    pub(super) fn published_run(&self) -> Option<Arc<RunEntry>> {
        self.runs.host.running_run(Source::Claude, &self.id)
    }

    /// Take back a run that was published for a turn there turns out not to be.
    ///
    /// Removed rather than closed: a turn that ended while this server was down
    /// needs no explanation, and a failure reported on every restart would be
    /// noise about something that went fine.
    pub(super) fn withdraw_published_run(&self) {
        self.runs.host.withdraw_run(Source::Claude, &self.id);
    }

    /// Write a turn's outcome, tell the phone, and go back to idle.
    pub(super) fn close_turn(&mut self, turn: &Turn, outcome: InjectionRunState, error: Option<String>) {
        // Every way a turn can end comes through here, which is why the journal
        // is cleared here rather than beside the `result` that usually does it:
        // an interrupted turn, a failed write and a dead process all have to
        // leave the record saying the same thing. The recording is written at
        // the same point and for the same reason, and so is the draft closed —
        // a no-op after the `result` that has already done it.
        self.note_progress(None);
        self.draft.end();
        self.recorder.lifecycle(
            Stage::Result,
            Some(&match (&error, outcome) {
                (Some(error), _) => format!("{outcome:?}: {error}"),
                (None, outcome) => format!("{outcome:?}"),
            }),
        );
        {
            let mut state = turn.run.state.lock().expect("run state poisoned");
            state.state = outcome;
            state.error = error.clone();
            state.finished_at = Some(iso8601(std::time::SystemTime::now()));
        }
        let _ = turn.run.finished.send(true);
        // The phase goes back to idle *before* the notification, and the order is
        // the whole point: [`droppen_notify::push`] now reads the driver's own account of
        // the session, and a snapshot taken a line earlier would still say
        // `Working` — it would report the turn that just ended as still in
        // flight. After this line what is left in the snapshot is only what is
        // genuinely outstanding: a message queued behind this turn, or background
        // work claude is still minding.
        self.set_phase(Phase::Idle);
        // Taken here rather than inside the spawned task. `on_result` pumps the
        // next queued message the moment this returns, so a snapshot read
        // asynchronously would be a coin toss between this turn and the next.
        let driver = self.runs.host.driver_states_of(&*self.runs);
        // Once per turn, where the old model could only manage once per process.
        // A resident answering ten messages now sends ten notifications, and the
        // throttle in [`droppen_notify::push`] is what keeps that from being ten lines in
        // the notification shade.
        if let Some(alerts) = self.runs.host.alerts.get() {
            alerts.turn(self.runs.host.paths.clone(), Source::Claude, self.id.clone(), outcome, error, driver);
        }
    }

    /// Record that claude is alive and talking.
    pub(super) fn touch(&mut self) {
        self.heard = Instant::now();
        self.shared.lock().expect("resident mirror poisoned").stalled = false;
    }

    pub(super) fn set_phase(&self, phase: Phase) {
        let moved = {
            let mut shared = self.shared.lock().expect("resident mirror poisoned");
            let moved = shared.phase != phase;
            shared.phase = phase;
            if phase != Phase::Working {
                shared.stalled = false;
            }
            // The turn's clock: started by whatever leaves `Idle`, and not restarted
            // by a recovery that passes back through `Starting` mid-turn.
            match phase {
                Phase::Idle => shared.turn_started_at = None,
                Phase::Starting | Phase::Working => {
                    if shared.turn_started_at.is_none() {
                        shared.turn_started_at = Some(iso8601(SystemTime::now()));
                    }
                }
            }
            moved
        };
        // A turn opening or closing is the one change a page following the
        // session most wants to see, and no file says it: the transcript has
        // usually stopped moving by the time the turn is over. Left to the
        // feed's poll, "working" outlived the turn by up to two seconds.
        //
        // The feed reads the state on its own task, possibly before a caller
        // that moves the phase and then publishes the queue has got to the
        // second half — so the publish that follows pokes once more, and the
        // frame that settles is the whole of the change rather than half of it.
        if moved {
            self.phase_moved.store(true, std::sync::atomic::Ordering::Relaxed);
            self.runs.host.poke_feed(Source::Claude, &self.id);
        }
    }

    /// Say — or stop saying — that a settings change is waiting on background work.
    pub(super) fn set_awaiting(&mut self, awaiting: bool) {
        self.awaiting_background = awaiting;
        if !awaiting {
            self.awaiting_since = None;
        }
        self.shared.lock().expect("resident mirror poisoned").awaiting_background = awaiting;
    }

    /// Whether a parked settings change has waited past the point of politeness.
    ///
    /// The park has no other way out. A parked driver sits at `Phase::Idle`, so
    /// [`ClaudeCodeRuns::interrupt_resident`] refuses it — there is no turn
    /// to stop — and the message at the head of the queue is neither delivered
    /// nor failed. Every release depends on the background set emptying, and
    /// the whole reason [`Driver::settle_background`] exists is that the set
    /// can be wrong about work that is not there.
    ///
    /// So there is a ceiling. Reaching it restarts the process, which does end
    /// whatever claude was genuinely minding — the trade this makes deliberately,
    /// because the alternative is a session that answers nothing, ever, and
    /// cannot be told to stop. It is loud in the log and on the row
    /// (`awaitingBackground`) for the whole wait, and `DROPPEN_AWAIT_BACKGROUND_MS=0`
    /// removes it for an installation that would rather wait for ever.
    pub(super) fn waited_long_enough(&mut self) -> bool {
        let Some(limit) = self.runs.await_background else { return false };
        let since = *self.awaiting_since.get_or_insert_with(Instant::now);
        if since.elapsed() < limit {
            return false;
        }
        tracing::warn!(
            session = %self.id,
            tasks = self.background.len(),
            waited = ?since.elapsed(),
            "a settings change has waited for background work past the limit; restarting claude anyway"
        );
        self.recorder.lifecycle(
            Stage::Respawn,
            Some(&format!(
                "a settings change waited {:?} for {} background task(s) and gave up",
                since.elapsed(),
                self.background.len()
            )),
        );
        self.awaiting_since = None;
        true
    }

    pub(super) fn publish(&self) {
        {
            let mut shared = self.shared.lock().expect("resident mirror poisoned");
            shared.queued = self.delivered_ahead.len() + self.queue.len();
            shared.queued_messages = self.backlog(Pending::preview);
            shared.background = self.background.len();
            shared.tasks = self.background.wire();
            shared.question = self.asked.first().map(|question| question.request_id.clone());
            shared.permission = self.pending_permissions.first().map(|perm| perm.request_id.clone());
            shared.model = self.live.as_ref().and_then(Fingerprint::running_name).map(str::to_owned);
        }
        // Only after a phase change: see `set_phase`. Everything else published
        // here waits for the feed's own poll, as it always has.
        if self.phase_moved.swap(false, std::sync::atomic::Ordering::Relaxed) {
            self.runs.host.poke_feed(Source::Claude, &self.id);
        }
    }

    /// Everything outstanding, in the order it will run.
    ///
    /// One function for the two shapes a backlog is written in — the mirror's
    /// and the journal's — because the *order* is the part that has to agree,
    /// and a second hand-rolled chain is how it would stop agreeing. Steered
    /// first, then the queue, each entry told which it was.
    pub(super) fn backlog<T>(&self, shape: impl Fn(&Pending, bool) -> T) -> Vec<T> {
        self.delivered_ahead
            .iter()
            .map(|pending| shape(pending, true))
            .chain(self.queue.iter().map(|pending| shape(pending, false)))
            .collect()
    }

    /// Write down what this resident is in the middle of.
    ///
    /// The disk counterpart of [`Driver::publish`], and the difference between
    /// them is who is asking: `publish` answers this server's own readers, while
    /// this answers the *next* server's. Called wherever the queue or the turn
    /// moves, and it restates both every time rather than tracking which changed
    /// — the record is small, and a rule of "always both" cannot leave one of
    /// them stale.
    pub(super) fn note_progress(&self, turn_at: Option<u64>) {
        self.runs
            .host.journal
            .mark_progress(&self.record, turn_at, &self.backlog(Pending::recorded));
    }

    /// Write down what the process this driver is holding is running.
    ///
    /// Only ever a correction, and only from the two places that can know
    /// better than the spawn did: the settings taken off the first message an
    /// adopted resident is given, and the model name `system:init` resolved. A
    /// driver that still knows nothing writes nothing rather than writing the
    /// default, which is the whole distinction this change turns on.
    pub(super) fn note_fingerprint(&self) {
        let Some(live) = self.live.as_ref() else { return };
        self.runs.host.journal.mark_fingerprint(&self.record, live);
    }

    /// Tell the receipt store what became of a message — a no-op for one no
    /// client named. See [`droppen_provider::receipts`].
    pub(super) fn receipt(&self, pending: &Pending, status: ReceiptStatus) {
        self.runs.host.note_receipt(Source::Claude, &self.id, &pending.id, status);
    }

    /// Report a message that will never be delivered, as its own failed turn.
    ///
    /// The message is already out of the queue; what is left is telling the
    /// person who sent it, and a failed injection is the only channel this
    /// session has for that.
    pub(super) fn fail_message(&mut self, why: &str) {
        let turn = Turn {
            run: self.open_turn(),
            at: self.link.at(),
            seq: self.seq,
            ladder: None,
            provisional: false,
            rotation_attempt: 0,
        };
        self.close_turn(&turn, InjectionRunState::Failed, Some(why.to_string()));
    }
}
