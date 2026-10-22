//! Persistent screens — saved pane layouts.
//!
//! Port of the former TypeScript server's `screens.ts`. A screen's layout is a
//! binary split tree whose leaves *reference* server-side resources (sessions,
//! terminals, chats); the server never interprets those ids and never owns what
//! they point at, so deleting a screen leaves the resources alone.
//!
//! The file is instance-scoped (`<data>/instances/<name>/screens.json`) rather
//! than shared like most sidecars: those leaf ids name terminals and resident
//! sessions that only their own server can resolve, so a layout from a sibling
//! instance is a screen of dead panes. [`crate::instance`] has the whole rule,
//! and copies a pre-instance file in once.
//!
//! The tree arrives from the web client, which makes the sanitizers below the
//! security boundary rather than a tidying step: nothing from the request is
//! echoed back into the file. Every node the server stores is rebuilt here from
//! values it has just checked, so an unknown pane kind, a hostile id, a
//! thousand-deep tree or a `ratio` of `1e9` cannot survive a round trip.
//!
//! # Where a screen actually lives
//!
//! In [`crate::db`], one row per layout, since the version that added it. The
//! file this module is named after is read exactly once, to drain it into the
//! table — see [`read_screens_file`] — and is never written. The bug that
//! motivated the move is worth naming: `screens.json` was written *in place*,
//! `tokio::fs::write` straight over the live file with no lock of any kind, and
//! every reader here turns a truncated file into an empty list. A crash in that
//! window therefore did not damage one layout, it silently deleted all of them.
//! A transaction has no such window.
//!
//! The release that added the table went on writing the file after every save,
//! so that a [`crate::supervisor`] demotion to a `.prev` binary — one that had
//! never heard of the database — would still find its layouts where it left
//! them. No such binary is left to demote to, so that write is gone: the file is
//! an input, once, and nothing else.
//!
//! # Two devices, one layout
//!
//! A phone and a laptop open the same screen. Until the version that added
//! [`Screen::revision`] the server could not tell their saves apart: every
//! layout save replaced the whole tree, create and delete re-wrote every row
//! from a list read outside the transaction, and the client never looked at
//! the server again once it had a tree. Whoever saved last erased the other
//! without either of them seeing it happen.
//!
//! Now every row carries a revision, bumped by each write, and a save names the
//! revision it started from: a save based on a revision that is no longer the
//! stored one is refused with `409` and the screen as it is now, so the client
//! adopts it rather than overwriting it. Membership and order carry a revision
//! of their own ([`ScreenList::list_revision`]), and every change to them
//! touches only the rows it is about — a create inserts one, a delete removes
//! one, a reorder moves positions — inside one transaction, so two devices'
//! creates and deletes compose instead of the later one restoring a list
//! without the earlier one's row. After each write the global feed says so
//! (`screens:changed`, `screens:list`), which is how the other device learns it
//! should look again.

use std::collections::HashSet;
use std::sync::{Arc, LazyLock};
use std::time::SystemTime;

use gotcha::axum::http::StatusCode;
use gotcha::{api, Json, Schematic, State};
use rusqlite::{Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use unicode_segmentation::UnicodeSegmentation;

use crate::api::error::ApiError;
use crate::api::extract::{ApiJson, ApiPath, ApiQuery};
use crate::api::{ApiRouter, AppState, SendResult};
use crate::db::Db;
use crate::feed::global::GlobalFeed;
use crate::openapi_ext::{AnyValue, ExtraResponses, Optional};
use crate::sessions::{iso8601, Paths};

/// How many leaves one layout may hold. A split turns one pending slot into
/// two, so the budget is spent on splits rather than on leaves directly.
const MAX_LEAVES: u32 = 6;
/// How long a screen title may be, in characters.
const MAX_TITLE: usize = 100;
/// Bound an emoji sequence without cutting a joined glyph in half.
const MAX_EMOJI: usize = 32;
/// The smallest share a split may give either side; the ratio is clamped into
/// `[MIN_RATIO, 1 - MIN_RATIO]` so a pane can never be squeezed out of sight.
const MIN_RATIO: f64 = 0.1;
/// The longest id a leaf may carry, in characters.
const MAX_LEAF_ID: usize = 100;
/// The longest resource id a pane's content may reference, in characters.
const MAX_CONTENT_ID: usize = 200;

/// One saved pane layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct Screen {
    /// Stable id, a uuid v4.
    pub id: String,
    /// Display name.
    pub title: String,
    /// Custom rail icon; absent uses the default grid icon.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub emoji: Optional<String>,
    /// The sanitized pane tree. Client-owned in shape, which is why the schema
    /// says `any` and why the server rebuilds it rather than storing it.
    pub layout: AnyValue,
    /// When it was created, ISO-8601.
    pub created_at: String,
    /// When it last changed, ISO-8601.
    pub updated_at: String,
    /// Bumped by every successful write to this screen, starting at 1. A
    /// `PATCH` names the revision it was based on and is refused with `409`
    /// when the screen has moved on since.
    pub revision: u64,
}

/// The screen list response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct ScreenList {
    /// Every saved screen, in order.
    pub screens: Vec<Screen>,
    /// Bumped by every create, delete and reorder, starting at 1. A list
    /// operation may name it as `baseListRevision` to be refused when stale.
    pub list_revision: u64,
}

/// The `409` a `PATCH` gets when the screen changed since its `baseRevision`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
pub struct ScreenConflict {
    /// Always `revision_conflict`.
    pub error: String,
    /// The screen as it is now, for the client to adopt.
    pub screen: Screen,
}

/// The `409` a list operation gets when its `baseListRevision` is stale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct ScreenListConflict {
    /// Always `list_revision_conflict`.
    pub error: String,
    /// The list as it is now.
    pub screens: Vec<Screen>,
    /// Its current revision.
    pub list_revision: u64,
}

/// The body of `POST /api/screens`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct ScreenCreate {
    /// Display name; a missing or blank one becomes `screen N`.
    ///
    /// [`Optional`] rather than `Option`: the contract is `z.string().optional()`
    /// without `.nullable()`, and a plain `Option` documents `string | null`.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub title: Optional<String>,
    /// The list revision the client last saw; when given and stale, nothing is
    /// created and the answer is `409` with the current list.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub base_list_revision: Optional<u64>,
}

/// The body of `PATCH /api/screens/{id}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct ScreenUpdate {
    /// The revision this change was made against. It must still be the stored
    /// one; otherwise nothing is written and the answer is `409` with the
    /// screen as it is now.
    pub base_revision: u64,
    /// New display name; absent leaves it alone.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub title: Optional<String>,
    /// One emoji; blank restores the default icon, absent leaves it alone.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub emoji: Optional<String>,
    /// New pane tree; absent leaves it alone.
    ///
    /// The custom deserializer is what keeps "absent" and "`null`" apart:
    /// [`Optional`] alone folds a JSON `null` into `None`, but the TypeScript
    /// treats an explicit `null` as a layout that was *supplied* and sanitizes
    /// it down to a single empty pane.
    #[serde(default, deserialize_with = "explicit_layout", skip_serializing_if = "Optional::is_none")]
    pub layout: Optional<AnyValue>,
}

/// The body of `PUT /api/screens/order`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct ScreenOrder {
    /// Screen ids in the order wanted. Ids the server does not have are
    /// ignored, and screens the list does not name keep their relative order
    /// after the ones it does — so a reorder from a device that has not seen a
    /// screen another one just made cannot lose it.
    pub ids: Vec<String>,
    /// The list revision the client last saw; when given and stale, nothing
    /// moves and the answer is `409` with the current list.
    #[serde(default, skip_serializing_if = "Optional::is_none")]
    pub base_list_revision: Optional<u64>,
}

/// Query for `DELETE /api/screens/{id}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Schematic)]
pub struct ScreenDeleteQuery {
    /// The list revision the client last saw; when given and stale, nothing is
    /// deleted and the answer is `409` with the current list.
    #[serde(rename = "baseListRevision")]
    pub base_list_revision: Option<u64>,
}

/// Read a present-but-possibly-null field as `Some`, so the handler can tell it
/// apart from a field that was not sent at all.
fn explicit_layout<'de, D>(deserializer: D) -> Result<Optional<AnyValue>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Value::deserialize(deserializer).map(|value| Optional(Some(AnyValue(value))))
}

/// A fresh uuid v4, the id every sanitized node falls back to.
fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Now, in the format the transcript timestamps already use.
fn now() -> String {
    iso8601(SystemTime::now())
}

/// The content of a pane showing nothing yet.
fn picker() -> Value {
    json!({ "kind": "picker" })
}

/// A leaf holding nothing — the shape an unusable node degrades to.
fn picker_leaf() -> Value {
    json!({ "type": "leaf", "id": new_id(), "content": picker() })
}

/// An id the client supplied, or a fresh one.
///
/// A blank id is as unusable as a missing one, and both `sanitizeId`'s length
/// test and the callers' truthiness test reject it in the TypeScript.
fn sanitize_id(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.chars().count() <= MAX_LEAF_ID)
        .map(str::to_owned)
        .unwrap_or_else(new_id)
}

/// The resource id a pane's content references, if it is usable.
fn content_id(raw: &serde_json::Map<String, Value>) -> Option<&str> {
    raw.get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.chars().count() <= MAX_CONTENT_ID)
}

/// Which device a pane's content is on, when it is not the device that keeps
/// the screen (`docs/local-aggregator.md` §6.5): a page holding several
/// devices — droppen.app — puts a session of one beside a terminal of
/// another. The server never resolves it (the web does, among the devices it
/// holds), so it is kept like an id: a non-empty string of at most
/// [`MAX_CONTENT_ID`] characters with no control or format characters. Absent, or
/// unusable, the content is this device's, as every layout from before was.
fn content_machine(raw: &serde_json::Map<String, Value>) -> Option<&str> {
    raw.get("machine").and_then(Value::as_str).filter(|machine| {
        !machine.is_empty() && machine.chars().count() <= MAX_CONTENT_ID && !machine.chars().any(|c| c.is_control() || is_format(c))
    })
}

/// Unicode's format characters (category Cf): the bidirectional controls,
/// the zero-width ones and the like, which draw nothing and can make one
/// string look like another. Never part of a device id.
fn is_format(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061C}'
            | '\u{06DD}'
            | '\u{070F}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08E2}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{110BD}'
            | '\u{110CD}'
            | '\u{13430}'..='\u{1343F}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
    )
}

