//! Step 03's folders: where the first session can run.
//!
//! Two places to pick from besides the device's own folder browser, each read
//! as little as the question needs:
//!
//! * **recent** (`GET /api/setup/projects/recent`) — the folders sessions ran
//!   in on this device: this instance's own, and the ones Claude Code and
//!   Codex recorded themselves. It is the session list's own `cwd` column,
//!   nothing more; no transcript is read for it beyond what that list already
//!   reads.
//! * **practice** (`POST /api/setup/practice-folder`) — an empty
//!   `~/Droppen/first-session` to try things in; a taken name gets a number.
//!   The one made last is handed out again while it is still an empty folder,
//!   so coming back to step 03 and asking again leaves no trail of empty
//!   `first-session-2`, `-3`… behind; nothing else that exists is ever reused
//!   or overwritten.
//!
//! Nothing here looks through the disk for projects: any other folder is
//! picked the usual way, in the folder browser.

use std::path::{Path, PathBuf};

use gotcha::{api, Json, Schematic, State};
use serde::{Deserialize, Serialize};

use crate::api::created::Created;
use crate::api::error::ApiError;
use crate::api::AppState;
use crate::db::scope;
use crate::openapi_ext::Nullable;
use crate::sessions::{SessionScope, SessionSummary, Source};

/// How many recent folders step 03 offers.
pub const RECENT_LIMIT: usize = 6;
/// The practice folder's place under the home.
pub const PRACTICE_PARENT: &str = "Droppen";
/// The practice folder's name; a taken one becomes `first-session-2`, `-3`…
pub const PRACTICE_NAME: &str = "first-session";
/// How many numbered names are tried before giving up.
const PRACTICE_TRIES: u32 = 100;
/// The `kv` key, under [`scope::SETUP`], of the practice folder made last.
const PRACTICE_KEY: &str = "practiceFolder";

/// Who ran sessions in a folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum FolderTool {
    /// This instance, through either CLI.
    Droppen,
    /// Claude Code, outside Droppen.
    Claude,
    /// Codex, outside Droppen.
    Codex,
}

/// A folder sessions ran in on this device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct RecentFolder {
    /// Absolute, as the sessions recorded it.
    pub path: String,
    /// Its last component.
    pub name: String,
    /// Who ran there.
    pub tools: Vec<FolderTool>,
    /// The newest activity of a session there, ISO-8601.
    pub last_used: Nullable<String>,
    /// How many sessions ran there.
    pub sessions: u32,
}

/// `GET /api/setup/projects/recent`.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct RecentFolders {
    /// At most six, the most recently used first; only folders that still exist.
    pub folders: Vec<RecentFolder>,
}

/// `POST /api/setup/practice-folder`.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct PracticeFolder {
    /// The practice folder, absolute and empty: the one made last if it still
    /// is, else one just made.
    pub path: String,
}

/// The last component of `path`, or the path itself for `/`.
fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

// -------------------------------------------------------------------- recent

/// The recent folders out of a session list, keeping those `exists` says are
/// still there: grouped by folder, newest activity first, ties by path.
pub fn recent_from(sessions: &[SessionSummary], exists: impl Fn(&Path) -> bool) -> Vec<RecentFolder> {
    let mut by_cwd: std::collections::BTreeMap<&str, RecentFolder> = std::collections::BTreeMap::new();
    for session in sessions {
        let Some(cwd) = session.cwd.0.as_deref().filter(|cwd| !cwd.is_empty()) else {
            continue;
        };
        let tool = match (session.scope, session.source) {
            (SessionScope::Owned, _) => FolderTool::Droppen,
            (SessionScope::External, Source::Claude) => FolderTool::Claude,
            (SessionScope::External, Source::Codex) => FolderTool::Codex,
        };
        let folder = by_cwd.entry(cwd).or_insert_with(|| RecentFolder {
            path: cwd.to_string(),
            name: name_of(Path::new(cwd)),
            tools: Vec::new(),
            last_used: Nullable(None),
            sessions: 0,
        });
        if !folder.tools.contains(&tool) {
            folder.tools.push(tool);
        }
        folder.sessions += 1;
        let used = Some(session.updated_at.as_str()).filter(|at| !at.is_empty());
        if used > folder.last_used.0.as_deref() {
            folder.last_used = Nullable(used.map(str::to_string));
        }
    }
    let mut folders: Vec<RecentFolder> = by_cwd.into_values().collect();
    // ISO-8601 compares lexicographically, as everywhere in the session list.
    folders.sort_by(|a, b| b.last_used.0.cmp(&a.last_used.0).then_with(|| a.path.cmp(&b.path)));
    folders
        .into_iter()
        .filter(|folder| exists(Path::new(&folder.path)))
        .take(RECENT_LIMIT)
        .map(|mut folder| {
            folder.tools.sort();
            folder
        })
        .collect()
}

