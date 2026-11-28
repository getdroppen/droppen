//! Interactive `AskUserQuestion` and tool permission handling for a Claude resident.

use super::*;

/// A question claude is blocked on, as the driver holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OpenQuestion {
    /// The CLI's id for the request; the answer has to name it.
    pub(super) request_id: String,
    /// The `tool_use` it is, which is how its answer is seen on stdout.
    pub(super) tool_use_id: Option<String>,
    /// The tool input's `questions`, verbatim: the answer echoes it.
    pub(super) questions: Value,
}

impl OpenQuestion {
    pub(super) fn of(request: &PermissionRequest) -> OpenQuestion {
        OpenQuestion {
            request_id: request.request_id.clone(),
            tool_use_id: request.tool_use_id.clone(),
            questions: request.questions(),
        }
    }
}

/// A tool permission prompt claude is blocked on, as the driver holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OpenPermission {
    pub(super) request_id: String,
    pub(super) tool_use_id: Option<String>,
    pub(super) tool_name: String,
    pub(super) input: Value,
    pub(super) reason: Option<String>,
    pub(super) permission_suggestions: Option<Value>,
    pub(super) options: Vec<PermissionOption>,
}

impl OpenPermission {
    pub(super) fn of(request: &PermissionRequest) -> OpenPermission {
        OpenPermission {
            request_id: request.request_id.clone(),
            tool_use_id: request.tool_use_id.clone(),
            tool_name: request.tool_name.clone(),
            input: request.input.clone(),
            reason: request.reason.clone(),
            permission_suggestions: request.permission_suggestions.clone(),
            options: permissions::claude_options(request),
        }
    }
}

impl Driver {
    // ----------------------------------------------------------- questions

    /// claude stopped to ask the user something: hold the question open.
    ///
    /// The turn stays in flight — claude is blocked mid-turn, and will be until
    /// the answer is written back — so a question is *working*, not idle, to
    /// everything that reads the phase. What changes is everything that would
    /// otherwise end the process or the turn while the user thinks: the stall
    /// watchdog, the ghost-turn check, the idle retirement, a settings respawn,
    /// a move ahead of a limit. Each of them stands down while [`Driver::asked`]
    /// is not empty. A stop does not: the user may take back their own turn.
    pub(super) fn ask(&mut self, request: &PermissionRequest) {
        // The `assistant` line carrying the call came first and normally
        // opened the turn already; this covers a question whose call this
        // driver never saw.
        self.on_speech();
        tracing::info!(session = %self.id, request = %request.request_id, "claude is asking the user a question");
        self.hold_question(OpenQuestion::of(request));
    }

    /// Hold a question claude is blocked on open: on record, on the mirror, on
    /// every screen following the session, and — the first time any server
    /// hears of it — on the phone. A question this driver inherited across a
    /// restart is already on record, which is what keeps the phone quiet.
    pub(super) fn hold_question(&mut self, question: OpenQuestion) {
        let fresh = self.runs.host.questions().open(
            &question.request_id,
            &self.id,
            question.tool_use_id.as_deref(),
            &question.questions,
            questions::now_ms(),
        );
        let headline = questions::headline(&question.questions);
        self.recorder
            .lifecycle(Stage::Question, Some(&format!("{} asked: {headline}", question.request_id)));
        if fresh {
            if let Some(alerts) = self.runs.host.alerts.get() {
                alerts.question(self.runs.host.paths.clone(), Source::Claude, self.id.clone(), headline);
            }
        }
        if !self.asked.iter().any(|open| open.request_id == question.request_id) {
            self.asked.push(question);
        }
        self.publish();
        self.announce();
    }

    /// Stop holding a question open, and say what became of it. A no-op for
    /// one this driver is not holding.
    pub(super) fn withdraw(&mut self, request_id: &str, to: QuestionState, why: &str) {
        let Some(at) = self.asked.iter().position(|question| question.request_id == request_id) else {
            return;
        };
        self.asked.remove(at);
        // Only out of `pending`: an answer this driver wrote has already moved
        // the record, and claude echoing it back must not rewrite it.
        if self.runs.host.questions().close(request_id, to, questions::now_ms()) {
            tracing::info!(session = %self.id, request = %request_id, state = to.as_str(), why, "a question closed without an answer");
            self.recorder
                .lifecycle(Stage::Question, Some(&format!("{request_id} {}: {why}", to.as_str())));
        }
        self.publish();
        self.announce();
    }

    /// Whether claude is currently blocked waiting on user input (a question
    /// or a tool permission prompt).
    pub(super) fn awaiting_user(&self) -> bool {
        !self.asked.is_empty() || !self.pending_permissions.is_empty()
    }

