//! The Droppen link in the local API (local-aggregator §3.5): signing this
//! computer in to Droppen from the local settings, and out again.
//!
//! | route | | who |
//! |---|---|---|
//! | `GET /api/cloud` | where this instance stands, nothing secret | any local user |
//! | `POST /api/cloud/link {hub?, aggregate?}` | open an attempt: a nonce, and the public keys the droppen.app popup checks | the cloud owner; anyone while there is none |
//! | `POST /api/cloud/link/finish {attempt, nonce, code, email?}` | hand over the popup's RH1 code; enroll in process (202) | the user who opened the attempt |
//! | `POST /api/cloud/link/return {attempt, sealed}` | the browser hand-off (§4.9): droppen.app's sealed reply, from the page at `/link/return`; enroll as `finish` does (202) | whoever holds the reply: no login (the system browser has none) — it opens only with the attempt's one-off key and state ([`super::link_handoff`]) |
//! | `POST /api/cloud/link/cancel {attempt}` | drop the attempt (and an enrollment it started that no device completed) | the same |
//! | `POST /api/cloud/aggregator {enabled}` | turn the aggregator off (deleting the other devices' keys) or on again | the owner |
//! | `POST /api/cloud/unlink {deleteCopies}` | disconnect this computer: stop the link, delete everything under `agent/` (and the copy on the hub) | the owner |
//! | `POST /api/cloud/re-enroll-fresh` | only once its identity key was replaced elsewhere (its own chain says so): archive the seed, forget the enrollment, make a fresh seed (`{fingerprint, state: "unenrolled"}`) | the owner |
//! | `PUT /api/cloud/access {user, remote}` | which local accounts may see other machines | the owner |
//! | `POST /api/cloud/claim` | "I added this device": become the owner of an ownerless link | whoever manages it |
//! | `GET /api/agg/ws` | the local gateway: other devices' copies (S1-7, [`crate::aggregator::gateway`]) | a local account that may see other devices ([`remote_viewer`]) |
//!
//! **The rules**, before anything else on every route:
//!
//! 1. A request bridged from Droppen (`cloud:<device>`) is refused, 403
//!    `not_from_cloud`: a device reaching this computer through the link may
//!    not change the link, nor use this computer to reach others.
//! 2. Local authentication must be on and the caller signed in — to a local
//!    account, or as this device's owner through `remote-hand-server open`
//!    while it has no user account ([`crate::auth::LOCAL_OWNER`]) — or the
//!    mutating routes answer 409 `local_auth_required`: once linked, other
//!    machines are reachable from here, which must never be open to anybody
//!    who can reach the port. Every request without a session is refused
//!    before it gets here, so this is only ever reached with
//!    `auth.enabled = false`.
//! 3. The `/api/*` Origin check ([`crate::origin`]) and the bearer token
//!    ([`crate::auth`]) apply as everywhere; there are no cookies, so there
//!    is no CSRF surface, and the attempt's nonce is a second binding.
//!
//! **What this computer ever gets from Droppen**: an RH1 enrollment code (one
//! machine, ten minutes, useless without the machine's proof) — never a
//! password, a login session or a device session. The code arrives from the
//! popup through the opener page and `finish`, or sealed to the attempt's
//! one-off key through the browser and `return` (the desktop app's window has
//! no `window.opener`, nor has a browser that blocks the popup); it is never
//! logged here.
//!
//! The attempt lives in memory only (one at a time, 15 minutes). Who owns the
//! link, and which local accounts may see other machines, is
//! `<instance>/cloud.json` (0600).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use gotcha::axum::http::StatusCode;
use gotcha::axum::response::{IntoResponse, Response};
use gotcha::{api, Json, Schematic, State};
use gotcha_core::EnhancedSchema;
use rand::RngCore;
use serde::{Deserialize, Serialize};

use super::error::ApiError;
use super::extract::ApiJson;
use super::hub_web::HubWebs;
use super::link_handoff::{self, Handoff, HandoffMachine, PageOrigin, Reply};
use super::{ApiRouter, AppState, SendResult};
use crate::agent::control::{machine_fingerprint, AgentStatus, Control, ControlError, Standing};
use crate::agent::enroll::BeginError;
use crate::auth::MaybeIdentity;
use crate::openapi_ext::ExtraResponses;
use crate::sessions::Paths;

/// How long an attempt waits for its code.
pub const ATTEMPT_TTL_MS: u64 = 15 * 60 * 1000;
/// The hub a new sign-in uses unless the request (or `DROPPEN_HUB_DEFAULT`) says otherwise.
pub const DEFAULT_HUB: &str = "https://api.droppen.app";

/// The link's local state: the hub link itself, the attempt in flight, and
/// `cloud.json`. One per server, in [`AppState`].
pub struct Cloud {
    /// The hub link ([`crate::agent::control`]).
    pub control: Arc<Control>,
    paths: Paths,
    attempt: Mutex<Option<Attempt>>,
    /// How the last browser hand-off ended, for the page that waits on it.
    handoff_outcome: Mutex<Option<HandoffOutcome>>,
    /// `cloud.json` is read and written one request at a time.
    file: Mutex<()>,
    /// Bumped whenever `cloud.json` changes: the gateway's connections look
    /// again at who may see other devices.
    changed: tokio::sync::watch::Sender<u64>,
    /// Where a browser signs in to each hub ([`super::hub_web`]).
    pub hub_webs: HubWebs,
}

/// A sign-in in flight.
#[derive(Debug, Clone)]
struct Attempt {
    id: String,
    nonce: String,
    /// The local user who opened it.
    owner: String,
    /// The hub, as the agent signs in to it (`wss://…`), and where a browser
    /// signs in to it ([`super::hub_web`]: its app origin, or its own as `https://`).
    hub: String,
    hub_web: String,
    aggregate: bool,
    expires_ms: u64,
    /// The browser hand-off's one-off key and state (§4.9): set when a page
    /// opened the attempt (it said its origin), so droppen.app can answer
    /// through the browser as well as through the popup. Either answer uses
    /// the attempt up.
    handoff: Option<Handoff>,
}

/// How a browser hand-off ended (`return`): the page that opened the attempt
/// polls `GET /api/cloud` for it, as no popup tells it.
#[derive(Debug, Clone)]
struct HandoffOutcome {
    attempt: String,
    owner: String,
    at_ms: u64,
    result: HandoffResult,
}

/// Where a browser hand-off stands.
#[derive(Debug, Clone)]
enum HandoffResult {
    /// Taken, and enrolling: the attempt is gone already, so the page that
    /// waits must not read its absence as "ran out".
    Receiving,
    /// The hub took the enrollment: the machine id it assigned.
    HandedOver(String),
    /// `(error code, message)`.
    Failed(String, String),
}

/// `<instance>/cloud.json`: who owns the link, and who may see other machines.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CloudFile {
    /// The hub's web origin it was linked through.
    #[serde(default)]
    hub: Option<String>,
    /// The local account that signed this computer in.
    #[serde(default)]
    owner: Option<String>,
    /// Whether the sign-in asked for the aggregator (S1-4: this computer
    /// controls the account's other devices with its one identity).
    #[serde(default)]
    aggregate: bool,
    /// The owner turned the aggregator off ([`crate::aggregator::wanted`]).
    /// A field of its own: an install enrolled from the command line has no
    /// sign-in here, and controls by default.
    #[serde(default)]
    aggregator_off: bool,
    /// When (ms) the sign-in finished.
    #[serde(default)]
    linked_at: Option<u64>,
    /// The account's e-mail address as the popup said it (display only).
    #[serde(default)]
    email: Option<String>,
    /// Local account → may see and use other machines.
    #[serde(default)]
    remote_access: BTreeMap<String, bool>,
    /// Local account → its `createdAt` when it was made the owner or allowed:
    /// what is said of a name holds for that account only — one removed and
    /// made again under the same name inherits nothing ([`same_account`]).
    #[serde(default)]
    accounts_since: BTreeMap<String, String>,
}

fn now_ms() -> u64 {
    crate::agent::now_ms()
}

