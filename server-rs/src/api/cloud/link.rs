//! Signing in to Droppen from the local settings (local-aggregator §3.5): the
//! attempt in flight, the popup's code, and the browser hand-off (§4.9).

use rand::RngCore;

use crate::agent::enroll::BeginError;
use crate::api::link_handoff::{self, Handoff, HandoffMachine, PageOrigin, Reply};
use crate::api::SendResult;

use crate::api::created::Accepted;
use super::*;

/// How long an attempt waits for its code.
pub const ATTEMPT_TTL_MS: u64 = 15 * 60 * 1000;
/// The hub a new sign-in uses unless the request (or `DROPPEN_HUB_DEFAULT`) says otherwise.
pub const DEFAULT_HUB: &str = "https://api.droppen.app";

/// A sign-in in flight.
#[derive(Debug, Clone)]
pub(super) struct Attempt {
    pub(super) id: String,
    pub(super) nonce: String,
    /// The local user who opened it.
    pub(super) owner: String,
    /// The hub, as the agent signs in to it (`wss://…`), and where a browser
    /// signs in to it ([`crate::api::hub_web`]: its app origin, or its own as `https://`).
    pub(super) hub: String,
    pub(super) hub_web: String,
    pub(super) aggregate: bool,
    pub(super) expires_ms: u64,
    /// The browser hand-off's one-off key and state (§4.9): set when a page
    /// opened the attempt (it said its origin), so droppen.app can answer
    /// through the browser as well as through the popup. Either answer uses
    /// the attempt up.
    pub(super) handoff: Option<Handoff>,
}

/// How a browser hand-off ended (`return`): the page that opened the attempt
/// polls `GET /api/cloud` for it, as no popup tells it.
#[derive(Debug, Clone)]
pub(super) struct HandoffOutcome {
    pub(super) attempt: String,
    pub(super) owner: String,
    pub(super) at_ms: u64,
    pub(super) result: HandoffResult,
}

/// Where a browser hand-off stands.
#[derive(Debug, Clone)]
pub(super) enum HandoffResult {
    /// Taken, and enrolling: the attempt is gone already, so the page that
    /// waits must not read its absence as "ran out".
    Receiving,
    /// The hub took the enrollment: the machine id it assigned.
    HandedOver(String),
    /// `(error code, message)`.
    Failed(String, String),
}

pub(super) fn random_hex(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    crate::agent::hub::hex(&bytes)
}

impl Cloud {
    /// The attempt still in flight, if any (an expired one is dropped).
    pub(super) fn live_attempt(&self) -> Option<Attempt> {
        let mut slot = self.attempt.lock().expect("attempt");
        if slot.as_ref().is_some_and(|a| a.expires_ms <= crate::agent::now_ms()) {
            *slot = None;
            return None;
        }
        slot.clone()
    }

    /// Remember where a browser hand-off stands, for the page that waits on it.
    pub(super) fn record_handoff(&self, attempt: &Attempt, result: HandoffResult) {
        *self.handoff_outcome.lock().expect("handoff outcome") = Some(HandoffOutcome {
            attempt: attempt.id.clone(),
            owner: attempt.owner.clone(),
            at_ms: crate::agent::now_ms(),
            result,
        });
    }

    /// How `user`'s last browser hand-off stands, while an attempt could
    /// still be waiting on it.
    pub(super) fn handoff_outcome_for(&self, user: Option<&str>) -> Option<CloudHandoff> {
        let user = user?;
        let slot = self.handoff_outcome.lock().expect("handoff outcome");
        let outcome = slot
            .as_ref()
            .filter(|o| o.owner == user && crate::agent::now_ms().saturating_sub(o.at_ms) < ATTEMPT_TTL_MS)?;
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
}

/// The hub a sign-in uses: the request's, else `DROPPEN_HUB_DEFAULT` (or its
/// old name, `RH_HUB_DEFAULT`), else
/// api.droppen.app; as the agent signs in to it, `wss://…`. Where a browser
/// signs in to it is the hub's to say ([`crate::api::hub_web`]).
pub(super) fn resolve_hub(requested: Option<&str>) -> Result<String, ApiError> {
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
/// its app's ([`crate::api::hub_web`]). The one implementation is
/// [`crate::aggregator::hub::HubOriginExt::http_base`].
pub fn web_origin(ws_origin: &str) -> String {
    use crate::aggregator::hub::HubOriginExt;
    ws_origin.http_base()
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
                account_fp: crate::agent::hub::hex(&st.root.account_fingerprint),
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
    // document names one (my.droppen.app), else its own (`crate::api::hub_web`).
    let hub_web = cloud.hub_webs.for_sign_in(&hub).await;
    let identity = cloud.control.identity().map_err(ApiError::internal)?;
    let aggregate = body.aggregate.unwrap_or(true);
    // Chain v3 (local-aggregator §3.3): this computer controls other devices
    // with its one identity, the one it runs AI with — no second key pair.
    // `aggregator` stays in the answer (always null) for pages from before.
    let aggregator: Option<LinkAggregator> = None;
    let id = random_hex(16);
    let expires_ms = crate::agent::now_ms() + ATTEMPT_TTL_MS;
    let hostname = crate::api::hostname();
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
pub(super) fn check_attempt(slot: &Option<Attempt>, id: &str, nonce: Option<&str>, user: &str, now: u64) -> Result<Attempt, ApiError> {
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
pub(super) fn live_attempt_by_id<'a>(slot: &'a Option<Attempt>, id: &str, now: u64) -> Result<&'a Attempt, ApiError> {
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
pub(super) fn take_handed_back(slot: &mut Option<Attempt>, id: &str, sealed: &[u8], now: u64) -> Result<(Attempt, Reply), ApiError> {
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
pub(super) fn take_attempt(slot: &mut Option<Attempt>, id: &str, nonce: &str, user: &str, now: u64) -> Result<Attempt, ApiError> {
    check_attempt(slot, id, Some(nonce), user, now)?;
    Ok(slot.take().expect("checked"))
}

/// The popup's code (`RH1-…`, or `DR1-…` from a newer popup), or `400 bad_code`.
pub(super) fn finish_code(text: &str) -> Result<rh_proto::identity::EnrollCode, ApiError> {
    text.parse().map_err(|e: &str| refusal(StatusCode::BAD_REQUEST, "bad_code", e))
}

/// 409 `already_enrolled` while this computer is in an account.
pub(super) fn refuse_if_enrolled(state: &AppState) -> Result<(), ApiError> {
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
        take_attempt(&mut slot, &body.attempt, &body.nonce, &user, crate::agent::now_ms())?
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
        let taken = take_handed_back(&mut slot, &body.attempt, &sealed, crate::agent::now_ms())?;
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
pub(super) async fn complete_sign_in(
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
            // The link is as it was (`Control::enroll` restores it).
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
        file.linked_at = Some(crate::agent::now_ms());
        file.email = email;
        file.remote_access.insert(user.clone(), true);
    })?;
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
        match check_attempt(&slot, &body.attempt, None, &user, crate::agent::now_ms()) {
            Ok(_) => *slot = None,
            // An attempt that expired meanwhile is gone either way.
            Err(e) if e.status() == StatusCode::GONE => *slot = None,
            Err(e) => return Err(e),
        }
    }
    Ok(Json(SendResult { accepted: true }))
}
