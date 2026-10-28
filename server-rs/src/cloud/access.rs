//! Who may see and manage this device's Droppen link (local-aggregator §4.6;
//! the user's decision of 2026-10-19): the one access policy, apart from the
//! routes that ask it.

use std::collections::BTreeMap;

use super::file::CloudFile;
use super::*;

/// Who is asking, by the rules in the module docs: a bridged device is
/// refused outright; `None` when nobody is signed in (local authentication off).
pub(super) fn caller(who: &MaybeIdentity) -> Result<Option<String>, ApiError> {
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
pub(super) async fn local_user(state: &AppState, who: &MaybeIdentity) -> Result<String, ApiError> {
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
pub(super) fn local_auth_off() -> ApiError {
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
pub(super) async fn local_owner_now(state: &AppState, user: &str) -> bool {
    let auth = &state.drive.auth;
    crate::auth::is_local_owner(user) && !auth.is_off() && !auth.users.has_users().await
}

/// Every local account's `createdAt`, by name.
pub(super) async fn accounts(state: &AppState) -> BTreeMap<String, String> {
    state.drive.auth.users.list().await.into_iter().map(|u| (u.name, u.created_at)).collect()
}

/// Whether what `file` says of `user` — the owner, or allowed — is about the
/// local account of that name now: the one it was said of, not one removed
/// and made again under the same name since (`accounts_since`). An owner
/// from before accounts were bound (signed in by an earlier release) holds
/// for the account made before that sign-in only; an allowance from then
/// holds for nobody (the owner allows again).
pub(super) fn same_account(file: &CloudFile, accounts: &BTreeMap<String, String>, user: &str) -> bool {
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
pub(super) fn owner_of<'a>(file: &'a CloudFile, accounts: &BTreeMap<String, String>) -> Option<&'a str> {
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
pub(super) async fn may_manage(state: &AppState, file: &CloudFile, user: &str) -> bool {
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

pub(super) async fn require_manager(state: &AppState, user: &str) -> Result<CloudFile, ApiError> {
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

/// Whether `user` may see what is copied of other devices (§4.6; the user's
/// decision of 2026-10-19): the owner, and the local accounts the owner
/// allowed — only while `cloud.json` reads (`file`), names an owner that is
/// that local account here now ([`owner_of`]; `accounts`: every local
/// account's `createdAt`), and local authentication is on. An allowance
/// holds for the account it was given to only ([`same_account`]).
pub(super) fn sees_other_devices(file: Option<&CloudFile>, accounts: &BTreeMap<String, String>, local_auth: bool, user: Option<&str>) -> bool {
    let (Some(file), true, Some(user)) = (file, local_auth, user) else {
        return false;
    };
    let Some(owner) = owner_of(file, accounts) else {
        return false;
    };
    owner == user || (file.remote_access.get(user) == Some(&true) && same_account(file, accounts, user))
}
