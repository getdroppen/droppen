//! Where the first-run guide stands: `GET/PUT /api/setup/progress`.
//!
//! The guide's page cannot keep this itself. The app's web page is served from
//! the server's own port, which moves between launches, and its storage moves
//! with it. So the progress is one row of this instance's `kv`
//! table (`setup` / `progress`), the shape the page sends, read whole and
//! written whole: which steps are done or skipped, what steps 02 to 04 chose,
//! when the guide started and finished, and "don't remind me".
//!
//! Two fields are the server's own, whatever a `PUT` says:
//!
//! * `flow` — the version of the guide this server ships ([`FLOW`]);
//! * `autoOpen` — whether the app's first window opens the guide by itself:
//!   the desktop app runs this server, the guide never started, and this
//!   instance owns no sessions. An install that was in use before the guide
//!   existed never gets it; its session list shows a bar at most.
//!
//! The row is a record of what the guide showed and the user chose, nothing
//! more, and nothing may act on it. Any device that may use this computer can
//! write it (`PUT` is a `Use` route, `agent::bridge::ROUTES`), so a choice in
//! it that changes the computer's posture — `where.lan`, `wrap.startAtLogin` —
//! must never be applied from here unless that `PUT` becomes a `Posture`
//! route. The app keeps start at login itself (`desktop/`, on by default at a
//! new install's first launch); the LAN is set in Settings → Security.

use gotcha::{api, Json, Schematic, State};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{Setup, SetupState};
use droppen_sessions::TranscriptSources;
use droppen_instance::db::scope;
use droppen_http::error::ApiError;
use droppen_http::extract::ApiJson;
use droppen_http::openapi_ext::Nullable;

/// The `kv` key, under [`scope::SETUP`].
const PROGRESS_KEY: &str = "progress";

/// The version of the guide this server ships. A step remembers the version
/// it was completed in (`stepFlow`), so a later guide that changes one step
/// can tell who has not seen the new one.
pub const FLOW: u32 = 1;

/// Where one step stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum StepState {
    /// Not reached yet, or reached and left without Continue or Skip.
    #[default]
    Pending,
    /// Continued past.
    Done,
    /// Skipped; the rail says so and it can be opened again.
    Skipped,
}

/// The four steps' states.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(default)]
pub struct StepStates {
    /// 01 · AI tools.
    pub tools: StepState,
    /// 02 · Where to use it.
    #[serde(rename = "where")]
    pub where_to_use: StepState,
    /// 03 · First session.
    pub first: StepState,
    /// 04 · Finishing up.
    pub wrap: StepState,
}

/// The guide version each step was last completed in; `0` for never.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(default)]
pub struct StepFlows {
    /// 01 · AI tools.
    pub tools: u32,
    /// 02 · Where to use it.
    #[serde(rename = "where")]
    pub where_to_use: u32,
    /// 03 · First session.
    pub first: u32,
    /// 04 · Finishing up.
    pub wrap: u32,
}

/// Step 02's choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum WhereChoice {
    /// Only on this Mac.
    Local,
    /// On the phone and other devices too, through a Droppen account.
    Droppen,
}

/// What step 02 chose.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(default)]
pub struct WhereProgress {
    /// The choice; `null` until the step is left with one on screen.
    pub choice: Nullable<WhereChoice>,
    /// Browsers on the local network may reach this device too: what step 02
    /// asked for. A record only; nothing applies it from here.
    pub lan: bool,
}

/// The tool step 03 picked: [`super::ToolId`]'s two values, as a type of its own.
/// A `Nullable` enum is nullable wherever the schema names it
/// (`openapi_ext::hoist_nullable_refs`), and `ToolId` must stay non-null for
/// the tools report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum FirstAgent {
    /// Claude Code.
    Claude,
    /// Codex.
    Codex,
}

/// What step 03 chose for the first session. Nothing is started from it: the
/// guide's Finish opens the empty-session view with it filled in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(default)]
pub struct FirstProgress {
    /// The folder it runs in.
    pub cwd: Nullable<String>,
    /// The tool it runs with.
    pub agent: Nullable<FirstAgent>,
    /// The first message, in the words the user saw.
    pub prompt: Nullable<String>,
}