/// What a leaf shows: [`sanitize_kind`], with the device it is on
/// ([`content_machine`]) kept on the kinds that are on one — a session, a
/// chat, a terminal, a folder, and a picker choosing from one.
fn sanitize_content(value: Option<&Value>) -> Value {
    let mut content = sanitize_kind(value);
    if let (Some(Value::Object(raw)), Some(kind)) = (value, content.get("kind").and_then(Value::as_str)) {
        // Only on the kind that was sent: content that degraded (to a picker) is nobody's.
        let sent = raw.get("kind").and_then(Value::as_str);
        if sent == Some(kind) && matches!(kind, "session" | "new-session" | "chat" | "terminal" | "files" | "picker") {
            if let Some(machine) = content_machine(raw) {
                content["machine"] = json!(machine);
            }
        }
    }
    content
}

/// What a leaf shows, without the device it is on.
///
/// Anything the server does not recognise — a newer client's pane kind, a
/// corrupt file, a leaf whose id went missing — degrades to an empty slot the
/// user can refill, which is always safe because the referenced resource is
/// never touched either way.
fn sanitize_kind(value: Option<&Value>) -> Value {
    let Some(Value::Object(raw)) = value else { return picker() };
    let id = content_id(raw);
    match raw.get("kind").and_then(Value::as_str) {
        Some("session") => match (id, raw.get("source").and_then(Value::as_str)) {
            (Some(id), Some(source @ ("claude" | "codex"))) => json!({ "kind": "session", "source": source, "id": id }),
            _ => picker(),
        },
        Some("chat") => match (id, raw.get("chatKind").and_then(Value::as_str)) {
            (Some(id), Some(kind @ ("claude" | "codex" | "api"))) => json!({ "kind": "chat", "chatKind": kind, "id": id }),
            _ => picker(),
        },
        Some("terminal") => id.map_or_else(picker, |id| json!({ "kind": "terminal", "id": id })),
        // The empty session view a session is started from: nothing of its own
        // to check, and it names no resource — it becomes a `session` pane
        // once its first message has started one.
        Some("new-session") => json!({ "kind": "new-session" }),
        // The built-in task checklist, from before Tasks became a plugin. Kept
        // for ever: a layout saved by an older build still holds it, and the web
        // reads it as the `tasks` plugin's `checklist` pane.
        Some("tasks") => json!({ "kind": "tasks" }),
        // A plugin's pane. Kept whether or not that plugin is on — or even
        // bundled — for the reason `browser` below is: the server does not own
        // these ids, and degrading one would rewrite the user's layout on the
        // next save, so turning a plugin back on would not bring its pane back.
        Some("plugin") => plugin_content(raw).unwrap_or_else(picker),
        Some("files") => {
            // Locations only; file API containment and authorization still
            // apply when the pane actually requests a directory or file.
            let mut content = json!({ "kind": "files" });
            for key in ["path", "file"] {
                if let Some(path) = raw
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty() && s.len() <= 4096 && !s.contains('\0'))
                {
                    content[key] = json!(path);
                }
            }
            content
        }
        // No route serves a browser pane any more, but a `screens.json` written
        // by an older build still holds them and the web still renders one as a
        // picker. Degrading it here would silently rewrite the user's file on
        // the next save; the server does not own these ids and must not.
        Some("browser") => id.map_or_else(picker, |id| json!({ "kind": "browser", "id": id })),
        _ => picker(),
    }
}

/// `{kind: "plugin", plugin, pane, state?}`, rebuilt from checked values: the
/// plugin a plugin id, the pane `^[a-z][a-z0-9-]{0,31}$`, and the state — the
/// pane's own, opaque to the server — only when it is a flat object of strings,
/// numbers, booleans and nulls of at most [`PLUGIN_PANE_STATE_BYTES`] as JSON.
/// A state that is not is dropped, and the pane kept.
fn plugin_content(raw: &serde_json::Map<String, Value>) -> Option<Value> {
    let plugin = raw.get("plugin").and_then(Value::as_str).filter(|id| droppen_plugin_sdk::valid_id(id))?;
    let pane = raw.get("pane").and_then(Value::as_str).filter(|pane| {
        let bytes = pane.as_bytes();
        (1..=32).contains(&bytes.len())
            && bytes[0].is_ascii_lowercase()
            && bytes.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
    })?;
    let mut content = json!({ "kind": "plugin", "plugin": plugin, "pane": pane });
    if let Some(Value::Object(state)) = raw.get("state") {
        let flat = state
            .values()
            .all(|value| matches!(value, Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null));
        let small = serde_json::to_string(state).is_ok_and(|text| text.len() <= PLUGIN_PANE_STATE_BYTES);
        if flat && small {
            content["state"] = Value::Object(state.clone());
        }
    }
    Some(content)
}

/// The most a plugin pane's `state` may weigh, as JSON.
const PLUGIN_PANE_STATE_BYTES: usize = 2048;

#[cfg(test)]
mod plugin_pane_tests {
    use super::*;

    #[test]
    fn a_plugin_pane_keeps_its_ids_and_a_small_flat_state() {
        let content = json!({
            "kind": "plugin", "plugin": "tasks", "pane": "checklist",
            "state": { "group": "inbox", "limit": 20, "done": false, "since": null },
        });
        assert_eq!(sanitize_content(Some(&content)), content);
        let mut extra = content.clone();
        extra["machine"] = json!("elsewhere");
        assert_eq!(sanitize_content(Some(&extra)), content, "unknown fields go, the pane stays");
        let bare = json!({ "kind": "plugin", "plugin": "tasks", "pane": "c" });
        assert_eq!(sanitize_content(Some(&bare)), bare);
        let layout = json!({ "type": "leaf", "id": "plugin-1", "content": content });
        assert_eq!(sanitize_node(Some(&layout), &mut 6), layout);
    }

    #[test]
    fn a_state_that_is_nested_or_too_big_is_dropped_and_the_pane_kept() {
        let bare = json!({ "kind": "plugin", "plugin": "tasks", "pane": "checklist" });
        for state in [
            json!({ "nested": { "a": 1 } }),
            json!({ "list": [1, 2] }),
            json!("not an object"),
            json!({ "big": "x".repeat(PLUGIN_PANE_STATE_BYTES) }),
        ] {
            let mut content = bare.clone();
            content["state"] = state;
            assert_eq!(sanitize_content(Some(&content)), bare);
        }
        let mut fits = bare.clone();
        fits["state"] = json!({ "s": "x".repeat(PLUGIN_PANE_STATE_BYTES - 8) });
        assert_eq!(serde_json::to_string(&fits["state"]).unwrap().len(), PLUGIN_PANE_STATE_BYTES);
        assert_eq!(sanitize_content(Some(&fits)), fits, "exactly the limit is within it");
    }

    #[test]
    fn a_plugin_pane_with_bad_ids_is_an_empty_slot() {
        for (plugin, pane) in [
            (json!("Tasks"), json!("checklist")),
            (json!("../x"), json!("checklist")),
            (json!(null), json!("checklist")),
            (json!("tasks"), json!("Checklist")),
            (json!("tasks"), json!("1pane")),
            (json!("tasks"), json!("")),
            (json!("tasks"), json!("x".repeat(33))),
            (json!("tasks"), json!(7)),
        ] {
            let content = json!({ "kind": "plugin", "plugin": plugin, "pane": pane });
            assert_eq!(sanitize_content(Some(&content)), picker(), "{content}");
        }
    }

    #[test]
    fn the_legacy_tasks_pane_is_kept_for_ever() {
        assert_eq!(sanitize_content(Some(&json!({ "kind": "tasks" }))), json!({ "kind": "tasks" }));
    }
}

#[cfg(test)]
mod file_pane_tests {
    use super::*;

    #[test]
    fn file_locations_survive_layout_sanitization_without_unrelated_fields() {
        let content = json!({ "kind": "files", "path": "/home/me/my files", "file": "/home/me/my files/a.png" });
        assert_eq!(sanitize_content(Some(&content)), content);
        let mut input = content.clone();
        input["extra"] = json!("discard");
        assert_eq!(sanitize_content(Some(&input)), content);
        let layout = json!({ "type": "leaf", "id": "files-1", "content": content });
        assert_eq!(sanitize_node(Some(&layout), &mut 6), layout);
    }

    #[test]
    fn a_pane_keeps_the_device_it_is_on() {
        for content in [
            json!({ "kind": "session", "source": "claude", "id": "s1", "machine": "d-7f3a" }),
            json!({ "kind": "chat", "chatKind": "codex", "id": "c1", "machine": "d-7f3a" }),
            json!({ "kind": "terminal", "id": "t1", "machine": "m-old-style" }),
            json!({ "kind": "files", "path": "/srv", "machine": "d-7f3a" }),
            json!({ "kind": "picker", "machine": "d-7f3a" }),
            json!({ "kind": "new-session", "machine": "d-7f3a" }),
        ] {
            assert_eq!(sanitize_content(Some(&content)), content, "{content}");
            let layout = json!({ "type": "leaf", "id": "p", "content": content });
            assert_eq!(sanitize_layout(Some(&layout)), layout);
        }
        // Absent — every layout from before, and a page with one device — stays absent.
        let plain = json!({ "kind": "session", "source": "claude", "id": "s1" });
        assert_eq!(sanitize_content(Some(&plain)), plain);
        let starting = json!({ "kind": "new-session" });
        assert_eq!(sanitize_content(Some(&starting)), starting, "a session being started stays one");
        let mut extra = starting.clone();
        extra["id"] = json!("s1");
        assert_eq!(sanitize_content(Some(&extra)), starting, "and carries nothing else");
    }

