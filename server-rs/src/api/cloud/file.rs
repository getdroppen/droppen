//! `<instance>/cloud.json`: the link's settings on disk — who owns the link
//! and who may see other machines (local-aggregator §3.5, §4.6) — and the one
//! reader and writer of it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::*;

/// `<instance>/cloud.json`: who owns the link, and who may see other machines.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CloudFile {
    /// The hub's web origin it was linked through.
    #[serde(default)]
    pub(super) hub: Option<String>,
    /// The local account that signed this computer in.
    #[serde(default)]
    pub(super) owner: Option<String>,
    /// Whether the sign-in asked for the aggregator (S1-4: this computer
    /// controls the account's other devices with its one identity).
    #[serde(default)]
    pub(super) aggregate: bool,
    /// The owner turned the aggregator off ([`Paths::wants_aggregator`]).
    /// A field of its own: an install enrolled from the command line has no
    /// sign-in here, and controls by default.
    #[serde(default)]
    pub(super) aggregator_off: bool,
    /// When (ms) the sign-in finished.
    #[serde(default)]
    pub(super) linked_at: Option<u64>,
    /// The account's e-mail address as the popup said it (display only).
    #[serde(default)]
    pub(super) email: Option<String>,
    /// Local account → may see and use other machines.
    #[serde(default)]
    pub(super) remote_access: BTreeMap<String, bool>,
    /// Local account → its `createdAt` when it was made the owner or allowed:
    /// what is said of a name holds for that account only — one removed and
    /// made again under the same name inherits nothing ([`same_account`]).
    #[serde(default)]
    pub(super) accounts_since: BTreeMap<String, String>,
}

impl Cloud {
    fn file_path(&self) -> std::path::PathBuf {
        file_path(&self.paths)
    }

    pub(super) fn read_file(&self) -> CloudFile {
        std::fs::read(self.file_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// `cloud.json` as it is — no file: the defaults; a file that does not
    /// read or parse: `None`. Who may see other devices is never guessed
    /// from a file that may say otherwise.
    pub(super) fn read_file_strict(&self) -> Option<CloudFile> {
        match std::fs::read(self.file_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes).ok(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(CloudFile::default()),
            Err(_) => None,
        }
    }

    /// Write `file` back and tell the gateway's connections. Under the file's
    /// lock, as its callers already hold it.
    fn write_file(&self, file: &CloudFile) -> Result<(), ApiError> {
        let bytes = serde_json::to_vec_pretty(file).map_err(|e| ApiError::internal(e.to_string()))?;
        crate::agent::write_private(&self.file_path(), &bytes).map_err(ApiError::internal)?;
        self.changed.send_modify(|n| *n += 1);
        Ok(())
    }

    /// Change `cloud.json` only if it reads, and only if `f` agrees — both
    /// decided under the file's lock, so two requests cannot both pass a
    /// check one of them is about to make false (two claims at once).
    pub(super) fn try_update_file(&self, f: impl FnOnce(&mut CloudFile) -> Result<(), ApiError>) -> Result<CloudFile, ApiError> {
        let _guard = self.file.lock().expect("cloud file");
        let Some(mut file) = self.read_file_strict() else {
            return Err(refusal(
                StatusCode::CONFLICT,
                "unreadable",
                "this device's Droppen settings do not read; disconnect it and sign in again",
            ));
        };
        f(&mut file)?;
        self.write_file(&file)?;
        Ok(file)
    }

    pub(super) fn update_file(&self, f: impl FnOnce(&mut CloudFile)) -> Result<CloudFile, ApiError> {
        let _guard = self.file.lock().expect("cloud file");
        let mut file = self.read_file();
        f(&mut file);
        self.write_file(&file)?;
        Ok(file)
    }

    /// [`hand_over_from_local_owner`], under this server's file lock.
    pub async fn hand_over_from_local_owner(&self, users: &crate::auth::users::UsersStore) {
        let accounts: Vec<(String, String, bool)> = users.list().await.into_iter().map(|u| (u.name, u.created_at, u.app)).collect();
        let _guard = self.file.lock().expect("cloud file");
        match hand_over_from_local_owner(&self.paths, &accounts) {
            Ok(Some(owner)) => {
                self.changed.send_modify(|n| *n += 1);
                tracing::info!(%owner, "the Droppen link this device's owner signed in passed to its first account");
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, "could not pass the Droppen link to the first account"),
        }
    }

    pub(super) fn delete_file(&self) -> Result<(), ApiError> {
        let _guard = self.file.lock().expect("cloud file");
        let removed = match std::fs::remove_file(self.file_path()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(ApiError::internal(e.to_string())),
        };
        self.changed.send_modify(|n| *n += 1);
        removed
    }
}

/// `<instance>/cloud.json`.
pub fn file_path(paths: &Paths) -> std::path::PathBuf {
    paths.instance_dir().join("cloud.json")
}

impl Paths {
    /// Whether the owner wants this computer to control other devices, by
    /// `<instance>/cloud.json`: not when they turned it off (`aggregatorOff`,
    /// or a sign-in from the local settings that asked for no aggregation); yes
    /// otherwise — also with no file at all (a computer enrolled from the
    /// command line). A file that cannot be read or does not parse says no
    /// (security review of S1-4, N3: it may be the owner's "off"), logged
    /// once.
    ///
    /// `{"aggregate": false}` with no `linkedAt` still says yes, on purpose:
    /// `cloud.json` is written whole, so any update of it (say `PUT
    /// /api/cloud/access` on a computer enrolled from the command line) writes
    /// the default `aggregate: false` without anybody asking for "off". Only
    /// `aggregatorOff`, or a sign-in from the local settings without
    /// aggregation (`linkedAt` set and `aggregate` false), is the owner's "off".
    ///
    /// This only says whether the controller works. Deleting what it holds is
    /// the local API's turning it off (`POST /api/cloud/aggregator`), which
    /// writes `aggregatorOff` and deletes `aggregator/`; editing `cloud.json`
    /// by hand stops the controller but leaves `aggregator/keys.json` where it
    /// is.
    pub(crate) fn wants_aggregator(&self) -> bool {
        let path = file_path(self);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return true,
            Err(error) => return unreadable(&path, &error.to_string()),
        };
        let file: CloudFile = match serde_json::from_slice(&bytes) {
            Ok(file) => file,
            Err(error) => return unreadable(&path, &error.to_string()),
        };
        !file.aggregator_off && (file.aggregate || file.linked_at.is_none())
    }
}

/// `cloud.json` does not read: off, said once per process.
fn unreadable(path: &std::path::Path, error: &str) -> bool {
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
        tracing::warn!(path = %path.display(), %error, "cannot read the Droppen link's settings: this computer does not control other devices until they read");
    }
    false
}

