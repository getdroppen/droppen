//! Command handling, message queue delivery, steer absorption, and process respawns.

use super::*;

/// How far the driver has read the session's transcript for steered messages
/// claude took into the turn in flight. See [`Driver::absorb`].
#[derive(Debug, Default)]
pub(super) struct AbsorbWatch {
    /// Where reading starts, or `None` while no steered message is outstanding.
    pub(super) anchor: Option<AbsorbAnchor>,
    /// Whether a poll is already on its way, so a steer does not start a second.
    pub(super) armed: bool,
    /// Whether something was taken out of the queue since the session's feed
    /// was last poked about it. See [`Driver::on_absorb`].
    pub(super) unannounced: bool,
}

#[derive(Debug)]
pub(super) struct AbsorbAnchor {
    /// The session id the transcript was looked up under. An `init` that moves
    /// the session — a fork — moves the file, and a fork's file opens with a
    /// copy of the history, folds and all: so the anchor is dropped and taken
    /// again at the new file's end rather than read from its start.
    pub(super) session: String,
    /// The transcript, once it exists.
    pub(super) file: Option<PathBuf>,
    /// The byte offset everything before which has been read, or predates the
    /// first steer it is watching for.
    pub(super) offset: u64,
}

/// What a fold read out of the transcript could be: the prompt as it went in,
/// and as the CLI wrote it back.
///
/// Whitespace at the ends and line endings are the only differences allowed:
/// the CLI records the very text it was handed, and anything looser would let a
/// short message — "继续" — match a fold of some other message.
pub(super) fn same_prompt(written: &str, recorded: &str) -> bool {
    let normal = |text: &str| text.trim().replace("\r\n", "\n");
    normal(written) == normal(recorded)
}

/// The whole lines appended to `path` past `offset`, and the offset after them.
///
/// A line still being written is left for the next read, and a file that is
/// now shorter than `offset` — replaced, not appended to — is read from its new
/// end on: whatever it says now cannot be told apart from what was read before.
/// `needle` filters lines before they are parsed, so megabytes of tool output
/// cost a byte scan and not a JSON parse.
pub(super) async fn appended_lines(path: &Path, offset: u64, needle: &[u8]) -> (Vec<Value>, u64) {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let Ok(mut file) = tokio::fs::File::open(path).await else {
        return (Vec::new(), offset);
    };
    let Ok(len) = file.metadata().await.map(|meta| meta.len()) else {
        return (Vec::new(), offset);
    };
    if len < offset {
        return (Vec::new(), len);
    }
    if len == offset || file.seek(std::io::SeekFrom::Start(offset)).await.is_err() {
        return (Vec::new(), offset);
    }
    let mut bytes = Vec::with_capacity((len - offset) as usize);
    if (&mut file).take(len - offset).read_to_end(&mut bytes).await.is_err() {
        return (Vec::new(), offset);
    }
    let Some(end) = memchr::memrchr(b'\n', &bytes) else {
        return (Vec::new(), offset);
    };
    let finder = memchr::memmem::Finder::new(needle);
    let lines = bytes[..end]
        .split(|byte| *byte == b'\n')
        .filter(|line| finder.find(line).is_some())
        .filter_map(|line| serde_json::from_slice::<Value>(line).ok())
        .collect();
    (lines, offset + end as u64 + 1)
}

/// The most recent write anywhere in a directory: its own mtime, and its
/// entries', whichever is newest. `None` when the directory is not there.
///
/// The entries are the whole point, and reading the directory alone was a bug.
/// A directory's mtime moves when an entry is created, renamed or removed — and
/// never when one is *written to*. Claude creates a background agent's
/// transcript when the agent starts and then streams into it for as long as the
/// agent runs, so a directory whose last agent was dispatched a minute ago looks
/// untouched while every agent in it is still working. Measured on a live
/// session with two running background agents: the directory 133 seconds stale,
/// the transcripts inside it written that same second — a probe reading only the
/// directory calls that session quiet, and drops the tasks that prove it is not.
///
/// One shallow pass, no recursion: both directories this is asked about hold
/// files, not trees.
pub(super) async fn newest_write(dir: &Path) -> Option<SystemTime> {
    let mut newest = tokio::fs::metadata(dir).await.ok()?.modified().ok();
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return newest;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Ok(at) = entry.metadata().await.and_then(|meta| meta.modified()) else {
            continue;
        };
        if newest.is_none_or(|seen| at > seen) {
            newest = Some(at);
        }
    }
    newest
}

impl Driver {
    // ------------------------------------------------------------ commands

    pub(super) async fn on_command(&mut self, command: Cmd) {
        match command {
            Cmd::Deliver { pending, recorded } => {
                if let Some(hold) = self.take_hold() {
                    self.release_hold(hold, Some(&pending), "a new message was sent");
                }
                self.queue.push_back(*pending);
                self.publish();
                // Recorded before it is pumped, so a message that ends up
                // waiting behind a turn is on disk from the moment it arrives
                // rather than from whenever the turn in front of it ends. The
                // pump rewrites it a moment later if it goes straight in.
                self.note_progress(self.turn.as_ref().map(|turn| turn.at));
                // And only now may the send be answered: the commit above has
                // returned, so a server killed after the `200` leaves this
                // message where the next one reads. Before the pump, because a
                // pump that has to respawn for it can take seconds, and the
                // message is already safe.
                let _ = recorded.send(());
                self.pump().await;
            }
            Cmd::Drop { qid, reply } => {
                let outcome = self.drop_queued(&qid).await;
                let _ = reply.send(outcome);
            }
            Cmd::Steer { qid, reply } => {
                let outcome = self.steer(&qid).await;
                let _ = reply.send(outcome);
            }
            Cmd::Interrupt => self.interrupt().await,
            Cmd::StopTask { task_id } => self.stop_task(&task_id).await,
            Cmd::Escalate { turn, rung } => self.escalate(turn, rung),
            Cmd::Settle { turn } => self.settle_delivered(turn).await,
            Cmd::Absorb { nudged } => self.on_absorb(nudged).await,
            Cmd::Recheck => self.recheck_hold().await,
            Cmd::Answer {
                request_id,
                reply,
                by,
                respond,
            } => {
                let outcome = self.answer(&request_id, reply, by).await;
                let _ = respond.send(outcome);
            }
            Cmd::RespondPermission {
                request_id,
                reply,
                by,
                respond,
            } => {
                let outcome = self.respond_permission(&request_id, reply, by).await;
                let _ = respond.send(outcome);
            }
        }
    }

