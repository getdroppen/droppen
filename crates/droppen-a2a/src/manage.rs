//! `/api/a2a/*`: the entries, the tokens, the task list and the open
//! contexts, for this device's own pages, behind the local login
//! (`docs/a2a.md` §2).
//!
//! A token's secret is in the answer to its creation and nowhere else, ever.
//! Revoking a token, switching an entry off and taking a device out of its
//! `allowedDevices` take back what the caller has there at once (§6).

use std::sync::Arc;

use gotcha::axum::extract::FromRef;
use gotcha::{api, GotchaRouter, Json, Schematic, State};
use serde::{Deserialize, Serialize};

use droppen_base::time::now_ms;
use droppen_http::error::ApiError;
use droppen_http::extract::{ApiJson, ApiPath, ApiQuery};
use droppen_http::openapi_ext::Nullable;
use droppen_http::SendResult;

use crate::backend::{EntrySource, Isolation};
use crate::devices::A2aDeviceList;
use crate::engine::{decode_page_token, encode_page_token, iso_ms, A2a};
use crate::entry::{
    clean_account, clean_cwd, clean_description, clean_devices, clean_flag, clean_name, clean_origins, clean_template, is_bypass, valid_id, A2aEntry,
    A2aEntryCreate, A2aEntryUpdate, BYPASS_PERMISSIONS, DEFAULT_TEMPLATE, MAX_CONCURRENT,
};
use crate::store::{TaskFilter, TokenRow};
use crate::wire::TaskState;

/// How many entries one instance may hold.
const MAX_ENTRIES: usize = 50;
/// How many unrevoked tokens one instance may hold.
const MAX_TOKENS: usize = 100;
/// How many tasks the list answers with.
const TASK_LIST_LIMIT: usize = 500;
/// How many contexts a page of the context list holds at most, and by
/// default.
const CONTEXT_PAGE: usize = 500;

/// What the management routes use of the server's state.
#[derive(Clone)]
pub struct A2aState {
    /// The A2A server.
    pub a2a: Arc<A2a>,
}
droppen_http::substate!(A2aState);

/// The entry list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct A2aEntryList {
    /// Every entry, oldest first.
    pub entries: Vec<A2aEntry>,
    /// The template an entry whose `template` is `null` uses.
    pub default_template: String,
    /// The base of the Agent Cards' interface URLs (`[a2a] public_base`);
    /// `null` when not set, and the card then uses the request's `Host`.
    pub public_base: Nullable<String>,
}

/// A token, without its secret.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct A2aToken {
    /// Its id: the `<id>` of `dpa_<id>_<secret>`. Its tasks' caller is
    /// `a2a:token:<id>`.
    pub id: String,
    /// Its name, for a person: two tokens may share one.
    pub name: String,
    /// The entries it reaches.
    pub entries: Vec<String>,
    /// When it was made, ISO-8601.
    pub created_at: String,
    /// When it last proved a request, ISO-8601, to the minute.
    pub last_used_at: Nullable<String>,
    /// When it was revoked, ISO-8601.
    pub revoked_at: Nullable<String>,
}

impl From<&TokenRow> for A2aToken {
    fn from(row: &TokenRow) -> Self {
        A2aToken {
            id: row.id.clone(),
            name: row.name.clone(),
            entries: row.entries.clone(),
            created_at: iso_ms(row.created_ms),
            last_used_at: Nullable(row.last_used_ms.map(iso_ms)),
            revoked_at: Nullable(row.revoked_ms.map(iso_ms)),
        }
    }
}

/// The token list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
pub struct A2aTokenList {
    /// Every token, revoked ones included, the newest first.
    pub tokens: Vec<A2aToken>,
}

/// The body of `POST /api/a2a/tokens`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
pub struct A2aTokenCreate {
    /// Its name, for a person: 1 to 100 characters.
    pub name: String,
    /// The entries it may reach: at least one.
    pub entries: Vec<String>,
}