    #[test]
    fn an_unusable_device_is_dropped_and_the_pane_kept() {
        let plain = json!({ "kind": "terminal", "id": "t1" });
        for machine in [
            Value::Null,
            json!(7),
            json!(""),
            json!("a\u{0}b"),
            json!("line\nbreak"),
            json!("d-\u{202E}evil"),
            json!("d-\u{200B}7f3a"),
            json!("\u{FEFF}d-7f3a"),
            json!("d-7f3a\u{2066}"),
            json!("d".repeat(201)),
            json!({ "id": "d-1" }),
        ] {
            let mut content = plain.clone();
            content["machine"] = machine.clone();
            assert_eq!(sanitize_content(Some(&content)), plain, "{machine}");
        }
        // Kinds that are on no device of their own never carry one.
        let tasks = sanitize_content(Some(&json!({ "kind": "tasks", "machine": "d-7f3a" })));
        assert_eq!(tasks, json!({ "kind": "tasks" }));
        let plugin = sanitize_content(Some(
            &json!({ "kind": "plugin", "plugin": "tasks", "pane": "checklist", "machine": "d-7f3a" }),
        ));
        assert_eq!(plugin, json!({ "kind": "plugin", "plugin": "tasks", "pane": "checklist" }));
        // A session that degrades to a picker does not take its device along.
        let broken = sanitize_content(Some(&json!({ "kind": "session", "source": "nope", "id": "s1", "machine": "d-7f3a" })));
        assert_eq!(broken, picker());
    }

    #[test]
    fn malformed_optional_locations_do_not_break_the_file_browser() {
        for path in [Value::Null, json!(123), json!(""), json!("bad\0path"), json!("a".repeat(4097))] {
            assert_eq!(
                sanitize_content(Some(&json!({ "kind": "files", "path": path, "file": [] }))),
                json!({ "kind": "files" })
            );
        }
        assert_eq!(
            sanitize_content(Some(&json!({ "kind": "files", "path": "~" }))),
            json!({ "kind": "files", "path": "~" })
        );
    }
}

/// One node of the pane tree, rebuilt from checked values.
///
/// `budget` is what bounds the recursion: a split costs one slot and yields two
/// children, so a tree with `MAX_LEAVES` slots can nest at most that many splits
/// no matter how deep the client's own tree goes. Once the budget is spent the
/// node collapses to a leaf and the remaining subtree is dropped.
fn sanitize_node(value: Option<&Value>, budget: &mut u32) -> Value {
    let Some(Value::Object(raw)) = value else { return picker_leaf() };

    let children = raw.get("children").and_then(Value::as_array).filter(|c| c.len() == 2);
    if let Some(children) = children {
        if raw.get("type").and_then(Value::as_str) == Some("split") && *budget >= 2 {
            *budget -= 1;
            let ratio = raw.get("ratio").and_then(Value::as_f64).unwrap_or(0.5).clamp(MIN_RATIO, 1.0 - MIN_RATIO);
            let direction = if raw.get("direction").and_then(Value::as_str) == Some("column") {
                "column"
            } else {
                "row"
            };
            return json!({
                "type": "split",
                "id": sanitize_id(raw.get("id")),
                "direction": direction,
                "ratio": ratio,
                "children": [sanitize_node(children.first(), budget), sanitize_node(children.get(1), budget)],
            });
        }
    }
    json!({ "type": "leaf", "id": sanitize_id(raw.get("id")), "content": sanitize_content(raw.get("content")) })
}

/// A whole pane tree, rebuilt within one leaf budget.
pub fn sanitize_layout(value: Option<&Value>) -> Value {
    let mut budget = MAX_LEAVES;
    sanitize_node(value, &mut budget)
}

/// A trimmed, length-capped title, or `fallback` when nothing usable was given.
fn sanitize_title(value: Option<&str>, fallback: &str) -> String {
    let Some(value) = value else { return fallback.to_string() };
    let title: String = value.trim().chars().take(MAX_TITLE).collect();
    if title.is_empty() {
        fallback.to_string()
    } else {
        title
    }
}

/// Match the web's single-grapheme check, keeping skin tones, flags, keycaps
/// and ZWJ sequences intact. Blank means reset, not an invalid icon.
fn parse_emoji(value: &str) -> Result<Option<String>, &'static str> {
    static EMOJI: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"\p{Extended_Pictographic}|\A\p{Regional_Indicator}{2}\z|\A[#*0-9]\x{FE0F}?\x{20E3}\z").expect("valid emoji regex")
    });
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().count() > MAX_EMOJI || value.graphemes(true).count() != 1 || !EMOJI.is_match(value) {
        return Err("enter a single emoji");
    }
    Ok(Some(value.to_string()))
}

/// The name this store's one-time drain of `screens.json` is recorded under.
///
/// The same string [`crate::db::Db::import_once`] would have used, and it lands
/// in the same `imported` scope; only the destination of the *rows* differs,
/// which is the whole reason this module carries its own copy of that function
/// below.
const IMPORT_MARKER: &str = "screens";

/// One screen, rebuilt field by field from whatever the store handed back.
///
/// The same coercion on both sides of the migration, and deliberately so: a
/// `payload` column holds the JSON a [`Screen`] serialises to, which is exactly
/// the shape an entry in `screens.json` always had. One function reads both, so
/// a hand-edited file and a row written by a build that has since changed cost
/// the field they broke rather than the screen.
fn screen_at(entry: &Value, index: usize) -> Screen {
    Screen {
        id: sanitize_id(entry.get("id")),
        title: sanitize_title(entry.get("title").and_then(Value::as_str), &format!("screen {}", index + 1)),
        emoji: entry
            .get("emoji")
            .and_then(Value::as_str)
            .and_then(|value| parse_emoji(value).ok().flatten())
            .into(),
        layout: AnyValue(sanitize_layout(entry.get("layout"))),
        created_at: entry.get("createdAt").and_then(Value::as_str).map_or_else(now, str::to_owned),
        updated_at: entry.get("updatedAt").and_then(Value::as_str).map_or_else(now, str::to_owned),
        // The `revision` column is the truth for a row; see [`screen_in_row`].
        // A file entry never had one, and starts where every screen does.
        revision: 1,
    }
}

/// [`screen_at`] for a row, with the revision its column holds.
///
/// The column rather than the copy inside `payload`, because the `.prev`
/// binary rewrites rows naming only `id, ordinal, payload` — its rows land at
/// the column's default and a stale `revision` left in their JSON must not
/// outrank that.
fn screen_in_row(entry: &Value, index: usize, revision: i64) -> Screen {
    Screen {
        revision: u64::try_from(revision).unwrap_or(1).max(1),
        ..screen_at(entry, index)
    }
}

/// A screen as the `payload` column holds it.
///
/// Infallible in practice — a [`Screen`] contains strings and a `Value` — and
/// the fallback is an empty object rather than a panic because a screen that
/// somehow will not encode should cost itself and not the save.
fn payload(screen: &Screen) -> String {
    serde_json::to_string(screen).unwrap_or_else(|_| "{}".to_string())
}

/// What `screens.json` says, for the one boot that drains it.
///
/// A missing or corrupt file reads as an empty list. That tolerance has not
/// changed, only moved: it used to guard every read, and now it guards the
/// import, which is the only thing that still opens the file.
async fn read_screens_file(paths: &Paths) -> Vec<Screen> {
    let Ok(text) = tokio::fs::read_to_string(paths.screens_file()).await else {
        return vec![];
    };
    let Ok(raw) = serde_json::from_str::<Value>(&text) else {
        return vec![];
    };
    let Some(entries) = raw.get("screens").and_then(Value::as_array) else {
        return vec![];
    };
    entries.iter().enumerate().map(|(index, entry)| screen_at(entry, index)).collect()
}

/// This instance's database, with `screens.json` drained into it once.
///
/// The marker is checked on every call rather than remembered in a `OnceLock`,
/// and that is not laziness: [`crate::db::at`] is keyed by path, a test holds
/// two homes at once, and a process-wide "already imported" flag would be wrong
/// for the second of them. One indexed lookup against a table of kilobytes is
/// cheaper than the `read(2)` this whole module used to do per request.
async fn store(paths: &Paths) -> Arc<Db> {
    let db = crate::db::at(paths.instance_db());
    if db.kv_get(crate::db::scope::IMPORTED, IMPORT_MARKER).is_none() {
        import_once(&db, read_screens_file(paths).await);
    }
    db
}

/// [`crate::db::Db::import_once`], for a store whose rows are a table.
///
/// That one drains into `kv` and this one into `screens`; everything else about
/// it is the same, including all three of the properties it documents. The mark
/// goes in the same transaction as the rows, so an interrupted import is an
/// import that did not happen; the file is left exactly where it was, so that a
/// restored backup can be dropped back in to seed a fresh database; and a file
/// that was never there still counts as drained, or a fresh install re-runs the
/// import every boot and a user who deleted every screen finds them back.
fn import_once(db: &Db, screens: Vec<Screen>) {
    let count = screens.len();
    let outcome = db.write(|tx| {
        // Re-checked inside the transaction. Two servers can boot at once, and
        // the loser must not import over layouts the winner has already handed
        // back to a user who has started editing them.
        let done: Option<String> = tx
            .query_row(
                "SELECT value FROM kv WHERE scope = ?1 AND key = ?2",
                (crate::db::scope::IMPORTED, IMPORT_MARKER),
                |row| row.get(0),
            )
            .ok();
        if done.is_some() {
            return Ok(false);
        }
        for (ordinal, screen) in screens.iter().enumerate() {
            tx.execute(
                "INSERT OR IGNORE INTO screens (id, ordinal, payload) VALUES (?1, ?2, ?3)",
                (&screen.id, ordinal as i64, payload(screen)),
            )?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO kv (scope, key, value) VALUES (?1, ?2, ?3)",
            (crate::db::scope::IMPORTED, IMPORT_MARKER, count.to_string()),
        )?;
        Ok(true)
    });
    match outcome {
        Ok(true) => tracing::info!(rows = count, "imported screens.json into the state database"),
        Ok(false) => {}
        Err(error) => tracing::error!(%error, "cannot import screens.json; the layouts stay in the file this boot"),
    }
}