fn random_hex(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl Cloud {
    /// The link's local state for one server.
    pub fn new(paths: Paths) -> Arc<Self> {
        Arc::new(Self {
            control: Control::new(paths.clone()),
            paths,
            attempt: Mutex::new(None),
            handoff_outcome: Mutex::new(None),
            file: Mutex::new(()),
            changed: tokio::sync::watch::channel(0).0,
            hub_webs: HubWebs::default(),
        })
    }

    /// Hear whenever `cloud.json` changes (remote access, the aggregator, a
    /// sign-in or a disconnection).
    pub fn changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changed.subscribe()
    }

    fn file_path(&self) -> std::path::PathBuf {
        file_path(&self.paths)
    }

    fn read_file(&self) -> CloudFile {
        std::fs::read(self.file_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// `cloud.json` as it is — no file: the defaults; a file that does not
    /// read or parse: `None`. Who may see other devices is never guessed
    /// from a file that may say otherwise.
    fn read_file_strict(&self) -> Option<CloudFile> {
        match std::fs::read(self.file_path()) {
            Ok(bytes) => serde_json::from_slice(&bytes).ok(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(CloudFile::default()),
            Err(_) => None,
        }
    }

    /// Change `cloud.json` only if it reads, and only if `f` agrees — both
    /// decided under the file's lock, so two requests cannot both pass a
    /// check one of them is about to make false (two claims at once).
    fn try_update_file(&self, f: impl FnOnce(&mut CloudFile) -> Result<(), ApiError>) -> Result<CloudFile, ApiError> {
        let _guard = self.file.lock().expect("cloud file");
        let Some(mut file) = self.read_file_strict() else {
            return Err(refusal(
                StatusCode::CONFLICT,
                "unreadable",
                "this device's Droppen settings do not read; disconnect it and sign in again",
            ));
        };
        f(&mut file)?;
        let bytes = serde_json::to_vec_pretty(&file).map_err(|e| ApiError::internal(e.to_string()))?;
        crate::agent::write_private(&self.file_path(), &bytes).map_err(ApiError::internal)?;
        self.changed.send_modify(|n| *n += 1);
        Ok(file)
    }

    fn update_file(&self, f: impl FnOnce(&mut CloudFile)) -> Result<CloudFile, ApiError> {
        let _guard = self.file.lock().expect("cloud file");
        let mut file = self.read_file();
        f(&mut file);
        let bytes = serde_json::to_vec_pretty(&file).map_err(|e| ApiError::internal(e.to_string()))?;
        crate::agent::write_private(&self.file_path(), &bytes).map_err(ApiError::internal)?;
        self.changed.send_modify(|n| *n += 1);
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

    fn delete_file(&self) -> Result<(), ApiError> {
        let _guard = self.file.lock().expect("cloud file");
        let removed = match std::fs::remove_file(self.file_path()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(ApiError::internal(e.to_string())),
        };
        self.changed.send_modify(|n| *n += 1);
        removed
    }

    /// The attempt still in flight, if any (an expired one is dropped, with
    /// its pending aggregator key).
    fn live_attempt(&self) -> Option<Attempt> {
        let mut slot = self.attempt.lock().expect("attempt");
        if slot.as_ref().is_some_and(|a| a.expires_ms <= now_ms()) {
            *slot = None;
            drop(slot);
            self.forget_pending_aggregator();
            return None;
        }
        slot.clone()
    }

    /// Remember where a browser hand-off stands, for the page that waits on it.
    fn record_handoff(&self, attempt: &Attempt, result: HandoffResult) {
        *self.handoff_outcome.lock().expect("handoff outcome") = Some(HandoffOutcome {
            attempt: attempt.id.clone(),
            owner: attempt.owner.clone(),
            at_ms: now_ms(),
            result,
        });
    }

    /// How `user`'s last browser hand-off stands, while an attempt could
    /// still be waiting on it.
    fn handoff_outcome_for(&self, user: Option<&str>) -> Option<CloudHandoff> {
        let user = user?;
        let slot = self.handoff_outcome.lock().expect("handoff outcome");
        let outcome = slot
            .as_ref()
            .filter(|o| o.owner == user && now_ms().saturating_sub(o.at_ms) < ATTEMPT_TTL_MS)?;
        let (name, machine, error, message) = match &outcome.result {
            HandoffResult::Receiving => ("receiving", None, None, None),
            HandoffResult::HandedOver(machine) => ("handed_over", Some(machine.clone()), None, None),
            HandoffResult::Failed(code, message) => ("failed", None, Some(code.clone()), Some(message.clone())),
        };
        Some(CloudHandoff {
            attempt: outcome.attempt.clone(),
            outcome: name.into(),
            machine,
            error,
            message,
            at_ms: outcome.at_ms,
        })
    }

    /// The pending aggregator key goes when no chain lists it (until S1-4
    /// admits the aggregator, none does).
    fn forget_pending_aggregator(&self) {
        if let Err(error) = crate::aggregator::forget(&self.paths) {
            tracing::warn!(%error, "cannot delete the pending aggregator key");
        }
    }

    /// The whole aggregator directory goes — other devices' machine keys and
    /// copies with the legacy key (local-aggregator §4.5, §4.7): this
    /// computer leaves its account, or turns aggregation off.
    fn forget_aggregator(&self) -> Result<(), ApiError> {
        // What the gateway shows of them goes at once, with the files.
        self.control.live().clear();
        crate::aggregator::forget_all(&self.paths).map_err(ApiError::internal)
    }
}

/// `<instance>/cloud.json`.
pub fn file_path(paths: &Paths) -> std::path::PathBuf {
    paths.instance_dir().join("cloud.json")
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

/// The hub a sign-in uses: the request's, else `DROPPEN_HUB_DEFAULT` (or its
/// old name, `RH_HUB_DEFAULT`), else
/// api.droppen.app; as the agent signs in to it, `wss://…`. Where a browser
/// signs in to it is the hub's to say ([`super::hub_web`]).
fn resolve_hub(requested: Option<&str>) -> Result<String, ApiError> {
    let configured = crate::env::var("DROPPEN_HUB_DEFAULT");
    let url = requested
        .filter(|h| !h.trim().is_empty())
        .map(str::to_string)
        .or(configured)
        .unwrap_or_else(|| DEFAULT_HUB.to_string());
    crate::agent::hub::origin(&url).map_err(|message| refusal(StatusCode::BAD_REQUEST, "bad_hub", &message))
}

/// `wss://host` → `https://host` (and `ws://` → `http://`, loopback only):
/// the hub's own web origin, where a browser signs in unless the hub names
/// its app's ([`super::hub_web`]).
pub fn web_origin(ws_origin: &str) -> String {
    if let Some(rest) = ws_origin.strip_prefix("wss://") {
        format!("https://{rest}")
    } else if let Some(rest) = ws_origin.strip_prefix("ws://") {
        format!("http://{rest}")
    } else {
        ws_origin.to_string()
    }
}

/// `{error: code, message}` at `status`.
fn refusal(status: StatusCode, code: &str, message: &str) -> ApiError {
    ApiError::with_body(status, serde_json::json!({"error": code, "message": message}))
}

/// Who is asking, by the rules in the module docs: a bridged device is
/// refused outright; `None` when nobody is signed in (local authentication off).
fn caller(who: &MaybeIdentity) -> Result<Option<String>, ApiError> {
    match &who.0 {
        Some(identity) if identity.username.starts_with("cloud:") => Err(refusal(
            StatusCode::FORBIDDEN,
            "not_from_cloud",
            "another device reaching this one through Droppen cannot change its Droppen link",
        )),
        Some(identity) => Ok(Some(identity.username.clone())),
        None => Ok(None),
    }
}

/// The local user — signed in to a local account, or as this device's owner
/// while it has no user account — with local authentication on; else 409
/// `local_auth_required`.
async fn local_user(state: &AppState, who: &MaybeIdentity) -> Result<String, ApiError> {
    let user = caller(who)?;
    let off = state.drive.auth.is_off();
    match user {
        Some(user) if !off => Ok(user),
        _ if off => Err(local_auth_off()),
        _ => Err(refusal(
            StatusCode::CONFLICT,
            "local_auth_required",
            "sign in first — with a local account, or on this device with remote-hand-server open: once it is signed in to Droppen, other devices are reachable from here",
        )),
    }
}

/// 409 `local_auth_required`: local authentication is switched off, so there
/// is nobody to sign in as — and with nobody, anybody who reaches the port
/// would count.
fn local_auth_off() -> ApiError {
    refusal(
        StatusCode::CONFLICT,
        "local_auth_required",
        "local authentication is off on this device (auth.enabled = false): turn it back on and sign in first, because once it is signed in to Droppen, other devices are reachable from here",
    )
}

/// Whether `user` is this device's owner by the local capability right now:
/// the local owner ([`crate::auth::LOCAL_OWNER`]) while the device has no user
/// account. Its session is only good then anyway ([`crate::auth::Auth::still_valid`]);
/// this says the same of the link.
///
/// **The one owner hook.** The local owner is the operating-system user the
/// AI runs as — who can read whatever the link keeps on disk — proved by the
/// running server's control socket ([`crate::auth::local_open`]). While there
/// is no user account it is the owner of the Droppen link whatever
/// `cloud.json` names: it manages it, and sees other devices' copies. Every
/// owner rule here asks this first; one that needs the local owner too
/// belongs beside it. When the first user account is made, a link it signed
/// in passes to that account ([`hand_over_from_local_owner`]).
async fn local_owner_now(state: &AppState, user: &str) -> bool {
    let auth = &state.drive.auth;
    crate::auth::is_local_owner(user) && !auth.is_off() && !auth.users.has_users().await
}

/// Every local account's `createdAt`, by name.
async fn accounts(state: &AppState) -> BTreeMap<String, String> {
    state.drive.auth.users.list().await.into_iter().map(|u| (u.name, u.created_at)).collect()
}

/// Whether what `file` says of `user` — the owner, or allowed — is about the
/// local account of that name now: the one it was said of, not one removed
/// and made again under the same name since (`accounts_since`). An owner
/// from before accounts were bound (signed in by an earlier release) holds
/// for the account made before that sign-in only; an allowance from then
/// holds for nobody (the owner allows again).
fn same_account(file: &CloudFile, accounts: &BTreeMap<String, String>, user: &str) -> bool {
    let Some(created) = accounts.get(user) else {
        return false;
    };
    match file.accounts_since.get(user) {
        Some(since) => since == created,
        None => {
            file.owner.as_deref() == Some(user)
                && match (crate::record::replay::parse_iso8601(created), file.linked_at) {
                    (Some(created_ms), Some(linked_ms)) => u64::try_from(created_ms).is_ok_and(|c| c <= linked_ms),
                    _ => false,
                }
        }
    }
}

/// The owner `cloud.json` names, while it is that local account here.
fn owner_of<'a>(file: &'a CloudFile, accounts: &BTreeMap<String, String>) -> Option<&'a str> {
    file.owner.as_deref().filter(|owner| same_account(file, accounts, owner))
}

/// Whether `user` may manage the link: the owner — or anybody when there is
/// no owner, or the owner's local account is gone (nobody is locked out).
///
/// The local owner while there is no user account always may
/// ([`local_owner_now`]). An app-internal account may only as the owner — it
/// is never the "anybody" of an ownerless link — except on a server the
/// desktop app runs ([`AppState::desktop_app`]), where the app-internal account
/// is the app's own login, its window's: signed in by the app for the
/// operating-system user who runs it, as the local owner is by `open`, and how
/// the app signs this device in to Droppen through the system browser
/// ([`post_return`]). A link it signed in stays its own.
///
/// The local owner manages only *while* it is the owner: once the device has
/// an account it manages nothing, ownerless link or not — a sign-in it began
/// before the first account and that came back after must not leave the link
/// to `local:owner` on a device that has an account.
async fn may_manage(state: &AppState, file: &CloudFile, user: &str) -> bool {
    if crate::auth::is_local_owner(user) {
        return local_owner_now(state, user).await;
    }
    match owner_of(file, &accounts(state).await) {
        Some(owner) => owner == user,
        None => state.desktop_app || !state.drive.auth.users.is_app(user).await,
    }
}

/// The local account that may see other devices' copies here — `Ok(user)` —
/// by the one rule `GET /api/cloud`'s `copy` and `seesOtherDevices` follow too
/// ([`sees_other_devices`]; security review of S1-5, F8; local-aggregator
/// §4.6; the user's decision of 2026-10-19): the owner — the local account
/// that signed this device in to Droppen, or claimed it — and the local
/// accounts the owner allowed. Nobody while there is no owner, the owner's
/// account is gone, or `cloud.json` does not read; the fallback that lets
/// anybody *manage* an ownerless link is not one for *seeing*.
///
/// Refused: a device bridged from Droppen (403 `not_from_cloud`); local
/// authentication off (409 `local_auth_required`: with no local account,
/// anybody who reaches the port — a phone on the LAN, a page that
/// DNS-rebinds onto it — would count as the owner); a login that was signed
/// out or revoked since its ticket was minted (401); anybody else (403
/// `no_remote_access`). The local gateway asks this before every connection
/// and again while it lasts ([`crate::aggregator::gateway`]).
pub async fn remote_viewer(state: &AppState, who: &MaybeIdentity) -> Result<String, ApiError> {
    let user = local_user(state, who).await?;
    let auth = &state.drive.auth;
    let owner_now = local_owner_now(state, &user).await;
    if !owner_now && !auth.users.exists(&user).await {
        return Err(refusal(StatusCode::UNAUTHORIZED, "unauthorized", "this local account is gone"));
    }
    // The login behind it (a ticket says which minted it) must still be one;
    // a caller whose login cannot be named is refused (fail closed).
    let Some(digest) = who.0.as_ref().and_then(|i| i.token_sha256.as_deref()) else {
        return Err(refusal(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "sign in again: this login cannot be checked",
        ));
    };
    if auth.sessions.alive(digest).as_deref() != Some(user.as_str()) {
        return Err(refusal(StatusCode::UNAUTHORIZED, "unauthorized", "this login was signed out"));
    }
    let file = state.cloud.read_file_strict();
    let sees = if owner_now {
        // The owner by the local capability: while `cloud.json` reads.
        file.is_some()
    } else {
        sees_other_devices(file.as_ref(), &accounts(state).await, true, Some(&user))
    };
    if !sees {
        return Err(refusal(
            StatusCode::FORBIDDEN,
            "no_remote_access",
            "this local account may not see your other devices here: the local account that signed this device in to Droppen can allow it in Settings → Droppen",
        ));
    }
    Ok(user)
}

async fn require_manager(state: &AppState, user: &str) -> Result<CloudFile, ApiError> {
    let file = state.cloud.read_file();
    if !may_manage(state, &file, user).await {
        return Err(refusal(
            StatusCode::FORBIDDEN,
            "not_owner",
            "only the local account that signed this device in to Droppen can do this",
        ));
    }
    Ok(file)
}

// ---------------------------------------------------------------- bodies

/// This computer's Droppen account, as far as it knows it.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct CloudAccount {
    /// The account id (`a-…`).
    pub id: String,
    /// The account key's fingerprint, as `agent enroll` shows it.
    pub fingerprint: String,
    /// The account's e-mail address, as the droppen.app popup said it (display only).
    pub email: Option<String>,
}

/// This computer as a machine of the account.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct CloudMachine {
    /// Its id in the account (`m-…`), once the hub assigned one.
    pub id: Option<String>,
    /// The host name it reports.
    pub hostname: String,
    /// Its keys' fingerprint: 8 groups of 4 hex digits, the same string the
    /// droppen.app consent page shows.
    pub fingerprint: Option<String>,
}

/// The hub connection.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct CloudLinkHealth {
    /// The link runs in this server.
    pub running: bool,
    /// Signed in to the hub now.
    pub connected: bool,
    /// Since when (ms) it has been connected, or not.
    pub since_ms: Option<u64>,
    /// Why the connection last failed.
    pub error: Option<String>,
    /// The hub refuses this computer's sign-in: retried, and never taken as
    /// a removal (only the verified chain removes it).
    pub refused: Option<String>,
    /// The verified device chain's seq.
    pub chain_seq: Option<u64>,
    /// Current devices and machines in the account.
    pub devices: Option<u64>,
    /// Current machines in the account.
    pub machines: Option<u64>,
    /// When this machine learnt it was removed.
    pub removed_ms: Option<u64>,
    /// Chain conflicts this machine saw.
    pub alarms: Vec<String>,
    /// Account changes this machine saw (newest last).
    pub changes: Vec<String>,
    /// Renewing this computer's identity key.
    pub rotation: CloudRotation,
}

/// Renewing this computer's identity key (`docs/host-identity-rotation.md`
/// §3.5, §3.7): what the local settings show.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct CloudRotation {
    /// `idle`; `staged` (a new key waits for the renewal to be written);
    /// `renewed` (it took a renewed key within the last day); `refused` (the
    /// hub refuses its key as renewed but showed no renewal: it retries,
    /// nothing else changes); `foreign` (its identity key was replaced
    /// elsewhere — its own chain gives its id keys it does not hold, for
    /// example after restoring a backup made before a renewal: the link
    /// stopped; `POST /api/cloud/re-enroll-fresh` adds it again under a new
    /// identity); `unreadable` (renewed, and a staged key it may have been
    /// renewed to does not read: the link stopped until the file beside its
    /// key is put right — never taken for `foreign`).
    pub state: String,
    /// For a renewal confirmed on this computer: the code to type on the
    /// admin device (`XXXX-XXXX`), for ten minutes.
    pub code: Option<String>,
    /// Until when (ms) the code is shown.
    pub code_expires_ms: Option<u64>,
    /// When (ms) it took the renewed key.
    pub renewed_ms: Option<u64>,
    /// Its fingerprint before (8 groups of 4 hex digits).
    pub previous_fingerprint: Option<String>,
    /// Why: `renewed_elsewhere` for `foreign`, `rotated` for `refused`,
    /// `staged_key_unreadable` for `unreadable`.
    pub reason: Option<String>,
}

impl From<crate::agent::rotation::Status> for CloudRotation {
    fn from(s: crate::agent::rotation::Status) -> Self {
        Self {
            state: s.state,
            code: s.code,
            code_expires_ms: s.code_expires_ms,
            renewed_ms: s.renewed_ms,
            previous_fingerprint: s.previous_fingerprint,
            reason: s.reason,
        }
    }
}

/// One device that runs AI and accepts this computer's control, and the
/// content keys (its machine-key epochs) held of it here.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AggregatorHost {
    /// Its id.
    pub id: String,
    /// Its newest committed machine-key epoch.
    pub latest_epoch: Option<u32>,
    /// The epochs held here.
    pub epochs: Vec<u32>,
    /// Whether its agent declared `policy/1` (refuses this computer its admin
    /// routes); `null` until the hub said.
    pub policy: Option<bool>,
    /// What is copied of it here (S1-5); `null` before anything is, and for
    /// a caller who may not see other devices (§4.6: only whoever manages the
    /// link, or a local account the owner let see them).
    pub copy: Option<AggregatorCopy>,
}

/// What this computer holds of one device's copy (S1-5): counts and times,
/// never content.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AggregatorCopy {
    /// Sessions in its index, as copied (`null`: no index yet).
    pub index_rows: Option<u64>,
    /// When its agent last listed them (ms).
    pub index_as_of_ms: Option<u64>,
    /// When its agent last signed its status (ms).
    pub status_at_ms: Option<u64>,
    /// Sessions copied here (its newest).
    pub sessions: u64,
    /// Whether it is online now (`null`: not known).
    pub online: Option<bool>,
}

/// The aggregator: this computer controlling the account's other devices
/// with its one identity (local-aggregator §5, S1-4).
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct CloudAggregator {
    /// Whether the owner wants it.
    pub enabled: bool,
    /// `on` (it runs: the link runs and the chain names this computer),
    /// `off`, or `pending` (a key an older release prepared for a sign-in).
    pub state: String,
    /// Signed in to the hub as a device now.
    pub signed_in: bool,
    /// Why its last round failed.
    pub error: Option<String>,
    /// The devices that accept this computer's control, and the keys held of each.
    pub hosts: Vec<AggregatorHost>,
    /// When it last asked the account's admins for keys it lacks.
    pub asked_ms: Option<u64>,
}

/// The sign-in in flight.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct CloudAttempt {
    /// Its id.
    pub id: String,
    /// When it expires (ms).
    pub expires_ms: u64,
    /// Whether the caller opened it (only they can finish it).
    pub mine: bool,
    /// The droppen.app address that answers it through the browser (§4.9),
    /// for the caller who opened it only (it carries the attempt's state).
    pub handoff_url: Option<String>,
}

/// How the caller's last browser hand-off ended (§4.9): droppen.app's reply
/// came back through `/link/return`.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct CloudHandoff {
    /// The attempt it answered.
    pub attempt: String,
    /// `receiving` (taken: enrolling now), `handed_over` (the hub took the
    /// enrollment: a device of the account completes it) or `failed`.
    pub outcome: String,
    /// The machine id the hub assigned.
    pub machine: Option<String>,
    /// Why it failed: the error code (`hub_unreachable`, `refused`, …).
    pub error: Option<String>,
    /// Why it failed, in words.
    pub message: Option<String>,
    /// When (ms).
    pub at_ms: u64,
}

/// A local account, and whether it may see the other devices here (S1-7).
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct LocalAccountAccess {
    /// The local account.
    pub user: String,
    /// Whether it sees the other devices here now.
    pub remote: bool,
    /// Whether it is the owner (it signed this device in, or claimed it):
    /// it always sees them, and decides who else does.
    pub owner: bool,
}