/// A new token, with the one copy of its secret there will ever be.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
pub struct A2aTokenCreated {
    /// The token.
    pub token: A2aToken,
    /// `dpa_<id>_<secret>`: shown now and never again.
    pub secret: String,
}

/// One task, for the page's list and the session list's "from A2A" mark.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct A2aTaskSummary {
    /// The task.
    pub id: String,
    /// Its context.
    pub context_id: String,
    /// Its entry.
    pub entry: String,
    /// Who sent it: `a2a:token:<token id>`, or an A2A proxy's
    /// `a2a:device:<device id>`.
    pub caller: String,
    /// The name of the token it came with, while that token is on file.
    pub caller_name: Nullable<String>,
    /// Where it stands.
    pub state: TaskState,
    /// Its status message's text, if any.
    pub status_text: Nullable<String>,
    /// Its session's CLI.
    pub source: EntrySource,
    /// Its session, once known.
    pub session_id: Nullable<String>,
    /// The directory the session runs in.
    pub cwd: String,
    /// The worktree's branch.
    pub branch: Nullable<String>,
    /// When it was made, ISO-8601.
    pub created_at: String,
    /// When its status last changed, ISO-8601.
    pub updated_at: String,
}

/// The task list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
pub struct A2aTaskList {
    /// The most recently changed first, at most 500.
    pub tasks: Vec<A2aTaskSummary>,
}

/// An open context: a session an A2A client started, for the session list's
/// "from A2A" mark.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct A2aContextSummary {
    /// The context.
    pub context_id: String,
    /// The entry it came in through.
    pub entry: String,
    /// Who opened it: `a2a:token:<token id>`.
    pub caller: String,
    /// The name of the token it came with, while that token is on file.
    pub caller_name: Nullable<String>,
    /// Its session's CLI.
    pub source: EntrySource,
    /// Its session, once known.
    pub session_id: Nullable<String>,
}

/// A page of the open contexts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct A2aContextList {
    /// The newest first.
    pub contexts: Vec<A2aContextSummary>,
    /// The `pageToken` of the next page; `null` on the last.
    pub next_page_token: Nullable<String>,
}

/// Query for `GET /api/a2a/contexts`.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct A2aContextQuery {
    /// How many contexts a page holds: 1 to 500, 500 when not said.
    pub page_size: Option<u32>,
    /// Where to go on from: the previous page's `nextPageToken`.
    pub page_token: Option<String>,
}

/// Query for `GET /api/a2a/tasks`.
#[derive(Debug, Clone, Deserialize, Serialize, Schematic)]
pub struct A2aTaskQuery {
    /// Only this entry's tasks.
    pub entry: Option<String>,
    /// Only tasks in this state: `TASK_STATE_WORKING`, or `working`.
    pub state: Option<String>,
}

fn bad(message: impl Into<String>) -> ApiError {
    ApiError::bad_request(message)
}

fn write_failed(error: rusqlite::Error) -> ApiError {
    ApiError::internal(format!("failed to write the A2A store: {error}"))
}

impl A2a {
    /// Check an entry's settings, the backend's word included.
    async fn checked(&self, mut entry: A2aEntry) -> Result<A2aEntry, ApiError> {
        if entry.source == EntrySource::Tck {
            if !self.settings().tck {
                return Err(bad("a tck entry is for the A2A TCK only: start the server with DROPPEN_A2A_TCK=1"));
            }
            entry.isolation = Isolation::None;
        }
        match (entry.source, entry.acp_agent.0.is_some()) {
            (EntrySource::Acp, false) => return Err(bad("an acp entry names its acpAgent")),
            (source, true) if source != EntrySource::Acp => return Err(bad("acpAgent is for an acp entry only")),
            _ => {}
        }
        // Again as the entry now stands: a `tck` entry's loopback origins do
        // not come along when it becomes another kind.
        entry.push_origins = clean_origins(&entry.push_origins, entry.source, self.settings().tck).map_err(bad)?;
        if !(1..=MAX_CONCURRENT).contains(&entry.max_concurrent) {
            return Err(bad(format!("maxConcurrent is 1 to {MAX_CONCURRENT}")));
        }
        // §6: an A2A proxy is somebody else's agent; it never drives a CLI
        // whose guard rails are off.
        if !entry.allowed_devices.is_empty() && is_bypass(entry.permission.0.as_deref()) {
            return Err(bad(format!(
                "an entry with allowedDevices cannot run in a bypass permission mode ({})",
                BYPASS_PERMISSIONS.join(", ")
            )));
        }
        if entry.source != EntrySource::Tck {
            self.backend().check_entry(&entry.run_spec()).await.map_err(bad)?;
        }
        Ok(entry)
    }

