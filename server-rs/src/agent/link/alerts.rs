use std::sync::Arc;

use rh_proto::chain::{Action, ChainState, Entry, Signer};
use serde_json::json;
use tokio::sync::mpsc;

use super::hub::now_ms;
use super::Shared;


/// What an entry changes about who is in the account, in words, if anything:
/// every addition, removal, recovery join and activation, approval and change
/// of recovery words — an alert replaces the day of distrust after a removal,
/// and a self-activation with the recovery words is the moment a recovered
/// device starts to count — and new key epochs committed on their own, not as
/// part of a removal's rotation or a machine's enrollment (`prev`: the entry
/// before it): a full device must not be able to commit key epochs without
/// anybody hearing of it.
///
/// Chain v3 (`docs/local-aggregator.md` §3.2 "告警"): every device added (its
/// flags in words), every grant of admin, every new host or host restarting,
/// every passkey slot, passkey join and activation, every change to whom a
/// host accepts control from — alerted from this agent's own chain, so a hub
/// cannot hold any of it back. A host renewing its identity key
/// (`docs/host-identity-rotation.md` §3.4 "告警") is a note with the new
/// fingerprint, as the device list shows it.
pub(in crate::agent) fn account_change(entry: &Entry, prev: Option<&Entry>) -> Option<String> {
    account_change_in(entry, prev, None)
}

/// [`account_change`], with what an ending renewal pause marker names
/// (`paused`, from this agent's chain: [`pause_notice_edges`]): its start's
/// removals, and for a completion only the control that comes back.
pub(in crate::agent) fn account_change_in(entry: &Entry, prev: Option<&Entry>, paused: Option<&[(String, Vec<String>)]>) -> Option<String> {
    let signer = match &entry.draft.signer {
        Signer::Device(id) => id.clone(),
        Signer::Recovery => "the recovery words".into(),
        Signer::Account => "the account key".into(),
        Signer::Passkey(slot) => format!("passkey slot {slot}"),
    };
    let flags = |f: rh_proto::chain::Flags| match (f.host, f.admin) {
        (true, true) => "runs AI, can manage the account",
        (true, false) => "runs AI",
        (false, true) => "can manage the account",
        (false, false) => "can control devices that accept it",
    };
    let v3 = entry.is_v3();
    Some(match &entry.draft.action {
        Action::AddDevice3(d) if matches!(entry.draft.signer, Signer::Passkey(_)) => {
            format!("device {} is joining with {signer}: approve it on an admin device, or remove it", d.id)
        }
        Action::AddDevice3(d) => format!("device {} ({}) was added by {signer}", d.id, flags(d.flags)),
        Action::RecoveryJoin3(d) => format!("device {} ({}) is coming back with the recovery words", d.id, flags(d.flags)),
        Action::RemoveDevice3(id) => format!("device {id} was removed by {signer}"),
        Action::SetAdmin { id, admin: true } => format!("device {id} can now manage the account (granted by {signer})"),
        Action::SetAdmin { id, admin: false } => format!("device {id} can no longer manage the account ({signer})"),
        Action::SetHost { id, host: true } => format!("device {id} runs AI again (set by {signer})"),
        Action::SetHost { id, host: false } => format!("device {id} no longer runs AI ({signer})"),
        Action::PasskeySlotSet(s) => format!("a passkey can now unlock new devices (slot {}, set by {signer})", s.slot),
        Action::PasskeySlotRemove(slot) => format!("passkey slot {slot} was removed by {signer}"),
        Action::PasskeyActivate(id) => format!("device {id} activated itself with its passkey"),
        Action::SetAccept { host, from } => {
            let whom = match from {
                rh_proto::chain::Accept::Admins => "admin devices only".to_string(),
                rh_proto::chain::Accept::All => "every device of the account".to_string(),
                rh_proto::chain::Accept::Devices(ids) => format!("admin devices and {}", ids.join(", ")),
            };
            format!("{host} now accepts control from {whom} (set by {signer})")
        }
        Action::RotateHostKey(r) => format!(
            "device {} renewed its identity key{} (new fingerprint {}, written by {signer})",
            r.host,
            if r.assurance == rh_proto::chain::Assurance::OnDevice {
                ", confirmed on the device"
            } else {
                ""
            },
            rh_proto::identity::device_fingerprint_text(&r.sign_key, &r.dh_key)
        ),
        Action::SetRenewalPolicy { always_assured: true } => {
            format!("identity key renewals must now be confirmed on the device (set by {signer})")
        }
        Action::SetRenewalPolicy { always_assured: false } => format!("identity key renewals may now be automatic (set by {signer})"),
        Action::RenewalPause { phase, removals, .. } => {
            // §3.4 rule 10: "control by {ids} over {host}", per host.
            let edges = |removals: &[(String, Vec<String>)]| {
                if removals.is_empty() {
                    "the paused devices".to_string()
                } else {
                    removals
                        .iter()
                        .map(|(host, ids)| format!("{} over {host}", ids.join(", ")))
                        .collect::<Vec<_>>()
                        .join("; ")
                }
            };
            match phase {
                rh_proto::chain::PausePhase::Start => {
                    format!("control by {} is paused while identity keys are renewed (by {signer})", edges(removals))
                }
                rh_proto::chain::PausePhase::Completed if paused.is_some_and(|p| p.is_empty()) => {
                    format!("a renewal pause was completed; none of the control it paused comes back (by {signer})")
                }
                rh_proto::chain::PausePhase::Completed => format!("control by {} resumes (by {signer})", edges(paused.unwrap_or(&[]))),
                rh_proto::chain::PausePhase::Cancelled => {
                    format!("the pause of control by {} was ended (by {signer})", edges(paused.unwrap_or(&[])))
                }
            }
        }
        Action::ApproveDevice(id) if v3 => format!("device {id} was approved by {signer}"),
        Action::AddDevice(d) => format!("device {} was added by {signer}", d.id),
        Action::RemoveDevice(id) => format!("device {id} was removed by {signer}"),
        Action::AddMachine(m) => format!("device {} (runs AI) was added by {signer}", m.id),
        Action::RemoveMachine(id) => format!("device {id} (runs AI) was removed by {signer}"),
        Action::RecoveryJoin(d) => format!("device {} is coming back with the recovery words", d.id),
        Action::ActivateRecovered(id) => format!("device {id} activated itself with the recovery words"),
        Action::ApproveDevice(id) => format!("device {id}, back with the recovery words, was approved by {signer}"),
        Action::SetRecoveryKey(Some(_)) => format!("the recovery words were replaced by {signer}"),
        Action::SetRecoveryKey(None) => format!("the recovery words were turned off by {signer}"),
        Action::KeyEpochs(k) if !part_of_a_batch(entry, prev) => {
            let user = k.user.map(|c| format!("user key epoch {}", c.epoch));
            let machines = (!k.machines.is_empty()).then(|| format!("{} content key epoch(s)", k.machines.len()));
            let what: Vec<String> = user.into_iter().chain(machines).collect();
            format!("new keys were committed by {signer}: {}", what.join(" and "))
        }
        _ => return None,
    })
}