/// Take back the screens an older build of the desktop app inherited.
///
/// Until [`crate::instance::Heir::DesktopApp`] existed, the app's instance
/// copied the shared `screens.json` on its first boot like any command-line
/// instance did, and drained the copy into its table on the first
/// `GET /api/screens`: the command-line instances' layouts, every pane naming
/// a terminal or a session this server does not have, which the app's rail
/// showed as a row of identical grid icons. This undoes exactly that, and
/// nothing else:
///
/// * only while the instance's copy is still byte for byte the shared file —
///   the evidence that it is the inheritance and not a file of the app's own;
/// * only screens the shared file names, untouched since (row revision 1), in
///   a list nobody has added to, deleted from or reordered (list revision 1).
///   If the user has made even one of them their own, all of them stay: they
///   have seen them, and what to delete is theirs to say.
///
/// A copy not drained yet is marked drained with nothing in it. Either way the
/// copy is then renamed `screens.json.inherited` — kept, never deleted — so the
/// next boot finds nothing to do. Returns how many screens were removed.
///
/// Called at boot by the desktop app's own server only, under the instance
/// lock and before the router exists.
pub async fn forget_inherited(paths: &Paths) -> usize {
    let (Ok(shared), Ok(copy)) = (
        tokio::fs::read(paths.legacy_screens_file()).await,
        tokio::fs::read(paths.screens_file()).await,
    ) else {
        return 0;
    };
    if shared != copy {
        return 0;
    }
    let inherited: HashSet<String> = serde_json::from_slice::<Value>(&copy)
        .ok()
        .and_then(|raw| raw.get("screens").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| entry.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect();

    let db = crate::db::at(paths.instance_db());
    let outcome = db.write(|tx| {
        let drained: Option<String> = tx
            .query_row(
                "SELECT value FROM kv WHERE scope = ?1 AND key = ?2",
                (crate::db::scope::IMPORTED, IMPORT_MARKER),
                |row| row.get(0),
            )
            .optional()?;
        if drained.is_none() {
            // Never drained: nothing to remove, and nothing ever will be.
            tx.execute(
                "INSERT OR REPLACE INTO kv (scope, key, value) VALUES (?1, ?2, ?3)",
                (crate::db::scope::IMPORTED, IMPORT_MARKER, "0"),
            )?;
            return Ok(0);
        }
        if list_revision_in(tx)? != 1 {
            return Ok(0);
        }
        let rows: Vec<(String, i64)> = {
            let mut statement = tx.prepare("SELECT id, revision FROM screens")?;
            let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        if rows.iter().any(|(id, revision)| *revision != 1 || !inherited.contains(id)) {
            return Ok(0);
        }
        for (id, _) in &rows {
            tx.execute("DELETE FROM screens WHERE id = ?1", [id])?;
        }
        if !rows.is_empty() {
            bump_list_revision(tx)?;
        }
        Ok(rows.len())
    });
    let removed = match outcome {
        Ok(removed) => removed,
        Err(error) => {
            tracing::error!(%error, "cannot take back the screens the desktop app inherited; they stay this boot");
            return 0;
        }
    };
    let kept = paths.screens_file().with_extension("json.inherited");
    if let Err(error) = tokio::fs::rename(paths.screens_file(), &kept).await {
        tracing::warn!(%error, "cannot set the inherited screens.json aside");
    }
    if removed > 0 {
        tracing::info!(
            removed,
            "removed the screens the desktop app had inherited from the command-line instances"
        );
    }
    removed
}

/// Where the list revision lives, inside [`crate::db::scope::SCREENS`].
const LIST_REVISION: &str = "list-revision";

/// Every saved screen, in order, read on a connection the caller holds.
///
/// A row that will not parse is dropped rather than fatal and an empty store
/// reads as an empty list, which is the same tolerance the file had and for the
/// same reason: nothing here may lock the user out of making new screens.
/// `ordinal` is what preserves the order the JSON array carried, where order was
/// the only thing saying which screen the user made first; `rowid` breaks the
/// tie two rows written by an older build could share.
fn screens_in(conn: &Connection) -> rusqlite::Result<Vec<Screen>> {
    let mut statement = conn.prepare("SELECT payload, revision FROM screens ORDER BY ordinal, rowid")?;
    let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)))?;
    Ok(rows
        .filter_map(Result::ok)
        .filter_map(|(payload, revision)| serde_json::from_str::<Value>(&payload).ok().map(|entry| (entry, revision)))
        .enumerate()
        .map(|(index, (entry, revision))| screen_in_row(&entry, index, revision))
        .collect())
}

/// The list's revision. A store that has never been bumped is at 1, like a
/// screen nobody has edited.
fn list_revision_in(conn: &Connection) -> rusqlite::Result<u64> {
    let value: Option<String> = conn
        .query_row(
            "SELECT value FROM kv WHERE scope = ?1 AND key = ?2",
            (crate::db::scope::SCREENS, LIST_REVISION),
            |row| row.get(0),
        )
        .optional()?;
    Ok(value.and_then(|value| value.parse().ok()).unwrap_or(1).max(1))
}

/// Move the list revision on by one, inside the write that changed the list.
fn bump_list_revision(tx: &Transaction<'_>) -> rusqlite::Result<u64> {
    let next = list_revision_in(tx)? + 1;
    tx.execute(
        "INSERT OR REPLACE INTO kv (scope, key, value) VALUES (?1, ?2, ?3)",
        (crate::db::scope::SCREENS, LIST_REVISION, next.to_string()),
    )?;
    Ok(next)
}

/// The whole list and its revision, read together.
fn list_in(conn: &Connection) -> rusqlite::Result<ScreenList> {
    Ok(ScreenList {
        screens: screens_in(conn)?,
        list_revision: list_revision_in(conn)?,
    })
}

/// Every saved screen, in order.
pub async fn read_screens(paths: &Paths) -> Vec<Screen> {
    read_list(paths).await.screens
}

/// Every saved screen and the list's revision, from one read.
pub async fn read_list(paths: &Paths) -> ScreenList {
    let db = store(paths).await;
    db.read(list_in).unwrap_or(ScreenList {
        screens: vec![],
        list_revision: 1,
    })
}

/// A screen write that did not happen, and what the client is told instead.
#[derive(Debug)]
enum Refused {
    /// No such screen.
    Missing,
    /// The screen moved on since the client's `baseRevision`.
    Stale(Box<Screen>),
    /// The list moved on since the client's `baseListRevision`.
    StaleList(ScreenList),
}

impl From<Refused> for ApiError {
    fn from(refused: Refused) -> Self {
        let conflict = |body: Result<Value, serde_json::Error>| {
            ApiError::with_body(StatusCode::CONFLICT, body.unwrap_or_else(|_| json!({ "error": "conflict" })))
        };
        match refused {
            Refused::Missing => ApiError::not_found("screen not found"),
            Refused::Stale(screen) => conflict(serde_json::to_value(ScreenConflict {
                error: "revision_conflict".into(),
                screen: *screen,
            })),
            Refused::StaleList(list) => conflict(serde_json::to_value(ScreenListConflict {
                error: "list_revision_conflict".into(),
                screens: list.screens,
                list_revision: list.list_revision,
            })),
        }
    }
}

/// [`Refused::StaleList`] when the client named a list revision that is no
/// longer current; `None` when it is, or when it named none.
fn stale_list(conn: &Connection, base: Option<u64>) -> rusqlite::Result<Option<Refused>> {
    let Some(base) = base else { return Ok(None) };
    if list_revision_in(conn)? == base {
        return Ok(None);
    }
    Ok(Some(Refused::StaleList(list_in(conn)?)))
}

/// A database that would not commit — the one failure the user has to be told
/// about, as a `500`.
fn write_failed(error: rusqlite::Error) -> ApiError {
    ApiError::internal(format!("failed to write screens: {error}"))
}

/// Tell every open client that one screen changed, or went.
fn announce_changed(feed: &GlobalFeed, id: &str, revision: u64, deleted: bool) {
    let mut frame = json!({ "type": "screens:changed", "id": id, "revision": revision });
    if deleted {
        frame["deleted"] = json!(true);
    }
    feed.publish(frame.to_string());
}

/// Tell every open client that the list's membership or order changed.
fn announce_list(feed: &GlobalFeed, list_revision: u64) {
    feed.publish(json!({ "type": "screens:list", "listRevision": list_revision }).to_string());
}

/// Create a screen holding one empty pane: one row inserted, nothing else
/// rewritten.
async fn create(paths: &Paths, feed: &GlobalFeed, body: ScreenCreate) -> Result<Screen, ApiError> {
    let db = store(paths).await;
    let (screen, list_revision) = db
        .write(|tx| {
            if let Some(stale) = stale_list(tx, body.base_list_revision.0)? {
                return Ok(Err(stale));
            }
            // Counted and placed inside the transaction, so two creates at once
            // get two different positions rather than one each of the same.
            let (count, ordinal): (i64, i64) = tx.query_row("SELECT count(*), coalesce(max(ordinal) + 1, 0) FROM screens", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?;
            let stamp = now();
            let screen = Screen {
                id: new_id(),
                title: sanitize_title(body.title.as_deref(), &format!("screen {}", count + 1)),
                emoji: Optional(None),
                layout: AnyValue(picker_leaf()),
                created_at: stamp.clone(),
                updated_at: stamp,
                revision: 1,
            };
            tx.execute(
                "INSERT INTO screens (id, ordinal, payload, revision) VALUES (?1, ?2, ?3, 1)",
                (&screen.id, ordinal, payload(&screen)),
            )?;
            Ok(Ok((screen, bump_list_revision(tx)?)))
        })
        .map_err(write_failed)??;
    announce_changed(feed, &screen.id, screen.revision, false);
    announce_list(feed, list_revision);
    Ok(screen)
}

/// Patch one screen, if the client's `baseRevision` is still its revision.
async fn update(paths: &Paths, feed: &GlobalFeed, id: &str, body: ScreenUpdate) -> Result<Screen, ApiError> {
    let emoji = body.emoji.as_deref().map(parse_emoji).transpose().map_err(ApiError::bad_request)?;
    let db = store(paths).await;
    // A layout autosave and an icon change can arrive together. Read and patch
    // this row in one transaction so neither rewrites the other's old value —
    // and refuse either one that was made against a revision since replaced.
    let next = db
        .write(|tx| {
            let row = tx
                .query_row("SELECT ordinal, payload, revision FROM screens WHERE id = ?1", [id], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?))
                })
                .optional()?;
            let Some((ordinal, stored, revision)) = row else {
                return Ok(Err(Refused::Missing));
            };
            let Ok(raw) = serde_json::from_str::<Value>(&stored) else {
                return Ok(Err(Refused::Missing));
            };
            let current = screen_in_row(&raw, usize::try_from(ordinal).unwrap_or(0), revision);
            if current.revision != body.base_revision {
                return Ok(Err(Refused::Stale(Box::new(current))));
            }
            let mut next = current;
            if let Some(title) = body.title.as_deref() {
                next.title = sanitize_title(Some(title), &next.title);
            }
            if let Some(emoji) = emoji {
                next.emoji = emoji.into();
            }
            if let Some(layout) = body.layout.as_ref() {
                next.layout = AnyValue(sanitize_layout(Some(&layout.0)));
            }
            next.updated_at = now();
            next.revision += 1;
            tx.execute(
                "UPDATE screens SET payload = ?1, revision = ?2 WHERE id = ?3",
                (payload(&next), next.revision as i64, id),
            )?;
            Ok(Ok(next))
        })
        .map_err(write_failed)??;
    announce_changed(feed, &next.id, next.revision, false);
    Ok(next)
}