    /// `POST /api/a2a/entries`.
    pub async fn create_entry(&self, body: A2aEntryCreate) -> Result<A2aEntry, ApiError> {
        if !valid_id(&body.id) {
            return Err(bad("id is ^[a-z0-9][a-z0-9-]{0,39}$"));
        }
        if self.store().entries().len() >= MAX_ENTRIES {
            return Err(ApiError::conflict(format!("at most {MAX_ENTRIES} A2A entries")));
        }
        let now = now_ms();
        let entry = A2aEntry {
            id: body.id,
            name: clean_name(&body.name).map_err(bad)?,
            description: clean_description(body.description.as_deref().unwrap_or_default()).map_err(bad)?,
            enabled: body.enabled.0.unwrap_or(false),
            source: body.source,
            acp_agent: Nullable(clean_flag("acpAgent", body.acp_agent.as_deref()).map_err(bad)?),
            cwd: clean_cwd(body.source, body.cwd.as_deref()).map_err(bad)?,
            account: clean_account(body.account.0.unwrap_or_default()).map_err(bad)?,
            model: Nullable(clean_flag("model", body.model.as_deref()).map_err(bad)?),
            effort: Nullable(clean_flag("effort", body.effort.as_deref()).map_err(bad)?),
            permission: Nullable(clean_flag("permission", body.permission.as_deref()).map_err(bad)?),
            isolation: body.isolation.0.unwrap_or_default(),
            max_concurrent: body.max_concurrent.0.unwrap_or(1),
            template: Nullable(clean_template(body.template.as_deref()).map_err(bad)?),
            allow_remote_answers: body.allow_remote_answers.0.unwrap_or(true),
            push_origins: clean_origins(body.push_origins.0.as_deref().unwrap_or_default(), body.source, self.settings().tck).map_err(bad)?,
            allowed_devices: clean_devices(body.allowed_devices.0.as_deref().unwrap_or_default()).map_err(bad)?,
            created_at: iso_ms(now),
            updated_at: iso_ms(now),
        };
        let entry = self.checked(entry).await?;
        if !self.store().insert_entry(&entry, now).map_err(write_failed)? {
            return Err(ApiError::conflict(format!("an entry {} exists already", entry.id)));
        }
        Ok(entry)
    }