    /// Take a message out of the queue before it is written anywhere.
    ///
    /// Only ever the queue: a message that has been steered is in claude's
    /// stdin and there is no taking it back, and one whose turn has started is
    /// not this driver's to cancel — that is what Stop is for.
    async fn drop_queued(&mut self, qid: &str) -> QueueOutcome {
        if self.delivered_ahead.iter().any(|pending| pending.id == qid) {
            return QueueOutcome::AlreadySteered;
        }
        let Some(at) = self.queue.iter().position(|pending| pending.id == qid) else {
            return QueueOutcome::Unknown;
        };
        let dropped = self.queue.remove(at).expect("the index came from this queue");
        tracing::info!(session = %self.id, qid, text = %clip(&dropped.text), "a queued message was withdrawn before it went in");
        self.receipt(&dropped, ReceiptStatus::Failed);
        self.publish();
        self.note_progress(self.turn.as_ref().map(|turn| turn.at));
        // The head of the queue is what a settings-change park waits for, so
        // removing it has to let the driver ask the question again: otherwise a
        // session sits `awaitingBackground` for a message that is gone.
        self.pump().await;
        QueueOutcome::Done
    }

    /// Write a queued message to claude now, ahead of the turn in flight.
    ///
    /// The point of the whole feature: a user watching an agent go the wrong way
    /// wants the correction *in this turn's context*, not in twenty minutes when
    /// the turn ends. claude buffers a `user` line written mid-turn and runs it
    /// as the next turn, so this is a delivery, not an interruption — nothing in
    /// flight is disturbed, and the message runs before anything still queued.
    ///
    /// A message that wants different settings is not written anywhere: it
    /// needs a *different process*, and there is no stdin to write it to that
    /// would mean what the caller asked for. From idle the steer becomes that
    /// restart instead of a refusal — see [`Driver::restart_for_steer`].
    async fn steer(&mut self, qid: &str) -> QueueOutcome {
        if self.delivered_ahead.iter().any(|pending| pending.id == qid) {
            return QueueOutcome::AlreadySteered;
        }
        let Some(at) = self.queue.iter().position(|pending| pending.id == qid) else {
            return QueueOutcome::Unknown;
        };
        // "Now" overrides a queue held after a limit, as it overrides a park.
        if let Some(hold) = self.take_hold() {
            self.release_hold(hold, None, "a held message was steered");
        }
        if !self.queue[at].fingerprint.accepted_by(self.live.as_ref()) {
            return self.restart_for_steer(at).await;
        }
        // Before the write, so the fold this message may become cannot land in
        // the transcript ahead of where it is read from: see `absorb`. Taken
        // afresh when nothing else is outstanding, so a fold from an earlier
        // life of the same words — a message requeued off a dead process and
        // steered again — is behind the anchor; kept when something is, since
        // the older anchor is what still covers that.
        if self.delivered_ahead.is_empty() || !self.absorb_anchored() {
            self.anchor_absorb().await;
        }
        let pending = self.queue.remove(at).expect("the index came from this queue");
        let (message, delivery) = match self.user_line_for(&pending).await {
            Ok(prepared) => prepared,
            Err(why) => {
                self.receipt(&pending, ReceiptStatus::Failed);
                self.fail_message(&why);
                return QueueOutcome::Done;
            }
        };
        self.recorder.lifecycle(
            Stage::Steer,
            Some(&format!("{qid} was handed to claude ahead of the queue: {}", clip(&pending.text))),
        );
        self.recorder.stdin(&message);
        if let Err(error) = self.link.send_line(&message).await {
            // Nothing was written, so nothing was delivered: the message goes
            // back exactly where it was and the caller is told to try again.
            // The recv loop is about to see the same broken link and recover,
            // which is what will eventually deliver it.
            tracing::warn!(session = %self.id, qid, %error, "cannot steer a message into claude");
            self.queue.insert(at, pending);
            return QueueOutcome::Unreachable;
        }
        tracing::info!(session = %self.id, qid, "a queued message was steered into the turn in flight");
        // Only after the line went in. A failed steer returns above and leaves
        // no row; the retry replaces the row under the same queue id.
        if let Some(delivery) = &delivery {
            self.record_delivery(delivery);
        }
        // In claude's stdin is handed over, whatever turn it ends up running in.
        self.receipt(&pending, ReceiptStatus::Sent);
        // The same reason [`Driver::pump`] takes one: a message that fits the
        // running process describes a process equivalent to it, which is what a
        // re-attached driver needs before it can respawn anything.
        //
        // The *fingerprint* is pointedly not taken here, and the asymmetry is
        // the whole point of a steer: this message jumps the queue. Everything
        // the pump's take-once could re-judge is behind the message that healed
        // an unknown live and brings a recipe of its own; a steer is the one
        // thing that puts a message in front of messages inherited under an
        // unknown fingerprint, and narrowing live from it would fail them for
        // settings nobody ever compared them against. The recipe is only ever
        // added information, so it is safe on any order.
        if self.recipe.is_none() {
            self.recipe = pending.recipe.clone();
        }
        self.delivered_ahead.push_back(pending);
        self.arm_absorb();
        self.publish();
        self.note_progress(self.turn.as_ref().map(|turn| turn.at));
        // A steer into an *idle* resident — the queue was parked behind a
        // settings change, or the process is still starting — has nothing to
        // wait for, so its turn opens here rather than at the next `result`.
        self.pump().await;
        QueueOutcome::Done
    }

    /// Steer a message the running process cannot answer: restart for it now.
    ///
    /// The button this is behind means *get on with it*, and the case it exists
    /// for is a queue parked behind background work. A settings change waits at
    /// the head of the queue while claude minds a task — see [`Driver::pump`] —
    /// and that wait is bounded by `await_background`, which is thirty minutes.
    /// A long-lived background shell holds the whole of it.
    ///
    /// Every settings change used to be refused here, which left that user with
    /// no way out at all: the driver is `Idle` while it is parked, so Stop
    /// refuses it; the message cannot be delivered, because it does not fit the
    /// process; and the only remaining moves were to wait out the ceiling or
    /// throw the message away. Pressing the one button that says "now" is the
    /// clearest possible statement that the background work is worth less than
    /// the message, so from idle it is taken at its word and the restart
    /// happens — the confirmation the UI asks for first is where that choice is
    /// actually made.
    ///
    /// Two cases still cannot be served, and both keep the old answer:
    ///
    /// * a **turn in flight**, because a process is only ever replaced from
    ///   idle: doing it here would kill the turn the user is watching, which is
    ///   not what they asked for. It becomes possible the moment the turn ends,
    ///   by itself.
    /// * a message with **no recipe**, read back from a journal that cannot
    ///   hold the environment it needs. There is nothing to spawn from, and the
    ///   pump fails it out loud when it reaches the head.
    ///
    /// Both carry what differs, because a refusal that does not name the
    /// setting in the way is a mystery rather than a message.
    async fn restart_for_steer(&mut self, at: usize) -> QueueOutcome {
        // `live` is `Some` by construction: an unknown fingerprint accepts every
        // message, so nothing reaches here without two of them to compare.
        let differences = self
            .live
            .as_ref()
            .map(|live| self.queue[at].fingerprint.differences(live))
            .unwrap_or_else(|| "different settings".to_string());
        if self.turn.is_some() {
            return QueueOutcome::NeedsRestart { differences };
        }
        let Some(wanted) = self.queue[at].recipe.clone() else {
            return QueueOutcome::NeedsRestart { differences };
        };
        let pending = self.queue.remove(at).expect("the index came from this queue");
        tracing::info!(
            session = %self.id,
            qid = %pending.id,
            tasks = self.background.len(),
            %differences,
            "a queued message that needs a different process was steered; restarting for it now"
        );
        self.recorder.lifecycle(
            Stage::Steer,
            Some(&format!(
                "{} was steered and needs {differences}: restarting this session for it now, ahead of {} background task(s)",
                pending.id,
                self.background.len()
            )),
        );
        if !self.respawn_for(pending, &wanted).await {
            // Nothing started. `fatal` is set, the link is closed, and the
            // driver's own loop is winding the session up — the message is at
            // the head of a queue that goes with it, which is exactly what
            // `Unreachable` tells the caller.
            return QueueOutcome::Unreachable;
        }
        self.pump().await;
        QueueOutcome::Done
    }