/// One local account's access to other machines.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAccess {
    /// The local account.
    pub user: String,
    /// Whether it may see and use other machines.
    pub remote: bool,
}

/// `GET /api/cloud`: where this computer stands with Droppen. Nothing secret.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct CloudStatus {
    /// `none` (never signed in), `enrolling` (waiting for a device to add
    /// it), `linked`, `not_hosting` (still in the account, but not running
    /// AI in it: no streams, no channels, until a device makes it run AI
    /// again), `removed` (the account removed it) or `broken` (its state
    /// does not load).
    pub state: String,
    /// Whether local authentication is on (signing in needs it).
    pub local_auth: bool,
    /// The hub, as the agent signs in to it (`wss://…`).
    pub hub: String,
    /// Where a browser signs in to the hub, which the sign-in popup and the
    /// browser hand-off open: the app origin the hub's
    /// `/.well-known/droppen-hub` names (`https://my.droppen.app`), else the
    /// hub's own origin as `https://`.
    pub hub_web: String,
    /// The account, once known.
    pub account: Option<CloudAccount>,
    /// This computer as a machine.
    pub machine: CloudMachine,
    /// The hub connection, once there is a state.
    pub link: Option<CloudLinkHealth>,
    /// The aggregator.
    pub aggregator: CloudAggregator,
    /// The sign-in in flight.
    pub attempt: Option<CloudAttempt>,
    /// How the caller's last browser hand-off ended (kept for as long as an
    /// attempt lives).
    pub handoff: Option<CloudHandoff>,
    /// The local account that signed this computer in.
    pub owner: Option<String>,
    /// Whether the caller may manage the link.
    pub can_manage: bool,
    /// Which local accounts may see other machines.
    pub remote_access: Vec<RemoteAccess>,
    /// Whether the caller sees the other devices here (the local gateway,
    /// S1-7): who manages the link, or a local account it let see them;
    /// nobody while local authentication is off.
    pub sees_other_devices: bool,
    /// Every local account and whether it sees the other devices here — for
    /// whoever manages the link only (empty for anyone else).
    pub accounts: Vec<LocalAccountAccess>,
    /// Nobody owns this device's link (it was enrolled from the command line,
    /// or its owner's local account is gone), so nobody sees the other
    /// devices here; the caller manages the link and may claim it
    /// (`POST /api/cloud/claim`).
    pub can_claim: bool,
}

/// `POST /api/cloud/link`.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct LinkRequest {
    /// Another hub than the default (advanced).
    pub hub: Option<String>,
    /// Also run this computer as an aggregator (default `true`).
    pub aggregate: Option<bool>,
}

/// An already-enrolled computer's place, for the popup's "only add the
/// aggregator" variant.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct LinkEnrolled {
    /// The account key's fingerprint, 32 hex digits.
    pub account_fp: String,
    /// The machine id.
    pub id: String,
}

/// The machine's keys for the popup to check the enrollment proof against.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct LinkMachine {
    /// The host name (for reference only).
    pub hostname: String,
    /// Ed25519, 64 hex digits.
    pub sign_key: String,
    /// X25519, 64 hex digits.
    pub dh_key: String,
    /// The keys' fingerprint.
    pub fingerprint: String,
    /// Set when this computer is enrolled already.
    pub enrolled: Option<LinkEnrolled>,
}

/// The aggregator's pending keys.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct LinkAggregator {
    /// Ed25519, 64 hex digits.
    pub sign_key: String,
    /// X25519, 64 hex digits.
    pub dh_key: String,
}

/// An attempt: what the opener page hands the popup.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct LinkAttempt {
    /// The attempt's id.
    pub attempt: String,
    /// The nonce the popup's messages must carry (32 hex digits). Only ever
    /// between this page and the popup — never in a URL.
    pub nonce: String,
    /// The popup's origin, where the browser signs in: the hub's app origin
    /// (`https://my.droppen.app`), else its own (`https://droppen.app`).
    pub hub_web: String,
    /// When the attempt expires (ms).
    pub expires_ms: u64,
    /// This computer as a machine.
    pub machine: LinkMachine,
    /// The aggregator's pending keys (when aggregation was asked for).
    pub aggregator: Option<LinkAggregator>,
    /// The droppen.app address that answers this attempt through the browser
    /// instead of the popup (§4.9: the desktop app's window, a blocked popup):
    /// `<hub>/link#droppen-link=…`. It carries the attempt's state in its
    /// fragment — never in a query, never logged. `null` when the request
    /// said no `Origin` to come back to.
    pub handoff_url: Option<String>,
}

/// `POST /api/cloud/link/finish`.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct FinishRequest {
    /// The attempt's id.
    pub attempt: String,
    /// Its nonce.
    pub nonce: String,
    /// The popup's RH1 enrollment code.
    pub code: String,
    /// The account's e-mail address, as the popup said it (display only).
    pub email: Option<String>,
}

/// `POST /api/cloud/link/return`: droppen.app's sealed reply, as the page at
/// `/link/return` took it from its address.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct ReturnRequest {
    /// The attempt's id.
    pub attempt: String,
    /// The sealed reply, base64url.
    pub sealed: String,
}

/// The enrollment the hub took (202: a device completes it).
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct FinishResult {
    /// The machine id the hub assigned.
    pub machine: String,
    /// The account.
    pub account: String,
}

/// `POST /api/cloud/link/cancel`.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct CancelRequest {
    /// The attempt's id.
    pub attempt: String,
}

/// `POST /api/cloud/aggregator`.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AggregatorRequest {
    /// `false` turns it off and deletes the other devices' keys here; `true`
    /// turns it on again (chain v3 needs nothing from droppen.app for it: this
    /// computer controls with the identity it already has).
    pub enabled: bool,
}

/// `POST /api/cloud/unlink`.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct UnlinkRequest {
    /// Also delete this machine's encrypted copy on the hub.
    pub delete_copies: Option<bool>,
}

/// `POST /api/cloud/re-enroll-fresh`: this computer's new identity.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct ReEnrollFresh {
    /// The new identity key's fingerprint (8 groups of 4 hex digits).
    pub fingerprint: String,
    /// `unenrolled`: it waits for a registration code.
    pub state: String,
}

/// `PUT /api/cloud/access`.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "camelCase")]
pub struct AccessRequest {
    /// The local account.
    pub user: String,
    /// Whether it may see and use other machines.
    pub remote: bool,
}

/// A body sent with `202 Accepted`.
#[derive(Debug, Clone)]
pub struct Accepted<T>(pub T);

impl<T: Serialize> IntoResponse for Accepted<T> {
    fn into_response(self) -> Response {
        (StatusCode::ACCEPTED, Json(self.0)).into_response()
    }
}

impl<T: Schematic> Schematic for Accepted<T> {
    fn name() -> &'static str {
        T::name()
    }
    fn required() -> bool {
        T::required()
    }
    fn nullable() -> Option<bool> {
        T::nullable()
    }
    fn type_() -> &'static str {
        T::type_()
    }
    fn doc() -> Option<String> {
        T::doc()
    }
    fn fields() -> Vec<(&'static str, EnhancedSchema)> {
        T::fields()
    }
    fn generate_schema() -> EnhancedSchema {
        T::generate_schema()
    }
}

// ---------------------------------------------------------------- handlers

fn standing_name(state: Standing) -> &'static str {
    match state {
        Standing::None => "none",
        Standing::Enrolling => "enrolling",
        Standing::Linked => "linked",
        Standing::NotHosting => "not_hosting",
        Standing::Removed => "removed",
        Standing::Broken => "broken",
    }
}

/// Whether `user` may see what is copied of other devices (§4.6; the user's
/// decision of 2026-10-19): the owner, and the local accounts the owner
/// allowed — only while `cloud.json` reads (`file`), names an owner that is
/// that local account here now ([`owner_of`]; `accounts`: every local
/// account's `createdAt`), and local authentication is on. An allowance
/// holds for the account it was given to only ([`same_account`]).
fn sees_other_devices(file: Option<&CloudFile>, accounts: &BTreeMap<String, String>, local_auth: bool, user: Option<&str>) -> bool {
    let (Some(file), true, Some(user)) = (file, local_auth, user) else {
        return false;
    };
    let Some(owner) = owner_of(file, accounts) else {
        return false;
    };
    owner == user || (file.remote_access.get(user) == Some(&true) && same_account(file, accounts, user))
}

