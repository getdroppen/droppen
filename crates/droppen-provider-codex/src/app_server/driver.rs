//! Per-session Codex App Server driver loop, RPC dispatch, and replay handling.

use super::*;

pub(super) struct Driver {
    pub(super) runs: Arc<CodexRuns>,
    pub(super) record: runs::RunRecord<CodexRun>,
    pub(super) link: shim::Link,
    /// The session's claim. Dropped with the driver, except on
    /// [`Cmd::Retire`] — which hands the session straight to a replacement and
    /// so has to let go of the claim before it answers.
    pub(super) lease: SessionLease,
    pub(super) state: State,
    pub(super) shared: Arc<Mutex<State>>,
    pub(super) rx: mpsc::UnboundedReceiver<Cmd>,
    pub(super) replies: HashMap<String, oneshot::Sender<QueueOutcome>>,
    pub(super) pending_since: Instant,
    // Target turn (unknown until turn/started), time sent, escalation rung.
    pub(super) interrupt: Option<(Option<String>, Instant, u8)>,
    /// A Stop for a message that waits to start by itself, read before its
    /// `turn/start` went out: kept for that message's own start, whatever
    /// starts before it, and let go once the message no longer waits to start
    /// ([`Driver::waits_to_start`]).
    pub(super) stop_next: Option<String>,
    /// Its handle's [`Handle::starting`], which this driver clears once the
    /// message the start was for is in its queue.
    pub(super) starting: Arc<Mutex<Option<String>>>,
    pub(super) archive_reply: Option<oneshot::Sender<Result<(), InjectError>>>,
    pub(super) archive_since: Instant,
    /// Set by [`Cmd::Retire`]: leave the loop without reporting a death.
    pub(super) retiring: bool,
    /// How long [`Cmd::Retire`] waits for the CLI to exit; a field rather than
    /// the constant so a test can drive the timeout in milliseconds.
    pub(super) retire_wait: Duration,
}

impl Driver {
    pub(super) fn checkpoint(&mut self) {
        // Every change to the state passes here, so a kept Stop is let go
        // here, once its message no longer waits to start by itself: it left
        // the queue, or the queue paused and it starts only when the user
        // says so.
        if self.stop_next.as_deref().is_some_and(|message| !self.waits_to_start(message)) {
            self.stop_next = None;
        }
        // The cursor only advances with the state it describes, in one DB row.
        self.state.cursor = self.link.at().max(self.state.cursor);
        self.record.payload.codex = Some(self.state.clone());
        self.runs.host.journal.add(self.record.clone());
        *self.shared.lock().unwrap() = self.state.clone();
    }

    fn running(&mut self) {
        self.state.injection = Some(Injection {
            state: InjectionRunState::Running,
            error: droppen_http::openapi_ext::Nullable(None),
            interrupted: false,
            started_at: iso8601(SystemTime::now()),
            finished_at: droppen_http::openapi_ext::Nullable(None),
        });
    }

    pub(super) fn finish(&mut self, error: Option<String>, interrupted: bool) {
        let started_at = self
            .state
            .injection
            .as_ref()
            .map(|i| i.started_at.clone())
            .unwrap_or_else(|| self.record.started_at.clone());
        self.state.injection = Some(Injection {
            state: if error.is_some() || interrupted {
                InjectionRunState::Failed
            } else {
                InjectionRunState::Ok
            },
            error: droppen_http::openapi_ext::Nullable(error),
            interrupted,
            started_at,
            finished_at: droppen_http::openapi_ext::Nullable(Some(iso8601(SystemTime::now()))),
        });
    }