    /// Ask claude to end the turn, and arm the ladder in case it will not.
    ///
    /// # What Stop does not stop
    ///
    /// Only the turn in flight. A message that was *steered* is already in
    /// claude's stdin, and nothing in this server can reach into another
    /// process' buffer to take it out: the CLI will run it as the next turn,
    /// interrupt or no interrupt. That is the price of steering being a
    /// delivery rather than an interruption, and it is why the row keeps
    /// showing those messages as outstanding — a user who stops a turn and sees
    /// another one start is watching something they asked for.
    ///
    /// Pressing Stop again ends that turn too, which is the way out.
    ///
    /// A Stop pressed during the launch window skips the polite rung entirely.
    /// The request is a line of stdin, and nothing reads a resident's stdin
    /// before `system:init` — so sending it would be asking a process that is
    /// not listening, and then waiting the full rung for an answer that cannot
    /// come. The ladder is armed instead, which gives the launch one step's
    /// grace to finish on its own before the first signal.
    pub(super) async fn interrupt(&mut self) {
        if self.turn.is_none() {
            if self.ready {
                return; // idle between turns: there is nothing to stop
            }
            tracing::info!(session = %self.id, "stop pressed before claude said it was listening; arming the signal ladder");
            self.recorder
                .lifecycle(Stage::Interrupt, Some("stop pressed before claude said it was listening"));
            let run = self.launching_turn();
            let seq = self.seq;
            run.state.lock().expect("run state poisoned").interrupted = true;
            // No prompt has been written, so there is no offset to mark: this
            // turn exists to be stopped, not to be reconstructed after a restart.
            self.turn = Some(Turn {
                run,
                at: self.link.at(),
                seq,
                ladder: Some(0),
                provisional: false,
                rotation_attempt: 0,
            });
            self.set_phase(Phase::Working);
            self.arm(seq, 1);
            return;
        }
        // A stop takes back the turn, and the question the turn is blocked on
        // with it: claude withdraws it once the interrupt lands, and an answer
        // from a second screen in the meantime must not go in after the stop.
        self.close_questions(QuestionState::Cancelled, "the turn was stopped");
        let Some(turn) = &mut self.turn else { return };
        if turn.ladder.is_some() {
            return; // already asked; the rungs are already running
        }
        turn.ladder = Some(0);
        turn.run.state.lock().expect("run state poisoned").interrupted = true;
        let seq = turn.seq;
        let request = StdinLine::ControlRequest {
            request_id: format!("int-{}", new_uuid()),
            request: ControlRequest::Interrupt,
        }
        .line();
        self.recorder.lifecycle(Stage::Interrupt, Some("asking claude to end the turn"));
        self.recorder.stdin(&request);
        // Arm the signal ladder before awaiting the stdin write so a wedged
        // process whose pipe is full still escalates on schedule.
        self.arm(seq, 1);
        if let Err(error) = self.link.send_line(&request).await {
            tracing::warn!(session = %self.id, %error, "cannot ask claude to stop; the ladder will have to do it");
        }
    }

    /// Ask claude to stop one background task, and nothing else.
    ///
    /// The CLI's own `stop_task` control request, which is a different verb from
    /// [`Driver::interrupt`] in every way that matters: it names a task, it does
    /// not touch the turn in flight, and it has no ladder behind it. There is
    /// nothing to escalate *to* — a signal reaches the process group, which is
    /// the CLI and every other task it is minding, so the only honest failure
    /// mode for a request about one child is to have not worked.
    ///
    /// Fire-and-forget on purpose. The answer to "did it stop" is not the
    /// `control_response` — that says the request was accepted, and claude
    /// answers success even for a task it cannot find — it is the
    /// `background_tasks_changed` snapshot that arrives a moment later without
    /// the task in it. That snapshot is the only thing this server ever believed
    /// about which tasks are alive, so a stop that did not take shows up as a
    /// row that is simply still there, which is the truth.
    ///
    /// Nothing reads a resident's stdin before `system:init`, so a request sent
    /// during the launch window would be dropped on the floor. A background task
    /// cannot exist before the process that published it is up, so this is a
    /// guard against races rather than a case with a user behind it.
    async fn stop_task(&mut self, task_id: &str) {
        if !self.ready {
            tracing::info!(session = %self.id, task = %task_id, "cannot stop a task before claude says it is listening");
            return;
        }
        let request = StdinLine::ControlRequest {
            request_id: format!("stop-{}", new_uuid()),
            request: ControlRequest::StopTask { task_id },
        }
        .line();
        self.recorder
            .lifecycle(Stage::Interrupt, Some("asking claude to stop one background task"));
        self.recorder.stdin(&request);
        if let Err(error) = self.link.send_line(&request).await {
            tracing::warn!(session = %self.id, task = %task_id, %error, "cannot ask claude to stop that task");
        }
    }

    /// Signal claude's process group, one rung harder than last time.
    ///
    /// Reached only when the polite request produced no `result`, which for a
    /// healthy CLI it always does — so every rung below is a report that
    /// something is wedged, and is logged as one.
    fn escalate(&mut self, turn: u64, rung: u8) {
        // A rung that comes due after its turn has ended is exactly what
        // success looks like: the request worked and the timer lost the race.
        if self.turn.as_ref().map(|turn| turn.seq) != Some(turn) {
            return;
        }
        let (name, signal) = match rung {
            1 => ("SIGINT", libc::SIGINT),
            2 => ("SIGTERM", libc::SIGTERM),
            _ => ("SIGKILL", libc::SIGKILL),
        };
        tracing::warn!(session = %self.id, rung, signal = name, "claude did not stop when asked; escalating");
        self.recorder.lifecycle_with(Stage::Escalate, Some(rung), None, Some(name));
        if let Some(turn) = &mut self.turn {
            turn.ladder = Some(rung);
        }
        self.link.kill_claude(signal);
        if rung < 3 {
            self.arm(turn, rung + 1);
        }
    }

    /// Schedule the next rung, without holding the driver up for it.
    fn arm(&self, turn: u64, rung: u8) {
        let tx = self.tx.clone();
        let step = self.runs.host.interrupt_step;
        tokio::spawn(async move {
            tokio::time::sleep(step).await;
            let _ = tx.send(Cmd::Escalate { turn, rung });
        });
    }

    // --------------------------------------------------------------- queue