/// When the device gets its first user account: a Droppen link the local owner
/// signed in ([`crate::auth::LOCAL_OWNER`] named as `cloud.json`'s owner)
/// passes to the first user account — whoever makes it is the OS user (a
/// shell, or a page opened with `remote-hand-server open`) or holds the setup
/// code only that user can read — bound to that account by its `createdAt`
/// ([`same_account`]), with every allowance cleared: who else sees other
/// devices is the new owner's to say. `accounts`: every local account's
/// `(name, createdAt, app-internal)`. `Ok(Some(owner))` when it passed.
///
/// Only ever called by the server that owns the instance, under its file lock
/// ([`Cloud::hand_over_from_local_owner`]) — `user add` leaves it to that
/// server, which does it within a few seconds or when it next starts.
pub fn hand_over_from_local_owner(paths: &Paths, accounts: &[(String, String, bool)]) -> Result<Option<String>, String> {
    let path = file_path(paths);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let Ok(mut file) = serde_json::from_slice::<CloudFile>(&bytes) else {
        return Ok(None);
    };
    if !file.owner.as_deref().is_some_and(crate::auth::is_local_owner) {
        return Ok(None);
    }
    // The first user account: the oldest that is not app-internal.
    let Some((first, since)) = accounts
        .iter()
        .filter(|(_, _, app)| !app)
        .min_by(|a, b| a.1.cmp(&b.1))
        .map(|(name, since, _)| (name.clone(), since.clone()))
    else {
        return Ok(None);
    };
    file.owner = Some(first.clone());
    file.remote_access.clear();
    file.accounts_since.clear();
    file.accounts_since.insert(first.clone(), since);
    file.remote_access.insert(first.clone(), true);
    let bytes = serde_json::to_vec_pretty(&file).map_err(|e| e.to_string())?;
    crate::agent::write_private(&path, &bytes)?;
    Ok(Some(first))
}
