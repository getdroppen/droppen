//! Restart replay and state reconciliation for a re-attached Claude resident.

use std::time::Instant;

use droppen_provider::common::Phase;
use droppen_sessions::delivery::QuestionState;

use super::*;

impl Driver {

// -------------------------------------------------------------- replay

    /// Read one line of history, for the state it implies and nothing else.
    ///
    /// Everything a re-attached driver knows about the session it inherited
    /// comes from here: the shim replays claude's whole stdout from byte zero,
    /// and this walks it. Nothing may act — a `result` in the replay is a turn
    /// that ended before this server existed, and notifying a phone about it, or
    /// writing the next queued message into the middle of a live turn on the
    /// strength of it, is the failure mode this split exists to prevent.
    ///
    /// The one exception is the session id. `system:init` is authoritative about
    /// which transcript claude is writing, and a fork that happened while this
    /// server was down is something the replay is the *only* record of.
    pub(super) async fn replay(&mut self, line: &str) {
        self.cross_turn_mark();
        match classify(line) {
            Event::Init { session, model } => {
                self.ready = true;
                // An `init` closes the start-up *preamble* — the hook lines and
                // junk above it must not read as an open turn. It does not close
                // a turn whose journal mark it comes after: claude says `init`
                // only once it has read a first message, so an init past the
                // mark is the answer's first line, not evidence the turn ended.
                self.replay_open = self.replay_marked;
                self.canonicalize_model(model);
                if let Some(id) = session {
                    if id != self.id {
                        self.adopt(&id).await;
                    }
                }
            }
            Event::Result { .. } => {
                self.replay_open = false;
                self.replay_marked = false;
                // A turn cannot end with a question open — it blocks until it is
                // answered — so whatever the history had asked was settled
                // before this line, by whoever was listening then.
                self.replay_asked.clear();
                self.replay_permissions.clear();
                self.replay_declines.clear();
            }
            // Mid-turn by definition: claude stops to ask, and the turn waits.
            // Remembered rather than acted on, like everything in a replay —
            // whether it is still waiting is the rest of the history's to say,
            // and [`Driver::settle_questions`] acts on what is left.
            Event::Permission(request) => {
                self.replay_open = true;
                if request.is_question() {
                    self.replay_asked.push(OpenQuestion::of(&request));
                } else {
                    self.replay_permissions.push(OpenPermission::of(&request));
                }
            }
            Event::ControlCancel { request_id } => {
                self.replay_asked.retain(|question| question.request_id != request_id);
                self.replay_permissions.retain(|perm| perm.request_id != request_id);
                self.replay_declines.retain(|declined| *declined != request_id);
            }
            // Declined again at the boundary, like a refusal: a request nobody
            // answered is a CLI waiting for ever, and answering twice is free.
            Event::ControlRequest { request_id } => {
                self.replay_open = true;
                self.replay_declines.extend(request_id);
            }
            // claude echoes the result it built from an answer — or a refusal —
            // as a `user` line, which is how the history says the request it
            // answers is over.
            Event::Speech => {
                self.replay_open = true;
                if !self.replay_asked.is_empty() || !self.replay_permissions.is_empty() {
                    let answered = answered_in(line);
                    let settled = |tool_use: &Option<String>| tool_use.as_ref().is_some_and(|id| answered.contains(id));
                    self.replay_asked.retain(|question| !settled(&question.tool_use_id));
                    self.replay_permissions.retain(|perm| !settled(&perm.tool_use_id));
                }
            }
            Event::BackgroundTasks { tasks } => {
                self.background.snapshot(tasks);
            }
            Event::TaskStarted { task } => {
                self.background.observe(task);
            }
            // Reconstructed like the rest of the list, and — like the rest of the
            // background events — not work: a progress line is published while
            // the session sits idle, so counting it would leave every session
            // that ever ran a background task replaying as mid-turn.
            Event::TaskProgress { progress } => {
                self.background.progress(progress);
            }
            Event::TaskFinished { id } => {
                self.background.remove(&id);
            }
            // Not work, and this is the arm that says so. The CLI publishes
            // `system` lines while a session sits idle — `commands_changed` when
            // a hook edits the command list, `task_updated` for a background
            // task — and one of those after the last `result` used to leave the
            // whole replay reading as mid-turn. The live path has always refused
            // to open a turn on one; this is the replay agreeing.
            Event::Housekeeping => {}
            // Anything else claude wrote — an assistant message, a tool result,
            // a thinking-token count, a line that is not JSON — is work, and
            // work after the last `result` is a turn that had not ended.
            //
            // The background events above are pointedly *not* work: on 2.1.231
            // a task finishing while the session is idle writes
            // `task_updated` and `task_notification` with no turn around them,
            // and counting those would leave every session that ever ran a
            // background task looking permanently busy.
            //
            // [`Event::Speech`] is read the same way, in an arm of its own only
            // because it can also settle a question: a replay must reconstruct,
            // never open. The turn it would have opened live is the same turn
            // the `replay_open` below reconstructs at the boundary, and erring
            // towards "in flight" is the whole policy of this function — see
            // [`Driver::settle_replay`].
            //
            // A `rate_limit_event` is here because it was [`Event::Other`]
            // until the limit log and the quota readings wanted it told apart,
            // and the replay reads it exactly as it always did. It is neither
            // recorded nor filed: a replay reconstructs, a limit hit before
            // this server existed was recorded by the one that saw it or is
            // history, and a reading replayed now would be stamped with the
            // wrong time.
            Event::ControlResponse | Event::Other | Event::RateLimit { .. } | Event::UsageReading { .. } | Event::Unparseable => {
                self.replay_open = true
            }
        }
    }