    /// Start the next message, if the process is free and fits it.
    ///
    /// One message per call, which is what the state machine says: the turn
    /// this opens ends with a `result`, and that is what calls this again.
    pub(super) async fn pump(&mut self) {
        // Deliberately not gated on `ready`: stdin is buffered, so a message
        // written before `init` is simply the first thing claude reads — and
        // waiting for `init` to write it deadlocks a fresh session outright,
        // because claude 2.1.231 given `--session-id` under stream-json does
        // not say `init` until it has read a first message. (Its hooks run and
        // report; the init line just never comes.) remote-hand mints the id,
        // so delivery must not wait for a line that is itself waiting for
        // delivery.
        // A question open is a turn in flight, whatever `turn` says: claude is
        // blocked mid-turn, and the respawn a queued settings change could
        // bring would end it — and take the question with it.
        if self.turn.is_some() || self.replaying || self.awaiting_user() {
            return;
        }
        // Steered messages first, and not as a policy choice: they are already
        // in claude's stdin, so the CLI will answer them before anything
        // written afterwards whatever this driver believes. Reading them in a
        // different order would only make the bookkeeping disagree with the
        // process.
        //
        // Read for folds once more first: a message claude took into the turn
        // that just ended has no turn of its own coming, and the poll may not
        // have come round since it landed. See `absorb`.
        if !self.delivered_ahead.is_empty() {
            self.absorb().await;
        }
        if let Some(delivered) = self.delivered_ahead.pop_front() {
            self.set_awaiting(false);
            // Published before the phase moves, exactly as the ordinary path
            // below does it: a reader that caught the two in the other order
            // would see a session `working` on a message it was still counting
            // as queued, and report the queue as one longer than it is.
            self.publish();
            self.start_delivered_turn(&delivered);
            return;
        }
        // A limit with nowhere to go: the account that refused gets nothing
        // more until its limit lifts or somebody acts. Checked after the
        // steered messages, because those are in claude's stdin already and
        // run whatever this driver decides.
        if self.hold.is_some() {
            self.set_awaiting(false);
            self.set_phase(Phase::Idle);
            return;
        }
        let Some(mut next) = self.queue.pop_front() else {
            self.set_awaiting(false);
            self.set_phase(Phase::Idle);
            return;
        };
        // The other quiet point: an idle session about to start a turn. A
        // reading another session made since may have taken this account to the
        // edge; a move now retargets this message too, and sends it down the
        // respawn path below instead of into the account that is running out.
        if self.runs.rotate_ahead.is_some() && next.fingerprint.accepted_by(self.live.as_ref()) {
            self.queue.push_front(next);
            self.rotate_ahead().await;
            let Some(again) = self.queue.pop_front() else { return };
            next = again;
        }
        if !next.fingerprint.accepted_by(self.live.as_ref()) {
            // A journalled message for the account the session is bound to now
            // gets a recipe for it; see [`Driver::recipe_for_binding`].
            if next.recipe.is_none() {
                if let Some(recipe) = self.recipe_for_binding(&next).await {
                    next.fingerprint = recipe.fingerprint.clone();
                    next.recipe = Some(recipe);
                }
            }
            let Some(wanted) = next.recipe.clone() else {
                // Reachable only where the process has been *replaced* under a
                // message the journal kept: adoption keeps one because it fitted
                // what the record said, or because the record said nothing, and
                // a recovery respawn can then bring back a process neither
                // description covers. Such a message can be neither delivered
                // nor spawned for, and would park the queue for ever, so it is
                // failed out loud instead.
                //
                // Out loud, and *specific*: the environment this message would
                // need carries an account token and was never journalled, so
                // there is nothing to rebuild a process from and nothing the
                // driver can do about it. What it can do is say which setting
                // is in the way, so the answer — send it again — is obvious
                // rather than a mystery about a server restart. `live` is known
                // here by construction: an unknown one accepts every message.
                let differs = self
                    .live
                    .as_ref()
                    .map(|live| next.fingerprint.differences(live))
                    .unwrap_or_else(|| "different settings".to_string());
                tracing::error!(session = %self.id, %differs, "a re-delivered message no longer fits the process running this session");
                self.receipt(&next, ReceiptStatus::Failed);
                self.fail_message(&format!(
                    "this message needs {differs}, which means a new process, and the settings to start one \
                     did not survive the server restart — send it again"
                ));
                self.publish();
                Box::pin(self.pump()).await;
                return;
            };
            // A settings change is a *restart*, and a restart kills whatever the
            // process was minding in the background — the exact thing residents
            // exist to stop happening. So the message waits at the head of the
            // queue instead, and the session says why: the alternative is
            // silently trading somebody's half-hour background job for a model
            // switch they could have made a minute later.
            if self.minding_background().await && !self.waited_long_enough() {
                tracing::info!(
                    session = %self.id,
                    tasks = self.background.len(),
                    "a settings change is waiting for claude's background work to finish"
                );
                self.queue.push_front(next);
                self.set_awaiting(true);
                self.set_phase(Phase::Idle);
                self.publish();
                return;
            }
            // A rotation's continuation that waited for background work, and
            // is going now with some of it still running, says what it cut.
            let next = self.account_for_interruptions(next).await;
            // The replacement says nothing until it reads something — the same
            // CLI behaviour the gate above documents — so the head of the queue
            // goes straight in rather than waiting on its `init`.
            if self.respawn_for(next, &wanted).await {
                Box::pin(self.pump()).await;
            }
            return;
        }
        self.take_settings(&next);
        self.set_awaiting(false);
        self.publish();
        self.start_turn(next).await;
    }

    /// Take what the message going in says about the process it is going into.
    ///
    /// Two things a driver can be missing, both taken **once** and only from a
    /// message that fits the running process — which is to say from a message
    /// describing a process equivalent to it:
    ///
    /// * the **recipe**, which is what a re-attached driver has no other way to
    ///   get: the journal remembers the settings but cannot hold an environment
    ///   (it carries the account's token), so until a message brings one the
    ///   recovery ladder has nothing to spawn;
    /// * the **fingerprint**, when the record this driver adopted predates the
    ///   field. Unknown accepted this message rather than judging it, and the
    ///   message is now the best evidence there is about what the process will
    ///   answer to — so it becomes the answer, and every message after it is
    ///   compared properly again.
    ///
    /// Both from the same message, deliberately: taken apart they could describe
    /// two different processes, and the pair is what a respawn re-creates.
    ///
    /// The heal is written to disk immediately. An unknown that stayed in the
    /// record would come back unknown on the next restart, and the next server
    /// would have to guess all over again — which is the whole failure this is
    /// here to end.
    fn take_settings(&mut self, pending: &Pending) {
        // A message read back out of the journal has neither, so there is
        // nothing to take: it is delivered on the strength of the vetting the
        // *previous* server did, and says nothing this one did not already know.
        let Some(recipe) = pending.recipe.as_ref() else { return };
        if self.recipe.is_none() {
            self.recipe = Some(recipe.clone());
        }
        if self.live.is_some() {
            return;
        }
        tracing::info!(
            session = %self.id,
            "adopted a resident whose settings were never recorded; taking them from the message going into it"
        );
        // `Settle` because this is the attach still working out what it
        // inherited, a message later than the replay: the record named no
        // settings, and this is the line that says which ones it now has.
        self.recorder.lifecycle(
            Stage::Settle,
            Some("the record of this resident named no settings; taking the ones of the message going into it"),
        );
        self.live = Some(pending.fingerprint.clone());
        self.note_fingerprint();
    }

