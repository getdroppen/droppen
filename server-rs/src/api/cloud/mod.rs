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

use std::sync::{Arc, Mutex};

use gotcha::axum::http::StatusCode;
use gotcha::{api, Json, Schematic, State};
use serde::{Deserialize, Serialize};

use super::error::ApiError;
use super::extract::ApiJson;
use super::hub_web::HubWebs;
use super::{ApiRouter, AppState};
use crate::agent::control::{machine_fingerprint, AgentStatus, Control, ControlError, Standing};
use crate::aggregator::controller::AggregatorHost;
use crate::auth::MaybeIdentity;
use crate::openapi_ext::ExtraResponses;
use crate::sessions::Paths;

mod access;
mod file;
mod link;

pub use access::remote_viewer;
pub use file::{file_path, hand_over_from_local_owner};
pub use link::{post_cancel, post_finish, post_link, post_return, web_origin, ATTEMPT_TTL_MS, DEFAULT_HUB};

use access::*;
use link::*;

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

}

impl Cloud {
    /// The whole aggregator directory goes — other devices' machine keys and
    /// copies with the legacy key (local-aggregator §4.5, §4.7): this
    /// computer leaves its account, or turns aggregation off.
    fn forget_aggregator(&self) -> Result<(), ApiError> {
        // What the gateway shows of them goes at once, with the files.
        self.control.live().clear();
        crate::aggregator::forget_all(&self.paths).map_err(ApiError::internal)
    }
}

/// `{error: code, message}` at `status`.
fn refusal(status: StatusCode, code: &str, message: &str) -> ApiError {
    ApiError::with_body(status, serde_json::json!({"error": code, "message": message}))
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

/// The status for `user` (`None`: nobody signed in, local authentication off).
async fn status_for(state: &AppState, user: Option<&str>) -> CloudStatus {
    let cloud = &state.cloud;
    let agent: AgentStatus = cloud.control.status();
    // Read once: the lenient view (for what the status shows of the owner)
    // and the strict one (for who may see other devices) are the same read.
    let strict = cloud.read_file_strict();
    let file = strict.clone().unwrap_or_default();
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
    let fingerprint = cloud.paths.machine_key_path()
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
                enabled: cloud.paths.wants_aggregator(),
                state: if crate::aggregator::key_path(&cloud.paths).exists() {
                    "pending".into()
                } else if control.is_some() {
                    "on".into()
                } else {
                    "off".into()
                },
                signed_in: control.as_ref().is_some_and(|c| c.signed_in),
                error: control.as_ref().and_then(|c| c.error.clone()),
                hosts: {
                    let mut hosts = control.as_ref().map(|c| c.hosts.clone()).unwrap_or_default();
                    if !sees_other_devices {
                        for host in &mut hosts {
                            host.copy = None;
                        }
                    }
                    hosts
                },
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
    use std::collections::BTreeMap;

    use crate::api::link_handoff::{self, Handoff, PageOrigin};
    use super::file::CloudFile;

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
    /// off deleted only the legacy pending key; other devices' machine keys
    /// and copies stayed. The whole directory goes now.
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
    }

    /// A pre-chain-v3 pending key is deleted at boot, whatever the link state,
    /// so an install that carries the leftover does not report `pending` (and
    /// hide the other devices' copies) for ever.
    #[tokio::test]
    async fn a_pre_chain_v3_pending_key_is_not_pending_after_the_next_boot() {
        let home = tempfile::tempdir().unwrap();
        let state = crate::api::test_support::state(home.path()).await;
        let key = crate::aggregator::key_path(&state.cloud.paths);
        std::fs::create_dir_all(key.parent().unwrap()).unwrap();
        std::fs::write(&key, b"{}").unwrap();
        assert_eq!(status_for(&state, None).await.aggregator.state, "pending");

        // The next boot builds the state from the same home again.
        let rebooted = crate::api::test_support::state(home.path()).await;
        assert!(!key.exists(), "the leftover key is deleted at startup");
        assert_ne!(status_for(&rebooted, None).await.aggregator.state, "pending");
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
        let attempt = attempt(owner, crate::agent::now_ms() + 60_000).unwrap();
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