    /// Take the journal's turn mark, once the replay has reached where it sits.
    ///
    /// Called before each replayed item, and compared against the offset *after*
    /// that item: an item ending at or before the mark was written before the
    /// prompt went in and says nothing about it, and one ending after it came
    /// back from claude in answer. So a `result` from an earlier turn cannot
    /// clear this turn, and a `result` from this one still does — which is the
    /// whole reason the journal records a place rather than a flag.
    pub(super) fn cross_turn_mark(&mut self) {
        let Some(at) = self.pending_turn else { return };
        if self.link.at() <= at {
            return;
        }
        self.pending_turn = None;
        self.replay_open = true;
        // Remembered past the conversion, because the turn's own `init` may
        // still be ahead: everything from here to the next `result` is the
        // marked turn, whatever shape its lines take.
        self.replay_marked = true;
    }

    /// Stop replaying once the history has run out, and act on what it said.
    ///
    /// The boundary is a byte offset and has to be exact: the shim says how far
    /// claude had written when the link was made, and the line that ends there
    /// is the last one that already happened. Deciding here rather than when the
    /// *next* line arrives is what makes a session that has been sitting quietly
    /// since the restart come back as anything at all — the next line may be
    /// hours away, or may never come.
    ///
    /// Erring towards "a turn is in flight" is deliberate. Reading a live turn
    /// as idle unblocks the composer and lets the next message be written into
    /// the middle of it — two prompts in one turn, which is the interleaving
    /// this whole mechanism exists to stop. Reading an idle session as working
    /// costs a run that shows as running until the next `result`, the process'
    /// own exit, or the user pressing Stop.
    pub(super) async fn settle_replay(&mut self) {
        if self.link.at() < self.link.total() {
            return;
        }
        self.replaying = false;
        self.heard = Instant::now();
        self.settle_background().await;
        // A mark still pending here sits past everything claude has written,
        // which is the plainest case there is: a prompt went in and the process
        // has not said a word since. It is also the one the stdout alone can
        // never show, and the reason this record exists.
        if self.pending_turn.take().is_some() {
            self.replay_open = true;
        }
        self.replay_marked = false;
        let mut open = std::mem::take(&mut self.replay_open);
        if std::mem::take(&mut self.replay_gapped) {
            self.recall_questions_lost_in_a_gap();
        }
        // A question the history leaves open is a turn in flight by definition:
        // claude is blocked on it, mid-turn, for as long as it takes. It is also
        // the one case the transcript must not be asked about, because the
        // transcript reads a tail ending on `AskUserQuestion` as a session
        // waiting for input — which is true, and would let go of the very turn
        // the question belongs to.
        let asking = !self.replay_asked.is_empty() || !self.replay_permissions.is_empty();
        open |= asking;
        // The reconstruction above is stdout's account of the session, and
        // stdout is not the only witness. Before a turn *nobody can see the
        // start of* is opened — and one opened here stays open for as long as
        // claude stays quiet, which for a resident that is between turns is for
        // ever — the transcript is asked whether it agrees. See
        // [`Driver::transcript_is_between_turns`] for why it is worth asking and
        // when it declines to answer.
        if open && !asking && self.transcript_is_between_turns().await {
            tracing::warn!(
                session = %self.id,
                "the replay ended mid-turn but the transcript says the session is between turns; not opening one"
            );
            self.recorder.lifecycle(
                Stage::Settle,
                Some("the replay read as mid-turn and the transcript disagreed; no turn opened"),
            );
            open = false;
        }
        if open {
            tracing::info!(session = %self.id, "the resident's replay ended mid-turn; assuming it is still working");
            self.turn = Some(Turn {
                // The run adoption published, where there is one: the turn the
                // replay just reconstructed *is* the one it was published for,
                // and opening a second would leave the first running for ever.
                run: self.launching_turn(),
                // Re-marked at the boundary rather than left where it was: from
                // here on everything replayed is history and the next `result`
                // is this turn's. A second restart therefore reconstructs the
                // same turn from the same evidence instead of re-reading the
                // work that opened it.
                at: self.link.at(),
                seq: self.seq,
                ladder: None,
                provisional: false,
                rotation_attempt: 0,
            });
            self.set_phase(Phase::Working);
        } else {
            tracing::info!(
                session = %self.id,
                background = self.background.len(),
                "the resident's replay ended between turns"
            );
            // Nothing was in flight and nothing is waiting, so the run adoption
            // published on the pessimistic side has to come back off: leaving it
            // would block the composer on a turn that ended before this server
            // started.
            if self.queue.is_empty() {
                self.withdraw_published_run();
            }
        }
        // Either way the record is restated: a turn the journal remembered and
        // the replay showed ending has to stop being remembered, or a session
        // that has been idle since the restart comes back busy after the next one.
        let at = self.turn.as_ref().map(|turn| turn.at);
        self.note_progress(at);
        self.settle_questions().await;
        self.publish();
        self.pump().await;
    }