    /// Whether claude is still looking after work a turn left running.
    ///
    /// Two signals, and the first is the real one. `system:background_tasks_changed`
    /// carries claude's whole live task list every time it changes (2.1.231), so
    /// the set is authoritative whenever it has been seen.
    ///
    /// The second is a mtime on the subagent transcripts, and it is here for the
    /// case the first cannot cover: the shim's replay ring can overflow while no
    /// server is attached, and a `task_started` lost in that gap would leave the
    /// set saying "nothing running" about an agent that very much is. Reading a
    /// directory per settings change is cheap, and the window is short enough
    /// that the false positives it also produces — a turn that ran a *foreground*
    /// subagent looks the same — cost seconds rather than minutes.
    pub(super) async fn minding_background(&self) -> bool {
        !self.background.is_empty() || self.subagents_recently_active().await
    }

    /// Whether anything claude's background work writes to was written to just now.
    ///
    /// The corroborating half of [`Driver::minding_background`], and the only
    /// evidence available about work whose events did not survive. Deliberately
    /// short-windowed, because it also fires for a *foreground* subagent and
    /// every second of the window is a second a settings change waits for
    /// nothing.
    ///
    /// Two places, because background work comes in two shapes and neither one
    /// covers the other. A background **Agent** streams a transcript into the
    /// session's `subagents/` directory; a background **Bash** writes no
    /// transcript at all, only its `<task-id>.output` under
    /// [`ClaudeCodeRuns::task_root`]. A replayed set of the second kind has
    /// no witness in the first place, so both are read and the newest wins.
    async fn subagents_recently_active(&self) -> bool {
        let Some(file) = crate::sessions::claude::find_claude_session_file(&self.runs.host.paths, &self.id).await else {
            return false;
        };
        let mut newest = newest_write(&crate::sessions::transcript::subagent_dir(&file)).await;
        if let Some(tasks) = self.task_output_dir().await {
            // `Option<SystemTime>` orders `None` below every `Some`, so this is
            // "the newer of the two, or whichever one exists".
            newest = newest.max(newest_write(&tasks).await);
        }
        newest
            .and_then(|at| SystemTime::now().duration_since(at).ok())
            .is_some_and(|quiet| quiet < SUBAGENT_QUIET)
    }

    /// Where this session's background tasks write their output, if it exists yet.
    ///
    /// Found rather than built: the directory sits under a slug of the working
    /// directory, and that slug mangles separators and non-ASCII in ways not
    /// worth reproducing from this side. The session id below it is unique, so
    /// one shallow scan of the root finds it without knowing the encoding.
    ///
    /// Not cached, because the answer changes from `None` to a path the first
    /// time the session runs a background task, and a cache primed before that
    /// would be wrong for the life of the process. The scan is a `read_dir` of a
    /// directory with one entry per project and a `stat` each, and it only runs
    /// on the rare paths — a replay boundary, a parked settings change, an idle
    /// sweep — never per line.
    pub(super) async fn task_output_dir(&self) -> Option<PathBuf> {
        let mut entries = tokio::fs::read_dir(&self.runs.task_root).await.ok()?;
        while let Ok(Some(entry)) = entries.next_entry().await {
            let candidate = entry.path().join(&self.id).join("tasks");
            if tokio::fs::metadata(&candidate).await.is_ok() {
                return Some(candidate);
            }
        }
        None
    }

    /// Throw away background tasks the replay invented.
    ///
    /// A replayed `task_started` whose `task_finished` fell off the end of the
    /// shim's ring — or was written while no server was attached, or belonged
    /// to a process that died mid-task — comes back as a task that will never
    /// finish. Nothing ever removes it: `task_finished` is the only thing that
    /// does, and it already went past. The set is then permanently non-empty,
    /// [`Driver::minding_background`] is permanently true, and the first
    /// message that asks for a different model is parked behind it for the life
    /// of the process. Since a parked session sits at `Phase::Idle`, Stop
    /// refuses it too: there is no way out at all.
    ///
    /// So a replayed set is *provisional* until something outside the replay
    /// agrees with it. The witness is a fresh file: claude streams a background
    /// agent's transcript and a background command's output while they run, so
    /// real background work at the moment of an attach has written something in
    /// the last few seconds — see [`Driver::subagents_recently_active`] for the
    /// two places it writes. Nothing on disk means nothing is running, and the
    /// set is dropped.
    ///
    /// Only ever runs at the replay boundary. A `task_started` seen live is
    /// claude telling this driver, in this process' lifetime, that a task
    /// began; there is nothing provisional about that one.
    pub(super) async fn settle_background(&mut self) {
        if self.background.is_empty() {
            return;
        }
        if self.subagents_recently_active().await {
            tracing::info!(
                session = %self.id,
                tasks = self.background.len(),
                "the replay's background tasks are corroborated by fresh subagent transcripts; keeping them"
            );
            return;
        }
        let ghosts = self.background.len();
        self.background.clear();
        tracing::warn!(
            session = %self.id,
            ghosts,
            "the replay left background tasks with no sign of life on disk; dropping them rather than holding the queue for ever"
        );
        self.recorder.lifecycle(
            Stage::Settle,
            Some(&format!(
                "dropped {ghosts} background task(s) the replay reconstructed: nothing has been written to a subagent transcript or a task output file in {SUBAGENT_QUIET:?}"
            )),
        );
        self.publish();
    }

    /// The stdin line for one message: its text plus whatever its chat
    /// attachments become at this moment, and the delivery to record **after**
    /// the line is written.
    ///
    /// A chat carries upload paths rather than bytes, so the bytes can be
    /// megabytes each and the journal stays small. They are re-fenced and
    /// re-read here, at the moment they go to the CLI: an upload swept since
    /// the send fails the message with its own reason rather than sending an
    /// empty block or a stale prompt.
    async fn user_line_for(&self, pending: &Pending) -> Result<(String, Option<DeliveryRecord>), String> {
        let paths = &self.runs.host.paths;
        if pending.images.is_empty() && pending.files.is_empty() {
            // A chat message with nothing attached still gets a delivery row:
            // it is what tells the parser that this identical text had no
            // attachments, rather than reaching back to an earlier one.
            let delivery = pending.fingerprint.chat.then(|| DeliveryRecord {
                id: pending.id.clone(),
                text: pending.text.clone(),
                images: Vec::new(),
                files: Vec::new(),
            });
            return Ok((user_line(&pending.text), delivery));
        }
        let mut images = Vec::with_capacity(pending.images.len());
        let mut image_total = 0u64;
        for path in &pending.images {
            let image = droppen_provider::chat_attachments::read_chat_image(paths, path)
                .await
                .map_err(|error| error.to_string())?;
            image_total = image_total.saturating_add(image.raw_len);
            if image_total > droppen_provider::chat_attachments::MAX_IMAGE_TOTAL_BYTES {
                return Err(
                    droppen_provider::chat_attachments::ChatAttachmentError::ImagesTooLarge {
                        limit: droppen_provider::chat_attachments::MAX_IMAGE_TOTAL_BYTES,
                    }
                    .to_string(),
                );
            }
            images.push((image.media_type, image.data));
        }
        let mut inlined = String::new();
        let mut total = 0u64;
        for path in &pending.files {
            let content = droppen_provider::chat_attachments::read_chat_text_file(paths, path, &mut total)
                .await
                .map_err(|error| error.to_string())?;
            inlined.push_str(&droppen_sessions::util::chat_file_block(path, &content));
        }
        let text = if inlined.is_empty() {
            pending.text.clone()
        } else {
            format!("{}{inlined}", pending.text)
        };
        let delivery = pending.fingerprint.chat.then(|| DeliveryRecord {
            id: pending.id.clone(),
            text: text.clone(),
            images: pending.images.clone(),
            files: pending.files.clone(),
        });
        Ok((user_line_with_images(&text, &images), delivery))
    }