/// What step 04 chose.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(default, rename_all = "camelCase")]
pub struct WrapProgress {
    /// Start Droppen at login: `null` unless the guide asks, which it does not
    /// (the app turns it on at a new install's first launch, and the menu bar
    /// changes it). A record only; nothing applies it from here.
    pub start_at_login: Nullable<bool>,
}

/// `GET/PUT /api/setup/progress`: how far this instance's guide got.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(default, rename_all = "camelCase")]
pub struct SetupProgress {
    /// The guide version this server ships. Read-only: a `PUT`'s is ignored.
    pub flow: u32,
    /// When the guide was first started or skipped, ISO-8601; `null` until then.
    pub started_at: Nullable<String>,
    /// When Finish was pressed, ISO-8601; `null` while unfinished.
    pub finished_at: Nullable<String>,
    /// "Don't remind me" on the session list's resume bar.
    pub dismissed: bool,
    /// Each step's state.
    pub steps: StepStates,
    /// Each step's guide version when it was completed.
    pub step_flow: StepFlows,
    /// Step 02.
    #[serde(rename = "where")]
    pub where_to_use: WhereProgress,
    /// Step 03.
    pub first: FirstProgress,
    /// Step 04.
    pub wrap: WrapProgress,
    /// Read-only, the server's: the app's first window opens the guide by
    /// itself — the desktop app runs this server, the guide never started, and
    /// this instance owns no sessions.
    pub auto_open: bool,
}

impl Default for SetupProgress {
    fn default() -> Self {
        Self {
            flow: FLOW,
            started_at: Nullable(None),
            finished_at: Nullable(None),
            dismissed: false,
            steps: StepStates::default(),
            step_flow: StepFlows::default(),
            where_to_use: WhereProgress::default(),
            first: FirstProgress::default(),
            wrap: WrapProgress::default(),
            auto_open: false,
        }
    }
}

impl Setup {
    /// The stored progress, or a guide never started. Read field by field
    /// ([`lenient`]), so a value this build does not know costs that field
    /// and nothing else, and a row that is not JSON at all reads as never
    /// started rather than failing the page.
    pub fn stored_progress(&self) -> SetupProgress {
        let stored = self
            .db()
            .kv_get(scope::SETUP, PROGRESS_KEY)
            .map(|raw| lenient(&raw))
            .unwrap_or_default();
        SetupProgress {
            flow: FLOW,
            auto_open: false,
            ..stored
        }
    }

    /// Store `progress`, without the two fields that are the server's.
    pub fn store_progress(&self, progress: &SetupProgress) -> SetupProgress {
        let stored = SetupProgress {
            flow: FLOW,
            auto_open: false,
            ..progress.clone()
        };
        let raw = serde_json::to_string(&stored).unwrap_or_else(|_| "{}".to_string());
        self.db().kv_set(scope::SETUP, PROGRESS_KEY, &raw);
        stored
    }
}

/// `raw` as progress, one field at a time: a field this build cannot read — a
/// step state or a choice a later guide added, a value of the wrong type —
/// reads as its default, and every other field stands, `startedAt` and
/// `finishedAt` above all (losing them would open the guide again by itself).
/// A row that is not a JSON object is a guide never started.
fn lenient(raw: &str) -> SetupProgress {
    let Ok(Value::Object(row)) = serde_json::from_str::<Value>(raw) else {
        return SetupProgress::default();
    };
    fn field<T: DeserializeOwned + Default>(object: Option<&Map<String, Value>>, key: &str) -> T {
        object
            .and_then(|object| object.get(key))
            .and_then(|value| T::deserialize(value).ok())
            .unwrap_or_default()
    }
    let top = Some(&row);
    let part = |key: &str| row.get(key).and_then(Value::as_object);
    let (steps, step_flow, where_to_use, first, wrap) = (part("steps"), part("stepFlow"), part("where"), part("first"), part("wrap"));
    SetupProgress {
        flow: FLOW,
        started_at: field(top, "startedAt"),
        finished_at: field(top, "finishedAt"),
        dismissed: field(top, "dismissed"),
        steps: StepStates {
            tools: field(steps, "tools"),
            where_to_use: field(steps, "where"),
            first: field(steps, "first"),
            wrap: field(steps, "wrap"),
        },
        step_flow: StepFlows {
            tools: field(step_flow, "tools"),
            where_to_use: field(step_flow, "where"),
            first: field(step_flow, "first"),
            wrap: field(step_flow, "wrap"),
        },
        where_to_use: WhereProgress {
            choice: field(where_to_use, "choice"),
            lan: field(where_to_use, "lan"),
        },
        first: FirstProgress {
            cwd: field(first, "cwd"),
            agent: field(first, "agent"),
            prompt: field(first, "prompt"),
        },
        wrap: WrapProgress {
            start_at_login: field(wrap, "startAtLogin"),
        },
        auto_open: false,
    }
}