    /// Take the record's word for the questions a gap in the history hid.
    ///
    /// A question asked inside the stretch the shim's ring dropped never reaches
    /// the replay, and a CLI blocked on it writes nothing more of its turn — so
    /// the history ends looking like a turn that is over, the transcript (whose
    /// tail is the `AskUserQuestion` call) agrees, and without this the question
    /// would be expired, no turn opened, and the next message written into a
    /// CLI that is waiting for an answer it would then never get. A question
    /// still `pending` on record was answered by nobody — every answer moves the
    /// record before it is written, and so does a stop — so here the record is
    /// the better witness: it is held open again, oldest first, ahead of
    /// whatever the history after the gap did show.
    fn recall_questions_lost_in_a_gap(&mut self) {
        let mut recalled: Vec<OpenQuestion> = self
            .runs
            .host.questions()
            .pending_for(&self.id)
            .into_iter()
            .filter(|stored| !self.replay_asked.iter().any(|question| question.request_id == stored.request_id))
            .map(|stored| OpenQuestion {
                request_id: stored.request_id,
                tool_use_id: stored.tool_use_id,
                questions: stored.questions,
            })
            .collect();
        if recalled.is_empty() {
            return;
        }
        tracing::warn!(
            session = %self.id,
            questions = recalled.len(),
            "the history lost a stretch to the shim's ring; holding the questions still pending on record open"
        );
        self.recorder.lifecycle(
            Stage::Settle,
            Some(&format!(
                "{} question(s) pending on record, unseen across a replay gap: held open",
                recalled.len()
            )),
        );
        recalled.append(&mut self.replay_asked);
        self.replay_asked = recalled;
    }