    /// `PATCH /api/a2a/entries/{id}`.
    pub async fn update_entry(&self, id: &str, body: A2aEntryUpdate) -> Result<A2aEntry, ApiError> {
        let before = self.store().entry(id).ok_or_else(|| ApiError::not_found("A2A entry not found"))?;
        let mut entry = before.clone();
        let nullable = |field: &str, current: &Nullable<String>, patch: Option<&str>| -> Result<Nullable<String>, ApiError> {
            match patch {
                None => Ok(current.clone()),
                Some(value) => clean_flag(field, Some(value)).map(Nullable).map_err(bad),
            }
        };
        if let Some(name) = body.name.as_deref() {
            entry.name = clean_name(name).map_err(bad)?;
        }
        if let Some(description) = body.description.as_deref() {
            entry.description = clean_description(description).map_err(bad)?;
        }
        if let Some(enabled) = body.enabled.0 {
            entry.enabled = enabled;
        }
        if let Some(source) = body.source.0 {
            entry.source = source;
        }
        entry.acp_agent = nullable("acpAgent", &entry.acp_agent, body.acp_agent.as_deref())?;
        if body.cwd.0.is_some() || body.source.0.is_some() {
            entry.cwd = clean_cwd(entry.source, Some(body.cwd.as_deref().unwrap_or(&entry.cwd))).map_err(bad)?;
        }
        if let Some(account) = body.account.0.clone() {
            entry.account = clean_account(account).map_err(bad)?;
        }
        entry.model = nullable("model", &entry.model, body.model.as_deref())?;
        entry.effort = nullable("effort", &entry.effort, body.effort.as_deref())?;
        entry.permission = nullable("permission", &entry.permission, body.permission.as_deref())?;
        if let Some(isolation) = body.isolation.0 {
            entry.isolation = isolation;
        }
        if let Some(max) = body.max_concurrent.0 {
            entry.max_concurrent = max;
        }
        if let Some(template) = body.template.as_deref() {
            entry.template = Nullable(clean_template(Some(template)).map_err(bad)?);
        }
        if let Some(allow) = body.allow_remote_answers.0 {
            entry.allow_remote_answers = allow;
        }
        if let Some(origins) = &body.push_origins.0 {
            entry.push_origins = clean_origins(origins, entry.source, self.settings().tck).map_err(bad)?;
        }
        if let Some(devices) = &body.allowed_devices.0 {
            entry.allowed_devices = clean_devices(devices).map_err(bad)?;
        }
        let now = now_ms();
        entry.updated_at = iso_ms(now);
        let entry = self.checked(entry).await?;
        // A change of where and how its sessions run closes the contexts it
        // had (§6).
        self.replace_entry(&before, &entry, now).await.map_err(write_failed)?;
        if before.enabled && !entry.enabled {
            self.revoke(None, Some(&entry.id)).await;
        }
        for device in before.allowed_devices.iter().filter(|device| !entry.allowed_devices.contains(device)) {
            self.revoke(Some(&format!("a2a:device:{device}")), Some(&entry.id)).await;
        }
        self.pump(&entry.id);
        Ok(entry)
    }

    /// `DELETE /api/a2a/entries/{id}`: its unfinished tasks are cancelled
    /// first, and it leaves every token's scope.
    pub async fn delete_entry(&self, id: &str) -> Result<(), ApiError> {
        self.store().entry(id).ok_or_else(|| ApiError::not_found("A2A entry not found"))?;
        self.revoke(None, Some(id)).await;
        self.store().delete_entry(id).map_err(write_failed)?;
        self.store().drop_entry_from_tokens(id).map_err(write_failed)?;
        Ok(())
    }

    /// `POST /api/a2a/tokens`.
    pub fn create_token(&self, body: A2aTokenCreate) -> Result<A2aTokenCreated, ApiError> {
        let name = body.name.trim().to_string();
        if !crate::token::valid_name(&name) {
            return Err(bad("a token's name is 1 to 100 characters, none of them a control character"));
        }
        let tokens = self.store().tokens();
        let live: Vec<&TokenRow> = tokens.iter().filter(|token| token.revoked_ms.is_none()).collect();
        if live.len() >= MAX_TOKENS {
            return Err(ApiError::conflict(format!("at most {MAX_TOKENS} A2A tokens")));
        }
        let mut entries: Vec<String> = Vec::new();
        for entry in &body.entries {
            if self.store().entry(entry).is_none() {
                return Err(bad(format!("no A2A entry {entry}")));
            }
            if !entries.contains(entry) {
                entries.push(entry.clone());
            }
        }
        if entries.is_empty() {
            return Err(bad("a token reaches at least one entry"));
        }
        let (id, secret, digest) = crate::token::mint();
        let row = TokenRow {
            id,
            name,
            entries,
            secret_hash: digest,
            created_ms: now_ms(),
            last_used_ms: None,
            revoked_ms: None,
        };
        self.store().insert_token(&row).map_err(write_failed)?;
        Ok(A2aTokenCreated {
            token: A2aToken::from(&row),
            secret,
        })
    }