// ------------------------------------------------------------------ practice

/// Make a new, empty practice folder under `home` and say where.
pub fn make_practice_folder(home: &Path) -> std::io::Result<PathBuf> {
    let parent = home.join(PRACTICE_PARENT);
    std::fs::create_dir_all(&parent)?;
    for n in 1..=PRACTICE_TRIES {
        let name = if n == 1 {
            PRACTICE_NAME.to_string()
        } else {
            format!("{PRACTICE_NAME}-{n}")
        };
        let path = parent.join(name);
        // `create_dir`, not `_all`: it fails on a folder that is there, which
        // is what keeps an existing one, empty or not, from being taken.
        match std::fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "every practice folder name is taken",
    ))
}

/// The practice folder to hand out: `earlier` (the one made last) while it is
/// still one of ours and an empty folder, else a new one under `home`.
///
/// "One of ours" is a `first-session…` folder right in `~/Droppen` — the
/// stored path is the server's own, but it is checked rather than trusted —
/// and the folder itself, not a link to one. Anything put in it, by the first
/// session or the user, makes the next one a new folder.
pub fn practice_folder_for(home: &Path, earlier: Option<&Path>) -> std::io::Result<PathBuf> {
    let ours = |path: &Path| {
        path.parent() == Some(home.join(PRACTICE_PARENT).as_path())
            && path.file_name().is_some_and(|name| name.to_string_lossy().starts_with(PRACTICE_NAME))
    };
    let still_empty = |path: &Path| {
        std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())
            && std::fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_none())
    };
    match earlier {
        Some(earlier) if ours(earlier) && still_empty(earlier) => Ok(earlier.to_path_buf()),
        _ => make_practice_folder(home),
    }
}

// -------------------------------------------------------------------- routes

/// The folders sessions ran in on this device, for step 03.
#[api(id = "setup_recent_folders", group = "setup", summary = "List folders sessions ran in")]
pub async fn recent_folders(State(state): State<AppState>) -> Json<RecentFolders> {
    let bindings = state.drive.accounts.session_account_map().await;
    let sessions = crate::sessions::list_and_filter(
        &state.drive.paths,
        &bindings,
        &crate::sessions::DriverStates::default(),
        None,
        None,
        crate::sessions::ScopeFilter::All,
    )
    .await;
    Json(RecentFolders {
        folders: recent_from(&sessions, Path::is_dir),
    })
}