    pub(super) async fn submit(&mut self, index: usize, steer: bool) -> Result<(), InjectError> {
        let id = format!("{}:{}", if steer { "steer" } else { "start" }, self.state.queue[index].id);
        if !steer {
            self.running();
            // A Stop kept for a message is for that message's start: taken
            // when it starts, and left for it when another starts first.
            if self.stop_next.as_deref() == Some(self.state.queue[index].id.as_str()) {
                self.stop_next = None;
                self.state.interrupt_requested = true;
            }
        }
        self.state.queue[index].request = Some(id.clone());
        self.pending_since = Instant::now();
        self.checkpoint();
        let message = &self.state.queue[index];
        let delivery = message.delivery.clone();
        let message_id = message.id.clone();
        let thread_id = self.record.session_id.as_str();
        let outcome = if steer {
            let request = Request {
                id: &id,
                method: "turn/steer",
                params: TurnSteerParams {
                    thread_id,
                    expected_turn_id: self.state.active.as_deref(),
                    input: &message.params["input"],
                    client_user_message_id: &message.id,
                },
            };
            write(&mut self.link, &request).await
        } else {
            let request = Request {
                id: &id,
                method: "turn/start",
                params: TurnStartOnThread {
                    params: StoredParams(&message.params),
                    thread_id,
                    client_user_message_id: &message.id,
                },
            };
            write(&mut self.link, &request).await
        };
        if outcome.is_ok() {
            if let Some(delivery) = delivery {
                droppen_sessions::chat_delivery::record(
                    &self.runs.host.paths,
                    Source::Codex,
                    &self.record.session_id,
                    &message_id,
                    &delivery.text,
                    &delivery.images,
                    &delivery.files,
                );
            }
        }
        outcome
    }