    /// `DELETE /api/a2a/tokens/{id}`: the token stops proving anything, and
    /// what its caller has is taken back (§6).
    pub async fn revoke_token(&self, id: &str) -> Result<(), ApiError> {
        let token = self.store().token(id).ok_or_else(|| ApiError::not_found("A2A token not found"))?;
        let newly = self
            .store()
            .revoke_token(id, now_ms())
            .map_err(write_failed)?
            .ok_or_else(|| ApiError::not_found("A2A token not found"))?;
        if newly {
            let caller = crate::token::caller_of(&token.id);
            for entry in &token.entries {
                self.revoke(Some(&caller), Some(entry)).await;
            }
        }
        Ok(())
    }

    /// `GET /api/a2a/contexts`.
    pub fn context_list(&self, query: &A2aContextQuery) -> Result<A2aContextList, ApiError> {
        let size = match query.page_size {
            None => CONTEXT_PAGE,
            Some(n) if (1..=CONTEXT_PAGE as u32).contains(&n) => n as usize,
            Some(_) => return Err(bad(format!("pageSize is 1 to {CONTEXT_PAGE}"))),
        };
        let after = match query.page_token.as_deref().filter(|token| !token.is_empty()) {
            None => None,
            Some(token) => Some(decode_page_token(token).ok_or_else(|| bad("pageToken is not one this server gave"))?),
        };
        let mut rows = self.store().open_contexts(after, size + 1);
        let next_page_token = (rows.len() > size).then(|| {
            rows.truncate(size);
            rows.last()
                .map(|last| encode_page_token(last.created_ms, &last.context_id))
                .unwrap_or_default()
        });
        let contexts = rows
            .into_iter()
            .map(|row| A2aContextSummary {
                caller_name: Nullable(
                    row.caller
                        .strip_prefix("a2a:token:")
                        .and_then(|id| self.store().token(id))
                        .map(|token| token.name),
                ),
                context_id: row.context_id,
                entry: row.entry,
                caller: row.caller,
                source: row.session.source,
                session_id: Nullable(row.session.session_id),
            })
            .collect();
        Ok(A2aContextList {
            contexts,
            next_page_token: Nullable(next_page_token),
        })
    }

    /// `GET /api/a2a/tasks`.
    pub fn task_list(&self, query: &A2aTaskQuery) -> Result<A2aTaskList, ApiError> {
        let state = match query.state.as_deref().map(str::trim).filter(|state| !state.is_empty()) {
            None => None,
            Some(name) => Some(TaskState::parse(name).ok_or_else(|| bad(format!("no task state {name}")))?),
        };
        let filter = TaskFilter {
            entry: query.entry.clone().filter(|entry| !entry.is_empty()),
            state,
            ..TaskFilter::default()
        };
        let (rows, _) = self.store().list(&filter, None, TASK_LIST_LIMIT);
        let tasks = rows
            .into_iter()
            .map(|row| {
                let context = self.store().context(&row.context_id);
                let status_text = row.status_message.as_ref().and_then(|message| {
                    message.parts.iter().find_map(|part| match &part.content {
                        crate::wire::PartContent::Text(text) => Some(text.clone()),
                        _ => None,
                    })
                });
                A2aTaskSummary {
                    id: row.id.clone(),
                    context_id: row.context_id.clone(),
                    entry: row.entry.clone(),
                    caller: row.caller.clone(),
                    caller_name: Nullable(
                        row.caller
                            .strip_prefix("a2a:token:")
                            .and_then(|id| self.store().token(id))
                            .map(|token| token.name),
                    ),
                    state: row.state,
                    status_text: Nullable(status_text),
                    source: context.as_ref().map_or(EntrySource::Claude, |context| context.session.source),
                    session_id: Nullable(context.as_ref().and_then(|context| context.session.session_id.clone())),
                    cwd: context.as_ref().map(|context| context.session.cwd.clone()).unwrap_or_default(),
                    branch: Nullable(context.as_ref().and_then(|context| context.session.branch.clone())),
                    created_at: iso_ms(row.created_ms),
                    updated_at: iso_ms(row.status_ms),
                }
            })
            .collect();
        Ok(A2aTaskList { tasks })
    }
}