    /// Act on what the history left open: the questions claude is still
    /// blocked on, and the permission prompts nobody refused yet.
    ///
    /// The record and the history are two witnesses, and each knows something
    /// the other cannot. stdout says whether claude is *still waiting* — an
    /// answer it has taken is echoed as a result, a withdrawn request as a
    /// cancel. The record says what the user *did*, which never reaches stdout
    /// until claude acts on it. So:
    ///
    /// * a question waiting in both is held open again, as if this server had
    ///   heard it asked — without a second notification;
    /// * a question claude is waiting on that the record says was answered is
    ///   one whose answer never got written, because the last server died
    ///   between the compare-and-set and the write. The answer is written again:
    ///   claude ignores a response it has already had, so this is safe even when
    ///   the first one did get through and its echo is merely still on its way;
    /// * an expired one — expired by a boot that could not yet see the process
    ///   was alive — is taken back to pending, unless somebody already answered
    ///   it as a message, in which case claude is told so and its turn goes on;
    /// * a question the record has *cancelled* — a stop — and claude is still
    ///   waiting on is a stop whose interrupt never reached claude: the last
    ///   server died between moving the record and writing the line. The stop is
    ///   made again, so the turn ends as the user asked rather than carrying on
    ///   from a dismissal;
    /// * a question the record has pending and the history has not is over,
    ///   whatever ended it, and is expired — unless the history lost a stretch
    ///   to the shim's ring, which [`Driver::recall_questions_lost_in_a_gap`]
    ///   has already answered for.
    ///
    /// Refusals and declined requests are simply sent again. Doing it twice
    /// costs nothing, and a request left unanswered is a turn blocked for ever.
    pub(super) async fn settle_questions(&mut self) {
        for request_id in std::mem::take(&mut self.replay_declines) {
            self.decline_request(Some(request_id)).await;
        }
        let mut stop_again = false;
        let waiting = std::mem::take(&mut self.replay_asked);
        let store = self.runs.host.questions().clone();
        let now = questions::now_ms();
        for stale in store.pending_for(&self.id) {
            if !waiting.iter().any(|question| question.request_id == stale.request_id) {
                store.close(&stale.request_id, QuestionState::Expired, now);
            }
        }
        for question in waiting {
            let stored = store.get(&question.request_id);
            let state = stored.as_ref().map(|stored| stored.state);
            let reopened =
                state == Some(QuestionState::Expired) && store.rewind(&question.request_id, QuestionState::Expired, QuestionState::Pending, now);
            match (state, reopened) {
                (None, _) | (Some(QuestionState::Pending), _) | (Some(QuestionState::Expired), true) => {
                    tracing::info!(session = %self.id, request = %question.request_id, "claude is still waiting on a question it asked before the restart");
                    self.hold_question(question);
                }
                (Some(QuestionState::Cancelled), _) => {
                    tracing::info!(session = %self.id, request = %question.request_id, "a stop never reached claude before the restart; stopping the turn again");
                    stop_again = true;
                }
                _ => {
                    // Answered or dismissed while claude still waits — or
                    // expired and answered as a message in the moment between
                    // the two reads above. The stored response if there
                    // is one, and otherwise a refusal that says why.
                    let stored = store.get(&question.request_id);
                    let response = match stored.as_ref() {
                        Some(Stored {
                            delivery: Some(delivery), ..
                        }) => delivery.clone(),
                        Some(Stored {
                            state: QuestionState::Answered,
                            ..
                        }) => questions::deny(questions::ANSWERED_ELSEWHERE),
                        _ => questions::deny(questions::DISMISSED),
                    };
                    tracing::info!(session = %self.id, request = %question.request_id, "writing a question's settled answer again after the restart");
                    let line = questions::control_response(&question.request_id, &response);
                    self.recorder.lifecycle(
                        Stage::Question,
                        Some(&format!("{} was settled before the restart; telling claude again", question.request_id)),
                    );
                    self.recorder.stdin(&line);
                    if let Err(error) = self.link.send_line(&line).await {
                        tracing::warn!(session = %self.id, %error, "cannot write a settled answer to claude");
                    }
                }
            }
        }
        // Settle tool permission prompts the replay left open.
        let waiting_permissions = std::mem::take(&mut self.replay_permissions);
        let perm_store = self.runs.host.permissions().clone();
        for stale in perm_store.pending_for(&self.id) {
            if !waiting_permissions.iter().any(|perm| perm.request_id == stale.request_id) {
                perm_store.close(&stale.request_id, PermissionState::Cancelled, now);
            }
        }
        for perm in waiting_permissions {
            let stored = perm_store.get(&perm.request_id);
            match stored.as_ref().map(|s| s.state) {
                None | Some(PermissionState::Pending) => {
                    tracing::info!(session = %self.id, request = %perm.request_id, "claude is still waiting on a permission prompt from before the restart");
                    self.hold_permission(perm);
                }
                Some(PermissionState::Cancelled) => {
                    let response = stored
                        .as_ref()
                        .and_then(|s| s.delivery.clone())
                        .unwrap_or_else(|| permissions::claude_deny(&perm.tool_name, true));
                    let line = questions::control_response(&perm.request_id, &response);
                    self.recorder.stdin(&line);
                    if let Err(error) = self.link.send_line(&line).await {
                        tracing::warn!(session = %self.id, %error, "cannot write a cancelled permission response to claude");
                    }
                }
                Some(PermissionState::Resolved) => {
                    let response = match stored.as_ref() {
                        Some(StoredPermission {
                            delivery: Some(delivery), ..
                        }) => delivery.clone(),
                        _ => permissions::claude_deny(&perm.tool_name, true),
                    };
                    let line = questions::control_response(&perm.request_id, &response);
                    self.recorder.stdin(&line);
                    if let Err(error) = self.link.send_line(&line).await {
                        tracing::warn!(session = %self.id, %error, "cannot write a settled permission response to claude");
                    }
                }
            }
        }
        if stop_again {
            self.recorder.lifecycle(
                Stage::Interrupt,
                Some("a question was stopped before the restart and claude is still waiting on it"),
            );
            self.interrupt().await;
        }
        self.announce();
    }