    /// Write the row the transcript parser reads for this delivery.
    ///
    /// Called only after the line reached the CLI: a write that failed leaves
    /// no row, and a retry travels under the same queue id and replaces the
    /// row a failed attempt might have written.
    fn record_delivery(&self, delivery: &DeliveryRecord) {
        droppen_sessions::chat_delivery::record(
            &self.runs.host.paths,
            Source::Claude,
            &self.id,
            &delivery.id,
            &delivery.text,
            &delivery.images,
            &delivery.files,
        );
    }

    /// Write one message to claude's stdin and open a turn for it.
    pub(super) async fn start_turn(&mut self, pending: Pending) {
        let (message, delivery) = match self.user_line_for(&pending).await {
            Ok(prepared) => prepared,
            Err(why) => {
                self.receipt(&pending, ReceiptStatus::Failed);
                self.fail_message(&why);
                return;
            }
        };
        let turn = Turn {
            run: self.open_turn(),
            at: self.link.at(),
            seq: self.seq,
            ladder: None,
            provisional: false,
            rotation_attempt: pending.rotation_attempt,
        };
        // Written *before* the prompt, in one rewrite that both takes this
        // message out of the queue and says where its turn began. A crash in the
        // microsecond between this and the write below leaves a mark for a turn
        // that never started, which the next boot reads as in flight and the
        // process' own silence never contradicts — recoverable with Stop. The
        // other order sends the same message twice.
        self.note_progress(Some(turn.at));
        self.recorder.stdin(&message);
        self.recorder.lifecycle(Stage::TurnOpen, None);
        if let Err(error) = self.link.send_line(&message).await {
            // The socket is gone, which means the shim is: the recv loop is
            // about to see the same thing, so the turn is failed here and the
            // driver is left to wind up normally.
            self.receipt(&pending, ReceiptStatus::Failed);
            self.close_turn(&turn, InjectionRunState::Failed, Some(format!("cannot reach claude: {error}")));
            return;
        }
        // Only now, after the line really went in: a write that failed leaves
        // no row, and a retry overwrites the row under the same queue id.
        if let Some(delivery) = &delivery {
            self.record_delivery(delivery);
        }
        self.receipt(&pending, ReceiptStatus::Sent);
        self.touch();
        self.set_phase(Phase::Working);
        self.turn = Some(turn);
    }

    /// Open a turn for a message claude was already handed.
    ///
    /// The steered half of [`Driver::start_turn`], and the difference is the
    /// whole of it: nothing is written. The prompt went into claude's stdin
    /// when the steer was accepted, possibly minutes ago and certainly during
    /// somebody else's turn; writing it again here would run it twice.
    ///
    /// # Why this turn is only provisional
    ///
    /// Because the CLI has two answers to a message written mid-turn and does
    /// not say which it gave. Against claude 2.1.231, both are real:
    ///
    /// * it **buffers** the line and runs it as the next turn, with a
    ///   `system:init` of its own and its own `result` — the case this driver
    ///   was written for, and the one a probe that writes between turns sees;
    /// * it **injects** the line into the turn already running, at the next
    ///   tool boundary, and the model answers both messages in one reply under
    ///   the *original* turn's `result`. Nothing on stdout marks it: there is
    ///   no `user` echo, no second `result`, nothing. Only the transcript,
    ///   which this driver does not read, records a `queued_command`.
    ///
    /// A turn opened unconditionally for the second case is a turn nothing will
    /// ever end — the session shows `working` for ever and everything behind it
    /// in the queue is held there — which is exactly what happened to a live
    /// session and is why this function no longer simply asserts a turn exists.
    ///
    /// So it opens one and gives it [`ClaudeCodeRuns::steer_settle`] to
    /// prove itself. The journal mark is the one piece of bookkeeping that
    /// waits for that proof: a mark is what the next boot reconstructs a turn
    /// in flight from, and a mark for a turn that was folded into a previous
    /// one would resurrect the ghost across a restart.
    fn start_delivered_turn(&mut self, pending: &Pending) {
        let turn = Turn {
            run: self.open_turn(),
            // Where the *turn* starts, not where the prompt was written: from
            // this offset on, everything claude says belongs to this message,
            // which is what a replay after a restart has to reconstruct.
            at: self.link.at(),
            seq: self.seq,
            ladder: None,
            provisional: true,
            rotation_attempt: pending.rotation_attempt,
        };
        tracing::info!(session = %self.id, qid = %pending.id, "a steered message reached the front of the queue");
        // Deliberately `None`: the queue moved and has to be written down, but
        // the turn is not yet one a restart should believe in.
        self.note_progress(None);
        self.recorder.lifecycle(Stage::TurnOpen, Some("a steered message claude already had"));
        self.set_phase(Phase::Working);
        let seq = turn.seq;
        self.turn = Some(turn);
        self.arm_settle(seq, self.runs.steer_settle);
    }