/// Every A2A entry.
#[api(
    id = "list_a2a_entries",
    group = "a2a",
    summary = "List A2A entries",
    responses(200 = "Every A2A entry, the default template and the cards' public base")
)]
pub async fn list_a2a_entries(State(state): State<A2aState>) -> Json<A2aEntryList> {
    Json(A2aEntryList {
        entries: state.a2a.store().entries(),
        default_template: DEFAULT_TEMPLATE.to_string(),
        public_base: Nullable(state.a2a.settings().public_base.clone()),
    })
}

/// Create an A2A entry. It is off until `enabled` says otherwise.
#[api(
    id = "create_a2a_entry",
    group = "a2a",
    summary = "Create an A2A entry",
    responses(200 = "The entry"),
    errors(400 = "An unusable setting, or one the entry's CLI does not take", 409 = "The id is taken, or there are too many entries")
)]
pub async fn create_a2a_entry(State(state): State<A2aState>, ApiJson(body): ApiJson<A2aEntryCreate>) -> Result<Json<A2aEntry>, ApiError> {
    Ok(Json(state.a2a.create_entry(body).await?))
}

/// One A2A entry.
#[api(id = "get_a2a_entry", group = "a2a", summary = "Get an A2A entry", responses(200 = "The entry"), errors(404 = "A2A entry not found"))]
pub async fn get_a2a_entry(State(state): State<A2aState>, ApiPath(id): ApiPath<String>) -> Result<Json<A2aEntry>, ApiError> {
    state
        .a2a
        .store()
        .entry(&id)
        .map(Json)
        .ok_or_else(|| ApiError::not_found("A2A entry not found"))
}

/// Change an A2A entry. Switching it off cancels its unfinished tasks,
/// taking a device out of `allowedDevices` cancels that device's, and a new
/// source, directory, account, permission mode or isolation closes its
/// contexts.
#[api(
    id = "update_a2a_entry",
    group = "a2a",
    summary = "Update an A2A entry",
    responses(200 = "The entry"),
    errors(400 = "An unusable setting, or one the entry's CLI does not take", 404 = "A2A entry not found")
)]
pub async fn update_a2a_entry(
    State(state): State<A2aState>,
    ApiPath(id): ApiPath<String>,
    ApiJson(body): ApiJson<A2aEntryUpdate>,
) -> Result<Json<A2aEntry>, ApiError> {
    Ok(Json(state.a2a.update_entry(&id, body).await?))
}

/// Delete an A2A entry. Its unfinished tasks are cancelled; their sessions
/// stay.
#[api(id = "delete_a2a_entry", group = "a2a", summary = "Delete an A2A entry", responses(200 = "A2A entry deleted"), errors(404 = "A2A entry not found"))]
pub async fn delete_a2a_entry(State(state): State<A2aState>, ApiPath(id): ApiPath<String>) -> Result<Json<SendResult>, ApiError> {
    state.a2a.delete_entry(&id).await?;
    Ok(Json(SendResult { accepted: true }))
}

/// Every A2A token, without its secret.
#[api(id = "list_a2a_tokens", group = "a2a", summary = "List A2A tokens", responses(200 = "Every A2A token, revoked ones included"))]
pub async fn list_a2a_tokens(State(state): State<A2aState>) -> Json<A2aTokenList> {
    Json(A2aTokenList {
        tokens: state.a2a.store().tokens().iter().map(A2aToken::from).collect(),
    })
}