/// The status for `user` (`None`: nobody signed in, local authentication off).
async fn status_for(state: &AppState, user: Option<&str>) -> CloudStatus {
    let cloud = &state.cloud;
    let agent: AgentStatus = cloud.control.status();
    let file = cloud.read_file();
    let attempt = cloud.live_attempt();
    // Signed in with authentication on: nobody without a session gets here.
    let local_auth = !state.drive.auth.is_off() && user.is_some();
    let owner_now = match user {
        Some(user) => local_owner_now(state, user).await,
        None => false,
    };
    let can_manage = match user {
        Some(user) => local_auth && may_manage(state, &file, user).await,
        None => false,
    };
    // What is copied of the other devices is theirs (§4.6; the user's
    // decision of 2026-10-19): shown to the owner, and to the local accounts
    // the owner allowed — never while there is no owner (or its account is
    // gone, or `cloud.json` does not read), never with local authentication off.
    let strict = cloud.read_file_strict();
    let created = self::accounts(state).await;
    let owner = strict.as_ref().and_then(|f| owner_of(f, &created));
    let sees_other_devices = if owner_now {
        strict.is_some()
    } else {
        sees_other_devices(strict.as_ref(), &created, local_auth, user)
    };
    // Whoever manages an ownerless link may say "I added this device" — a
    // local account; the local owner already is the owner while it lasts.
    let can_claim = can_manage && !owner_now && strict.is_some() && owner.is_none() && !matches!(agent.state, Standing::None);
    let accounts = if can_manage {
        created
            .keys()
            .map(|name| LocalAccountAccess {
                remote: self::sees_other_devices(strict.as_ref(), &created, local_auth, Some(name)),
                owner: owner == Some(name.as_str()),
                user: name.clone(),
            })
            .collect()
    } else {
        Vec::new()
    };
    let hub = match (&agent.hub, &attempt) {
        (Some(hub), _) => hub.clone(),
        (None, Some(a)) => a.hub.clone(),
        (None, None) => resolve_hub(None).unwrap_or_else(|_| "wss://api.droppen.app".into()),
    };
    let hub_web = match &attempt {
        // The attempt's own: the page that opened it follows that one.
        Some(a) if a.hub == hub => a.hub_web.clone(),
        _ => cloud.hub_webs.for_status(&hub).await,
    };
    // The fingerprint only when the key exists already: a status is never
    // what makes one.
    let fingerprint = crate::agent::key_path(&cloud.paths)
        .exists()
        .then(|| cloud.control.identity().ok())
        .flatten()
        .map(|identity| machine_fingerprint(&identity));
    let has_state = !matches!(agent.state, Standing::None);
    CloudStatus {
        state: standing_name(agent.state).into(),
        local_auth,
        hub,
        hub_web,
        account: agent.account.clone().map(|id| CloudAccount {
            id,
            fingerprint: agent.account_fingerprint.clone().unwrap_or_default(),
            email: file.email.clone(),
        }),
        machine: CloudMachine {
            id: agent.machine.clone(),
            hostname: super::hostname(),
            fingerprint,
        },
        link: has_state.then(|| CloudLinkHealth {
            running: agent.running,
            connected: agent.connected,
            since_ms: agent.since_ms,
            error: agent.error.clone(),
            refused: agent.refused.clone(),
            chain_seq: agent.chain_seq,
            devices: agent.devices.map(|n| n as u64),
            machines: agent.machines.map(|n| n as u64),
            removed_ms: agent.removed_ms,
            alarms: agent.alarms.iter().map(|a| a.what.clone()).collect(),
            changes: agent.changes.iter().map(|a| a.what.clone()).collect(),
            rotation: agent.rotation.clone().into(),
        }),
        aggregator: {
            let control = cloud.control.control_status().filter(|c| c.running);
            CloudAggregator {
                enabled: crate::aggregator::wanted(&cloud.paths),
                state: if crate::aggregator::key_path(&cloud.paths).exists() {
                    "pending".into()
                } else if control.is_some() {
                    "on".into()
                } else {
                    "off".into()
                },
                signed_in: control.as_ref().is_some_and(|c| c.signed_in),
                error: control.as_ref().and_then(|c| c.error.clone()),
                hosts: control
                    .as_ref()
                    .map(|c| {
                        c.hosts
                            .iter()
                            .map(|h| AggregatorHost {
                                id: h.id.clone(),
                                latest_epoch: h.latest_epoch,
                                epochs: h.epochs.clone(),
                                policy: h.policy,
                                copy: h.copy.as_ref().filter(|_| sees_other_devices).map(|c| AggregatorCopy {
                                    index_rows: c.index_rows,
                                    index_as_of_ms: c.index_as_of_ms,
                                    status_at_ms: c.status_at_ms,
                                    sessions: c.sessions,
                                    online: c.online,
                                }),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                asked_ms: control.as_ref().and_then(|c| c.asked_ms),
            }
        },
        attempt: attempt.map(|a| {
            let mine = user == Some(a.owner.as_str());
            CloudAttempt {
                mine,
                handoff_url: a.handoff.as_ref().filter(|_| mine).map(|h| h.url().to_string()),
                id: a.id,
                expires_ms: a.expires_ms,
            }
        }),
        handoff: cloud.handoff_outcome_for(user),
        owner: file.owner.clone(),
        can_manage,
        remote_access: file
            .remote_access
            .iter()
            .map(|(user, remote)| RemoteAccess {
                user: user.clone(),
                remote: *remote,
            })
            .collect(),
        sees_other_devices,
        accounts,
        can_claim,
    }
}

/// Where this computer stands with Droppen.
#[api(id = "cloud_status", group = "cloud", summary = "Droppen link status")]
pub async fn get_cloud(State(state): State<AppState>, #[api(skip)] who: MaybeIdentity) -> Result<Json<CloudStatus>, ApiError> {
    let user = caller(&who)?;
    Ok(Json(status_for(&state, user.as_deref()).await))
}

/// Open a sign-in attempt: a nonce, and the keys the popup checks — and,
/// for a page that said its origin, the droppen.app address that answers it
/// through the browser instead (§4.9).
#[api(id = "cloud_link", group = "cloud", summary = "Start signing in to Droppen")]
pub async fn post_link(
    State(state): State<AppState>,
    #[api(skip)] who: MaybeIdentity,
    #[api(skip)] PageOrigin(page): PageOrigin,
    ApiJson(body): ApiJson<LinkRequest>,
) -> Result<Json<LinkAttempt>, ApiError> {
    let user = local_user(&state, &who).await?;
    require_manager(&state, &user).await?;
    let cloud = &state.cloud;
    cloud
        .control
        .available()
        .map_err(|e| refusal(StatusCode::SERVICE_UNAVAILABLE, "unavailable", &e.to_string()))?;
    // A seed a renewal retired never signs up again: this sign-in is then a
    // new device, under a fresh identity (`docs/host-identity-rotation.md` §3.8).
    if cloud.control.renew_retired_seed().await.map_err(|e| ApiError::internal(e.to_string()))? {
        tracing::warn!(by = %user, "this computer's identity key was retired: signing in under a new identity");
    }
    let agent = cloud.control.status();
    let enrolled = match agent.state {
        // The account removed it: a new sign-in is a new machine, with new keys.
        Standing::Removed => {
            cloud.control.unenroll().await.map_err(|e| ApiError::internal(e.to_string()))?;
            None
        }
        Standing::Linked | Standing::NotHosting => {
            let st = crate::agent::state::AgentState::load(&cloud.paths)
                .map_err(ApiError::internal)?
                .ok_or_else(|| ApiError::internal("the agent state went away"))?;
            Some(LinkEnrolled {
                account_fp: st.root.account_fingerprint.iter().map(|b| format!("{b:02x}")).collect(),
                id: st.machine.clone(),
            })
        }
        Standing::Broken => {
            return Err(refusal(
                StatusCode::CONFLICT,
                "broken",
                "this device's Droppen state does not load; disconnect it first",
            ))
        }
        Standing::None | Standing::Enrolling => None,
    };
    let hub = match (&enrolled, &agent.hub) {
        // An enrolled computer stays with its hub.
        (Some(_), Some(hub)) => hub.clone(),
        _ => resolve_hub(body.hub.as_deref())?,
    };
    // Where the browser signs in: the hub's app origin, when its discovery
    // document names one (my.droppen.app), else its own (`super::hub_web`).
    let hub_web = cloud.hub_webs.for_sign_in(&hub).await;
    let identity = cloud.control.identity().map_err(ApiError::internal)?;
    let aggregate = body.aggregate.unwrap_or(true);
    // Chain v3 (local-aggregator §3.3): this computer controls other devices
    // with its one identity, the one it runs AI with — no second key pair.
    // `aggregator` stays in the answer (always null) for pages from before.
    let aggregator: Option<LinkAggregator> = None;
    let id = random_hex(16);
    let expires_ms = now_ms() + ATTEMPT_TTL_MS;
    let hostname = super::hostname();
    // The browser hand-off (§4.9), for a page that said where it is: the
    // reply comes back to that origin, which the `/api/*` origin check has
    // already accepted as this server's.
    let handoff = page.as_deref().map(|origin| {
        Handoff::new(
            &hub_web,
            &id,
            expires_ms,
            origin,
            &HandoffMachine {
                hostname: hostname.clone(),
                sign_key: identity.sign_public(),
                dh_key: identity.dh_public(),
                enrolled: enrolled.as_ref().map(|e| (e.account_fp.clone(), e.id.clone())),
            },
        )
    });
    let attempt = Attempt {
        id,
        nonce: random_hex(16),
        owner: user,
        hub,
        hub_web: hub_web.clone(),
        aggregate,
        expires_ms,
        handoff,
    };
    *cloud.attempt.lock().expect("attempt") = Some(attempt.clone());
    tracing::info!(
        attempt = %attempt.id,
        hub = %attempt.hub,
        enrolled = enrolled.is_some(),
        browser = attempt.handoff.is_some(),
        "a Droppen sign-in attempt opened"
    );
    Ok(Json(LinkAttempt {
        handoff_url: attempt.handoff.as_ref().map(|h| h.url().to_string()),
        attempt: attempt.id,
        nonce: attempt.nonce,
        hub_web,
        expires_ms: attempt.expires_ms,
        machine: LinkMachine {
            hostname,
            sign_key: crate::agent::hub::hex(&identity.sign_public()),
            dh_key: crate::agent::hub::hex(&identity.dh_public()),
            fingerprint: machine_fingerprint(&identity),
            enrolled,
        },
        aggregator,
    }))
}

/// Check an attempt against what the caller sent: it exists (404), it has
/// not expired (410), the caller opened it (403) and the nonce matches (403).
fn check_attempt(slot: &Option<Attempt>, id: &str, nonce: Option<&str>, user: &str, now: u64) -> Result<Attempt, ApiError> {
    let attempt = live_attempt_by_id(slot, id, now)?;
    if attempt.owner != user {
        return Err(refusal(
            StatusCode::FORBIDDEN,
            "not_your_attempt",
            "another local account opened this sign-in attempt",
        ));
    }
    if let Some(nonce) = nonce {
        let same = subtle::ConstantTimeEq::ct_eq(nonce.as_bytes(), attempt.nonce.as_bytes());
        if !bool::from(same) {
            return Err(refusal(StatusCode::FORBIDDEN, "bad_nonce", "the sign-in attempt's nonce does not match"));
        }
    }
    Ok(attempt.clone())
}

/// The attempt `id`, while it lives: 404 `no_attempt`, 410 `expired`.
fn live_attempt_by_id<'a>(slot: &'a Option<Attempt>, id: &str, now: u64) -> Result<&'a Attempt, ApiError> {
    let Some(attempt) = slot.as_ref().filter(|a| a.id == id) else {
        return Err(refusal(StatusCode::NOT_FOUND, "no_attempt", "no such sign-in attempt (start again)"));
    };
    if attempt.expires_ms <= now {
        return Err(refusal(StatusCode::GONE, "expired", "the sign-in attempt expired (start again)"));
    }
    Ok(attempt)
}

/// Check droppen.app's sealed reply against the attempt `id` and, when it
/// opens, use the attempt up in the same breath (one answer per attempt,
/// whichever way it comes): 404, 410, 409 `no_handoff` (the attempt was
/// opened without a page to come back to), 403 `bad_reply` (it does not
/// open: another attempt's, another state's — a page that never saw the
/// address — or tampered; the attempt stays). The attempt and the reply.
fn take_handed_back(slot: &mut Option<Attempt>, id: &str, sealed: &[u8], now: u64) -> Result<(Attempt, Reply), ApiError> {
    let attempt = live_attempt_by_id(slot, id, now)?;
    let Some(handoff) = attempt.handoff.as_ref() else {
        return Err(refusal(
            StatusCode::CONFLICT,
            "no_handoff",
            "this sign-in attempt does not take an answer through the browser (start again)",
        ));
    };
    let bad = || {
        refusal(
            StatusCode::FORBIDDEN,
            "bad_reply",
            "droppen.app's answer does not belong to this sign-in attempt",
        )
    };
    let opened = handoff.open(id, sealed).map_err(|_| bad())?;
    let reply: Reply = serde_json::from_slice(&opened).map_err(|_| bad())?;
    Ok((slot.take().expect("checked"), reply))
}

/// Check an attempt as [`check_attempt`] does and, when it passes, use it up
/// in the same breath (under the caller's lock): one `finish` per attempt,
/// however many race for it.
fn take_attempt(slot: &mut Option<Attempt>, id: &str, nonce: &str, user: &str, now: u64) -> Result<Attempt, ApiError> {
    check_attempt(slot, id, Some(nonce), user, now)?;
    Ok(slot.take().expect("checked"))
}

/// The popup's code (`RH1-…`, or `DR1-…` from a newer popup), or `400 bad_code`.
fn finish_code(text: &str) -> Result<rh_proto::identity::EnrollCode, ApiError> {
    text.parse().map_err(|e: &str| refusal(StatusCode::BAD_REQUEST, "bad_code", e))
}

/// 409 `already_enrolled` while this computer is in an account.
fn refuse_if_enrolled(state: &AppState) -> Result<(), ApiError> {
    if matches!(state.cloud.control.status().state, Standing::Linked | Standing::NotHosting) {
        return Err(refusal(
            StatusCode::CONFLICT,
            "already_enrolled",
            "this device is in a Droppen account already",
        ));
    }
    Ok(())
}

/// Hand over the popup's code and enroll in process.
#[api(id = "cloud_link_finish", group = "cloud", summary = "Finish signing in to Droppen")]
pub async fn post_finish(
    State(state): State<AppState>,
    #[api(skip)] who: MaybeIdentity,
    ApiJson(body): ApiJson<FinishRequest>,
) -> Result<Accepted<FinishResult>, ApiError> {
    let user = local_user(&state, &who).await?;
    let code = finish_code(&body.code)?;
    refuse_if_enrolled(&state)?;
    // Checked and used up under the lock: a second `finish` racing this one
    // finds no attempt. Whatever happens next, this attempt is spent — a
    // failure below is said, and signing in again opens a new one.
    let attempt = {
        let mut slot = state.cloud.attempt.lock().expect("attempt");
        take_attempt(&mut slot, &body.attempt, &body.nonce, &user, now_ms())?
    };
    complete_sign_in(&state, attempt, &code, body.email).await
}

/// Hand back droppen.app's answer through the browser (§4.9): the page at
/// `/link/return` posts the sealed reply it found in its address; it opens
/// with the attempt's one-off key and state only, and then enrolls exactly as
/// `finish` does, for the local account that opened the attempt.
///
/// No login is asked: the reply comes through the system browser, which has
/// none here (the desktop app's window has it, the browser does not). The
/// attempt is the credential — its key never leaves this process and its
/// state only went to the page that opened it — and a reply that does not
/// open is refused without using the attempt up. Refused as everywhere: a
/// device bridged from Droppen (403), local authentication off (409).
///
/// Whether this device is in an account already is said only once the reply
/// opened (409 `already_enrolled`, and the attempt is used up): before that,
/// the caller is anybody who reaches the port.
#[api(id = "cloud_link_return", group = "cloud", summary = "Hand back droppen.app's answer from the browser")]
pub async fn post_return(
    State(state): State<AppState>,
    #[api(skip)] who: MaybeIdentity,
    ApiJson(body): ApiJson<ReturnRequest>,
) -> Result<Accepted<FinishResult>, ApiError> {
    caller(&who)?;
    if state.drive.auth.is_off() {
        return Err(local_auth_off());
    }
    let sealed = link_handoff::sealed_bytes(&body.sealed)
        .ok_or_else(|| refusal(StatusCode::BAD_REQUEST, "bad_reply", "droppen.app's answer is not readable"))?;
    let (attempt, reply) = {
        let mut slot = state.cloud.attempt.lock().expect("attempt");
        let taken = take_handed_back(&mut slot, &body.attempt, &sealed, now_ms())?;
        // Said before the attempt's lock goes: no look at the status ever
        // finds the attempt gone without this.
        state.cloud.record_handoff(&taken.0, HandoffResult::Receiving);
        taken
    };
    tracing::info!(attempt = %attempt.id, "droppen.app answered a sign-in through the browser");
    let result = async {
        refuse_if_enrolled(&state)?;
        let code = finish_code(&reply.code)?;
        // Who opened it must still be one who manages the link.
        let file = state.cloud.read_file();
        if !may_manage(&state, &file, &attempt.owner).await {
            return Err(refusal(
                StatusCode::FORBIDDEN,
                "not_owner",
                "only the local account that signed this device in to Droppen can do this",
            ));
        }
        complete_sign_in(&state, attempt.clone(), &code, reply.email).await
    }
    .await;
    let outcome = match &result {
        Ok(Accepted(done)) => HandoffResult::HandedOver(done.machine.clone()),
        Err(error) => {
            let body = error.body();
            let code = body["error"].as_str().unwrap_or("failed").to_string();
            let message = body["message"].as_str().unwrap_or(&code).to_string();
            HandoffResult::Failed(code, message)
        }
    };
    state.cloud.record_handoff(&attempt, outcome);
    result
}

/// What `finish` and `return` share once the attempt is used up: enroll in
/// process with the code, and make the local account that opened the attempt
/// the link's owner.
async fn complete_sign_in(
    state: &AppState,
    attempt: Attempt,
    code: &rh_proto::identity::EnrollCode,
    email: Option<String>,
) -> Result<Accepted<FinishResult>, ApiError> {
    let cloud = &state.cloud;
    let user = attempt.owner.clone();
    let (account, machine) = match cloud.control.enroll(code, &attempt.hub).await {
        Ok(done) => done,
        Err(error) => {
            // The link is as it was (`Control::enroll` restores it); the
            // pending aggregator key goes with the spent attempt.
            cloud.forget_pending_aggregator();
            return Err(match error {
                ControlError::Begin(BeginError::Unreachable(message)) | ControlError::Unreachable(message) => {
                    tracing::warn!(%message, "the hub is unreachable; the sign-in failed");
                    refusal(StatusCode::BAD_GATEWAY, "hub_unreachable", &message)
                }
                ControlError::Begin(BeginError::Refused(message)) => refusal(StatusCode::CONFLICT, "refused", &message),
                ControlError::Begin(BeginError::Enrolled(message)) => refusal(StatusCode::CONFLICT, "already_enrolled", &message),
                ControlError::Unavailable(message) => refusal(StatusCode::SERVICE_UNAVAILABLE, "unavailable", &message),
                error => ApiError::internal(error.to_string()),
            });
        }
    };

    let email = email
        .filter(|e| e.len() <= 254 && !e.chars().any(char::is_control))
        .map(|e| e.trim().to_string());
    let since = accounts(state).await.get(&user).cloned();
    cloud.update_file(|file| {
        // Another owner (or another account under the owner's name): what
        // the last one allowed was theirs to allow, and goes.
        let same = file.owner.as_deref() == Some(user.as_str()) && file.accounts_since.get(&user) == since.as_ref();
        if !same {
            file.remote_access.clear();
            file.accounts_since.clear();
        }
        if let Some(since) = &since {
            file.accounts_since.insert(user.clone(), since.clone());
        }
        file.hub = Some(attempt.hub_web.clone());
        file.owner = Some(user.clone());
        file.aggregate = attempt.aggregate;
        file.aggregator_off = !attempt.aggregate;
        file.linked_at = Some(now_ms());
        file.email = email;
        file.remote_access.insert(user.clone(), true);
    })?;
    // The aggregator is not admitted in this release, so its pending key goes.
    cloud.forget_pending_aggregator();
    tracing::info!(%account, %machine, owner = %user, "signed in to Droppen from the local settings; a device completes the enrollment");
    Ok(Accepted(FinishResult { machine, account }))
}

/// Drop the attempt (and an enrollment it started that no device completed).
#[api(id = "cloud_link_cancel", group = "cloud", summary = "Cancel signing in to Droppen")]
pub async fn post_cancel(
    State(state): State<AppState>,
    #[api(skip)] who: MaybeIdentity,
    ApiJson(body): ApiJson<CancelRequest>,
) -> Result<Json<SendResult>, ApiError> {
    let user = local_user(&state, &who).await?;
    let cloud = &state.cloud;
    {
        let mut slot = cloud.attempt.lock().expect("attempt");
        match check_attempt(&slot, &body.attempt, None, &user, now_ms()) {
            Ok(_) => *slot = None,
            // An attempt that expired meanwhile is gone either way.
            Err(e) if e.status() == StatusCode::GONE => *slot = None,
            Err(e) => return Err(e),
        }
    }
    cloud.forget_pending_aggregator();
    Ok(Json(SendResult { accepted: true }))
}

/// Turn the aggregator off — the other devices' keys here are deleted — or
/// on again (S1-4: it needs nothing from droppen.app since chain v3).
#[api(id = "cloud_aggregator", group = "cloud", summary = "Turn the aggregator off or on")]
pub async fn post_aggregator(
    State(state): State<AppState>,
    #[api(skip)] who: MaybeIdentity,
    ApiJson(body): ApiJson<AggregatorRequest>,
) -> Result<Json<CloudStatus>, ApiError> {
    let user = local_user(&state, &who).await?;
    require_manager(&state, &user).await?;
    let cloud = &state.cloud;
    if body.enabled {
        cloud.update_file(|file| {
            file.aggregate = true;
            file.aggregator_off = false;
        })?;
        // The controller sees it on its next round; a chain change wakes it sooner.
        return Ok(Json(status_for(&state, Some(&user)).await));
    }
    // Off first (the controller stores nothing once it reads that), then
    // the directory, under the lock it stores under.
    cloud.update_file(|file| {
        file.aggregate = false;
        file.aggregator_off = true;
    })?;
    cloud.forget_aggregator()?;
    Ok(Json(status_for(&state, Some(&user)).await))
}

/// Disconnect this computer from Droppen.
#[api(id = "cloud_unlink", group = "cloud", summary = "Disconnect this computer from Droppen")]
pub async fn post_unlink(
    State(state): State<AppState>,
    #[api(skip)] who: MaybeIdentity,
    ApiJson(body): ApiJson<UnlinkRequest>,
) -> Result<Json<CloudStatus>, ApiError> {
    let user = local_user(&state, &who).await?;
    require_manager(&state, &user).await?;
    let cloud = &state.cloud;
    if body.delete_copies.unwrap_or(false) {
        match cloud.control.delete_copies().await {
            Ok(n) => tracing::info!(n, "deleted the copy on the hub before disconnecting"),
            Err(ControlError::Unreachable(message)) => {
                // Nothing was disconnected: the link starts again, and the
                // owner can retry, or disconnect without deleting.
                cloud.control.start_if_enrolled().await;
                return Err(refusal(StatusCode::BAD_GATEWAY, "hub_unreachable", &message));
            }
            Err(error) => {
                cloud.control.start_if_enrolled().await;
                return Err(ApiError::internal(error.to_string()));
            }
        }
    }
    cloud.control.unenroll().await.map_err(|e| ApiError::internal(e.to_string()))?;
    *cloud.attempt.lock().expect("attempt") = None;
    cloud.forget_aggregator()?;
    cloud.delete_file()?;
    tracing::info!(by = %user, "disconnected this computer from Droppen");
    Ok(Json(status_for(&state, Some(&user)).await))
}

/// "Add this device again with a new identity" (`docs/host-identity-rotation.md`
/// §3.8 「只剩退役钥时」): only while this computer's identity key was replaced
/// elsewhere — its own verified chain gives its id keys it does not hold, for
/// example after restoring a backup made before a renewal
/// (`link.rotation.state` `foreign`; 409 `not_renewed_elsewhere` otherwise):
/// it holds only a retired key, which nothing renews in place.
/// Its seed is archived (`agent/machine-key.retired-<ms>.json`, never used
/// again), the old enrollment forgotten as a disconnection forgets it, and a
/// fresh seed made: the computer is signed in to Droppen again like a new
/// one, with a registration code, under a new id.
#[api(id = "cloud_re_enroll_fresh", group = "cloud", summary = "Add this computer again with a new identity")]
pub async fn post_re_enroll_fresh(State(state): State<AppState>, #[api(skip)] who: MaybeIdentity) -> Result<Json<ReEnrollFresh>, ApiError> {
    let user = local_user(&state, &who).await?;
    require_manager(&state, &user).await?;
    let cloud = &state.cloud;
    if cloud.control.status().rotation.state != "foreign" {
        return Err(refusal(
            StatusCode::CONFLICT,
            "not_renewed_elsewhere",
            "this computer's identity key was not replaced elsewhere: renew it from an admin device, or disconnect it",
        ));
    }
    let identity = cloud.control.re_enroll_fresh().await.map_err(|e| ApiError::internal(e.to_string()))?;
    *cloud.attempt.lock().expect("attempt") = None;
    cloud.forget_aggregator()?;
    cloud.delete_file()?;
    tracing::warn!(by = %user, "this computer starts again under a new identity; its old identity key was replaced elsewhere");
    Ok(Json(ReEnrollFresh {
        fingerprint: machine_fingerprint(&identity),
        state: "unenrolled".into(),
    }))
}

/// "I added this device": the caller becomes the owner of an ownerless link
/// (enrolled from the command line, or its owner's local account is gone) —
/// and so, alone at first, sees the other devices here (the user's decision
/// of 2026-10-19). Only whoever manages the link may; a link with an owner
/// here is refused (409 `has_owner`, decided under the file's lock: of two
/// claims at once, one wins), as is a `cloud.json` that does not read (409
/// `unreadable`: claiming must not write over the owner's settings). What
/// was allowed before goes: who else sees them is the new owner's to say.
#[api(id = "cloud_claim", group = "cloud", summary = "Claim this device's Droppen link")]
pub async fn post_claim(State(state): State<AppState>, #[api(skip)] who: MaybeIdentity) -> Result<Json<CloudStatus>, ApiError> {
    let user = local_user(&state, &who).await?;
    require_manager(&state, &user).await?;
    if matches!(state.cloud.control.status().state, Standing::None) {
        return Err(refusal(StatusCode::CONFLICT, "not_linked", "this device is not signed in to Droppen"));
    }
    let accounts = accounts(&state).await;
    let Some(since) = accounts.get(&user).cloned() else {
        return Err(refusal(StatusCode::UNAUTHORIZED, "unauthorized", "this local account is gone"));
    };
    state.cloud.try_update_file(|f| {
        if owner_of(f, &accounts).is_some() {
            return Err(refusal(
                StatusCode::CONFLICT,
                "has_owner",
                "a local account here already owns this device's Droppen link",
            ));
        }
        f.owner = Some(user.clone());
        f.remote_access.clear();
        f.accounts_since.clear();
        f.accounts_since.insert(user.clone(), since);
        Ok(())
    })?;
    tracing::info!(owner = %user, "a local account claimed this device's Droppen link");
    Ok(Json(status_for(&state, Some(&user)).await))
}

/// Which local accounts may see and use other machines.
#[api(id = "cloud_access", group = "cloud", summary = "Set a local account's access to other machines")]
pub async fn put_access(
    State(state): State<AppState>,
    #[api(skip)] who: MaybeIdentity,
    ApiJson(body): ApiJson<AccessRequest>,
) -> Result<Json<CloudStatus>, ApiError> {
    let user = local_user(&state, &who).await?;
    let accounts = accounts(&state).await;
    let Some(since) = accounts.get(&body.user).cloned() else {
        return Err(refusal(StatusCode::NOT_FOUND, "no_user", "no such local account"));
    };
    // The desktop app's own login is not somebody to let see other devices.
    if body.remote && state.drive.auth.users.is_app(&body.user).await {
        return Err(refusal(
            StatusCode::BAD_REQUEST,
            "app_account",
            "the Droppen app's own account cannot be let see other devices",
        ));
    }
    // Only the owner says who else sees them — not anybody who may manage an
    // ownerless link (an allowance written then would hold for whoever
    // claims it later). Decided under the file's lock.
    state.cloud.try_update_file(|file| {
        if owner_of(file, &accounts) != Some(user.as_str()) {
            return Err(refusal(
                StatusCode::FORBIDDEN,
                "not_owner",
                "only the local account that owns this device's Droppen link can say who else sees your other devices here",
            ));
        }
        file.remote_access.insert(body.user.clone(), body.remote);
        if body.remote {
            file.accounts_since.insert(body.user.clone(), since);
        } else if file.owner.as_deref() != Some(body.user.as_str()) {
            file.accounts_since.remove(&body.user);
        }
        Ok(())
    })?;
    Ok(Json(status_for(&state, Some(&user)).await))
}

/// `POST` here hands back droppen.app's answer from the browser; it is the
/// one route of this module the login guard lets through without a login
/// ([`crate::auth`]): the reply is its own credential.
pub const RETURN_ROUTE: &str = "/api/cloud/link/return";

/// Register this module's routes.
pub fn routes(router: ApiRouter) -> ApiRouter {
    router
        .get("/api/cloud", get_cloud)
        .post("/api/cloud/link", post_link)
        .post("/api/cloud/link/finish", post_finish)
        .post(RETURN_ROUTE, post_return)
        .post("/api/cloud/link/cancel", post_cancel)
        .post("/api/cloud/aggregator", post_aggregator)
        .post("/api/cloud/unlink", post_unlink)
        .post("/api/cloud/re-enroll-fresh", post_re_enroll_fresh)
        .put("/api/cloud/access", put_access)
        .post("/api/cloud/claim", post_claim)
}

/// Everything gotcha cannot express about those routes.
pub fn describe(reg: &mut ExtraResponses) {
    reg.describe("GET", "/api/cloud", 200, "Where this computer stands with Droppen")
        .error("GET", "/api/cloud", 403, "Bridged from Droppen (`not_from_cloud`)")
        .describe("POST", "/api/cloud/link", 200, "The attempt: its nonce and the keys the popup checks")
        .error("POST", "/api/cloud/link", 403, "Not the owner, or bridged from Droppen")
        .error("POST", "/api/cloud/link", 409, "Local authentication is off (`local_auth_required`)")
        .restatus(
            "POST",
            "/api/cloud/link/finish",
            200,
            202,
            "Enrolling: a device of the account completes it",
        )
        .error("POST", "/api/cloud/link/finish", 403, "Another user's attempt, or a wrong nonce")
        .error("POST", "/api/cloud/link/finish", 404, "No such attempt")
        .error("POST", "/api/cloud/link/finish", 410, "The attempt expired")
        .error("POST", "/api/cloud/link/finish", 502, "The hub could not be reached (`hub_unreachable`)")
        .restatus(
            "POST",
            "/api/cloud/link/return",
            200,
            202,
            "Enrolling: a device of the account completes it",
        )
        .error("POST", "/api/cloud/link/return", 400, "Not a readable reply (`bad_reply`), or not a code (`bad_code`)")
        .error(
            "POST",
            "/api/cloud/link/return",
            403,
            "The reply does not open for this attempt (`bad_reply`), the one who opened it no longer manages the link (`not_owner`), or bridged from Droppen",
        )
        .error("POST", "/api/cloud/link/return", 404, "No such attempt")
        .error(
            "POST",
            "/api/cloud/link/return",
            409,
            "Local authentication is off, the attempt takes no answer through the browser (`no_handoff`), or — once the reply opened — this device is in an account already (`already_enrolled`)",
        )
        .error("POST", "/api/cloud/link/return", 410, "The attempt expired")
        .error("POST", "/api/cloud/link/return", 502, "The hub could not be reached (`hub_unreachable`)")
        .describe("POST", "/api/cloud/link/cancel", 200, "Cancelled")
        .describe("POST", "/api/cloud/aggregator", 200, "The status after")
        .describe("POST", "/api/cloud/unlink", 200, "The status after")
        .error(
            "POST",
            "/api/cloud/unlink",
            502,
            "The copy on the hub could not be deleted (`hub_unreachable`)",
        )
        .describe("POST", "/api/cloud/re-enroll-fresh", 200, "The new identity's fingerprint; the computer waits for a registration code")
        .error(
            "POST",
            "/api/cloud/re-enroll-fresh",
            409,
            "Local authentication is off, or this computer's identity key was not replaced elsewhere (`not_renewed_elsewhere`)",
        )
        .describe("PUT", "/api/cloud/access", 200, "The status after")
        .error(
            "PUT",
            "/api/cloud/access",
            403,
            "Not the owner of the link (`not_owner`), or bridged from Droppen",
        )
        .error("PUT", "/api/cloud/access", 404, "No such local account (`no_user`)")
        .error(
            "PUT",
            "/api/cloud/access",
            409,
            "Local authentication is off, or the link's settings do not read (`unreadable`)",
        )
        .describe("POST", "/api/cloud/claim", 200, "The status after: the caller owns the link")
        .error("POST", "/api/cloud/claim", 403, "Does not manage the link, or bridged from Droppen")
        .error(
            "POST",
            "/api/cloud/claim",
            409,
            "Local authentication is off, the link has an owner here (`has_owner`), it is not signed in, or its settings do not read",
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attempt(owner: &str, expires_ms: u64) -> Option<Attempt> {
        Some(Attempt {
            id: "a1".into(),
            nonce: "n1".into(),
            owner: owner.into(),
            hub: "wss://api.droppen.app".into(),
            hub_web: DEFAULT_HUB.into(),
            aggregate: true,
            expires_ms,
            handoff: None,
        })
    }

    fn code_of(e: ApiError) -> (u16, String) {
        (e.status().as_u16(), e.body()["error"].as_str().unwrap_or_default().to_string())
    }

    #[test]
    fn an_attempt_is_only_the_opener_s_with_its_nonce_until_it_expires() {
        let slot = attempt("alice", 1_000);
        assert!(check_attempt(&slot, "a1", Some("n1"), "alice", 999).is_ok());
        assert_eq!(
            code_of(check_attempt(&slot, "a1", Some("n1"), "bob", 999).unwrap_err()),
            (403, "not_your_attempt".into())
        );
        assert_eq!(
            code_of(check_attempt(&slot, "a1", Some("n2"), "alice", 999).unwrap_err()),
            (403, "bad_nonce".into())
        );
        assert_eq!(
            code_of(check_attempt(&slot, "a1", Some("n1"), "alice", 1_000).unwrap_err()),
            (410, "expired".into())
        );
        assert_eq!(
            code_of(check_attempt(&slot, "a2", Some("n1"), "alice", 999).unwrap_err()),
            (404, "no_attempt".into())
        );
        assert_eq!(
            code_of(check_attempt(&None, "a1", Some("n1"), "alice", 999).unwrap_err()),
            (404, "no_attempt".into())
        );
    }

    /// Review of the local sign-in: `finish` checked the attempt, let go of
    /// the lock, enrolled, and only then dropped it — two racing finishes
    /// both got through. The attempt is checked and used up at once now.
    #[test]
    fn an_attempt_is_used_up_by_the_first_finish() {
        let mut slot = attempt("alice", 1_000);
        assert!(
            take_attempt(&mut slot, "a1", "n2", "alice", 999).is_err(),
            "a wrong nonce uses nothing up"
        );
        assert!(slot.is_some());
        assert!(take_attempt(&mut slot, "a1", "n1", "bob", 999).is_err(), "nor does someone else");
        assert!(slot.is_some());
        assert!(take_attempt(&mut slot, "a1", "n1", "alice", 999).is_ok());
        assert!(slot.is_none(), "used up");
        assert_eq!(
            code_of(take_attempt(&mut slot, "a1", "n1", "alice", 999).unwrap_err()),
            (404, "no_attempt".into())
        );
    }

    #[test]
    fn the_popup_s_code_is_read_with_either_prefix_and_anything_else_is_a_bad_code() {
        let code = rh_proto::identity::EnrollCode {
            account_fingerprint: [3; 16],
            secret: [4; 16],
        };
        let text = code.to_string();
        let body = &text["RH1-".len()..];
        for ok in [
            text.clone(),
            format!("DR1-{body}"),
            format!("dr1-{body}").to_lowercase(),
            format!(" {text}\n"),
        ] {
            assert_eq!(finish_code(&ok).unwrap(), code, "{ok}");
        }
        for bad in [
            format!("DRP1-{body}"),
            format!("RHP1-{body}"),
            format!("DR2-{body}"),
            "RH1-nope".into(),
            String::new(),
        ] {
            assert_eq!(code_of(finish_code(&bad).unwrap_err()), (400, "bad_code".into()), "{bad}");
        }
    }

    #[test]
    fn a_bridged_device_is_refused_before_anything_else() {
        let bridged = MaybeIdentity(Some(crate::auth::AuthIdentity {
            username: "cloud:d-abc".into(),
            token_sha256: None,
        }));
        assert_eq!(code_of(caller(&bridged).unwrap_err()), (403, "not_from_cloud".into()));
        let local = MaybeIdentity(Some(crate::auth::AuthIdentity {
            username: "alice".into(),
            token_sha256: None,
        }));
        assert_eq!(caller(&local).unwrap().as_deref(), Some("alice"));
        assert_eq!(caller(&MaybeIdentity(None)).unwrap(), None);
    }

    /// Chain v3 review: disconnecting the computer and turning aggregation
    /// off deleted only the legacy pending key (`aggregator::forget`); other
    /// devices' machine keys and copies stayed. The whole directory goes now.
    #[tokio::test]
    async fn leaving_or_turning_aggregation_off_deletes_the_whole_aggregator_directory() {
        let home = tempfile::tempdir().unwrap();
        let state = crate::api::test_support::state(home.path()).await;
        state.drive.auth.users.add("alice", "correct horse battery staple").await.unwrap();
        let alice = || {
            MaybeIdentity(Some(crate::auth::AuthIdentity {
                username: "alice".into(),
                token_sha256: None,
            }))
        };
        let dir = crate::aggregator::dir(&state.cloud.paths);
        let fill = || {
            std::fs::create_dir_all(&dir).unwrap();
            for name in ["keys.json", "copies.db", "device-key.json"] {
                std::fs::write(dir.join(name), b"x").unwrap();
            }
        };
        fill();
        let _ = post_aggregator(State(state.clone()), alice(), ApiJson(AggregatorRequest { enabled: false }))
            .await
            .unwrap();
        assert!(!dir.exists(), "turning aggregation off deletes other devices' keys and copies");
        fill();
        let _ = post_unlink(State(state.clone()), alice(), ApiJson(UnlinkRequest { delete_copies: None }))
            .await
            .unwrap();
        assert!(!dir.exists(), "disconnecting deletes them too");
        // An attempt that ends only drops the legacy pending key.
        fill();
        state.cloud.forget_pending_aggregator();
        assert!(!crate::aggregator::key_path(&state.cloud.paths).exists());
        assert!(dir.join("keys.json").exists());
    }

    /// `docs/host-identity-rotation.md` §3.8 「只剩退役钥时」:
    /// only a computer whose identity key was renewed elsewhere starts again
    /// with a new identity — its seed archived, never used again, the
    /// enrollment forgotten, a fresh seed — and is then signed in like a new one.
    #[tokio::test]
    async fn only_a_computer_renewed_elsewhere_starts_again_with_a_new_identity() {
        let home = tempfile::tempdir().unwrap();
        let state = crate::api::test_support::state(home.path()).await;
        state.drive.auth.users.add("alice", "correct horse battery staple").await.unwrap();
        let alice = || {
            MaybeIdentity(Some(crate::auth::AuthIdentity {
                username: "alice".into(),
                token_sha256: None,
            }))
        };
        let paths = state.cloud.paths.clone();
        let old = state.cloud.control.identity().unwrap();
        let mut st = crate::agent::state::AgentState::enrolling("wss://hub.example", "a-1", "m-1", [9; 16], 5);
        st.save(&paths).unwrap();
        let refused = post_re_enroll_fresh(State(state.clone()), alice()).await.unwrap_err();
        assert_eq!(code_of(refused), (409, "not_renewed_elsewhere".into()));
        assert_eq!(state.cloud.control.identity().unwrap().sign_public(), old.sign_public(), "nothing changed");
        // Refused by the hub as renewed, without a chain to show for it: not
        // evidence, nothing to start again for.
        st.rotation.refused_ms = Some(6);
        st.save(&paths).unwrap();
        let refused = post_re_enroll_fresh(State(state.clone()), alice()).await.unwrap_err();
        assert_eq!(code_of(refused), (409, "not_renewed_elsewhere".into()));
        let shown = get_cloud(State(state.clone()), alice()).await.unwrap().0.link.unwrap().rotation;
        assert_eq!((shown.state.as_str(), shown.reason.as_deref()), ("refused", Some("rotated")));
        st.rotation.foreign_ms = Some(6);
        st.rotation.foreign_reason = Some(crate::agent::rotation::FOREIGN_RENEWED_ELSEWHERE.into());
        st.save(&paths).unwrap();
        let before = get_cloud(State(state.clone()), alice()).await.unwrap().0;
        let rotation = before.link.unwrap().rotation;
        assert_eq!((rotation.state.as_str(), rotation.reason.as_deref()), ("foreign", Some("renewed_elsewhere")));
        let fresh = post_re_enroll_fresh(State(state.clone()), alice()).await.unwrap().0;
        assert_eq!(fresh.state, "unenrolled");
        let new = state.cloud.control.identity().unwrap();
        assert_ne!(new.sign_public(), old.sign_public(), "a fresh seed");
        assert_eq!(fresh.fingerprint, machine_fingerprint(&new));
        let archived: Vec<_> = std::fs::read_dir(paths.instance_dir().join("agent"))
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter(|n| n.starts_with("machine-key.retired-"))
            .collect();
        assert_eq!(archived.len(), 1, "the retired seed is archived: {archived:?}");
        let after = get_cloud(State(state.clone()), alice()).await.unwrap().0;
        assert_eq!(after.state, "none", "back at entering a registration code");
        assert!(after.link.is_none());
    }

    fn created(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(u, c)| (u.to_string(), c.to_string())).collect()
    }

    /// Security review of S1-5, F8, and the user's decision of 2026-10-19:
    /// other devices' copies go only to the owner and the local accounts the
    /// owner allowed — never while there is no owner, it is gone, or
    /// `cloud.json` does not read; and what was said of a name holds for that
    /// account only (review of S1-7, round 3).
    #[test]
    fn only_the_owner_and_whom_it_allowed_see_other_devices() {
        let t = "2026-10-19T00:00:00.000Z";
        let accounts = created(&[("alice", t), ("bob", t), ("carol", t), ("dave", t)]);
        let file = CloudFile {
            owner: Some("alice".into()),
            remote_access: [("bob".to_string(), true), ("carol".to_string(), false)].into_iter().collect(),
            accounts_since: created(&[("alice", t), ("bob", t)]),
            ..CloudFile::default()
        };
        let sees = |file: Option<&CloudFile>, accounts: &BTreeMap<String, String>, auth: bool, user: Option<&str>| {
            sees_other_devices(file, accounts, auth, user)
        };
        assert!(sees(Some(&file), &accounts, true, Some("alice")), "the owner");
        assert!(sees(Some(&file), &accounts, true, Some("bob")), "let see them");
        assert!(!sees(Some(&file), &accounts, true, Some("carol")), "not let");
        assert!(!sees(Some(&file), &accounts, true, Some("dave")), "never asked about");
        assert!(!sees(Some(&file), &accounts, false, Some("alice")), "local authentication off");
        assert!(!sees(Some(&file), &accounts, true, None), "nobody signed in");
        let gone = created(&[("bob", t)]);
        assert!(
            !sees(Some(&file), &gone, true, Some("bob")),
            "the owner's account is gone: nor whom it allowed"
        );
        // Removed and made again under the same name: another account, which inherits nothing.
        let again = "2026-10-19T09:00:00.000Z";
        assert!(
            !sees(Some(&file), &created(&[("alice", again), ("bob", t)]), true, Some("alice")),
            "a new alice is not the owner"
        );
        assert!(
            !sees(Some(&file), &created(&[("alice", t), ("bob", again)]), true, Some("bob")),
            "a new bob is not allowed"
        );
        // An allowance from before accounts were bound: nobody's (the owner allows again).
        let unbound = CloudFile {
            accounts_since: created(&[("alice", t)]),
            ..file.clone()
        };
        assert!(!sees(Some(&unbound), &accounts, true, Some("bob")));
        // An owner from before accounts were bound: the account made before the sign-in only.
        let legacy = CloudFile {
            owner: Some("alice".into()),
            linked_at: crate::record::replay::parse_iso8601(t).map(|ms| ms as u64 + 1),
            ..CloudFile::default()
        };
        assert!(sees(Some(&legacy), &accounts, true, Some("alice")));
        assert!(!sees(Some(&legacy), &created(&[("alice", again)]), true, Some("alice")));
        let ownerless = CloudFile {
            remote_access: [("bob".to_string(), true)].into_iter().collect(),
            accounts_since: created(&[("bob", t)]),
            ..CloudFile::default()
        };
        assert!(!sees(Some(&ownerless), &accounts, true, Some("bob")), "no owner: nobody");
        assert!(!sees(None, &accounts, true, Some("alice")), "cloud.json does not read");
    }

    /// A caller as the guard hands it over: signed in, with its login's digest.
    async fn signed_in(state: &AppState, user: &str) -> MaybeIdentity {
        let login = state.drive.auth.sessions.issue(user, None).await.unwrap();
        MaybeIdentity(Some(crate::auth::AuthIdentity {
            username: user.into(),
            token_sha256: Some(crate::auth::sessions::digest_of(&login.token)),
        }))
    }

    /// Make `user` the owner, as a sign-in from the local settings does.
    async fn own(state: &AppState, user: &str) {
        let since = accounts(state).await.get(user).cloned().unwrap();
        state
            .cloud
            .update_file(|f| {
                f.owner = Some(user.into());
                f.accounts_since.insert(user.into(), since);
            })
            .unwrap();
    }

    /// Pretend the link is enrolled (a device completes it): something to claim.
    fn enrolled(state: &AppState) {
        crate::agent::state::AgentState::enrolling("wss://hub.example", "a-1", "m-1", [7; 16], 1)
            .save(&state.cloud.paths)
            .unwrap();
    }

    fn access(user: &str, remote: bool) -> ApiJson<AccessRequest> {
        ApiJson(AccessRequest { user: user.into(), remote })
    }

    /// S1-7: who sees the other devices here, by the one rule — and only
    /// whoever manages the link is told about every local account.
    #[tokio::test]
    async fn the_status_says_who_sees_the_other_devices_here() {
        let home = tempfile::tempdir().unwrap();
        let state = crate::api::test_support::state(home.path()).await;
        let none = status_for(&state, None).await;
        assert!(!none.sees_other_devices && none.accounts.is_empty(), "local authentication off: nobody");
        for user in ["alice", "bob", "carol"] {
            state.drive.auth.users.add(user, "correct horse battery staple").await.unwrap();
        }
        own(&state, "alice").await;
        let alice = signed_in(&state, "alice").await;
        let _ = put_access(State(state.clone()), alice.clone(), access("carol", true)).await.unwrap();
        let _ = put_access(State(state.clone()), alice.clone(), access("bob", false)).await.unwrap();
        let status = status_for(&state, Some("alice")).await;
        assert!(status.sees_other_devices);
        assert!(!status.can_claim, "it has an owner");
        let rows: Vec<(String, bool, bool)> = status.accounts.iter().map(|a| (a.user.clone(), a.remote, a.owner)).collect();
        assert_eq!(
            rows,
            vec![("alice".into(), true, true), ("bob".into(), false, false), ("carol".into(), true, false)]
        );
        let bob = status_for(&state, Some("bob")).await;
        assert!(!bob.sees_other_devices && bob.accounts.is_empty());
        assert!(status_for(&state, Some("carol")).await.sees_other_devices);
        // The gateway's own gate, the same rule.
        let carol = signed_in(&state, "carol").await;
        assert_eq!(remote_viewer(&state, &alice).await.unwrap(), "alice");
        assert_eq!(remote_viewer(&state, &carol).await.unwrap(), "carol");
        assert_eq!(
            code_of(remote_viewer(&state, &signed_in(&state, "bob").await).await.unwrap_err()),
            (403, "no_remote_access".into())
        );
        let bridged = MaybeIdentity(Some(crate::auth::AuthIdentity {
            username: "cloud:d-1".into(),
            token_sha256: None,
        }));
        assert_eq!(
            code_of(remote_viewer(&state, &bridged).await.unwrap_err()),
            (403, "not_from_cloud".into())
        );
        // A caller whose login cannot be named: refused (fail closed).
        let unnamed = MaybeIdentity(Some(crate::auth::AuthIdentity {
            username: "alice".into(),
            token_sha256: None,
        }));
        assert_eq!(code_of(remote_viewer(&state, &unnamed).await.unwrap_err()), (401, "unauthorized".into()));
        // A login signed out since its ticket was minted: refused.
        let MaybeIdentity(Some(identity)) = &carol else { unreachable!() };
        state.drive.auth.sessions.revoke(identity.token_sha256.as_deref().unwrap()).await;
        assert_eq!(code_of(remote_viewer(&state, &carol).await.unwrap_err()).0, 401);
        // Only the owner says who else sees them.
        let bob = signed_in(&state, "bob").await;
        assert_eq!(
            code_of(put_access(State(state.clone()), bob.clone(), access("bob", true)).await.unwrap_err()),
            (403, "not_owner".into())
        );
        // The owner's account gone: anybody may manage the link, but nobody sees.
        state.drive.auth.users.remove("alice").await.unwrap();
        assert_eq!(
            code_of(remote_viewer(&state, &signed_in(&state, "carol").await).await.unwrap_err()),
            (403, "no_remote_access".into())
        );
        assert!(!status_for(&state, Some("bob")).await.sees_other_devices);
        // A cloud.json that does not read: nobody either.
        std::fs::write(file_path(&state.cloud.paths), b"{\"owner\": tr").unwrap();
        assert!(state.cloud.read_file_strict().is_none());
        assert!(!status_for(&state, Some("carol")).await.sees_other_devices);
    }

    /// The user's decision of 2026-10-19: an ownerless link (enrolled from the
    /// command line) shows nobody the other devices until someone who manages
    /// it says "I added this device" — and what anybody allowed while nobody
    /// owned it does not come with the claim (review of S1-7, round 3).
    #[tokio::test]
    async fn an_ownerless_link_is_seen_by_nobody_until_it_is_claimed() {
        let home = tempfile::tempdir().unwrap();
        let state = crate::api::test_support::state(home.path()).await;
        for user in ["alice", "bob", "mallory"] {
            state.drive.auth.users.add(user, "correct horse battery staple").await.unwrap();
        }
        let alice = signed_in(&state, "alice").await;
        let bob = signed_in(&state, "bob").await;
        let mallory = signed_in(&state, "mallory").await;
        // No cloud.json at all (enrolled from the command line): nobody sees.
        assert_eq!(
            code_of(remote_viewer(&state, &alice).await.unwrap_err()),
            (403, "no_remote_access".into())
        );
        // Not signed in to Droppen: nothing to claim.
        assert_eq!(
            code_of(post_claim(State(state.clone()), alice.clone()).await.unwrap_err()),
            (409, "not_linked".into())
        );
        assert!(!status_for(&state, Some("alice")).await.can_claim);
        enrolled(&state);
        assert!(status_for(&state, Some("alice")).await.can_claim);
        assert!(may_manage(&state, &state.cloud.read_file(), "bob").await, "anybody manages it");
        // Anybody may manage it, but nobody may allow anybody while nobody owns it.
        assert_eq!(
            code_of(
                put_access(State(state.clone()), mallory.clone(), access("mallory", true))
                    .await
                    .unwrap_err()
            ),
            (403, "not_owner".into())
        );
        // An allowance written by hand meanwhile does not survive a claim either.
        let t = accounts(&state).await;
        state
            .cloud
            .update_file(|f| {
                f.remote_access.insert("mallory".into(), true);
                f.accounts_since.insert("mallory".into(), t["mallory"].clone());
            })
            .unwrap();
        let claimed = post_claim(State(state.clone()), bob.clone()).await.unwrap();
        assert!(claimed.sees_other_devices && !claimed.can_claim);
        assert_eq!(remote_viewer(&state, &bob).await.unwrap(), "bob", "claimed: bob alone");
        assert!(remote_viewer(&state, &mallory).await.is_err(), "nothing allowed before the claim holds");
        assert!(remote_viewer(&state, &alice).await.is_err());
        assert!(state.cloud.read_file().remote_access.is_empty());
        // A second claim: refused.
        assert_eq!(
            code_of(post_claim(State(state.clone()), alice.clone()).await.unwrap_err()).1,
            "not_owner",
            "only the owner manages it now"
        );
    }

    /// Review of S1-7, round 3: of two claims at once, one wins; the other
    /// hears `has_owner`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_claims_at_once_make_one_owner() {
        let home = tempfile::tempdir().unwrap();
        let state = crate::api::test_support::state(home.path()).await;
        let names: Vec<String> = (0..8).map(|n| format!("u{n}")).collect();
        for user in &names {
            state.drive.auth.users.add(user, "correct horse battery staple").await.unwrap();
        }
        enrolled(&state);
        let mut callers = Vec::new();
        for user in &names {
            callers.push(signed_in(&state, user).await);
        }
        let claims = callers.into_iter().map(|who| {
            let state = state.clone();
            tokio::spawn(async move { post_claim(State(state), who).await.map(|_| ()) })
        });
        let claims: Vec<_> = claims.collect();
        let mut won = 0;
        for claim in claims {
            match claim.await.unwrap() {
                Ok(()) => won += 1,
                // Lost under the file's lock (`has_owner`), or after the winner wrote (`not_owner`).
                Err(e) => {
                    let code = code_of(e);
                    assert!(code == (409, "has_owner".into()) || code == (403, "not_owner".into()), "{code:?}");
                }
            }
        }
        assert_eq!(won, 1, "one owner");
        let owner = state.cloud.read_file().owner.unwrap();
        assert!(names.contains(&owner));
    }

    /// Review of S1-7, round 3: an account removed and made again under the
    /// owner's or an allowed name inherits nothing.
    #[tokio::test]
    async fn an_account_made_again_under_the_same_name_inherits_nothing() {
        let home = tempfile::tempdir().unwrap();
        let state = crate::api::test_support::state(home.path()).await;
        for user in ["alice", "carol"] {
            state.drive.auth.users.add(user, "correct horse battery staple").await.unwrap();
        }
        own(&state, "alice").await;
        let alice = signed_in(&state, "alice").await;
        let _ = put_access(State(state.clone()), alice.clone(), access("carol", true)).await.unwrap();
        assert!(remote_viewer(&state, &signed_in(&state, "carol").await).await.is_ok());
        // carol goes, and a new carol comes (a ms later at least).
        state.drive.auth.users.remove("carol").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        state.drive.auth.users.add("carol", "another horse battery staple").await.unwrap();
        assert_eq!(
            code_of(remote_viewer(&state, &signed_in(&state, "carol").await).await.unwrap_err()),
            (403, "no_remote_access".into())
        );
        // So does alice: the new one is not the owner — nobody is, until a claim.
        state.drive.auth.users.remove("alice").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        state.drive.auth.users.add("alice", "another horse battery staple").await.unwrap();
        let again = signed_in(&state, "alice").await;
        assert!(remote_viewer(&state, &again).await.is_err());
        enrolled(&state);
        assert!(status_for(&state, Some("alice")).await.can_claim, "an ownerless link again");
    }

    /// A port nothing listens on: a hub that cannot be reached.
    fn dead_hub() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        format!("ws://127.0.0.1:{port}")
    }

    fn valid_code() -> String {
        rh_proto::identity::EnrollCode {
            account_fingerprint: [3; 16],
            secret: [4; 16],
        }
        .to_string()
    }

    /// The browser hand-off (§4.9) over HTTP, as the server serves it — the
    /// login guard outermost: the attempt carries a droppen.app address for a
    /// page that said its origin; droppen.app's sealed answer comes back
    /// without a login and opens only for its attempt and state, once, before
    /// the attempt expires; the page that waits sees how it ended.
    #[tokio::test]
    async fn droppen_app_s_answer_comes_back_through_the_browser_bound_to_its_attempt() {
        let home = tempfile::tempdir().unwrap();
        let (state, addr) = crate::api::test_support::served(home.path()).await;
        // What a link needs, as the owning server attaches it.
        state.cloud.control.attach(crate::agent::link::LinkEnv {
            addr,
            credential: "test".into(),
            drive: state.drive.clone(),
            cancel: tokio_util::sync::CancellationToken::new(),
        });
        let base = format!("http://{addr}");
        let client = reqwest::Client::new();
        let hub = dead_hub();
        let link = |token: Option<String>, origin: Option<String>| {
            let mut request = client
                .post(format!("{base}/api/cloud/link"))
                .json(&serde_json::json!({"hub": hub.clone()}));
            if let Some(token) = token {
                request = request.bearer_auth(token);
            }
            if let Some(origin) = origin {
                request = request.header("Origin", origin);
            }
            request.send()
        };
        let give_back = |attempt: String, sealed: String| {
            client
                .post(format!("{base}/api/cloud/link/return"))
                .header("Origin", base.clone())
                .json(&serde_json::json!({"attempt": attempt, "sealed": sealed}))
                .send()
        };
        let error_of = |body: serde_json::Value| body["error"].as_str().unwrap_or_default().to_string();

        // No account yet: the device is in its setup state, which is no way
        // round the attempt — an answer for none finds nothing.
        let res = give_back("a".repeat(32), "AAAA".into()).await.unwrap();
        assert_eq!(res.status(), 404);

        state.drive.auth.users.add("alice", "correct horse battery staple").await.unwrap();
        let token = state.drive.auth.sessions.issue("alice", None).await.unwrap().token;

        // Without an Origin there is no page to come back to.
        let res = link(Some(token.clone()), None).await.unwrap();
        assert_eq!(res.status(), 200);
        let body: serde_json::Value = res.json().await.unwrap();
        assert!(body["handoffUrl"].is_null(), "{body}");
        let first = body["attempt"].as_str().unwrap().to_string();

        // From the page: the droppen.app address, with everything in its fragment.
        let res = link(Some(token.clone()), Some(base.clone())).await.unwrap();
        assert_eq!(res.status(), 200);
        let body: serde_json::Value = res.json().await.unwrap();
        let url = body["handoffUrl"].as_str().unwrap().to_string();
        let attempt = body["attempt"].as_str().unwrap().to_string();
        assert_ne!(attempt, first, "a new attempt replaces the first");
        assert!(url.starts_with(&format!("{}/link#droppen-link=", web_origin(&hub))), "{url}");
        assert!(!url.contains('?'));
        let status = status_for(&state, Some("alice")).await;
        assert_eq!(
            status.attempt.as_ref().and_then(|a| a.handoff_url.clone()).as_deref(),
            Some(url.as_str()),
            "the opener sees it"
        );
        let bob = status_for(&state, Some("bob")).await;
        assert!(bob.attempt.unwrap().handoff_url.is_none(), "nobody else does");

        let reply = serde_json::json!({"code": valid_code(), "email": "me@example.com"});
        // The first attempt is gone: its answer finds nothing.
        let (_, sealed) = crate::api::link_handoff::tests::answer(&url, &reply, None);
        let res = give_back(first, sealed.clone()).await.unwrap();
        assert_eq!(res.status(), 404);
        // A page that never saw the address cannot answer for it (login CSRF), and uses nothing up.
        let (_, forged) = crate::api::link_handoff::tests::answer(&url, &reply, Some([0; 16]));
        let res = give_back(attempt.clone(), forged).await.unwrap();
        assert_eq!(res.status(), 403);
        assert_eq!(error_of(res.json().await.unwrap()), "bad_reply");
        let res = give_back(attempt.clone(), "not base64!".into()).await.unwrap();
        assert_eq!(res.status(), 400);
        // A device bridged from Droppen never answers, even with the right reply.
        let bridged = post_return(
            State(state.clone()),
            MaybeIdentity(Some(crate::auth::AuthIdentity {
                username: "cloud:d-1".into(),
                token_sha256: None,
            })),
            ApiJson(ReturnRequest {
                attempt: attempt.clone(),
                sealed: sealed.clone(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(code_of(bridged), (403, "not_from_cloud".into()));
        assert!(state.cloud.live_attempt().is_some(), "still there");

        // The real answer, without a login: taken, and the attempt is used up
        // (the hub is unreachable here, so the enrollment itself fails).
        let res = give_back(attempt.clone(), sealed.clone()).await.unwrap();
        assert_eq!(res.status(), 502);
        assert_eq!(error_of(res.json().await.unwrap()), "hub_unreachable");
        assert!(state.cloud.live_attempt().is_none(), "used up");
        let res = give_back(attempt.clone(), sealed).await.unwrap();
        assert_eq!(res.status(), 404, "once only");
        // The page that waits sees how it ended; nobody else does.
        let seen = status_for(&state, Some("alice")).await.handoff.expect("an outcome");
        assert_eq!((seen.attempt.as_str(), seen.outcome.as_str()), (attempt.as_str(), "failed"));
        assert_eq!(seen.error.as_deref(), Some("hub_unreachable"));
        assert!(status_for(&state, Some("bob")).await.handoff.is_none());
        assert!(status_for(&state, None).await.handoff.is_none());
    }

    /// A server that owns its instance and listens, as `return` needs it (the
    /// link can start), with the local account `alice` — and, with `enroll`,
    /// already in an account ([`linked`]): the state, and alice's attempt
    /// opened from the server's own page towards `hub` (`None`: the default,
    /// or the hub an enrolled device stays with).
    async fn opened_from_a_page(home: &std::path::Path, hub: Option<String>, enroll: bool) -> (AppState, LinkAttempt) {
        opened_from_a_page_with(home, hub, enroll, false).await
    }

    /// [`opened_from_a_page`], asking the hub where its app is when
    /// `discover` ([`super::super::hub_web`]).
    async fn opened_from_a_page_with(home: &std::path::Path, hub: Option<String>, enroll: bool, discover: bool) -> (AppState, LinkAttempt) {
        let (state, addr) = crate::api::test_support::served(home).await;
        state.cloud.hub_webs.set_discover(discover);
        state.cloud.control.attach(crate::agent::link::LinkEnv {
            addr,
            credential: "test".into(),
            drive: state.drive.clone(),
            cancel: tokio_util::sync::CancellationToken::new(),
        });
        state.drive.auth.users.add("alice", "correct horse battery staple").await.unwrap();
        if enroll {
            linked(&state);
        }
        let alice = signed_in(&state, "alice").await;
        let Json(opened) = post_link(
            State(state.clone()),
            alice,
            PageOrigin(Some(format!("http://{addr}"))),
            ApiJson(LinkRequest { hub, aggregate: None }),
        )
        .await
        .unwrap();
        (state, opened)
    }

    /// Pretend this device is in an account and runs AI in it (`linked`): a
    /// verified chain that adds it, saved as the link would.
    fn linked(state: &AppState) {
        use rh_proto::chain::{genesis_draft, Action, ChainClock, ChainState, DeviceInfo, Signer};
        use rh_proto::identity::{fingerprint, MachineIdentity};
        let ak = MachineIdentity::from_seed(&[1; 32]);
        let device = MachineIdentity::from_seed(&[2; 32]);
        let machine = MachineIdentity::from_seed(&[3; 32]);
        let info = DeviceInfo {
            id: "d-1".into(),
            sign_key: device.sign_public(),
            dh_key: device.dh_public(),
            full: true,
        };
        let genesis = genesis_draft("a-1", ak.sign_public(), info, None, 1_000).sign(ak.signing_key());
        let mut chain = ChainState::verify(std::slice::from_ref(&genesis), None, &mut ChainClock::history()).unwrap();
        let add = chain
            .draft(Signer::Device("d-1".into()), Action::AddMachine(machine.info("m-1")), 2_000)
            .sign(device.signing_key());
        chain.apply(&add, &mut ChainClock::history()).unwrap();
        let mut st = crate::agent::state::AgentState::enrolling("wss://hub.example", "a-1", "m-1", fingerprint(&ak.sign_public()), 5_000);
        assert_eq!(st.absorb(&[genesis, add], 5_000).error, None);
        st.save(&state.cloud.paths).unwrap();
        assert_eq!(state.cloud.control.status().state, Standing::Linked);
    }

    /// The sign-in opens where the hub says its app is
    /// (`/.well-known/droppen-hub`, [`super::super::hub_web`]): the popup's
    /// origin, the hand-off address and the status the page opens the popup
    /// from — while the agent keeps signing in to the hub itself.
    #[tokio::test]
    async fn the_sign_in_opens_the_app_origin_the_hub_names() {
        use crate::api::hub_web::tests::{hub_answering, port};
        let (hub, asked) = hub_answering(|hub| {
            let doc = serde_json::json!({"origin": hub, "app": format!("http://my.localhost:{}", port(hub))});
            (200, None, doc.to_string())
        })
        .await;
        let app = format!("http://my.localhost:{}", port(&hub));
        let home = tempfile::tempdir().unwrap();
        let (state, opened) = opened_from_a_page_with(home.path(), Some(hub.clone()), false, true).await;
        assert_eq!(opened.hub_web, app);
        let url = opened.handoff_url.clone().expect("opened from a page");
        assert!(url.starts_with(&format!("{app}/link#droppen-link=")), "{url}");
        let status = status_for(&state, Some("alice")).await;
        assert_eq!((status.hub.as_str(), status.hub_web.as_str()), (hub.as_str(), app.as_str()), "the agent stays on the hub");
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
        // The answer through the browser is bound to the page's origin, not
        // to the host droppen.app's page was on: it opens as before.
        let reply = serde_json::json!({"code": valid_code()});
        let (attempt, sealed) = crate::api::link_handoff::tests::answer(&url, &reply, None);
        let held = state.cloud.live_attempt().expect("the attempt");
        assert_eq!(attempt, held.id);
        assert!(held.handoff.as_ref().unwrap().open(&attempt, &link_handoff::sealed_bytes(&sealed).unwrap()).is_ok());
    }

    /// Review of the hand-off: between taking the attempt and the hub's
    /// answer the attempt is gone, and the page that waits read that as "ran
    /// out". Through `return` itself, while the hub keeps the enrollment
    /// waiting: every look finds the attempt or that its answer was taken
    /// (`receiving`), never neither — and only the page of the one who
    /// opened it sees the answer.
    #[tokio::test]
    async fn while_it_enrolls_the_page_that_waits_sees_the_answer_was_taken() {
        let home = tempfile::tempdir().unwrap();
        // A hub that takes the connection and never answers: the enrollment waits.
        let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let hub = format!("ws://{}", silent.local_addr().unwrap());
        let (state, opened) = opened_from_a_page(home.path(), Some(hub), false).await;
        let url = opened.handoff_url.clone().expect("opened from a page");
        let reply = serde_json::json!({"code": valid_code()});
        let (_, sealed) = crate::api::link_handoff::tests::answer(&url, &reply, None);
        let before = status_for(&state, Some("alice")).await;
        assert!(before.attempt.is_some() && before.handoff.is_none());

        let returning = tokio::spawn(post_return(
            State(state.clone()),
            MaybeIdentity(None),
            ApiJson(ReturnRequest {
                attempt: opened.attempt.clone(),
                sealed,
            }),
        ));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let seen = status_for(&state, Some("alice")).await;
            match (seen.attempt.is_some(), seen.handoff) {
                (true, None) => {}
                (false, Some(handoff)) => {
                    assert_eq!(
                        (handoff.attempt.as_str(), handoff.outcome.as_str()),
                        (opened.attempt.as_str(), "receiving")
                    );
                    break;
                }
                (attempt, handoff) => panic!("attempt {attempt}, an answer {}", handoff.is_some()),
            }
            assert!(std::time::Instant::now() < deadline, "the answer was never taken");
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(!returning.is_finished(), "still enrolling: the hub has not answered");
        assert!(status_for(&state, Some("bob")).await.handoff.is_none());
        assert!(status_for(&state, None).await.handoff.is_none());
        returning.abort();
        drop(silent);
    }

    /// Review of the hand-off: whether this device is in an account already
    /// is said only to a reply that opens — not to anybody who reaches the
    /// port — and saying it uses the attempt up.
    #[tokio::test]
    async fn only_a_reply_that_opens_learns_the_device_is_in_an_account() {
        let home = tempfile::tempdir().unwrap();
        let (state, opened) = opened_from_a_page(home.path(), None, true).await;
        let url = opened.handoff_url.clone().expect("opened from a page");
        let reply = serde_json::json!({"code": valid_code()});
        let give_back = |sealed: String| {
            post_return(
                State(state.clone()),
                MaybeIdentity(None),
                ApiJson(ReturnRequest {
                    attempt: opened.attempt.clone(),
                    sealed,
                }),
            )
        };

        let (_, forged) = crate::api::link_handoff::tests::answer(&url, &reply, Some([0; 16]));
        assert_eq!(code_of(give_back(forged).await.unwrap_err()), (403, "bad_reply".into()));
        assert!(state.cloud.live_attempt().is_some(), "a refusal leaves the attempt be");

        let (_, sealed) = crate::api::link_handoff::tests::answer(&url, &reply, None);
        assert_eq!(code_of(give_back(sealed).await.unwrap_err()), (409, "already_enrolled".into()));
        assert!(state.cloud.live_attempt().is_none(), "used up");
        let seen = status_for(&state, Some("alice")).await.handoff.expect("an outcome");
        assert_eq!(
            (seen.outcome.as_str(), seen.error.as_deref()),
            ("failed", Some("already_enrolled"))
        );
    }

    /// An attempt's browser answer after it expired, or for an attempt that
    /// takes none, is refused — and a refusal leaves the attempt be.
    #[test]
    fn an_answer_through_the_browser_is_refused_once_the_attempt_expired() {
        let machine = crate::api::link_handoff::HandoffMachine {
            hostname: "laptop".into(),
            sign_key: [1; 32],
            dh_key: [2; 32],
            enrolled: None,
        };
        let handoff = Handoff::new("https://droppen.app", "a1", 1_000, "http://127.0.0.1:8700", &machine);
        let reply = serde_json::json!({"code": valid_code()});
        let (_, sealed) = crate::api::link_handoff::tests::answer(handoff.url(), &reply, None);
        let sealed = link_handoff::sealed_bytes(&sealed).unwrap();
        let with = |handoff: Option<Handoff>| attempt("alice", 1_000).map(|a| Attempt { handoff, ..a });
        let mut slot = with(Some(handoff.clone()));
        assert_eq!(
            code_of(take_handed_back(&mut slot, "a1", &sealed, 1_000).unwrap_err()),
            (410, "expired".into())
        );
        assert_eq!(
            code_of(take_handed_back(&mut slot, "a2", &sealed, 999).unwrap_err()),
            (404, "no_attempt".into())
        );
        let mut none = with(None);
        assert_eq!(
            code_of(take_handed_back(&mut none, "a1", &sealed, 999).unwrap_err()),
            (409, "no_handoff".into())
        );
        let (taken, reply) = take_handed_back(&mut slot, "a1", &sealed, 999).unwrap();
        assert_eq!((taken.owner.as_str(), reply.code), ("alice", valid_code()));
        assert!(slot.is_none());
    }

    #[test]
    fn hubs_resolve_to_an_agent_origin_and_a_web_origin() {
        assert_eq!(DEFAULT_HUB, "https://api.droppen.app");
        assert_eq!(resolve_hub(Some("https://api.droppen.app")).unwrap(), "wss://api.droppen.app");
        assert_eq!(web_origin("wss://api.droppen.app"), "https://api.droppen.app");
        // The apex is a temporary machine alias during the compatibility window.
        assert_eq!(resolve_hub(Some("https://droppen.app")).unwrap(), "wss://droppen.app");
        assert_eq!(web_origin("wss://droppen.app"), "https://droppen.app");
        assert_eq!(resolve_hub(Some("http://127.0.0.1:9000/")).unwrap(), "ws://127.0.0.1:9000");
        assert_eq!(web_origin("ws://127.0.0.1:9000"), "http://127.0.0.1:9000");
        assert_eq!(code_of(resolve_hub(Some("http://evil.example")).unwrap_err()).1, "bad_hub");
    }

    /// The device's owner by the local capability (`remote-hand-server open`)
    /// while it has no user account: it manages the link and sees the other
    /// devices, whatever `cloud.json` names — and stops the moment the first
    /// account exists, to which a link it signed in passes, allowances cleared.
    #[tokio::test]
    async fn the_local_owner_owns_the_link_until_the_first_account_takes_it() {
        let home = tempfile::tempdir().unwrap();
        let state = crate::api::test_support::state(home.path()).await;
        state
            .drive
            .auth
            .users
            .add_app("droppen-app", "correct horse battery staple")
            .await
            .unwrap();
        let owner = signed_in(&state, crate::auth::LOCAL_OWNER).await;
        enrolled(&state);
        // Signed in by the local owner: what `post_finish` records.
        state
            .cloud
            .update_file(|f| {
                f.owner = Some(crate::auth::LOCAL_OWNER.into());
                f.linked_at = Some(1);
                f.remote_access.insert(crate::auth::LOCAL_OWNER.into(), true);
                f.remote_access.insert("droppen-app".into(), true);
            })
            .unwrap();
        assert_eq!(remote_viewer(&state, &owner).await.unwrap(), crate::auth::LOCAL_OWNER);
        let status = status_for(&state, Some(crate::auth::LOCAL_OWNER)).await;
        assert!(
            status.can_manage && status.sees_other_devices && !status.can_claim,
            "the owner, with nothing to claim"
        );
        let app = signed_in(&state, "droppen-app").await;
        assert!(remote_viewer(&state, &app).await.is_err(), "the app's own account is no viewer by itself");
        assert!(!may_manage(&state, &state.cloud.read_file(), "droppen-app").await);

        // The first account: the link is ada's, bound to her account, and the
        // owner's sessions are over.
        state.drive.auth.users.add("ada", "correct horse battery staple").await.unwrap();
        state.drive.auth.sessions.revoke_user(crate::auth::LOCAL_OWNER, None).await;
        state.cloud.hand_over_from_local_owner(&state.drive.auth.users).await;
        let file = state.cloud.read_file();
        assert_eq!(file.owner.as_deref(), Some("ada"));
        assert_eq!(file.remote_access.keys().collect::<Vec<_>>(), ["ada"], "allowances cleared");
        assert_eq!(file.accounts_since.get("ada"), accounts(&state).await.get("ada"));
        let ada = signed_in(&state, "ada").await;
        assert_eq!(remote_viewer(&state, &ada).await.unwrap(), "ada");
        assert!(remote_viewer(&state, &owner).await.is_err(), "the owner's session is gone");
        let fresh = signed_in(&state, crate::auth::LOCAL_OWNER).await;
        assert!(
            remote_viewer(&state, &fresh).await.is_err(),
            "and no owner session counts with an account"
        );
        assert!(!may_manage(&state, &state.cloud.read_file(), crate::auth::LOCAL_OWNER).await);
        assert!(!status_for(&state, Some(crate::auth::LOCAL_OWNER)).await.sees_other_devices);
    }

    /// The desktop app signs this device in to Droppen from its own window,
    /// whose login is the app's app-internal account: on the app's server that
    /// account may manage a link nobody owns — it is the app's, for the
    /// operating-system user who runs it — but not one somebody else owns.
    /// Anywhere else an app-internal account is never the "anybody" of an
    /// ownerless link.
    #[tokio::test]
    async fn the_desktop_apps_own_account_may_sign_the_device_in_on_the_apps_server() {
        let home = tempfile::tempdir().unwrap();
        let mut state = crate::api::test_support::state(home.path()).await;
        state
            .drive
            .auth
            .users
            .add_app("droppen-app", "correct horse battery staple")
            .await
            .unwrap();
        assert!(
            !may_manage(&state, &state.cloud.read_file(), "droppen-app").await,
            "not on a server the app does not run"
        );
        state.desktop_app = true;
        assert!(may_manage(&state, &state.cloud.read_file(), "droppen-app").await, "on the app's own server");

        // A link a person's account owns stays theirs to manage.
        state.drive.auth.users.add("ada", "correct horse battery staple").await.unwrap();
        let since = accounts(&state).await.get("ada").cloned().unwrap();
        state
            .cloud
            .update_file(|f| {
                f.owner = Some("ada".into());
                f.linked_at = Some(1);
                f.accounts_since.insert("ada".into(), since);
            })
            .unwrap();
        assert!(!may_manage(&state, &state.cloud.read_file(), "droppen-app").await);
        assert!(may_manage(&state, &state.cloud.read_file(), "ada").await);
    }

    /// A sign-in the local owner began before the first account was made,
    /// and whose answer came back after: the owner is nobody's manager any
    /// more, so it cannot finish — an ownerless link included.
    #[tokio::test]
    async fn the_local_owner_manages_nothing_once_the_device_has_an_account() {
        let home = tempfile::tempdir().unwrap();
        let state = crate::api::test_support::state(home.path()).await;
        let owner = crate::auth::LOCAL_OWNER;
        assert!(may_manage(&state, &state.cloud.read_file(), owner).await, "the owner, while there is no account");
        state.drive.auth.users.add("ada", "correct horse battery staple").await.unwrap();
        let file = state.cloud.read_file();
        assert!(file.owner.is_none(), "nobody owns the link");
        assert!(!may_manage(&state, &file, owner).await, "not the owner any more");
        assert!(may_manage(&state, &file, "ada").await, "an account manages an ownerless link");
        // What `post_return` asks of the attempt's opener.
        let attempt = attempt(owner, now_ms() + 60_000).unwrap();
        assert!(!may_manage(&state, &file, &attempt.owner).await);
    }

    /// The hand-over moves only a link the local owner signed in: an owner
    /// that is an account — the desktop app's included — keeps it.
    #[test]
    fn only_a_link_the_local_owner_signed_in_is_handed_over() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(home.path().to_path_buf(), "handover");
        std::fs::create_dir_all(paths.instance_dir()).unwrap();
        let t = "2026-10-19T00:00:00.000Z".to_string();
        let accounts = vec![("droppen-app".to_string(), t.clone(), true), ("ada".to_string(), t.clone(), false)];
        assert_eq!(hand_over_from_local_owner(&paths, &accounts), Ok(None), "no cloud.json");
        let write = |owner: &str| {
            let file = CloudFile {
                owner: Some(owner.into()),
                ..CloudFile::default()
            };
            std::fs::write(file_path(&paths), serde_json::to_vec(&file).unwrap()).unwrap();
        };
        write("droppen-app");
        assert_eq!(
            hand_over_from_local_owner(&paths, &accounts),
            Ok(None),
            "the app linked it: the app keeps it"
        );
        write(crate::auth::LOCAL_OWNER);
        assert_eq!(
            hand_over_from_local_owner(&paths, &accounts[..1]),
            Ok(None),
            "no user account yet: nobody to hand it to"
        );
        assert_eq!(hand_over_from_local_owner(&paths, &accounts), Ok(Some("ada".into())));
        let file: CloudFile = serde_json::from_slice(&std::fs::read(file_path(&paths)).unwrap()).unwrap();
        assert_eq!(file.owner.as_deref(), Some("ada"));
        assert_eq!(file.accounts_since.get("ada"), Some(&t));
    }
}