/// Whether this instance has started a session of its own, from its own
/// records and without reading a transcript, so the answer is quick on the
/// app's first window however much the user ran before: the registry of the
/// sessions it started outside its own Claude root (on the user's own
/// Claude login, and every Codex thread), or any transcript in that root.
/// Sessions the user ran in their own `~/.claude` or `~/.codex` outside
/// Droppen do not count: an install that only sees those is still a new one.
async fn owns_sessions(paths: &droppen_instance::paths::Paths, transcripts: &TranscriptSources) -> bool {
    if !droppen_sessions::owned_sessions::read(paths).await.is_empty() {
        return true;
    }
    for reader in transcripts.iter() {
        if reader.has_own_sessions(paths).await {
            return true;
        }
    }
    false
}

/// `progress` as answered: with `autoOpen` worked out. This instance's own
/// records are only read when everything else already says yes.
pub async fn answered(state: &SetupState, progress: SetupProgress) -> SetupProgress {
    let auto_open = state.desktop_app && progress.started_at.0.is_none() && !owns_sessions(&state.paths, &state.transcripts).await;
    SetupProgress { auto_open, ..progress }
}

/// How far the first-run guide got on this instance.
#[api(id = "setup_progress", group = "setup", summary = "Read the first-run guide's progress")]
pub async fn get_progress(State(state): State<SetupState>) -> Json<SetupProgress> {
    let stored = state.setup.stored_progress();
    Json(answered(&state, stored).await)
}