/// Whether a key-epoch entry follows, from the same signer, the entry its
/// batch is for, so that entry's alert covers it — and only for what that
/// entry needs:
///
/// - after a removal, the rotation it calls for (user key and hosts' keys);
/// - after a host joins (`AddMachine`, `AddDevice3{host}`), starts running
///   AI again (`SetHost true`) or renews its identity key (`RotateHostKey`),
///   exactly that host's own next machine key epoch: one commitment, for that
///   host, and no user key.
///
/// Anything else is its own alert: a user-key rotation
/// after adding a plain device, a key epoch entry that also commits other
/// hosts' keys or a user key next to a new host's, or key epochs after key
/// epochs: the cover never chains from one to the next.
pub(in crate::agent) fn part_of_a_batch(entry: &Entry, prev: Option<&Entry>) -> bool {
    let Action::KeyEpochs(keys) = &entry.draft.action else { return false };
    let Some(prev) = prev.filter(|p| p.draft.signer == entry.draft.signer) else {
        return false;
    };
    let only_the_hosts_own = |host: &str| keys.user.is_none() && matches!(keys.machines.as_slice(), [(id, _)] if id == host);
    match &prev.draft.action {
        Action::RemoveDevice(_) | Action::RemoveMachine(_) | Action::RemoveDevice3(_) => true,
        Action::AddMachine(m) => only_the_hosts_own(&m.id),
        Action::AddDevice3(d) if d.flags.host => only_the_hosts_own(&d.id),
        Action::SetHost { id, host: true } => only_the_hosts_own(id),
        Action::RotateHostKey(r) => only_the_hosts_own(&r.host),
        _ => false,
    }
}