/// Delete one screen: one row removed, nothing else rewritten.
async fn delete(paths: &Paths, feed: &GlobalFeed, id: &str, base_list_revision: Option<u64>) -> Result<(), ApiError> {
    let db = store(paths).await;
    let (revision, list_revision) = db
        .write(|tx| {
            if let Some(stale) = stale_list(tx, base_list_revision)? {
                return Ok(Err(stale));
            }
            let revision: Option<i64> = tx
                .query_row("SELECT revision FROM screens WHERE id = ?1", [id], |row| row.get(0))
                .optional()?;
            let Some(revision) = revision else { return Ok(Err(Refused::Missing)) };
            tx.execute("DELETE FROM screens WHERE id = ?1", [id])?;
            // The deletion is the screen's last write, and is numbered as one.
            Ok(Ok((u64::try_from(revision).unwrap_or(1) + 1, bump_list_revision(tx)?)))
        })
        .map_err(write_failed)??;
    announce_changed(feed, id, revision, true);
    announce_list(feed, list_revision);
    Ok(())
}

/// `wanted` first, as far as it names screens that exist, then every screen it
/// does not name in the order it already had.
fn merged_order(existing: &[String], wanted: &[String]) -> Vec<String> {
    let known: HashSet<&str> = existing.iter().map(String::as_str).collect();
    let mut placed: HashSet<&str> = HashSet::new();
    let mut order: Vec<String> = wanted
        .iter()
        .filter(|id| known.contains(id.as_str()) && placed.insert(id.as_str()))
        .cloned()
        .collect();
    order.extend(existing.iter().filter(|id| !placed.contains(id.as_str())).cloned());
    order
}

/// Move screens' positions, in one transaction, without touching any payload.
async fn reorder(paths: &Paths, feed: &GlobalFeed, body: ScreenOrder) -> Result<ScreenList, ApiError> {
    let db = store(paths).await;
    let (list, moved) = db
        .write(|tx| {
            if let Some(stale) = stale_list(tx, body.base_list_revision.0)? {
                return Ok(Err(stale));
            }
            let existing: Vec<String> = {
                let mut statement = tx.prepare("SELECT id FROM screens ORDER BY ordinal, rowid")?;
                let ids = statement.query_map([], |row| row.get::<_, String>(0))?;
                ids.collect::<rusqlite::Result<_>>()?
            };
            let order = merged_order(&existing, &body.ids);
            let moved = order != existing;
            if moved {
                for (ordinal, id) in order.iter().enumerate() {
                    tx.execute("UPDATE screens SET ordinal = ?1 WHERE id = ?2", (ordinal as i64, id))?;
                }
                bump_list_revision(tx)?;
            }
            Ok(Ok((list_in(tx)?, moved)))
        })
        .map_err(write_failed)??;
    if moved {
        announce_list(feed, list.list_revision);
    }
    Ok(list)
}

/// Persistent screens, saved pane layouts.
#[api(id = "list_screens", group = "screens", summary = "List screens")]
pub async fn list_screens(State(state): State<AppState>) -> Json<ScreenList> {
    Json(read_list(state.paths.as_ref()).await)
}

/// Create a screen holding one empty pane.
#[api(id = "create_screen", group = "screens", summary = "Create a screen")]
pub async fn create_screen(State(state): State<AppState>, ApiJson(body): ApiJson<ScreenCreate>) -> Result<Json<Screen>, ApiError> {
    create(state.paths.as_ref(), &state.global, body).await.map(Json)
}

/// Update a screen's name, emoji, and/or layout without touching omitted fields.
///
/// Refused with `409` and the current screen when `baseRevision` is not the
/// stored revision.
#[api(id = "update_screen", group = "screens", summary = "Update a screen")]
pub async fn update_screen(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiJson(body): ApiJson<ScreenUpdate>,
) -> Result<Json<Screen>, ApiError> {
    update(state.paths.as_ref(), &state.global, &id, body).await.map(Json)
}

/// Delete a screen. The resources its leaves reference are left untouched.
#[api(id = "delete_screen", group = "screens", summary = "Delete a screen")]
pub async fn delete_screen(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
    ApiQuery(query): ApiQuery<ScreenDeleteQuery>,
) -> Result<Json<SendResult>, ApiError> {
    delete(state.paths.as_ref(), &state.global, &id, query.base_list_revision).await?;
    Ok(Json(SendResult { accepted: true }))
}

/// Put screens in a new order. Screens the request does not name keep their
/// relative order after the ones it does.
#[api(id = "reorder_screens", group = "screens", summary = "Reorder screens")]
pub async fn reorder_screens(State(state): State<AppState>, ApiJson(body): ApiJson<ScreenOrder>) -> Result<Json<ScreenList>, ApiError> {
    reorder(state.paths.as_ref(), &state.global, body).await.map(Json)
}

/// Register this subsystem's routes.
pub fn routes(router: ApiRouter) -> ApiRouter {
    router
        .get("/api/screens", list_screens)
        .post("/api/screens", create_screen)
        // A static segment, which the router prefers over `{id}`; screen ids
        // are uuids, so no screen is ever named `order`.
        .put("/api/screens/order", reorder_screens)
        .patch("/api/screens/{id}", update_screen)
        .delete("/api/screens/{id}", delete_screen)
}