    /// Schedule the moment a provisional turn is judged, without waiting for it.
    ///
    /// The sibling of [`Driver::arm`], and separate from it for the same reason
    /// the two commands are separate: this one must not touch a turn that has
    /// since been replaced, and the only thing that can say so is the sequence
    /// number it carries.
    fn arm_settle(&self, turn: u64, after: Duration) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            let _ = tx.send(Cmd::Settle { turn });
        });
    }

    /// Decide what became of a steered message whose turn has proved nothing.
    ///
    /// Two conditions, and both are needed:
    ///
    /// * **no `init`** for this turn. That is the positive signal — a buffered
    ///   message is always answered with one — and it is what
    ///   [`Driver::on_init`] clears `provisional` on;
    /// * **silence** for the whole window. Belt to the init's braces: anything
    ///   at all on stdout, background chatter included, pushes the decision out
    ///   rather than risking a verdict against a CLI that is mid-sentence.
    ///
    /// When both hold, the message was answered inside the turn it was handed
    /// to. The turn is closed `Ok` — the work really did happen, and the person
    /// who sent it has already read the answer — and the queue behind it moves.
    /// The alternative, which is what shipped and what a live session died of,
    /// is a turn nobody will ever close.
    ///
    /// A Stop pressed inside the window is not overridden: the ladder is armed
    /// against this turn's sequence number, so closing the turn here disarms it
    /// on its own — see [`Driver::escalate`] — and a process that was going to
    /// be signalled for a turn that had already finished is left alone instead.
    async fn settle_delivered(&mut self, seq: u64) {
        let Some(turn) = self.turn.as_ref().filter(|turn| turn.seq == seq && turn.provisional) else {
            return; // ended, replaced, or claude said `init` and proved it real
        };
        let window = self.runs.steer_settle;
        let quiet = self.heard.elapsed();
        if quiet < window {
            // claude is talking; ask again once the silence would be long
            // enough. Re-armed rather than looped so the driver stays free to
            // answer the very lines this is waiting on, and floored so that a
            // stream arriving exactly at the edge of the window cannot turn
            // this into a spin.
            self.arm_settle(turn.seq, (window - quiet).max(SETTLE_RECHECK));
            return;
        }
        let turn = self.turn.take().expect("checked just above");
        tracing::info!(
            session = %self.id,
            ?window,
            "claude never opened a turn for the steered message; it was answered inside the turn it was handed to"
        );
        self.recorder.lifecycle(
            Stage::Absorb,
            Some(&format!(
                "no turn of its own after {window:?}: the steered message was folded into the turn it was handed to"
            )),
        );
        self.close_turn(&turn, InjectionRunState::Ok, None);
        self.pump().await;
    }

    /// Take out of [`Driver::delivered_ahead`] every steered message claude has
    /// already folded into the turn in flight, by reading the transcript for it.
    ///
    /// # Why
    ///
    /// The CLI does not always hold a `user` line written mid-turn until the
    /// turn ends. At the next tool boundary it may fold the line into the turn
    /// that is running instead: the model reads it in its next request and
    /// answers it inside the same turn, and the transcript records it — as a
    /// `queued_command` attachment, where it landed — while stdout says nothing
    /// at all. A driver that only found out at the turn's end kept the message
    /// queued for the whole rest of that turn: the row said "delivered, waiting
    /// to start" beside a Stop hint, and the row, the list badge and the
    /// "1 message queued" line all counted it, while the transcript above them
    /// showed the agent already answering it.
    ///
    /// # How
    ///
    /// The transcript is read from the moment of the steer on — only the bytes
    /// past [`AbsorbAnchor::offset`], only lines that name a queued command, and
    /// only the folds of a person's message (see
    /// [`crate::sessions::transcript::absorbed_prompt`]). A fold whose text is a
    /// steered message's takes the oldest such message out. Its turn will never
    /// open, nothing about it is queued any more, and it is not this driver's to
    /// deliver again: it is in the conversation now rather than in the pipe, so
    /// a process that dies after this must not have [`Driver::requeue_delivered`]
    /// hand it to the replacement a second time.
    ///
    /// A message the CLI *buffers* for a turn of its own leaves no such entry —
    /// its record is an ordinary `user` line, written as that turn starts — so
    /// it stays exactly where it was, for [`Driver::start_delivered_turn`] and
    /// the settle to decide as before. A fold that matches nothing outstanding
    /// is left alone, and a fold this misses (inherited across a restart, say)
    /// is still caught by the settle once the turn ends: this only ever makes
    /// the queue right sooner.
    ///
    /// Returns whether anything was taken.
    async fn absorb(&mut self) -> bool {
        if self.delivered_ahead.is_empty() {
            self.absorb.anchor = None;
            return false;
        }
        // History still being replayed may change what is outstanding.
        if self.replaying {
            return false;
        }
        if !self.absorb_anchored() {
            // Nothing this driver steered is in the file yet: the messages were
            // inherited from the last server, or the session moved to a fork.
            // Starting from the end can only miss a fold, and a missed fold is
            // the settle's to find at the turn's end.
            self.anchor_absorb().await;
            return false;
        }
        let anchor = self.absorb.anchor.as_mut().expect("checked just above");
        if anchor.file.is_none() {
            // There was no transcript when the watch began, so all of it is newer.
            anchor.file = crate::sessions::claude::find_claude_session_file(&self.runs.host.paths, &self.id).await;
        }
        let Some(path) = anchor.file.clone() else { return false };
        let (lines, offset) = appended_lines(&path, anchor.offset, b"\"queued_command\"").await;
        anchor.offset = offset;
        let mut taken = Vec::new();
        for text in lines.iter().filter_map(crate::sessions::transcript::absorbed_prompt) {
            if let Some(at) = self.delivered_ahead.iter().position(|pending| same_prompt(&pending.text, &text)) {
                taken.push(self.delivered_ahead.remove(at).expect("the index came from this queue"));
            }
        }
        if self.delivered_ahead.is_empty() {
            self.absorb.anchor = None;
        }
        if taken.is_empty() {
            return false;
        }
        for pending in &taken {
            tracing::info!(session = %self.id, qid = %pending.id, "claude folded a steered message into the turn in flight");
            self.recorder.lifecycle(
                Stage::Absorb,
                Some(&format!(
                    "{} was folded into the turn in flight; the transcript holds it: {}",
                    pending.id,
                    clip(&pending.text)
                )),
            );
        }
        self.absorb.unannounced = true;
        self.publish();
        // The list's `+1` and queued chips count it too.
        self.runs.host.refresh_list();
        // Out of the journal too, or the next server inherits a steered message
        // that is already in the conversation and opens a turn for it.
        self.note_progress(self.turn.as_ref().filter(|turn| !turn.provisional).map(|turn| turn.at));
        true
    }

    /// Whether the transcript watch is reading this session's current file.
    fn absorb_anchored(&self) -> bool {
        self.absorb.anchor.as_ref().is_some_and(|anchor| anchor.session == self.id)
    }

    /// Watch the transcript from where it ends now.
    ///
    /// Called before a steer writes anything, so the fold that message may
    /// become is always past the anchor. The end of the file may be the middle
    /// of a line claude is still writing; that fragment fails to parse and is
    /// stepped over, and it cannot be the fold of a message not yet written.
    async fn anchor_absorb(&mut self) {
        let file = crate::sessions::claude::find_claude_session_file(&self.runs.host.paths, &self.id).await;
        let offset = match &file {
            Some(path) => tokio::fs::metadata(path).await.map(|meta| meta.len()).unwrap_or(0),
            None => 0,
        };
        self.absorb.anchor = Some(AbsorbAnchor {
            session: self.id.clone(),
            file,
            offset,
        });
    }

    /// [`Cmd::Absorb`]: look now, and keep looking while anything is outstanding.
    ///
    /// The feed's nudge is the moment to poke it, too: it has just sent the
    /// transcript frame that shows the folded message, so the state frame that
    /// takes the message off the queue follows right behind — not seconds
    /// later, when the screen would show it twice, and not ahead of it, when it
    /// would briefly be in neither place. The driver's own poll can read the
    /// file before the feed has, so it leaves the poke to that nudge, or to the
    /// feed's own tick for a session nobody has open.
    pub(super) async fn on_absorb(&mut self, nudged: bool) {
        if !nudged {
            self.absorb.armed = false;
        }
        self.absorb().await;
        if nudged && std::mem::take(&mut self.absorb.unannounced) {
            self.runs.host.poke_feed(Source::Claude, &self.id);
        }
        self.arm_absorb();
    }

    /// Schedule the next look, unless one is coming or nothing is outstanding.
    pub(super) fn arm_absorb(&mut self) {
        if self.absorb.armed || self.delivered_ahead.is_empty() {
            return;
        }
        self.absorb.armed = true;
        let tx = self.tx.clone();
        let after = self.runs.absorb_poll;
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            let _ = tx.send(Cmd::Absorb { nudged: false });
        });
    }

    /// Put `next` back at the head of the queue and replace the process with
    /// one it fits.
    ///
    /// The one place a settings change is carried out, shared by the two ways
    /// of asking for it: [`Driver::pump`] reaching a head the running process
    /// cannot answer, and [`Driver::restart_for_steer`] being told to stop
    /// waiting. Both have to leave the same state behind — the message first in
    /// the queue, no park outstanding, and the mirror saying so before a
    /// restart nobody is watching takes seconds — and two copies of that is how
    /// they would stop agreeing.
    ///
    /// Returns whether there is a process to pump the queue into: a respawn
    /// that failed set `fatal` and closed the link, and the driver's own loop
    /// is what winds the session up, with nothing written at it.
    pub(super) async fn respawn_for(&mut self, next: Pending, wanted: &Recipe) -> bool {
        self.queue.push_front(next);
        // Whatever this session was waiting for, it is not waiting any more.
        self.set_awaiting(false);
        self.publish();
        self.respawn(wanted).await;
        self.fatal.is_none()
    }

    /// Replace the running process with one started under `recipe`.
    ///
    /// Only ever from idle, so the polite shutdown is the whole of it: stdin
    /// closes, claude exits zero, and the kill below is for a CLI that will
    /// not. A replacement that cannot be started leaves the closed link in
    /// place on purpose — the driver's own `recv` sees the end immediately and
    /// winds the session up through the one path that does that properly.
    pub(super) async fn respawn(&mut self, recipe: &Recipe) {
        tracing::info!(session = %self.id, "the session's settings changed; restarting claude");
        self.recorder.lifecycle_with(
            Stage::Respawn,
            None,
            Some(Selector::Resume.name()),
            Some("the settings this session's next message needs differ from the ones it is running"),
        );
        if self.link.close_stdin().await.is_err() {
            self.link.kill_claude(libc::SIGTERM);
        }
        let deadline = tokio::time::Instant::now() + CLOSE_GRACE;
        loop {
            match tokio::time::timeout_at(deadline, self.link.recv()).await {
                Ok(Some(_)) => continue,
                Ok(None) => break,
                Err(_) => {
                    tracing::warn!(session = %self.id, "claude did not exit when its stdin closed; killing it");
                    self.link.kill_claude(libc::SIGKILL);
                    break;
                }
            }
        }
        // Always a resume from here: the process being replaced has written the
        // transcript this one has to continue, whatever selector started it.
        match self.relink(recipe, Selector::Resume).await {
            Ok(()) => {}
            Err(error) => {
                tracing::error!(session = %self.id, %error, "cannot restart claude under the new settings");
                self.fatal = Some(error.to_string());
            }
        }
    }

    /// Put a freshly spawned process behind this driver, retiring the old one.
    ///
    /// Shared by the settings respawn and by the recovery ladder, which is what
    /// keeps the two from drifting: both have to retire the outgoing journal
    /// record, drop the outgoing sidecars, and go back to `Starting` with
    /// nothing writable until the replacement says `init`.
    pub(super) async fn relink(&mut self, recipe: &Recipe, selector: Selector) -> Result<(), InjectError> {
        // Nothing is steered into a process this driver is replacing: whatever
        // was written to the outgoing one's stdin is going nowhere, and the
        // record about to be written names the *new* process. Keeping that
        // invariant here rather than at each caller is what lets the journal's
        // `steered` flag mean one thing — "the process this record names has
        // this line in its stdin" — which is the whole basis on which an attach
        // declines to send it again.
        self.requeue_delivered();
        // Nothing replaces a process with a question open — see `pump` — so
        // this is the recovery ladder's, already done; and if not, the process
        // that asked is about to be gone either way.
        self.close_questions(QuestionState::Expired, "the process that asked it is being replaced");
        // Whatever the outgoing process was thinking, it is not thinking it
        // any more.
        self.draft.end();
        // The backlog moves to the new record with the process it is waiting
        // for, so a crash between the two spawns leaves the messages attached to
        // whichever record survives rather than to neither.
        let queued = self.backlog(Pending::recorded);
        let spawned = self
            .runs
            .spawn_link(&self.lease.respawn_permit(), &self.id, recipe, selector, Some(&self.record), &queued, &self.recorder)
            .await?;
        let _ = std::fs::remove_file(self.sidecars.exit());
        let _ = std::fs::remove_file(self.sidecars.stderr());
        self.sidecars = spawned.sidecars;
        self.link = spawned.link;
        self.record = spawned.record;
        // A queue held because one account refused is not held against a
        // process on another; one restarted on the same account (the recovery
        // ladder) still is, and the new record has to say so.
        if self
            .hold
            .as_ref()
            .is_some_and(|hold| recipe.fingerprint.account.as_deref() != Some(hold.account.as_str()))
        {
            self.hold = None;
        }
        if self.hold.is_some() {
            self.runs.host.journal.mark_hold(&self.record, self.hold.clone());
        }
        // Known again, whatever it was before: this driver started the process
        // itself, and the record it just wrote says the same thing.
        self.live = Some(recipe.fingerprint.clone());
        self.recipe = Some(recipe.clone());
        self.ready = false;
        // A replacement is a fresh process with nothing behind it: its shim has
        // written no bytes, so there is no history and nothing to suppress.
        self.replaying = false;
        self.replay_open = false;
        self.replay_marked = false;
        self.replay_gapped = false;
        self.pending_turn = None;
        self.idle_since = None;
        // The background tasks belonged to the process that is gone, and they
        // went with it — claude does not hand a subagent to its successor. A
        // fresh process only announces the list when it *changes*, so a set
        // carried across would never be contradicted and would hold every later
        // settings change hostage to work that stopped existing here.
        if !self.background.is_empty() {
            tracing::info!(
                session = %self.id,
                tasks = self.background.len(),
                "the replaced process took its background tasks with it"
            );
            self.background.clear();
        }
        // The mirror first, the phase second: the phase pokes the feed, and
        // the frame that poke produces should already have the cleared tasks
        // and the moved backlog in it.
        self.publish();
        self.set_phase(Phase::Starting);
        self.publish();
        Ok(())
    }
}