/// An empty practice folder for the first session: the one made last while it
/// is still empty, else a new one.
#[api(id = "setup_practice_folder", group = "setup", summary = "Make a practice folder")]
pub async fn practice_folder(State(state): State<AppState>) -> Result<Created<PracticeFolder>, ApiError> {
    let home = state.drive.paths.home.clone();
    let db = state.setup.db();
    let earlier = db.kv_get(scope::SETUP, PRACTICE_KEY).map(PathBuf::from);
    let made = tokio::task::spawn_blocking(move || practice_folder_for(&home, earlier.as_deref()))
        .await
        .map_err(|_| ApiError::internal("the practice folder was not made"))?
        .map_err(|error| ApiError::internal(format!("cannot make the practice folder: {error}")))?;
    let path = made.to_string_lossy().into_owned();
    db.kv_set(scope::SETUP, PRACTICE_KEY, &path);
    Ok(Created(PracticeFolder { path }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(cwd: &str, source: Source, scope: SessionScope, updated_at: &str) -> SessionSummary {
        let mut row: SessionSummary = serde_json::from_value(serde_json::json!({
            "pinned": false, "id": "s", "source": "claude", "scope": "owned", "server": "s", "title": "t",
            "cwd": null, "repoKey": null, "gitBranch": null, "updatedAt": "", "createdAt": "",
            "sizeBytes": 1, "filePath": "/x", "status": "idle", "activity": "idle", "resident": null,
            "claudeRoot": null, "accountId": null, "attentionAt": null, "unread": false, "unreadCount": 0,
        }))
        .unwrap_or_else(|error| panic!("a session row: {error}"));
        row.cwd = Nullable(Some(cwd.to_string()));
        row.source = source;
        row.scope = scope;
        row.updated_at = updated_at.to_string();
        row
    }

    #[test]
    fn recent_folders_are_grouped_newest_first_and_only_those_still_there() {
        let sessions = [
            session("/w/site", Source::Claude, SessionScope::External, "2026-10-10T00:00:00Z"),
            session("/w/site", Source::Codex, SessionScope::Owned, "2026-10-18T00:00:00Z"),
            session("/w/site", Source::Claude, SessionScope::Owned, "2026-10-01T00:00:00Z"),
            session("/w/api", Source::Codex, SessionScope::External, "2026-10-19T00:00:00Z"),
            session("/w/gone", Source::Claude, SessionScope::Owned, "2026-10-20T00:00:00Z"),
            session("", Source::Claude, SessionScope::Owned, "2026-10-20T00:00:00Z"),
        ];
        let folders = recent_from(&sessions, |path| path != Path::new("/w/gone"));
        assert_eq!(folders.iter().map(|folder| folder.path.as_str()).collect::<Vec<_>>(), ["/w/api", "/w/site"]);
        let site = &folders[1];
        assert_eq!(site.name, "site");
        assert_eq!(site.tools, [FolderTool::Droppen, FolderTool::Claude]);
        assert_eq!(site.sessions, 3);
        assert_eq!(site.last_used.0.as_deref(), Some("2026-10-18T00:00:00Z"));
        assert_eq!(folders[0].tools, [FolderTool::Codex]);

        let many: Vec<SessionSummary> = (0..10)
            .map(|n| session(&format!("/w/p{n}"), Source::Claude, SessionScope::Owned, &format!("2026-10-1{n}T00:00:00Z")))
            .collect();
        let folders = recent_from(&many, |_| true);
        assert_eq!(folders.len(), RECENT_LIMIT);
        assert_eq!(folders[0].path, "/w/p9");
    }

    #[test]
    fn a_practice_folder_is_always_a_new_empty_one() {
        let home = tempfile::tempdir().expect("tempdir");
        let first = make_practice_folder(home.path()).expect("made");
        assert_eq!(first, home.path().join("Droppen/first-session"));
        std::fs::write(first.join("index.html"), "mine").expect("write");
        let second = make_practice_folder(home.path()).expect("made");
        assert_eq!(second, home.path().join("Droppen/first-session-2"));
        assert_eq!(std::fs::read_to_string(first.join("index.html")).expect("kept"), "mine");
        assert_eq!(std::fs::read_dir(&second).expect("dir").count(), 0);
        std::fs::remove_dir_all(&first).expect("remove");
        assert_eq!(make_practice_folder(home.path()).expect("made"), first, "a freed name is used again");
    }

    #[test]
    fn the_practice_folder_made_last_is_handed_out_again_while_it_is_empty() {
        let home = tempfile::tempdir().expect("tempdir");
        let first = practice_folder_for(home.path(), None).expect("made");
        assert_eq!(first, home.path().join("Droppen/first-session"));
        // Step 03 opened again, "Make a practice folder" again: the same one.
        assert_eq!(practice_folder_for(home.path(), Some(&first)).expect("again"), first);
        let folders = || std::fs::read_dir(home.path().join("Droppen")).expect("dir").count();
        assert_eq!(folders(), 1, "no empty first-session-2 left behind");

        // Once something is in it, the next one is new.
        std::fs::write(first.join("index.html"), "mine").expect("write");
        let second = practice_folder_for(home.path(), Some(&first)).expect("made");
        assert_eq!(second, home.path().join("Droppen/first-session-2"));
        assert_eq!(std::fs::read_to_string(first.join("index.html")).expect("kept"), "mine");

        // Gone, or not one of ours: a new one, and the stranger is untouched.
        std::fs::remove_dir(&second).expect("remove");
        assert_eq!(practice_folder_for(home.path(), Some(&second)).expect("made"), second);
        let elsewhere = home.path().join("work");
        std::fs::create_dir(&elsewhere).expect("dir");
        let made = practice_folder_for(home.path(), Some(&elsewhere)).expect("made");
        assert_ne!(made, elsewhere);
        assert_eq!(made, home.path().join("Droppen/first-session-3"));
        let other = home.path().join("Droppen/notes");
        std::fs::create_dir(&other).expect("dir");
        assert_ne!(practice_folder_for(home.path(), Some(&other)).expect("made"), other);

        // A link in its place is not the folder.
        let target = home.path().join("target");
        std::fs::create_dir(&target).expect("dir");
        let link = home.path().join("Droppen/first-session-9");
        std::os::unix::fs::symlink(&target, &link).expect("link");
        assert_ne!(practice_folder_for(home.path(), Some(&link)).expect("made"), link);
    }

    #[tokio::test]
    async fn the_route_remembers_the_practice_folder_it_made() {
        let home = tempfile::tempdir().expect("tempdir");
        let state = crate::api::test_support::state(home.path()).await;
        let first = practice_folder(State(state.clone())).await.expect("made").0.path;
        let again = practice_folder(State(state.clone())).await.expect("again").0.path;
        assert_eq!(again, first, "still empty: the same folder");
        std::fs::write(Path::new(&first).join("a.txt"), "x").expect("write");
        let next = practice_folder(State(state)).await.expect("next").0.path;
        assert_ne!(next, first);
        assert!(next.ends_with("first-session-2"), "{next}");
    }
}