/// Create an A2A token. Its secret is in this answer and never again.
#[api(
    id = "create_a2a_token",
    group = "a2a",
    summary = "Create an A2A token",
    responses(200 = "The token and its secret, shown once"),
    errors(400 = "An unusable name, or an entry that does not exist", 409 = "The name is taken, or there are too many tokens")
)]
pub async fn create_a2a_token(State(state): State<A2aState>, ApiJson(body): ApiJson<A2aTokenCreate>) -> Result<Json<A2aTokenCreated>, ApiError> {
    Ok(Json(state.a2a.create_token(body)?))
}

/// Revoke an A2A token. Its caller's unfinished tasks are cancelled, their
/// push configurations deleted and their streams closed.
#[api(id = "revoke_a2a_token", group = "a2a", summary = "Revoke an A2A token", responses(200 = "A2A token revoked"), errors(404 = "A2A token not found"))]
pub async fn revoke_a2a_token(State(state): State<A2aState>, ApiPath(id): ApiPath<String>) -> Result<Json<SendResult>, ApiError> {
    state.a2a.revoke_token(&id).await?;
    Ok(Json(SendResult { accepted: true }))
}

/// The A2A tasks, the most recently changed first.
#[api(
    id = "list_a2a_tasks",
    group = "a2a",
    summary = "List A2A tasks",
    responses(200 = "The A2A tasks, the most recently changed first"),
    errors(400 = "Not a task state")
)]
pub async fn list_a2a_tasks(State(state): State<A2aState>, ApiQuery(query): ApiQuery<A2aTaskQuery>) -> Result<Json<A2aTaskList>, ApiError> {
    Ok(Json(state.a2a.task_list(&query)?))
}

/// The open A2A contexts: the sessions A2A clients started, newest first.
#[api(
    id = "list_a2a_contexts",
    group = "a2a",
    summary = "List open A2A contexts",
    responses(200 = "A page of the open A2A contexts, the newest first"),
    errors(400 = "A page size out of range, or a page token this server did not give")
)]
pub async fn list_a2a_contexts(State(state): State<A2aState>, ApiQuery(query): ApiQuery<A2aContextQuery>) -> Result<Json<A2aContextList>, ApiError> {
    Ok(Json(state.a2a.context_list(&query)?))
}

/// The A2A proxies this device takes A2A calls from, as its device chain
/// says now: what an entry's `allowedDevices` may usefully name. None on a
/// device not linked to Droppen.
#[api(
    id = "list_a2a_devices",
    group = "a2a",
    summary = "List the A2A proxies this device takes calls from",
    responses(200 = "The A2A proxies, by id")
)]
pub async fn list_a2a_devices(State(state): State<A2aState>) -> Json<A2aDeviceList> {
    Json(A2aDeviceList {
        devices: state.a2a.admitted_devices(),
    })
}

impl A2aState {
    /// Register the management routes.
    pub fn routes<S>(router: GotchaRouter<S>) -> GotchaRouter<S>
    where
        S: Clone + Send + Sync + 'static,
        A2aState: FromRef<S>,
    {
        router
            .get("/api/a2a/entries", list_a2a_entries)
            .post("/api/a2a/entries", create_a2a_entry)
            .get("/api/a2a/entries/{id}", get_a2a_entry)
            .patch("/api/a2a/entries/{id}", update_a2a_entry)
            .delete("/api/a2a/entries/{id}", delete_a2a_entry)
            .get("/api/a2a/tokens", list_a2a_tokens)
            .post("/api/a2a/tokens", create_a2a_token)
            .delete("/api/a2a/tokens/{id}", revoke_a2a_token)
            .get("/api/a2a/tasks", list_a2a_tasks)
            .get("/api/a2a/contexts", list_a2a_contexts)
            .get("/api/a2a/devices", list_a2a_devices)
    }
}