    /// Close every question and permission prompt this driver is holding open.
    pub(super) fn close_questions(&mut self, to: QuestionState, why: &str) {
        let open: Vec<String> = self.asked.iter().map(|question| question.request_id.clone()).collect();
        for request_id in open {
            self.withdraw(&request_id, to, why);
        }
        let open_perms: Vec<String> = self.pending_permissions.iter().map(|perm| perm.request_id.clone()).collect();
        for request_id in open_perms {
            self.withdraw_permission(&request_id, why);
        }
    }

    /// claude stopped to ask permission to run a tool: hold it open for the user.
    pub(super) fn ask_permission(&mut self, request: &PermissionRequest) {
        self.on_speech();
        tracing::info!(session = %self.id, request = %request.request_id, tool = %request.tool_name, "claude is asking for tool permission");
        self.hold_permission(OpenPermission::of(request));
    }

    fn permission_spec(&self, perm: &OpenPermission) -> PermissionSpec {
        let redact = |text: &str| self.runs.host.redact(text);
        let req = PermissionRequest {
            request_id: perm.request_id.clone(),
            tool_name: perm.tool_name.clone(),
            tool_use_id: perm.tool_use_id.clone(),
            input: perm.input.clone(),
            reason: perm.reason.clone(),
            permission_suggestions: perm.permission_suggestions.clone(),
        };
        let title = permissions::claude_title(&req, redact);
        let reason = perm.reason.as_deref().map(|r| permissions::clip_label(r, redact));
        let input = permissions::format_input_preview(&perm.input, redact);
        PermissionSpec {
            request_id: perm.request_id.clone(),
            session_id: self.id.clone(),
            tool_call_id: perm.tool_use_id.clone(),
            title,
            tool_name: (!perm.tool_name.is_empty()).then(|| perm.tool_name.clone()),
            reason,
            input,
            options: perm.options.clone(),
        }
    }

    pub(super) fn hold_permission(&mut self, perm: OpenPermission) {
        let spec = self.permission_spec(&perm);
        let fresh = self.runs.host.permissions().open(&spec, permissions::now_ms());
        let headline = permissions::headline(&spec.title);
        self.recorder
            .lifecycle(Stage::Permission, Some(&format!("{} asked: {}", perm.request_id, spec.title)));
        if fresh {
            if let Some(alerts) = self.runs.host.alerts.get() {
                alerts.question(self.runs.host.paths.clone(), Source::Claude, self.id.clone(), headline);
            }
        }
        if !self.pending_permissions.iter().any(|open| open.request_id == perm.request_id) {
            self.pending_permissions.push(perm);
        }
        self.publish();
        self.announce();
    }

    pub(super) fn withdraw_permission(&mut self, request_id: &str, why: &str) {
        let Some(at) = self.pending_permissions.iter().position(|perm| perm.request_id == request_id) else {
            return;
        };
        self.pending_permissions.remove(at);
        if self.runs.host.permissions().close(request_id, PermissionState::Cancelled, permissions::now_ms()) {
            tracing::info!(session = %self.id, request = %request_id, why, "a permission prompt closed without a decision");
            self.recorder
                .lifecycle(Stage::Permission, Some(&format!("{request_id} cancelled: {why}")));
        }
        self.publish();
        self.announce();
    }

    pub(super) async fn respond_permission(&mut self, request_id: &str, reply: PermissionReply, by: Option<String>) -> PermissionOutcome {
        if self.replaying {
            return PermissionOutcome::NotReady;
        }
        let Some(at) = self.pending_permissions.iter().position(|perm| perm.request_id == request_id) else {
            return PermissionOutcome::Unknown;
        };
        let perm = self.pending_permissions[at].clone();
        let (to, selected, response) = match reply {
            PermissionReply::Select(option_id) => {
                if !perm.options.iter().any(|opt| opt.id == option_id) {
                    return PermissionOutcome::Invalid(format!("unknown permission option: {option_id}"));
                }
                let response = match option_id.as_str() {
                    permissions::CLAUDE_ALLOW => permissions::claude_allow(&perm.input, None),
                    permissions::CLAUDE_ALLOW_ALWAYS => permissions::claude_allow(&perm.input, perm.permission_suggestions.as_ref()),
                    _ => permissions::claude_deny(&perm.tool_name, false),
                };
                (PermissionState::Resolved, Some(option_id), response)
            }
            PermissionReply::Cancel => (
                PermissionState::Cancelled,
                None,
                permissions::claude_deny(&perm.tool_name, true),
            ),
        };
        let store = self.runs.host.permissions().clone();
        let now = permissions::now_ms();
        let stored = match store.resolve(request_id, to, selected.as_deref(), Some(&response), by.as_deref(), now) {
            Ok(stored) => stored,
            Err(Some(current)) => return PermissionOutcome::Resolved(current.wire()),
            Err(None) => {
                let spec = self.permission_spec(&perm);
                store.open(&spec, now);
                match store.resolve(request_id, to, selected.as_deref(), Some(&response), by.as_deref(), now) {
                    Ok(stored) => stored,
                    Err(current) => return current.map_or(PermissionOutcome::Unknown, |current| PermissionOutcome::Resolved(current.wire())),
                }
            }
        };
        let line = questions::control_response(request_id, &response);
        self.recorder.lifecycle(
            Stage::Permission,
            Some(&format!(
                "{request_id} {} ({}) by {}",
                to.as_str(),
                selected.as_deref().unwrap_or("cancel"),
                by.as_deref().unwrap_or("someone")
            )),
        );
        self.recorder.stdin(&line);
        if let Err(error) = self.link.send_line(&line).await {
            tracing::warn!(session = %self.id, request = %request_id, %error, "cannot write a permission response to claude");
            store.rewind(request_id, to, now);
            return PermissionOutcome::Unreachable;
        }
        tracing::info!(session = %self.id, request = %request_id, state = to.as_str(), "a permission prompt was decided");
        self.pending_permissions.remove(at);
        self.touch();
        self.publish();
        self.announce();
        PermissionOutcome::Delivered(stored.wire())
    }