/// Store how far the first-run guide got, and hand it back as stored.
#[api(id = "setup_store_progress", group = "setup", summary = "Store the first-run guide's progress")]
pub async fn put_progress(State(state): State<SetupState>, ApiJson(body): ApiJson<SetupProgress>) -> Result<Json<SetupProgress>, ApiError> {
    let stored = state.setup.store_progress(&body);
    Ok(Json(answered(&state, stored).await))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fixture::Fixture;
    use crate::testing::{guide, setup_state};

    #[test]
    fn the_wire_shape_is_the_guide_s() {
        let progress = SetupProgress::default();
        let json = serde_json::to_value(&progress).expect("json");
        assert_eq!(
            json,
            serde_json::json!({
                "flow": 1,
                "startedAt": null,
                "finishedAt": null,
                "dismissed": false,
                "steps": {"tools": "pending", "where": "pending", "first": "pending", "wrap": "pending"},
                "stepFlow": {"tools": 0, "where": 0, "first": 0, "wrap": 0},
                "where": {"choice": null, "lan": false},
                "first": {"cwd": null, "agent": null, "prompt": null},
                "wrap": {"startAtLogin": null},
                "autoOpen": false,
            })
        );
        let partial: SetupProgress = serde_json::from_value(serde_json::json!({"steps": {"tools": "done"}})).expect("lenient");
        assert_eq!(partial.steps.tools, StepState::Done);
        assert_eq!(partial.steps.where_to_use, StepState::Pending);
        assert!(serde_json::from_value::<SetupProgress>(serde_json::json!({"steps": {"tools": "half"}})).is_err());
    }

    #[tokio::test]
    async fn the_progress_is_stored_whole_and_the_server_s_fields_are_its_own() {
        let home = tempfile::tempdir().expect("tempdir");
        let state = setup_state(home.path(), guide(home.path(), Fixture::new(), None));
        assert_eq!(state.setup.stored_progress(), SetupProgress::default(), "never started");

        let sent = SetupProgress {
            flow: 7,
            started_at: Nullable(Some("2026-10-19T09:12:00Z".into())),
            steps: StepStates {
                tools: StepState::Done,
                where_to_use: StepState::Skipped,
                ..StepStates::default()
            },
            step_flow: StepFlows {
                tools: 1,
                ..StepFlows::default()
            },
            where_to_use: WhereProgress {
                choice: Nullable(Some(WhereChoice::Local)),
                lan: false,
            },
            first: FirstProgress {
                cwd: Nullable(Some("/Users/ada/app".into())),
                agent: Nullable(Some(FirstAgent::Codex)),
                prompt: Nullable(Some("Explain this project".into())),
            },
            wrap: WrapProgress {
                start_at_login: Nullable(Some(false)),
            },
            auto_open: true,
            ..SetupProgress::default()
        };
        let stored = state.setup.store_progress(&sent);
        assert_eq!(stored.flow, FLOW, "the server's version, not the page's");
        assert!(!stored.auto_open, "never stored");
        assert_eq!(state.setup.stored_progress(), stored);
        assert_eq!(stored.first, sent.first);
        assert_eq!(stored.wrap.start_at_login.0, Some(false));

        // Another server on the same instance reads the same row.
        let again = setup_state(home.path(), guide(home.path(), Fixture::new(), None));
        assert_eq!(again.setup.stored_progress(), stored);

        // A row that is not JSON at all is a guide never started, not an error.
        state.setup.db().kv_set(scope::SETUP, PROGRESS_KEY, "{not json");
        assert_eq!(state.setup.stored_progress(), SetupProgress::default());
    }

    #[tokio::test]
    async fn a_value_this_build_does_not_know_costs_its_field_and_nothing_else() {
        let home = tempfile::tempdir().expect("tempdir");
        let state = setup_state(home.path(), guide(home.path(), Fixture::new(), None));
        let whole = SetupProgress {
            started_at: Nullable(Some("2026-10-19T09:12:00Z".into())),
            finished_at: Nullable(Some("2026-10-19T09:20:00Z".into())),
            dismissed: true,
            steps: StepStates {
                tools: StepState::Done,
                where_to_use: StepState::Skipped,
                first: StepState::Done,
                wrap: StepState::Done,
            },
            step_flow: StepFlows {
                tools: 1,
                where_to_use: 0,
                first: 1,
                wrap: 1,
            },
            where_to_use: WhereProgress {
                choice: Nullable(Some(WhereChoice::Droppen)),
                lan: true,
            },
            first: FirstProgress {
                cwd: Nullable(Some("/Users/ada/app".into())),
                agent: Nullable(Some(FirstAgent::Claude)),
                prompt: Nullable(Some("Explain this project".into())),
            },
            wrap: WrapProgress {
                start_at_login: Nullable(Some(true)),
            },
            ..SetupProgress::default()
        };
        // Every field read back on its own is every field.
        let stored = state.setup.store_progress(&whole);
        assert_eq!(state.setup.stored_progress(), stored);

        // A later guide's row: a step state, a choice and a tool this build
        // has never heard of, and a value of the wrong type.
        let mut row = serde_json::to_value(&stored).expect("json");
        row["steps"]["where"] = "deferred".into();
        row["where"]["choice"] = "lan-only".into();
        row["first"]["agent"] = "gemini".into();
        row["stepFlow"]["tools"] = "one".into();
        row["somethingNew"] = serde_json::json!({"x": 1});
        state.setup.db().kv_set(scope::SETUP, PROGRESS_KEY, &row.to_string());
        let read = state.setup.stored_progress();
        assert_eq!(read.started_at, whole.started_at, "still started: the guide does not open by itself again");
        assert_eq!(read.finished_at, whole.finished_at);
        assert!(read.dismissed);
        assert_eq!(
            read.steps,
            StepStates {
                where_to_use: StepState::Pending,
                ..whole.steps
            },
            "the unknown state alone is lost"
        );
        assert_eq!(read.step_flow.tools, 0);
        assert_eq!(read.step_flow.first, 1);
        assert_eq!(read.where_to_use, WhereProgress { choice: Nullable(None), lan: true });
        assert_eq!(read.first.agent, Nullable(None));
        assert_eq!(read.first.cwd, whole.first.cwd);
        assert_eq!(read.first.prompt, whole.first.prompt);
        assert_eq!(read.wrap, whole.wrap);

        // A row that is JSON but not an object is never started too.
        state.setup.db().kv_set(scope::SETUP, PROGRESS_KEY, "[1, 2]");
        assert_eq!(state.setup.stored_progress(), SetupProgress::default());
    }
}