    pub(super) async fn run(mut self) {
        if self.state.archived {
            self.close_archived().await;
            return;
        }
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            let outcome = tokio::select! {
                output = self.link.recv() => match output {
                    Some(output) => self.output(output).await,
                    None => Err(InjectError::message("Codex App Server disconnected; undelivered messages remain in the session journal")),
                },
                cmd = self.rx.recv() => match cmd { Some(cmd) => self.command(cmd).await, None => break },
                _ = tick.tick() => self.tick().await,
            };
            if let Err(error) = outcome {
                // Its app server gone, this driver takes no more messages, the
                // one a start was for included: that is answered as lost below,
                // and the session no longer reads as starting.
                *self.starting.lock().unwrap() = None;
                if let Some(reply) = self.archive_reply.take() {
                    let _ = reply.send(Err(InjectError::message(self.runs.host.redact(&error.to_string()))));
                }
                self.state.paused = true;
                let reported = self.runs.host.redact(&error.to_string());
                self.finish(Some(reported), self.state.interrupt_requested);
                self.checkpoint();
                // Keep the claim while the process is alive, even if the socket
                // has failed: a replacement must never run beside an orphan.
                while procs::is_same_process(self.record.cli_pid, self.record.pid_token.as_deref()) {
                    match tokio::time::timeout(Duration::from_secs(2), self.rx.recv()).await {
                        Ok(Some(Cmd::Interrupt(_))) => self.link.kill_claude(libc::SIGTERM),
                        Ok(Some(Cmd::Deliver(_, reply))) => {
                            let _ = reply.send(Err(InjectError::message("Codex connection lost; restart the server to reattach")));
                        }
                        Ok(Some(Cmd::Archive(reply))) => {
                            let _ = reply.send(Err(InjectError::message(
                                "Codex connection lost; restart the server to reattach before archiving",
                            )));
                        }
                        Ok(Some(Cmd::Queue { reply, .. })) => {
                            let _ = reply.send(QueueOutcome::Unreachable);
                        }
                        Ok(Some(Cmd::Retire(reply))) => {
                            // The process is going anyway; the caller only needs
                            // to know it may not respawn yet.
                            let _ = reply.send(Err(InjectError::message("Codex connection lost; restart the server to reattach")));
                        }
                        Ok(None) => break,
                        Err(_) => {}
                    }
                }
                self.runs.codex_dead(self.record.clone());
                break;
            }
            if self.state.archived || self.retiring {
                break;
            }
            if self.link.at() >= self.link.total()
                && self.state.archive_request.is_none()
                && self.state.active.is_none()
                && !self.state.paused
                && !self.state.queue.is_empty()
                && self.state.queue.iter().all(|q| q.request.is_none())
            {
                if let Err(error) = self.submit(0, false).await {
                    self.state.paused = true;
                    self.finish(Some(error.to_string()), false);
                    self.checkpoint();
                }
            }
        }
    }

    pub(super) async fn command(&mut self, cmd: Cmd) -> Result<(), InjectError> {
        match cmd {
            Cmd::Archive(reply) => {
                if self.state.active.is_some() || !self.state.queue.is_empty() {
                    let _ = reply.send(Err(InjectError::message(
                        "Codex is running or has queued messages; stop it and clear the queue before archiving",
                    )));
                    return Ok(());
                }
                if self.state.archive_request.is_some() || self.link.at() < self.link.total() {
                    let _ = reply.send(Err(InjectError::message(
                        "Codex is awaiting archive confirmation or replaying session state; retry after it settles",
                    )));
                    return Ok(());
                }
                let id = format!("archive:{}", new_uuid());
                self.state.archive_request = Some(id.clone());
                self.archive_reply = Some(reply);
                self.archive_since = Instant::now();
                self.checkpoint();
                let request = Request {
                    id: &id,
                    method: "thread/archive",
                    params: ThreadArchiveParams {
                        thread_id: &self.record.session_id,
                    },
                };
                write(&mut self.link, &request).await?;
            }
            Cmd::Deliver(message, reply) => {
                let id = message.id.clone();
                if self.state.archived || self.state.archive_request.is_some() {
                    let _ = reply.send(Err(InjectError::message("Codex session is archived or awaiting archive confirmation")));
                } else {
                    let queued = self.state.active.is_some() || !self.state.queue.is_empty();
                    self.state.queue.push(message);
                    if self.state.queue.iter().all(|q| q.request.is_none() && !q.uncertain) {
                        self.state.paused = false;
                    }
                    self.checkpoint();
                    let _ = reply.send(Ok(Sent { queued }));
                }
                // The message the start was for is in the queue, on view, or
                // refused: a Stop finds it there from now on, not by the start.
                let mut starting = self.starting.lock().unwrap();
                if starting.as_deref() == Some(id.as_str()) {
                    *starting = None;
                }
            }
            Cmd::Retire(reply) => {
                if self.state.active.is_some() || !self.state.queue.is_empty() || self.state.archive_request.is_some() {
                    let _ = reply.send(Err(InjectError::message("a message is already running for this session")));
                    return Ok(());
                }
                // EOF is the polite end: codex finishes, writes nothing further
                // and exits. A link that will not take it is a process nobody
                // can replace safely, so the caller is told rather than raced.
                if tokio::time::timeout(RPC_TIMEOUT, self.link.close_stdin()).await.is_err() {
                    let _ = reply.send(Err(InjectError::message(
                        "the running Codex process did not accept the shutdown; try again",
                    )));
                    return Ok(());
                }
                // Waited out rather than assumed. A process that has not gone
                // when the budget runs out has *not* retired, and saying it has
                // is the worst answer available: the caller would immediately
                // start a second app-server on the same thread, under the new
                // key, beside one still holding the old key and the writer
                // lock. So nothing is torn down and nothing is handed over —
                // the run, the claim and the handle all stay exactly where they
                // are, and the send is refused. The EOF already sent still
                // stands, so the ordinary disconnect path cleans up when the
                // process does finally go.
                let deadline = Instant::now() + self.retire_wait;
                while procs::is_same_process(self.record.cli_pid, self.record.pid_token.as_deref()) && Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                if procs::is_same_process(self.record.cli_pid, self.record.pid_token.as_deref()) {
                    tracing::warn!(
                        session = %self.record.session_id,
                        pid = self.record.cli_pid,
                        "the Codex process did not exit after its stdin was closed; keeping the session on the account it is already running under"
                    );
                    let _ = reply.send(Err(InjectError::message(
                        "the running Codex process did not accept the shutdown; try again",
                    )));
                    return Ok(());
                }
                self.retiring = true;
                self.runs.host.journal.remove(&self.record.run_id);
                self.runs.host.withdraw_run(Source::Codex, &self.record.session_id);
                // Only its own entry: the send that retires it has put the
                // session's next app server, still starting, in its place.
                {
                    let mut servers = self.runs.codex_servers.lock().unwrap();
                    if servers.get(&self.record.session_id).is_some_and(|handle| Arc::ptr_eq(&handle.shared, &self.shared)) {
                        servers.remove(&self.record.session_id);
                    }
                }
                // Released before the reply, because the caller's very next move
                // is to claim this session for the process it is about to start.
                self.lease.release();
                #[cfg(test)]
                self.runs.host.step("codex retired", Source::Codex, Some(&self.record.session_id));
                let _ = reply.send(Ok(()));
            }
            Cmd::Interrupt(stop) => {
                // A Stop stops the turn it names and no other. One for a
                // message that waits to start by itself is kept for that
                // message's own start, whatever runs before it: the turn a
                // resume reports in progress, say, or messages the process
                // before this one left queued. Anything else names a turn that
                // is over, or a message that will not start by itself (the
                // queue paused behind one the process before this one left
                // unanswered, say), and is dropped. Kept, `interrupt_requested`
                // would stop whichever turn the user starts next, however much
                // later, and a restart would read it back from the journal.
                // The route has answered already that the Stop was taken: one
                // is taken while the app server starts ([`Handle::interrupt`]),
                // before anybody can tell whether the queue will be paused.
                // The message it was sent for waits in the paused queue.
                if !self.stops_running(&stop) {
                    if let Stop::Message(message) = &stop {
                        if self.waits_to_start(message) {
                            self.stop_next = Some(message.clone());
                        }
                    }
                    return Ok(());
                }
                self.state.interrupt_requested = true;
                self.checkpoint();
                self.send_interrupt().await?;
            }
            Cmd::Queue { id, steer, reply } => {
                let Some(index) = self.state.queue.iter().position(|q| q.id == id) else {
                    let _ = reply.send(QueueOutcome::Unknown);
                    return Ok(());
                };
                if self.state.queue[index].request.is_some() {
                    let _ = reply.send(QueueOutcome::AlreadySteered);
                    return Ok(());
                }
                if !steer {
                    let dropped = self.state.queue.remove(index);
                    self.checkpoint();
                    self.runs.host.note_receipt(
                        Source::Codex,
                        &self.record.session_id,
                        &dropped.id,
                        droppen_sessions::delivery::ReceiptStatus::Failed,
                    );
                    let _ = reply.send(QueueOutcome::Done);
                } else {
                    if self.state.queue[index].uncertain {
                        let _ = reply.send(QueueOutcome::Refused { reason: "delivery was not acknowledged before Codex exited; inspect the transcript, then remove this queued entry before resending".into() });
                        return Ok(());
                    }
                    let busy = self.state.active.is_some();
                    if busy
                        && ["model", "effort", "sandboxPolicy"].iter().any(|key| {
                            self.state.queue[index].params.get(*key).is_some_and(|wanted| {
                                if *key == "sandboxPolicy" {
                                    self.state.settings[*key]["type"] != wanted["type"]
                                } else {
                                    self.state.settings.get(*key) != Some(wanted)
                                }
                            })
                        })
                    {
                        let _ = reply.send(QueueOutcome::Refused {
                            reason: "Codex steer keeps the active turn's model, effort and permissions; wait for the next turn to change them".into(),
                        });
                        return Ok(());
                    }
                    if !busy && self.state.queue.iter().any(|q| q.request.is_some()) {
                        let _ = reply.send(QueueOutcome::Unreachable);
                        return Ok(());
                    }
                    self.state.paused = false;
                    self.replies.insert(id, reply);
                    self.submit(index, busy).await?;
                }
            }
        }
        Ok(())
    }

    pub(super) async fn output(&mut self, output: shim::Output) -> Result<(), InjectError> {
        if self.link.at() <= self.state.cursor {
            return Ok(());
        }
        let shim::Output::Line(line) = output else {
            return Err(InjectError::message("Codex replay buffer overflowed; refusing to guess delivery state"));
        };
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            return Ok(());
        };
        if value.get("method").is_some() && value.get("id").is_some() {
            return reject_request(&mut self.link, &value).await;
        }
        if self.state.archive_request.is_some() && self.state.archive_request.as_deref() == value["id"].as_str() {
            self.state.archive_request = None;
            let result = if let Some(error) = value.get("error") {
                self.checkpoint();
                Err(InjectError::message(self.runs.host.redact(&error.to_string())))
            } else {
                self.state.archived = true;
                self.state.injection = None;
                self.checkpoint();
                self.close_archived().await;
                Ok(())
            };
            if let Some(reply) = self.archive_reply.take() {
                let _ = reply.send(result);
            }
            return Ok(());
        }
        if let Some(index) = self
            .state
            .queue
            .iter()
            .position(|q| q.request.as_deref() == value["id"].as_str() && q.request.is_some())
        {
            let request = self.state.queue[index].request.take().unwrap();
            let qid = self.state.queue[index].id.clone();
            let outcome = if let Some(error) = value.get("error") {
                self.state.paused = true;
                if request.starts_with("start:") {
                    self.state.interrupt_requested = false;
                    self.interrupt = None;
                    let reported = self.runs.host.redact(&error.to_string());
                    self.finish(Some(reported), false);
                }
                QueueOutcome::Refused {
                    reason: self.runs.host.redact(&error.to_string()),
                }
            } else {
                let sent = self.state.queue.remove(index);
                // Acknowledged: a turn started with it, or it was steered into
                // the one running. Either way codex has it.
                self.runs
                    .host.note_receipt(Source::Codex, &self.record.session_id, &sent.id, droppen_sessions::delivery::ReceiptStatus::Sent);
                if request.starts_with("start:") {
                    if let Some(turn) = value["result"]["turn"]["id"].as_str() {
                        // A response may follow turn/completed; do not resurrect it.
                        if self.state.completed_turn.as_deref() != Some(turn) {
                            self.state.active = Some(turn.into());
                            self.state.active_message = Some(qid.clone());
                            if !self.state.injection.as_ref().is_some_and(|i| i.state == InjectionRunState::Running) {
                                self.running();
                            }
                        }
                    }
                    for key in ["model", "effort", "sandboxPolicy"] {
                        if let Some(v) = sent.params.get(key) {
                            self.state.settings[key] = v.clone();
                        }
                    }
                }
                QueueOutcome::Done
            };
            self.checkpoint();
            if let Some(reply) = self.replies.remove(&qid) {
                let _ = reply.send(outcome);
            }
        }
        // Ahead of the thread filter, which it would not pass: the update is
        // about the *account*, carries no `threadId`, and this process is that
        // account's for its whole life. Recorded, and otherwise ignored exactly
        // as it always was.
        if value["method"] == "account/rateLimits/updated" {
            self.note_rate_limits(&value["params"]);
            return Ok(());
        }
        if value["params"]["threadId"] != self.record.session_id {
            return Ok(());
        }
        match value["method"].as_str() {
            Some("turn/started") => {
                self.state.active = value["params"]["turn"]["id"].as_str().map(str::to_owned);
                if !self.state.injection.as_ref().is_some_and(|i| i.state == InjectionRunState::Running) {
                    self.running();
                }
                self.checkpoint();
                self.send_interrupt().await?;
            }
            Some("turn/completed") => {
                let turn = &value["params"]["turn"];
                if self.state.active.as_deref().is_none_or(|active| Some(active) == turn["id"].as_str()) {
                    self.state.active = None;
                    self.state.active_message = None;
                    self.state.completed_turn = turn["id"].as_str().map(str::to_owned);
                    self.state.interrupt_requested = false;
                    self.interrupt = None;
                    let interrupted = turn["status"] == "interrupted";
                    let failed = turn["status"] == "failed";
                    self.state.paused |= failed;
                    self.finish(
                        if failed {
                            Some(self.runs.host.redact(&turn["error"].to_string()))
                        } else {
                            None
                        },
                        interrupted,
                    );
                    self.checkpoint();
                    if failed {
                        self.note_turn_error(&turn["error"]);
                    } else if turn["status"] == "completed" {
                        self.note_turn_completed();
                    }
                    if self.link.at() > self.link.total() {
                        if let (Some(injection), Some(alerts)) = (&self.state.injection, self.runs.host.alerts.get()) {
                            alerts.turn(
                                self.runs.host.paths.clone(),
                                Source::Codex,
                                self.record.session_id.clone(),
                                injection.state,
                                injection.error.0.clone(),
                                self.runs.host.driver_states_of(&*self.runs),
                            );
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Whether the line just read is new, rather than history an attach is
    /// replaying — the same boundary the push notification keeps, and for the
    /// same reason: a limit hit before this server existed is not news.
    fn live(&self) -> bool {
        self.link.at() > self.link.total()
    }

    /// The account and model this process runs, as the limit log names them.
    fn limit_subject(&self) -> (String, Option<String>) {
        (
            droppen_accounts::rest::account_key(self.record.payload.codex_account.as_deref(), AccountProvider::Codex),
            self.state.settings.get("model").and_then(Value::as_str).map(str::to_owned),
        )
    }

    /// A turn that completed: the login it ran on took its credential, so a
    /// refused-credential rest on it is over. Codex has no reading to heal its
    /// local login the way claude's heals ([`droppen_accounts::local_login`]);
    /// this is how a `signed_out` codex login comes back.
    fn note_turn_completed(&self) {
        if !self.live() {
            return;
        }
        let (account_id, _) = self.limit_subject();
        self.runs.host.credential_accepted(&account_id, &self.record.session_id);
    }

    /// Record a failed turn whose error is an account limit. Recording only:
    /// the queue pauses exactly as it did before.
    fn note_turn_error(&self, error: &Value) {
        if !self.live() {
            return;
        }
        let (account_id, model) = self.limit_subject();
        let Some(verdict) = droppen_accounts::limits::read_codex_turn_error(error) else {
            return;
        };
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .or_else(|| error.as_str())
            .unwrap_or_default()
            .to_string();
        self.runs.host.record_limit(droppen_accounts::rest::Observation {
            account_id,
            session_id: Some(self.record.session_id.clone()),
            source: "codex:turn",
            model,
            verdict,
            raw: error.to_string(),
            reason: message,
            may_rest: true,
        });
    }

    /// Record an `account/rateLimits/updated` that says a limit was reached.
    ///
    /// Most of these are readings — codex sends one after every response — and
    /// are not events. One with a full window, or a limit or spend control
    /// reached, is; it rests the account only when it says until when.
    fn note_rate_limits(&self, params: &Value) {
        if !self.live() {
            return;
        }
        let Some(verdict) = droppen_accounts::limits::read_codex_rate_limits(params) else {
            return;
        };
        let (account_id, model) = self.limit_subject();
        let may_rest = verdict.until_ms.is_some();
        let reason = params
            .pointer("/rateLimits/rateLimitReachedType")
            .and_then(Value::as_str)
            .map(|kind| format!("codex rate limits: {kind}"))
            .unwrap_or_else(|| "codex rate limits: a window is at 100%".to_string());
        self.runs.host.record_limit(droppen_accounts::rest::Observation {
            account_id,
            session_id: Some(self.record.session_id.clone()),
            source: "codex:rate_limits",
            model,
            verdict,
            raw: params.to_string(),
            reason,
            may_rest,
        });
    }

    pub(super) async fn tick(&mut self) -> Result<(), InjectError> {
        if self.archive_reply.is_some() && self.archive_since.elapsed() > RPC_TIMEOUT {
            let _ = self.archive_reply.take().unwrap().send(Err(InjectError::message(
                "Codex archive acknowledgement timed out; waiting for confirmation without retrying or accepting new messages",
            )));
            // Keep the persisted ID for a late reply or restart replay.
        }
        if self.link.at() >= self.link.total() {
            self.send_interrupt().await?;
        }
        if !self.state.paused && self.state.queue.iter().any(|q| q.request.is_some()) && self.pending_since.elapsed() > RPC_TIMEOUT {
            self.state.paused = true;
            for (_, reply) in self.replies.drain() {
                let _ = reply.send(QueueOutcome::Unreachable);
            }
            // Keep the request IDs for late acknowledgements/replay. Never resend.
            if self.state.active.is_none() {
                self.finish(
                    Some("Codex delivery acknowledgement timed out; message retained without automatic retry".into()),
                    false,
                );
            }
            self.checkpoint();
        }
        if let Some((turn, at, rung)) = &self.interrupt {
            let patience = if *rung == 0 { RPC_TIMEOUT } else { self.runs.host.interrupt_step };
            let still_target = self.state.active == *turn && (turn.is_some() || self.state.queue.iter().any(|q| q.request.is_some()));
            if at.elapsed() > patience && still_target {
                // A CLI wedged before announcing a turn is cancellable too.
                self.link.kill_claude(if *rung == 0 { libc::SIGTERM } else { libc::SIGKILL });
                self.interrupt = Some((turn.clone(), Instant::now(), 1));
            }
        }
        Ok(())
    }

    async fn close_archived(&mut self) {
        // Closing this per-session server is safe only after archive succeeds.
        // Keep its archived checkpoint if EOF cannot be sent, for next boot.
        match tokio::time::timeout(RPC_TIMEOUT, self.link.close_stdin()).await {
            Ok(Ok(())) => self.runs.host.journal.remove(&self.record.run_id),
            _ => tracing::warn!(session = %self.record.session_id, "archived Codex server could not be closed; keeping its cleanup record"),
        }
    }

    /// Whether `stop` names the turn running, or the one being started: the
    /// turn by its id, or the turn a message's start started or is starting.
    fn stops_running(&self, stop: &Stop) -> bool {
        match stop {
            Stop::Turn(turn) => self.state.active.as_deref() == Some(turn.as_str()),
            Stop::Message(message) => {
                let its_start =
                    |queued: &Message| &queued.id == message && queued.request.as_deref().is_some_and(|request| request.starts_with("start:"));
                (self.state.active.is_some() && self.state.active_message.as_ref() == Some(message)) || self.state.queue.iter().any(its_start)
            }
        }
    }

    /// Whether `message` waits to start by itself: it is queued, and the queue
    /// is not paused, so this driver starts it in its turn.
    fn waits_to_start(&self, message: &str) -> bool {
        !self.state.paused && self.state.queue.iter().any(|queued| queued.id == message)
    }

    async fn send_interrupt(&mut self) -> Result<(), InjectError> {
        let waiting_for_id = self.interrupt.as_ref().is_some_and(|(turn, _, rung)| turn.is_none() && *rung == 0);
        if self.state.interrupt_requested && (self.interrupt.is_none() || waiting_for_id) && self.link.at() >= self.link.total() {
            if let Some(turn) = self.state.active.clone() {
                let id = new_uuid();
                self.interrupt = Some((Some(turn.clone()), Instant::now(), 0));
                let request = Request {
                    id: &id,
                    method: "turn/interrupt",
                    params: TurnInterruptParams {
                        thread_id: &self.record.session_id,
                        turn_id: &turn,
                    },
                };
                write(&mut self.link, &request).await?;
            } else if self.interrupt.is_none() && self.state.queue.iter().any(|q| q.request.is_some()) {
                self.interrupt = Some((None, Instant::now(), 0));
            }
        }
        Ok(())
    }
}