/// Everything gotcha cannot express about those routes.
///
/// `POST` re-states its `200` through [`ExtraResponses::body`] rather than
/// [`ExtraResponses::describe`], because that is the only way to drop the
/// `default` response gotcha derives from the handler's `Result`; the other
/// fallible routes lose theirs to their documented errors.
pub fn describe(reg: &mut ExtraResponses) {
    reg.describe(
        "GET",
        "/api/screens",
        200,
        "Persistent screens (saved pane layouts) and the list revision",
    )
    .body::<Screen>("POST", "/api/screens", 200, "Screen created with a single empty pane")
    .body::<ScreenListConflict>(
        "POST",
        "/api/screens",
        409,
        "baseListRevision is stale; nothing was created and the body carries the current list",
    )
    .describe("PATCH", "/api/screens/{id}", 200, "Screen updated, with its new revision")
    .error("PATCH", "/api/screens/{id}", 400, "Invalid screen emoji, or no baseRevision")
    .error("PATCH", "/api/screens/{id}", 404, "Screen not found")
    .body::<ScreenConflict>(
        "PATCH",
        "/api/screens/{id}",
        409,
        "The screen changed since baseRevision; nothing was written and the body carries it as it is now",
    )
    .describe(
        "DELETE",
        "/api/screens/{id}",
        200,
        "Screen deleted; referenced resources are left untouched",
    )
    .error("DELETE", "/api/screens/{id}", 404, "Screen not found")
    .body::<ScreenListConflict>(
        "DELETE",
        "/api/screens/{id}",
        409,
        "baseListRevision is stale; nothing was deleted and the body carries the current list",
    )
    .describe("PUT", "/api/screens/order", 200, "The list in its new order")
    .body::<ScreenListConflict>(
        "PUT",
        "/api/screens/order",
        409,
        "baseListRevision is stale; nothing moved and the body carries the current list",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A split of two leaves, `depth` levels deep.
    fn nested_split(depth: u32) -> Value {
        if depth == 0 {
            return json!({ "type": "leaf", "id": "leaf", "content": { "kind": "terminal", "id": "t1" } });
        }
        json!({
            "type": "split",
            "id": format!("split-{depth}"),
            "direction": "row",
            "ratio": 0.5,
            "children": [nested_split(depth - 1), nested_split(depth - 1)],
        })
    }

    /// How many leaves a sanitized tree ended up with.
    fn count_leaves(node: &Value) -> usize {
        match node.get("children").and_then(Value::as_array) {
            Some(children) => children.iter().map(count_leaves).sum(),
            None => 1,
        }
    }

    #[test]
    fn a_missing_layout_becomes_one_empty_pane() {
        let unusable = [json!(null), json!("nonsense"), json!([1, 2])];
        for input in std::iter::once(None).chain(unusable.iter().map(Some)) {
            let layout = sanitize_layout(input);
            assert_eq!(layout["type"], "leaf");
            assert_eq!(layout["content"], picker());
            assert_eq!(layout["id"].as_str().expect("id").len(), 36, "a fresh uuid stands in");
        }
    }

    #[test]
    fn a_known_pane_kind_survives_and_an_unknown_one_does_not() {
        let tasks = sanitize_layout(Some(&json!({
            "type": "leaf", "id": "tasks-1", "content": { "kind": "tasks", "id": "unused", "extra": "discard" },
        })));
        assert_eq!(tasks["content"], json!({ "kind": "tasks" }));

        let layout = sanitize_layout(Some(&json!({
            "type": "leaf",
            "id": "pane-1",
            "content": { "kind": "session", "source": "codex", "id": "abc" },
        })));
        assert_eq!(layout["id"], "pane-1");
        assert_eq!(layout["content"], json!({ "kind": "session", "source": "codex", "id": "abc" }));

        // A kind this build has never heard of, and a known kind whose id is
        // unusable, both degrade rather than being stored as-is.
        for content in [
            json!({ "kind": "hologram", "id": "abc", "danger": "<script>" }),
            json!({ "kind": "session", "source": "gpt", "id": "abc" }),
            json!({ "kind": "terminal", "id": "" }),
            json!({ "kind": "chat", "chatKind": "api", "id": "x".repeat(MAX_CONTENT_ID + 1) }),
        ] {
            let layout = sanitize_layout(Some(&json!({ "type": "leaf", "id": "p", "content": content })));
            assert_eq!(layout["content"], picker(), "unrecognised content must not survive");
        }
    }

    #[test]
    fn an_oversized_or_wrongly_typed_id_is_replaced() {
        let layout = sanitize_layout(Some(&json!({ "type": "leaf", "id": 7 })));
        assert_ne!(layout["id"], 7);
        assert_eq!(layout["id"].as_str().expect("id").len(), 36);

        let long = "x".repeat(MAX_LEAF_ID + 1);
        let layout = sanitize_layout(Some(&json!({ "type": "leaf", "id": long })));
        assert_eq!(layout["id"].as_str().expect("id").len(), 36);

        let exact = "x".repeat(MAX_LEAF_ID);
        let layout = sanitize_layout(Some(&json!({ "type": "leaf", "id": exact.clone() })));
        assert_eq!(layout["id"], exact, "the cap itself is allowed");
    }

    #[test]
    fn the_leaf_budget_bounds_an_arbitrarily_deep_tree() {
        // Four levels of splits is 16 leaves; the budget allows six.
        let layout = sanitize_layout(Some(&nested_split(4)));
        assert!(
            count_leaves(&layout) <= MAX_LEAVES as usize,
            "a client tree must not be able to outgrow the budget"
        );
        // The cost model is one slot per split, so five splits fit in six slots.
        assert_eq!(count_leaves(&layout), 6);
    }

    #[test]
    fn a_split_missing_two_children_collapses_to_a_leaf() {
        let layout = sanitize_layout(Some(&json!({
            "type": "split",
            "id": "s",
            "children": [{ "type": "leaf", "id": "a" }],
        })));
        assert_eq!(layout["type"], "leaf");
        assert_eq!(layout["id"], "s");
    }

    #[test]
    fn the_ratio_is_clamped_and_the_direction_defaulted() {
        let split = |ratio: Value, direction: Value| {
            sanitize_layout(Some(&json!({
                "type": "split",
                "id": "s",
                "ratio": ratio,
                "direction": direction,
                "children": [{ "type": "leaf" }, { "type": "leaf" }],
            })))
        };

        assert_eq!(split(json!(0.0), json!("row"))["ratio"], 0.1);
        assert_eq!(split(json!(1.0e9), json!("row"))["ratio"], 0.9);
        assert_eq!(split(json!(-3), json!("row"))["ratio"], 0.1);
        assert_eq!(split(json!(0.42), json!("column"))["ratio"], 0.42);
        assert_eq!(
            split(json!("half"), json!("row"))["ratio"],
            0.5,
            "a non-number falls back to an even split"
        );
        assert_eq!(split(json!(0.5), json!("column"))["direction"], "column");
        assert_eq!(split(json!(0.5), json!("diagonal"))["direction"], "row", "an unknown direction is a row");
    }

    #[test]
    fn a_title_is_trimmed_capped_and_defaulted() {
        assert_eq!(sanitize_title(Some("  named  "), "screen 1"), "named");
        assert_eq!(sanitize_title(Some("   "), "screen 1"), "screen 1", "a blank title is no title");
        assert_eq!(sanitize_title(None, "screen 3"), "screen 3");
        let long = "x".repeat(MAX_TITLE + 40);
        assert_eq!(sanitize_title(Some(&long), "screen 1").chars().count(), MAX_TITLE);
    }

    #[test]
    fn emoji_validation_preserves_whole_graphemes_and_rejects_text_or_multiple_icons() {
        for emoji in ["🐙", "🛠️", "👩🏽‍💻", "👨‍👩‍👧‍👦", "🇯🇵", "1️⃣", "#️⃣", "🏳️‍🌈"] {
            assert_eq!(parse_emoji(&format!("  {emoji}  ")), Ok(Some(emoji.to_string())));
        }
        assert_eq!(parse_emoji(" \n "), Ok(None));
        for invalid in ["work", "1", "#", "🇯", "🐙🚀", "🐙 work", "🐙\n🚀", "<script>"] {
            assert!(parse_emoji(invalid).is_err(), "{invalid:?}");
        }
        assert!(parse_emoji(&format!("🐙{}", "\u{FE0F}".repeat(MAX_EMOJI))).is_err());
    }

    #[test]
    fn missing_or_invalid_stored_emoji_keeps_the_screen_with_its_default_icon() {
        for emoji in [json!(null), json!(""), json!(42), json!("🐙🚀"), json!("not an emoji")] {
            let screen = screen_at(&json!({ "id": "work", "title": "Work", "emoji": emoji }), 0);
            assert_eq!(screen.id, "work");
            assert_eq!(screen.title, "Work");
            assert!(screen.emoji.is_none());
        }
        assert!(screen_at(&json!({ "id": "legacy" }), 0).emoji.is_none());
    }

    /// A home of its own per body: the file is drained into the database once,
    /// so three seeds into one instance would only ever test the first.
    #[tokio::test]
    async fn a_corrupt_file_reads_as_an_empty_list() {
        for body in ["not json at all", "{}", r#"{"version":1,"screens":"nope"}"#] {
            let home = tempfile::tempdir().expect("tempdir");
            let paths = Paths::new(home.path().to_path_buf());
            std::fs::create_dir_all(paths.screens_file().parent().expect("parent")).expect("config dir");
            std::fs::write(paths.screens_file(), body).expect("seed");
            assert!(read_screens(&paths).await.is_empty(), "{body} should read as no screens");
        }
    }

    /// The bug this store moved to end, and the contract that replaced it.
    ///
    /// `screens.json` was written in place, so a crash mid-write left a
    /// truncated file — and every reader here turned a truncated file into no
    /// screens at all, which is every saved layout gone at once rather than one
    /// damaged. Now the file is an import source and nothing more: a save does
    /// not touch it, and whatever it holds is left exactly as found.
    #[tokio::test]
    async fn a_truncated_file_costs_nothing_and_a_save_never_rewrites_it() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        let screen = Screen {
            id: "screen-1".into(),
            title: "work".into(),
            emoji: Optional(None),
            layout: AnyValue(sanitize_layout(Some(
                &json!({ "type": "leaf", "id": "l1", "content": { "kind": "terminal", "id": "t1" } }),
            ))),
            created_at: "2026-01-01T00:00:00.000Z".into(),
            updated_at: "2026-01-01T00:00:00.000Z".into(),
            revision: 1,
        };
        seed(&paths, std::slice::from_ref(&screen)).await;
        assert!(!paths.screens_file().exists(), "a save writes the table and creates no file");
        assert!(!paths.screens_file().with_extension("json.tmp").exists());

        // Exactly what the interrupted `tokio::fs::write` used to leave behind.
        std::fs::write(paths.screens_file(), "").expect("truncate");

        let screens = read_screens(&paths).await;
        assert_eq!(screens.len(), 1, "the layouts are in the database, not in the file");
        assert_eq!(screens[0], screen);

        // And the next save leaves it byte for byte as it was found, rather than
        // repairing it: there is no writer left.
        let feed = GlobalFeed::default();
        update(
            &paths,
            &feed,
            "screen-1",
            ScreenUpdate {
                title: Some("again".to_string()).into(),
                ..patch(1)
            },
        )
        .await
        .expect("save again");
        assert_eq!(std::fs::read_to_string(paths.screens_file()).expect("read"), "");
    }

    /// The shared file and an identical instance copy: what an older desktop
    /// app build left behind.
    fn inherit(paths: &Paths, body: &str) {
        std::fs::create_dir_all(paths.config_dir()).expect("config dir");
        std::fs::create_dir_all(paths.instance_dir()).expect("instance dir");
        std::fs::write(paths.legacy_screens_file(), body).expect("shared");
        std::fs::write(paths.screens_file(), body).expect("copy");
    }

    const INHERITED: &str = r#"{"version":1,"screens":[{"id":"screen-1","title":"screen 1"},{"id":"screen-2","title":"screen 2"}]}"#;

    #[tokio::test]
    async fn the_desktop_app_gives_back_screens_it_inherited_untouched() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        inherit(&paths, INHERITED);
        assert_eq!(read_screens(&paths).await.len(), 2, "the older build drained the copy");

        assert_eq!(forget_inherited(&paths).await, 2);
        assert!(read_screens(&paths).await.is_empty(), "the rail starts empty");
        assert!(!paths.screens_file().exists(), "the copy is set aside");
        assert!(paths.screens_file().with_extension("json.inherited").is_file(), "kept, not deleted");
        assert!(
            paths.legacy_screens_file().is_file(),
            "the shared file is the command-line instances' own"
        );
        assert!(read_list(&paths).await.list_revision > 1, "a client holding the old list re-reads it");

        assert_eq!(forget_inherited(&paths).await, 0, "once");
    }

    #[tokio::test]
    async fn a_screen_the_user_made_their_own_keeps_every_inherited_one() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        inherit(&paths, INHERITED);
        read_screens(&paths).await;
        update(
            &paths,
            &GlobalFeed::default(),
            "screen-1",
            ScreenUpdate {
                title: Some("mine now".to_string()).into(),
                ..patch(1)
            },
        )
        .await
        .expect("rename it");

        assert_eq!(forget_inherited(&paths).await, 0);
        assert_eq!(ids(&read_screens(&paths).await), vec!["screen-1", "screen-2"]);
    }

    #[tokio::test]
    async fn a_screen_added_since_keeps_the_inherited_ones_too() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        inherit(&paths, INHERITED);
        read_screens(&paths).await;
        create(&paths, &GlobalFeed::default(), ScreenCreate::default())
            .await
            .expect("a screen of the app's own");

        assert_eq!(forget_inherited(&paths).await, 0);
        assert_eq!(read_screens(&paths).await.len(), 3);
    }

    #[tokio::test]
    async fn an_instance_file_of_its_own_is_never_touched() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        inherit(&paths, INHERITED);
        std::fs::write(paths.screens_file(), r#"{"version":1,"screens":[{"id":"screen-1","title":"different"}]}"#).expect("own");

        assert_eq!(forget_inherited(&paths).await, 0);
        assert!(paths.screens_file().is_file(), "left exactly where it was");
        assert_eq!(read_screens(&paths).await.len(), 1);
    }

    #[tokio::test]
    async fn a_copy_never_drained_never_will_be() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        inherit(&paths, INHERITED);

        assert_eq!(forget_inherited(&paths).await, 0);
        assert!(read_screens(&paths).await.is_empty(), "marked drained, with nothing in it");
    }

    /// The file is drained exactly once, and never over what the user has done
    /// since — otherwise every boot puts a deleted screen back.
    #[tokio::test]
    async fn the_file_is_imported_once_and_never_over_newer_rows() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        std::fs::create_dir_all(paths.screens_file().parent().expect("parent")).expect("config dir");
        std::fs::write(paths.screens_file(), r#"{"version":1,"screens":[{"id":"from-the-file","title":"work"}]}"#).expect("seed");

        let screens = read_screens(&paths).await;
        assert_eq!(screens.len(), 1);
        assert_eq!(screens[0].id, "from-the-file");
        assert_eq!(screens[0].revision, 1, "an imported screen starts where every screen does");

        // The user deletes it, and a sibling process puts the old file back.
        delete(&paths, &GlobalFeed::default(), "from-the-file", None).await.expect("delete it");
        std::fs::write(paths.screens_file(), r#"{"version":1,"screens":[{"id":"from-the-file","title":"work"}]}"#).expect("the file comes back");
        assert!(read_screens(&paths).await.is_empty(), "a deleted screen stays deleted");
    }

    /// A second connection to the same file sees what the first committed —
    /// which is the whole claim a store makes about surviving a restart.
    #[tokio::test]
    async fn a_layout_survives_the_database_being_reopened() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        seed(
            &paths,
            &[Screen {
                id: "screen-1".into(),
                title: "work".into(),
                emoji: Some("👩🏽‍💻".to_string()).into(),
                layout: AnyValue(json!({ "type": "leaf", "id": "tasks-1", "content": { "kind": "tasks" } })),
                created_at: "2026-01-01T00:00:00.000Z".into(),
                updated_at: "2026-01-01T00:00:00.000Z".into(),
                revision: 4,
            }],
        )
        .await;

        let reopened = Db::open(paths.instance_db());
        let (stored, revision): (Value, i64) = reopened
            .read(|conn| {
                conn.query_row("SELECT payload, revision FROM screens", [], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
            })
            .map(|(payload, revision)| (serde_json::from_str::<Value>(&payload).expect("payload"), revision))
            .expect("the row is there for anyone who opens the file");
        assert_eq!(stored["title"], "work");
        assert_eq!(stored["emoji"], "👩🏽‍💻");
        assert_eq!(stored["layout"]["content"], json!({ "kind": "tasks" }));
        assert_eq!(revision, 4);
        let screens = read_screens(&paths).await;
        assert_eq!(screens[0].emoji.as_deref(), Some("👩🏽‍💻"));
        assert_eq!(screens[0].revision, 4);
        assert_eq!(screens[0].layout.0["content"], json!({ "kind": "tasks" }));
    }

    #[tokio::test]
    async fn a_screen_missing_its_metadata_is_filled_in_by_position() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        std::fs::create_dir_all(paths.screens_file().parent().expect("parent")).expect("config dir");
        std::fs::write(
            paths.screens_file(),
            r#"{"version":1,"screens":[{"id":"a"},{"id":"b","title":"named","createdAt":"then","updatedAt":"then"}]}"#,
        )
        .expect("seed");

        let screens = read_screens(&paths).await;
        assert_eq!(screens.len(), 2);
        assert_eq!(screens[0].title, "screen 1", "a missing title is numbered by position");
        assert!(screens[0].emoji.is_none(), "legacy screens keep the default icon");
        assert_eq!(screens[0].layout.0["content"], json!({ "kind": "picker" }));
        assert!(screens[0].created_at.ends_with('Z'), "a missing timestamp becomes now");
        assert_eq!(screens[1].title, "named");
        assert_eq!(screens[1].created_at, "then");
    }

    #[test]
    fn an_explicit_null_layout_is_told_apart_from_an_absent_one() {
        let absent: ScreenUpdate = serde_json::from_str(r#"{"baseRevision":1,"title":"x"}"#).expect("decode");
        assert_eq!(absent.layout, Optional(None));

        let explicit: ScreenUpdate = serde_json::from_str(r#"{"baseRevision":1,"layout":null}"#).expect("decode");
        assert_eq!(
            explicit.layout,
            Optional(Some(AnyValue(Value::Null))),
            "a supplied null resets the layout"
        );
    }

    /// A save that does not say what it was based on is not a save this server
    /// can check, so it is not taken.
    #[test]
    fn a_patch_without_a_base_revision_does_not_decode() {
        let error = serde_json::from_str::<ScreenUpdate>(r#"{"layout":{"type":"leaf"}}"#).expect_err("refused");
        assert!(error.to_string().contains("baseRevision"), "{error}");
    }

    /// A `screens.json` written by an older build still holds browser leaves.
    /// The web renders one as a picker; the server must not *rewrite* it as one,
    /// because the next save would put the degraded tree back on disk.
    #[tokio::test]
    async fn a_legacy_browser_leaf_round_trips_unchanged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        std::fs::create_dir_all(paths.screens_file().parent().expect("parent")).expect("config dir");
        let layout = json!({
            "type": "split",
            "id": "s1",
            "direction": "row",
            "ratio": 0.5,
            "children": [
                { "type": "leaf", "id": "l1", "content": { "kind": "browser", "id": "tab-7" } },
                { "type": "leaf", "id": "l2", "content": { "kind": "terminal", "id": "t1" } },
            ],
        });
        let seeded = json!({
            // The envelope's version, which only the importer reads and nothing
            // branches on; it was a constant back when this module wrote files.
            "version": 1,
            "screens": [{ "id": "screen-1", "title": "work", "layout": layout, "createdAt": "2026-01-01T00:00:00.000Z", "updatedAt": "2026-01-01T00:00:00.000Z" }],
        });
        std::fs::write(
            paths.screens_file(),
            format!("{}\n", serde_json::to_string_pretty(&seeded).expect("json")),
        )
        .expect("seed");

        let screens = read_screens(&paths).await;
        assert_eq!(screens.len(), 1);
        assert_eq!(screens[0].layout.0, layout, "the browser leaf must survive the sanitizer");

        let saved = update(
            &paths,
            &GlobalFeed::default(),
            "screen-1",
            ScreenUpdate {
                layout: Optional(Some(screens[0].layout.clone())),
                ..patch(screens[0].revision)
            },
        )
        .await
        .expect("save");
        assert_eq!(saved.layout.0, layout);
        let round_tripped = read_screens(&paths).await;
        assert_eq!(round_tripped[0].layout.0, layout, "and survive a save");
    }

    /// Everything the server does not recognise still degrades.
    #[test]
    fn a_browser_leaf_without_an_id_is_still_an_empty_slot() {
        assert_eq!(sanitize_content(Some(&json!({ "kind": "browser" }))), picker());
        assert_eq!(sanitize_content(Some(&json!({ "kind": "gopher", "id": "x" }))), picker());
    }

    // ---- two devices, one layout --------------------------------------

    /// Rows exactly as given, one each, at the revisions they carry — the
    /// shape an older build or the importer leaves behind.
    async fn seed(paths: &Paths, screens: &[Screen]) {
        let db = store(paths).await;
        db.write(|tx| {
            for (ordinal, screen) in screens.iter().enumerate() {
                tx.execute(
                    "INSERT OR REPLACE INTO screens (id, ordinal, payload, revision) VALUES (?1, ?2, ?3, ?4)",
                    (&screen.id, ordinal as i64, payload(screen), screen.revision as i64),
                )?;
            }
            Ok(())
        })
        .expect("seed");
    }

    /// A screen called `id`, with one empty pane, never edited.
    fn screen(id: &str) -> Screen {
        Screen {
            id: id.into(),
            title: id.into(),
            emoji: Optional(None),
            layout: AnyValue(picker_leaf()),
            created_at: "2026-01-01T00:00:00.000Z".into(),
            updated_at: "2026-01-01T00:00:00.000Z".into(),
            revision: 1,
        }
    }

    /// A patch that changes nothing, made against `base`.
    fn patch(base: u64) -> ScreenUpdate {
        ScreenUpdate {
            base_revision: base,
            ..ScreenUpdate::default()
        }
    }

    /// A one-leaf layout showing terminal `id`.
    fn terminal(id: &str) -> AnyValue {
        AnyValue(json!({ "type": "leaf", "id": "pane", "content": { "kind": "terminal", "id": id } }))
    }

    fn ids(screens: &[Screen]) -> Vec<&str> {
        screens.iter().map(|screen| screen.id.as_str()).collect()
    }

    /// Every frame published so far, decoded.
    fn drain(rx: &mut tokio::sync::broadcast::Receiver<Arc<str>>) -> Vec<Value> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .map(|frame| serde_json::from_str(&frame).expect("a JSON frame"))
            .collect()
    }

    /// The phone and the laptop both open revision 1. The laptop saves first;
    /// the phone's save, still based on 1, must not land on top of it — it is
    /// refused, and told what the screen now is so it can adopt it.
    #[tokio::test]
    async fn a_save_bumps_the_revision_and_a_stale_base_is_refused_with_the_current_screen() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        let feed = GlobalFeed::default();
        let created = create(&paths, &feed, ScreenCreate::default()).await.expect("create");
        assert_eq!(created.revision, 1);

        let laptop = update(
            &paths,
            &feed,
            &created.id,
            ScreenUpdate {
                layout: Optional(Some(terminal("laptop"))),
                ..patch(1)
            },
        )
        .await
        .expect("the first save lands");
        assert_eq!(laptop.revision, 2, "a successful write moves the revision on");

        let refused = update(
            &paths,
            &feed,
            &created.id,
            ScreenUpdate {
                layout: Optional(Some(terminal("phone"))),
                ..patch(1)
            },
        )
        .await
        .expect_err("a save based on a replaced revision is refused");
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(refused.body()["error"], "revision_conflict");
        let current: Screen = serde_json::from_value(refused.body()["screen"].clone()).expect("the current screen");
        assert_eq!(current, laptop, "the 409 carries the screen exactly as it now is");

        let stored = read_screens(&paths).await;
        assert_eq!(stored[0].layout.0["content"]["id"], "laptop", "the refused save wrote nothing");
        assert_eq!(stored[0].revision, 2);

        // Adopting what the 409 said and saving again is the way through.
        let phone = update(
            &paths,
            &feed,
            &created.id,
            ScreenUpdate {
                layout: Optional(Some(terminal("phone"))),
                ..patch(current.revision)
            },
        )
        .await
        .expect("a save based on the current revision lands");
        assert_eq!(phone.revision, 3);

        // A rename or an icon change is a write too, and numbered as one.
        let renamed = update(
            &paths,
            &feed,
            &created.id,
            ScreenUpdate {
                title: Some("renamed".to_string()).into(),
                ..patch(3)
            },
        )
        .await
        .expect("rename");
        assert_eq!((renamed.revision, renamed.title.as_str()), (4, "renamed"));
        assert_eq!(renamed.layout.0["content"]["id"], "phone", "an omitted layout is left alone");

        let missing = update(&paths, &feed, "nope", patch(1)).await.expect_err("no such screen");
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    /// The bug this design ends: create and delete used to read the whole list
    /// outside the transaction and write it all back, so two of them at once
    /// restored whichever list was read last — and one device's screen was gone.
    #[tokio::test]
    async fn interleaved_creates_and_deletes_from_two_devices_keep_each_others_rows() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        let feed = GlobalFeed::default();
        seed(&paths, &[screen("a"), screen("b")]).await;

        // The laptop deletes `a` while the phone creates a screen; neither has
        // seen the other's change and neither names a list revision.
        let (deleted, created) = tokio::join!(
            delete(&paths, &feed, "a", None),
            create(
                &paths,
                &feed,
                ScreenCreate {
                    title: Some("phone".to_string()).into(),
                    ..Default::default()
                }
            ),
        );
        deleted.expect("delete");
        let created = created.expect("create");
        assert_eq!(ids(&read_screens(&paths).await), vec!["b", created.id.as_str()]);

        // Many devices creating at once: every one of them gets a row.
        let made = futures_util::future::join_all((0..8).map(|_| create(&paths, &feed, ScreenCreate::default()))).await;
        let made: Vec<Screen> = made.into_iter().map(|made| made.expect("create")).collect();
        let stored = read_screens(&paths).await;
        assert_eq!(stored.len(), 10);
        for screen in &made {
            assert!(stored.iter().any(|stored| stored.id == screen.id), "{} survived", screen.id);
        }

        // Deleting one screen leaves every other row exactly as it was.
        let before: Vec<Screen> = stored.iter().filter(|screen| screen.id != "b").cloned().collect();
        delete(&paths, &feed, "b", None).await.expect("delete b");
        assert_eq!(read_screens(&paths).await, before);
        let gone = delete(&paths, &feed, "b", None).await.expect_err("already gone");
        assert_eq!(gone.status(), StatusCode::NOT_FOUND);
    }

    /// A reorder from a device that has not seen a screen another one just made
    /// moves what it names and keeps the rest, rather than dropping it.
    #[tokio::test]
    async fn a_reorder_from_a_device_that_missed_a_create_keeps_the_new_screen() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        let feed = GlobalFeed::default();
        seed(&paths, &[screen("a"), screen("b"), screen("c")]).await;

        let (reordered, created) = tokio::join!(
            reorder(
                &paths,
                &feed,
                ScreenOrder {
                    ids: vec!["c".into(), "a".into(), "b".into()],
                    ..Default::default()
                }
            ),
            create(&paths, &feed, ScreenCreate::default()),
        );
        reordered.expect("reorder");
        let created = created.expect("create");
        let stored = read_screens(&paths).await;
        assert_eq!(stored.len(), 4, "nobody's screen was lost");
        let order: Vec<&str> = ids(&stored).into_iter().filter(|id| *id != created.id).collect();
        assert_eq!(order, vec!["c", "a", "b"]);

        // Unknown and repeated ids are ignored; unnamed screens follow in order.
        let list = reorder(
            &paths,
            &feed,
            ScreenOrder {
                ids: vec!["b".into(), "ghost".into(), "b".into()],
                ..Default::default()
            },
        )
        .await
        .expect("reorder");
        let mut expected = vec!["b", "c", "a"];
        let tail: Vec<&str> = ids(&stored).into_iter().filter(|id| !["a", "b", "c"].contains(id)).collect();
        expected.extend(tail);
        assert_eq!(ids(&list.screens), expected);
        assert_eq!(ids(&read_screens(&paths).await), expected, "and it is what the store now says");
        assert!(
            list.screens.iter().all(|screen| screen.revision == 1),
            "a move is not an edit to any screen"
        );
    }

    /// `baseListRevision` is optional, and when it is given and stale the
    /// operation does nothing and answers with the list as it is now.
    #[tokio::test]
    async fn a_stale_list_revision_refuses_create_delete_and_reorder_with_the_current_list() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        let feed = GlobalFeed::default();
        assert_eq!(read_list(&paths).await.list_revision, 1, "an untouched list is at revision 1");

        let first = create(
            &paths,
            &feed,
            ScreenCreate {
                base_list_revision: Optional(Some(1)),
                ..Default::default()
            },
        )
        .await
        .expect("a current base is accepted");
        let list = read_list(&paths).await;
        assert_eq!(list.list_revision, 2);

        let check = |error: ApiError, what: &str| {
            assert_eq!(error.status(), StatusCode::CONFLICT, "{what}");
            assert_eq!(error.body()["error"], "list_revision_conflict", "{what}");
            assert_eq!(error.body()["listRevision"], 2, "{what}");
            let screens: Vec<Screen> = serde_json::from_value(error.body()["screens"].clone()).expect("the current list");
            assert_eq!(ids(&screens), vec![first.id.as_str()], "{what}");
        };
        check(
            create(
                &paths,
                &feed,
                ScreenCreate {
                    base_list_revision: Optional(Some(1)),
                    ..Default::default()
                },
            )
            .await
            .expect_err("stale create"),
            "create",
        );
        check(delete(&paths, &feed, &first.id, Some(1)).await.expect_err("stale delete"), "delete");
        check(
            reorder(
                &paths,
                &feed,
                ScreenOrder {
                    ids: vec![first.id.clone()],
                    base_list_revision: Optional(Some(1)),
                },
            )
            .await
            .expect_err("stale reorder"),
            "reorder",
        );
        assert_eq!(read_list(&paths).await, list, "a refused list operation changes nothing");

        // A layout save does not move the list revision; only membership and
        // order do.
        update(
            &paths,
            &feed,
            &first.id,
            ScreenUpdate {
                layout: Optional(Some(terminal("t"))),
                ..patch(1)
            },
        )
        .await
        .expect("save");
        assert_eq!(read_list(&paths).await.list_revision, 2);
        delete(&paths, &feed, &first.id, Some(2)).await.expect("a current base is accepted");
        assert_eq!(read_list(&paths).await.list_revision, 3);
    }

    /// Every successful write tells the global feed, so another device's open
    /// copy learns to look again without waiting for a poll; a refused one
    /// tells nobody, because nothing changed.
    #[tokio::test]
    async fn every_write_is_announced_on_the_global_feed() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        let feed = GlobalFeed::default();
        let mut rx = feed.listen();

        let a = create(&paths, &feed, ScreenCreate::default()).await.expect("create a");
        assert_eq!(
            drain(&mut rx),
            vec![
                json!({ "type": "screens:changed", "id": a.id, "revision": 1 }),
                json!({ "type": "screens:list", "listRevision": 2 }),
            ]
        );
        let b = create(&paths, &feed, ScreenCreate::default()).await.expect("create b");
        drain(&mut rx);

        update(
            &paths,
            &feed,
            &a.id,
            ScreenUpdate {
                layout: Optional(Some(terminal("t"))),
                ..patch(1)
            },
        )
        .await
        .expect("save");
        assert_eq!(drain(&mut rx), vec![json!({ "type": "screens:changed", "id": a.id, "revision": 2 })]);

        update(&paths, &feed, &a.id, patch(1)).await.expect_err("stale");
        reorder(
            &paths,
            &feed,
            ScreenOrder {
                ids: vec![a.id.clone(), b.id.clone()],
                ..Default::default()
            },
        )
        .await
        .expect("an order that is already so");
        assert_eq!(drain(&mut rx), Vec::<Value>::new(), "a refusal or a no-op announces nothing");

        reorder(
            &paths,
            &feed,
            ScreenOrder {
                ids: vec![b.id.clone(), a.id.clone()],
                ..Default::default()
            },
        )
        .await
        .expect("reorder");
        assert_eq!(drain(&mut rx), vec![json!({ "type": "screens:list", "listRevision": 4 })]);

        delete(&paths, &feed, &a.id, None).await.expect("delete");
        assert_eq!(
            drain(&mut rx),
            vec![
                json!({ "type": "screens:changed", "id": a.id, "revision": 3, "deleted": true }),
                json!({ "type": "screens:list", "listRevision": 5 }),
            ]
        );
    }

    /// A database the previous release left — the `screens` table without the
    /// `revision` column — gains it on open, with every layout it held at 1 and
    /// saveable against that.
    #[tokio::test]
    async fn a_database_without_the_revision_column_reads_every_screen_at_revision_one() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        std::fs::create_dir_all(paths.instance_db().parent().expect("parent")).expect("instance dir");
        {
            let conn = Connection::open(paths.instance_db()).expect("open");
            for step in &crate::db::SCHEMA[..7] {
                conn.execute_batch(step).expect("a step the previous release shipped");
            }
            conn.pragma_update(None, "user_version", 7).expect("version");
            for (ordinal, id) in ["old-1", "old-2"].into_iter().enumerate() {
                conn.execute(
                    "INSERT INTO screens (id, ordinal, payload) VALUES (?1, ?2, ?3)",
                    (id, ordinal as i64, payload(&screen(id))),
                )
                .expect("a row the previous release wrote");
            }
            conn.execute(
                "INSERT INTO kv (scope, key, value) VALUES (?1, ?2, '0')",
                (crate::db::scope::IMPORTED, IMPORT_MARKER),
            )
            .expect("already imported");
        }

        let list = read_list(&paths).await;
        assert_eq!(ids(&list.screens), vec!["old-1", "old-2"], "the migration moved nothing");
        assert!(list.screens.iter().all(|screen| screen.revision == 1));
        assert_eq!(list.list_revision, 1);

        let saved = update(
            &paths,
            &GlobalFeed::default(),
            "old-2",
            ScreenUpdate {
                layout: Optional(Some(terminal("t"))),
                ..patch(1)
            },
        )
        .await
        .expect("a save against revision 1");
        assert_eq!(saved.revision, 2);
    }

    #[test]
    fn the_merged_order_puts_named_screens_first_and_keeps_the_rest() {
        let existing: Vec<String> = ["a", "b", "c", "d"].map(String::from).to_vec();
        let order = |wanted: &[&str]| merged_order(&existing, &wanted.iter().map(|id| id.to_string()).collect::<Vec<_>>());
        assert_eq!(order(&["c", "a"]), ["c", "a", "b", "d"]);
        assert_eq!(order(&[]), existing);
        assert_eq!(order(&["x", "d", "d"]), ["d", "a", "b", "c"]);
    }
}
