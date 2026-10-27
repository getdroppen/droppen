use std::sync::Arc;

use rh_proto::chain::ChainError;
use rh_proto::wrap::{self, Purpose, SealContext, SealedKey, WrapError};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use super::local::unhex;
use super::state;
use super::Shared;

/// The most mail kept to try again as the chain catches up.
pub(in crate::agent) const MAX_DEFERRED: usize = 64;

/// Keep `value` (a mail not judgeable yet) to try again — once per mail id —
/// and return the oldest one it pushes out, if the list is full.
pub(in crate::agent) fn defer(deferred: &mut Vec<Value>, value: Value) -> Option<Value> {
    if deferred.iter().any(|v| v["id"] == value["id"]) {
        return None;
    }
    deferred.push(value);
    (deferred.len() > MAX_DEFERRED).then(|| deferred.remove(0))
}

/// What becomes of a mail pushed out of a full deferred list: it must not
/// vanish, neither acknowledged nor ever tried again — the hub, counting it
/// as unanswered, would send no further page of mail. One for an epoch this
/// agent is already past is settled, so acknowledged; any other is released
/// — left in the mailbox for the next sign-in, and counted as answered so
/// the next page comes.
pub(in crate::agent) fn evicted_message(mail: &Value, mk_epoch: Option<u32>) -> Value {
    let epoch = mail["epoch"].as_u64();
    let moot = matches!((epoch, mk_epoch), (Some(e), Some(held)) if e <= u64::from(held));
    if moot {
        tracing::info!(id = %mail["id"], "a deferred machine key for an epoch this machine is past: acknowledged");
        json!({"t": "mail_ack", "id": mail["id"]})
    } else {
        tracing::warn!(id = %mail["id"], "too much mail waiting for the chain: leaving the oldest in the mailbox for the next sign-in");
        json!({"t": "mail_release", "id": mail["id"]})
    }
}

/// What to do with a mailed machine key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::agent) enum MailVerdict {
    /// Taken, or refused for good: acknowledge it.
    Ack,
    /// Not the agent's to take (another purpose than 2): leave it in the
    /// mailbox — `mail_release`, never `mail_ack`. A host's mailbox is shared
    /// between its agent and its device role (chain v3), and what the agent
    /// acknowledges is gone for the device role too.
    Release,
    /// Not judgeable yet (no chain, or the chain has not caught up with the
    /// key's commitment or its sender): keep it and try again.
    Later,
}

/// Which mail the agent itself takes: only purpose 2, this host's own
/// machine key. Everything else in the mailbox
/// (a device's keys, purpose 5 host keys, purpose 4 requests…) belongs to
/// the device role of the same id, and is released.
pub(in crate::agent) fn agents_mail(value: &Value) -> bool {
    value["purpose"].as_u64() == Some(Purpose::MachineKey as u64)
}

/// Open a sealed machine key from the mailbox: from a current full device,
/// and exactly the key the chain committed for its epoch (a
/// device removed since it sealed it, or the hub, can hand over nothing the
/// chain does not already name, and no epoch out of reach). Mail of any
/// other purpose is not the agent's: released, never acknowledged.
pub(in crate::agent) fn open_mail(shared: &Arc<Shared>, value: &Value) -> MailVerdict {
    if !agents_mail(value) {
        return MailVerdict::Release;
    }
    let mut st = shared.state.lock().expect("agent state");
    let Some(chain) = st.chain.as_ref() else { return MailVerdict::Later };
    let (Some(sender), Some(epoch), Some(sealed)) = (
        value["sender"].as_str(),
        value["epoch"].as_u64().and_then(|e| u32::try_from(e).ok()),
        value["sealed"].as_str(),
    ) else {
        return MailVerdict::Ack;
    };
    let Some(sealed) = unhex(sealed).and_then(|b| SealedKey::decode(&b).ok()) else {
        return MailVerdict::Ack;
    };
    let ctx = SealContext {
        purpose: Purpose::MachineKey,
        account: st.account.clone(),
        sender: sender.into(),
        recipient: st.machine.clone(),
        epoch,
    };
    match wrap::open_machine_key(shared.identity.dh(), &st.machine, chain, &ctx, &sealed) {
        Ok(key) => {
            if let Err(error) = state::store_machine_key(&shared.paths, epoch, &key) {
                tracing::error!(%error, "cannot store a machine key");
                return MailVerdict::Later;
            }
            let newer = st.mk_epoch.is_none_or(|e| epoch > e);
            if newer {
                st.mk_epoch = Some(epoch);
            }
            drop(st);
            shared.save();
            tracing::info!(epoch, %sender, "took a machine key from the mailbox");
            if newer {
                shared.sync.send(super::streams::Event::KeysChanged { reencrypt: false });
            }
            MailVerdict::Ack
        }
        // A commitment or a sender this view has not seen yet: the chain may
        // be behind the mail (the hub pushes mail before the chain catches up).
        Err(WrapError::Uncommitted(ChainError::UncommittedKey { .. })) => MailVerdict::Later,
        Err(WrapError::UnknownSender(_)) if chain.device_record(sender).is_none() => MailVerdict::Later,
        Err(error) => {
            tracing::warn!(%error, %sender, epoch, "refused a machine-key delivery");
            MailVerdict::Ack
        }
    }
}

/// Try the deferred mail again (the chain advanced); acknowledge what is settled.
pub(in crate::agent) fn retry_mail(shared: &Arc<Shared>, text: &mpsc::UnboundedSender<String>) {
    let deferred = std::mem::take(&mut *shared.deferred_mail.lock().expect("deferred mail"));
    for value in deferred {
        match open_mail(shared, &value) {
            MailVerdict::Ack => {
                let _ = text.send(json!({"t": "mail_ack", "id": value["id"]}).to_string());
            }
            MailVerdict::Release => {
                let _ = text.send(json!({"t": "mail_release", "id": value["id"]}).to_string());
            }
            MailVerdict::Later => shared.deferred_mail.lock().expect("deferred mail").push(value),
        }
    }
}