    /// Whether claude's own transcript says this session is *not* mid-turn.
    ///
    /// The independent witness, and the answer to the one question this driver
    /// cannot ask the stream. Everything a re-attached driver knows about an
    /// inherited turn is inferred from stdout, and stdout cannot say what it
    /// never wrote: a `system` line published while the session sat idle, a
    /// replay gap, or a turn mark an earlier boot left behind all read as "still
    /// working", and nothing in the stream ever contradicts them — the `result`
    /// that would have said otherwise belongs to a turn that ended before this
    /// server existed. A turn reconstructed from one of those never ends, and
    /// the queue behind it never moves. That is the ghost this asks about.
    ///
    /// The transcript is written by claude itself and is already trusted for
    /// exactly this question elsewhere: [`ClaudeCodeRuns::was_cut_short`]
    /// reads it to decide whether a run that outlived its server was cut short.
    /// Reusing [`crate::sessions::claude::claude_activity_status`] keeps one
    /// definition of mid-turn rather than two that can drift — a tail ending on
    /// the assistant's answer is a turn that finished, and one ending on a
    /// `tool_use` or a person speaking is a turn that did not.
    ///
    /// Three ways it declines to answer, and every one of them errs the same
    /// way — towards "a turn is in flight" — because unblocking the composer on
    /// a live turn is the expensive mistake and a ghost is the cheap one:
    ///
    /// * **no transcript at all**: a session whose first turn has yet to write a
    ///   line, which is precisely a turn in flight;
    /// * **written within [`TRANSCRIPT_SETTLES_AFTER`]**: the file may be in the
    ///   middle of recording the very prompt being asked about;
    /// * **a scan that runs out of budget**, which `claude_activity_status`
    ///   already reports as `Running` for the same reason.
    pub(super) async fn transcript_is_between_turns(&self) -> bool {
        let Some(path) = crate::sessions::claude::find_claude_session_file(&self.runs.host.paths, &self.id).await else {
            return false;
        };
        let settled = tokio::fs::metadata(&path)
            .await
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age >= TRANSCRIPT_SETTLES_AFTER);
        if !settled {
            return false;
        }
        crate::sessions::claude::claude_activity_status(&path).await == SessionStatus::Waiting
    }

    /// Let go of a turn claude's transcript says is not happening.
    ///
    /// Withdrawn rather than failed, for the reason
    /// [`Driver::withdraw_published_run`] gives: nothing ran, so there is
    /// nothing to report, and a failure raised on every restart would be noise
    /// about something that went fine. What matters is the queue — parked on
    /// this turn since the attach, and released by the pump at the end.
    pub(super) async fn discard_ghost_turn(&mut self) {
        let Some(turn) = self.turn.take() else { return };
        tracing::warn!(
            session = %self.id,
            quiet_for = ?self.heard.elapsed(),
            "nothing has been heard for a long time and the transcript says this session is between turns; \
             letting go of a turn that is not happening"
        );
        self.recorder.lifecycle(
            Stage::Settle,
            Some("no output and a transcript between turns; the reconstructed turn was let go"),
        );
        // Only if it is still the run this turn was published for: `pump` below
        // may want it back, and a run some other path has since put there is not
        // this driver's to take away.
        if self.published_run().is_some_and(|published| Arc::ptr_eq(&published, &turn.run)) {
            self.withdraw_published_run();
        }
        // The mark goes with it, or the next boot reconstructs the same ghost
        // from the same record — which is how one of these survives a restart.
        self.note_progress(None);
        self.set_phase(Phase::Idle);
        self.publish();
        self.pump().await;
    }
}