/// What the notice of renewal pause marker `entry` names, from `chain` (the
/// marker applied): an ending marker names no removals, so its start's — and
/// "resumes" only the control that comes back, never an edge the base has
/// since dropped (a hand `SetAccept`, a removal, a host that stopped hosting)
/// or another open pause still holds. `None` for anything else.
pub(in crate::agent) fn pause_notice_edges(chain: &ChainState, entry: &Entry) -> Option<Vec<(String, Vec<String>)>> {
    let Action::RenewalPause { op, phase, .. } = &entry.draft.action else {
        return None;
    };
    let removals = chain.pause(op)?.removals.clone();
    if *phase != rh_proto::chain::PausePhase::Completed {
        return Some(removals);
    }
    Some(
        removals
            .into_iter()
            .filter_map(|(host, ids)| {
                let ids: Vec<String> = ids.into_iter().filter(|id| chain.may_control(id, &host).is_ok()).collect();
                (!ids.is_empty()).then_some((host, ids))
            })
            .collect(),
    )
}

/// Raise every change to who is in the account that `entries` make.
pub(in crate::agent) fn alert_account_changes(shared: &Arc<Shared>, entries: &[Entry]) {
    let prevs: Vec<Option<Entry>> = {
        let st = shared.state.lock().expect("agent state");
        entries
            .iter()
            .map(|e| {
                let seq = usize::try_from(e.seq()).ok()?.checked_sub(1)?;
                st.chain.as_ref()?.entries().get(seq).cloned()
            })
            .collect()
    };
    // An ending renewal pause marker names no removals: its start did.
    let paused: Vec<Option<Vec<(String, Vec<String>)>>> = {
        let st = shared.state.lock().expect("agent state");
        entries.iter().map(|e| pause_notice_edges(st.chain.as_ref()?, e)).collect()
    };
    let me = shared.state.lock().expect("agent state").machine.clone();
    let mut noted = false;
    for ((entry, prev), paused) in entries.iter().zip(&prevs).zip(&paused) {
        // This machine's own addition is its enrollment completing, not news to it.
        if matches!(&entry.draft.action, Action::AddMachine(m) if m.id == me) || matches!(&entry.draft.action, Action::AddDevice3(d) if d.id == me) {
            continue;
        }
        let Some(what) = account_change_in(entry, prev.as_ref(), paused.as_deref()) else { continue };
        tracing::warn!(seq = entry.seq(), "account change: {what}");
        // Kept, and published in the encrypted status stream: every device
        // reads what this machine saw change in the account.
        shared.state.lock().expect("agent state").note_change(now_ms(), &what);
        noted = true;
        crate::push::notify_account_change(Arc::new(shared.paths.clone()), "Droppen: account change".into(), what);
    }
    if noted {
        shared.save();
        let changes = shared.state.lock().expect("agent state").changes.clone();
        shared.sync.send(super::streams::Event::Changes(changes));
    }
}

/// An alarm every device must hear about: logged, kept in the state (and so
/// in the encrypted status stream and `/rh/info`, which every device of the
/// account reads), and pushed (Web Push and Bark): logged every time, pushed
/// at most hourly — a fork served on every reconnect must be said once an
/// hour, not once a reconnect.
pub(in crate::agent) fn raise_alarm(shared: &Arc<Shared>, what: String) {
    tracing::error!("ALARM: {what}");
    let alarms = {
        let mut st = shared.state.lock().expect("agent state");
        if !st.note_alarm(now_ms(), &what) {
            return;
        }
        st.alarms.clone()
    };
    shared.save();
    shared.sync.send(super::streams::Event::Alarms(alarms));
    crate::push::notify_account_change(Arc::new(shared.paths.clone()), "Droppen: device chain alarm".into(), what);
}

/// The hub sends each entry once a connection: an activation held back as too
/// early is asked for again (with what follows it) once this agent's own day
/// since the join has passed.
pub(in crate::agent) fn ask_again_for_held(shared: &Arc<Shared>, text: &mpsc::UnboundedSender<String>) {
    let due = {
        let mut held = shared.held_until.lock().expect("held");
        match *held {
            Some(at) if at <= now_ms() => held.take().is_some(),
            _ => false,
        }
    };
    if due {
        let from = shared.state.lock().expect("agent state").seq().map_or(0, |s| s + 1);
        tracing::info!(from, "asking the hub again for the chain a held-back activation stopped");
        let _ = text.send(json!({"t": "chain_sub", "from": from}).to_string());
    }
}