    /// Answer a control request this server has no handler for, so claude is
    /// not left waiting on one that will never come.
    pub(super) async fn decline_request(&mut self, request_id: Option<String>) {
        let Some(request_id) = request_id else { return };
        tracing::info!(session = %self.id, request = %request_id, "claude made a control request this server does not handle; declining it");
        let line = questions::control_error(&request_id, "Droppen does not handle this request");
        self.recorder.stdin(&line);
        if let Err(error) = self.link.send_line(&line).await {
            tracing::warn!(session = %self.id, %error, "cannot decline a control request");
        }
    }

    /// Write the user's answer — or their refusal to give one — to a question
    /// claude is blocked on.
    ///
    /// The record moves first, by compare-and-set out of `pending`: of two
    /// devices answering at once, the one that loses is told who won, and
    /// nothing of its answer reaches claude. Only then is the response
    /// written; a write that fails puts the record back, and the death the
    /// failure means expires it properly a moment later.
    pub(super) async fn answer(&mut self, request_id: &str, reply: Reply, by: Option<String>) -> AnswerOutcome {
        if self.replaying {
            return AnswerOutcome::NotReady;
        }
        let Some(at) = self.asked.iter().position(|question| question.request_id == request_id) else {
            return AnswerOutcome::Unknown;
        };
        let question = self.asked[at].clone();
        let (to, answer, response) = match reply {
            Reply::Answer(answer) => {
                if let Err(why) = questions::check_answer(&question.questions, &answer) {
                    return AnswerOutcome::Invalid(why);
                }
                let response = questions::allow(&question.questions, &answer);
                (QuestionState::Answered, Some(answer), response)
            }
            Reply::Dismiss => (QuestionState::Dismissed, None, questions::deny(questions::DISMISSED)),
        };
        let store = self.runs.host.questions().clone();
        let now = questions::now_ms();
        let stored = match store.resolve(
            request_id,
            &[QuestionState::Pending],
            to,
            answer.as_ref(),
            Some(&response),
            by.as_deref(),
            now,
        ) {
            Ok(stored) => stored,
            Err(Some(current)) => return AnswerOutcome::Resolved(current.wire()),
            // Not on record at all, which only a database that refused the
            // write when the question arrived can explain. The question is
            // real — claude is blocked on it — so it is put on record now and
            // answered the same way.
            Err(None) => {
                store.open(request_id, &self.id, question.tool_use_id.as_deref(), &question.questions, now);
                match store.resolve(
                    request_id,
                    &[QuestionState::Pending],
                    to,
                    answer.as_ref(),
                    Some(&response),
                    by.as_deref(),
                    now,
                ) {
                    Ok(stored) => stored,
                    Err(current) => return current.map_or(AnswerOutcome::Unknown, |current| AnswerOutcome::Resolved(current.wire())),
                }
            }
        };
        let line = questions::control_response(request_id, &response);
        self.recorder.lifecycle(
            Stage::Question,
            Some(&format!("{request_id} {} by {}", to.as_str(), by.as_deref().unwrap_or("someone"))),
        );
        self.recorder.stdin(&line);
        if let Err(error) = self.link.send_line(&line).await {
            tracing::warn!(session = %self.id, request = %request_id, %error, "cannot write an answer to claude");
            store.rewind(request_id, to, QuestionState::Pending, now);
            return AnswerOutcome::Unreachable;
        }
        tracing::info!(session = %self.id, request = %request_id, state = to.as_str(), "a question was answered");
        self.asked.remove(at);
        // The silence while the user thought was not claude being stuck.
        self.touch();
        self.publish();
        self.announce();
        AnswerOutcome::Delivered(stored.wire())
    }

    /// Tell the session's screens and the list that something no file shows
    /// has changed: a question opened or closed.
    pub(super) fn announce(&self) {
        self.runs.host.announce(Source::Claude, &self.id);
    }
}
