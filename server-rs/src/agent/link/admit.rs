use std::sync::{Arc, Mutex};
use std::time::Instant;

use rh_proto::bridge::{RequestHead, ResponseHead};
use rh_proto::chain::{Attestation, ChainError, Entry};
use rh_proto::channel::{PathOptions, Session};
use rh_proto::direct::{Admission, Admit, AgentDirect};
use rh_proto::driver::{Channel, Incoming};
use rh_proto::frame::Frame;
use rh_proto::handshake::{self, Accepted, AgentSide, HandshakeError, FRESHNESS_MAX_AGE_MS};
use rh_proto::mux::{Role, StreamClass};
use rh_proto::noise::{Binding, RekeyPolicy};
use rh_proto::path::{self, DirectBinding};
use serde_json::json;
use tokio::sync::mpsc;

use super::alerts::{alert_account_changes, raise_alarm};
use super::bridge;
use super::hub::now_ms;
use super::lapse::{close_lapsed, drop_removed, lapse_wait, on_removed, ChainStateExt, ChannelLapse};
use super::local::AgentLocal;
use super::mail::retry_mail;
use super::state::{self, AgentState};
use super::{Pending, Routes, Shared, AGENT_UNSAVED, CHANNEL_STALE, MAX_PENDING, PENDING_FOR};

/// Whether a chain error is a conflict between two views — a fork, a
/// rollback, another genesis or account key — rather than an entry refused.
pub(in crate::agent) trait ChainErrorExt {
    fn is_conflict(&self) -> bool;
}

impl ChainErrorExt for ChainError {
    fn is_conflict(&self) -> bool {
        matches!(
            self,
            ChainError::Fork { .. } | ChainError::Rollback { .. } | ChainError::GenesisMismatch | ChainError::PinMismatch
        )
    }
}

pub(in crate::agent) fn server_info(direct: bool) -> Vec<u8> {
    json!({"agent": "remote-hand", "version": crate::api::SERVER_VERSION, "features": super::direct::features(direct)})
        .to_string()
        .into_bytes()
}

pub(in crate::agent) fn now_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

/// The first message of a channel: on the hub or a relay, or on a direct
/// path (which names its binding too).
pub(in crate::agent) enum Hello<'a> {
    /// `Frame::Data` on a hub or relay connection.
    Channel(&'a [u8]),
    /// A direct path's hello, over `device`'s binding.
    Direct(&'a DirectBinding, &'a [u8]),
}

/// The admission step's refusal with no verified chain to admit against.
pub(in crate::agent) struct NoChain;

/// The step every admission — a channel on the hub or a relay, a direct
/// path — shares: this agent's live chain and clock, the freshness gate and
/// the replay guard, under the state lock, with the guard and the device's
/// record saved before the reply leaves.
///
/// `after_open` runs in that same critical section, on the opened channel's
/// behalf: the channel admission registers its control lapse there, so no
/// attestation can arrive between a channel being admitted and its lapse
/// being registered ([`take_attestation`] reads the lapses under this lock).
/// It gets the accepted state the lock still holds, and must not lock it
/// again. Entries a hello teaches are returned for the caller to act on as
/// on any chain change.
pub(in crate::agent) fn admit<R>(
    shared: &Arc<Shared>,
    device: &str,
    hello: Hello<'_>,
    now: u64,
    after_open: impl FnOnce(&mut AgentState, &Accepted) -> R,
) -> Result<(Result<Accepted, HandshakeError>, Vec<Entry>, Option<state::Recorded>, Option<R>), NoChain> {
    let mut st = shared.state.lock().expect("agent state");
    let st = &mut *st;
    let attestations = st.attestation_list();
    let Some(chain) = st.chain.as_mut() else {
        return Err(NoChain);
    };
    let before = chain.seq();
    let mut guard = shared.guard.lock().expect("replay guard");
    let side = AgentSide {
        chain,
        clock: &mut st.clock,
        guard: &mut guard,
        key: shared.identity.dh(),
        machine: &st.machine,
        policy: RekeyPolicy::default(),
        server_info: &server_info(shared.direct),
        attestations: &attestations,
        freshness_max_age_ms: FRESHNESS_MAX_AGE_MS,
    };
    let outcome = match hello {
        Hello::Channel(message) => handshake::accept(side, device, message, now),
        Hello::Direct(bind, message) => handshake::accept_direct(side, device, bind, message, now),
    };
    let mut recorded = None;
    let mut opened = None;
    if let Ok(accepted) = &outcome {
        if matches!(accepted, Accepted::Open { .. }) {
            st.save_guard(&guard);
            // Who ever opened a channel, recorded as it opens and saved with
            // the guard (and any entries the hello taught) before the reply
            // leaves — in this one step under the lock.
            recorded = Some(st.record_controller(&shared.paths, device, now));
            opened = Some(after_open(st, accepted));
        }
    }
    // Whatever the hello taught took effect, refused or not: a hello whose
    // later entry fails must not lose the earlier ones.
    let learnt = st.entries_after(Some(before));
    Ok((outcome, learnt, recorded, opened))
}

/// Direct hellos admitted by exactly the rules of [`accept`]: this agent's
/// live chain and clock, the freshness gate, the replay guard (persisted
/// before the reply leaves). Entries a hello teaches go to the connection,
/// which acts on them as on any chain change.
pub(in crate::agent) struct LinkAdmit {
    pub(in crate::agent) shared: Arc<Shared>,
}

impl Admit for LinkAdmit {
    fn admit(&self, device: &str, bind: &DirectBinding, hello: &[u8]) -> Admission {
        let shared = &self.shared;
        let now = now_ms();
        let (outcome, learnt, recorded, _) = match admit(shared, device, Hello::Direct(bind, hello), now, |_, _| ()) {
            Ok(admitted) => admitted,
            Err(NoChain) => {
                return Admission::Refused {
                    reply: None,
                    why: "no chain".into(),
                }
            }
        };
        if recorded.is_none() {
            shared.save();
        }
        let unsaved = recorded.as_ref().is_some_and(|r| !let_in(shared, device, r));
        // Acted on here, whether or not the hub link is up: entries a direct
        // hello teaches while it is down must still be alerted and swept
        // through every route table, not only saved.
        learnt_from_direct(shared, &learnt);
        if let Err(error) = &outcome {
            if error.is_alarm() {
                raise_alarm(
                    shared,
                    format!("device {device}'s view of the device chain conflicts with this device's ({error})"),
                );
            }
        }
        match outcome {
            Ok(Accepted::Open { .. }) if unsaved => Admission::Refused {
                reply: None,
                why: "cannot save the record of who connects".into(),
            },
            Ok(Accepted::Open { reply, transport, .. }) => Admission::Open { reply, transport },
            Ok(Accepted::NeedEntries { reply, .. }) => Admission::Refused {
                reply: Some(reply),
                why: "the device has seen more of the chain".into(),
            },
            Ok(Accepted::Stale { reply, .. }) => Admission::Refused {
                reply: Some(reply),
                why: "stale chain view".into(),
            },
            Err(error) => Admission::Refused {
                reply: None,
                why: error.to_string(),
            },
        }
    }
}

/// Whether a device a handshake let in may have its channel, as its
/// [`state::Recorded`] says — and the status told, when it is new and
/// saved. A device new to the record gets in only once it is on disk; one
/// on the saved record gets in whatever the disk does: a full disk or a
/// read-only state directory must not lock every device out of a computer
/// controlled from afar.
pub(in crate::agent) fn let_in(shared: &Arc<Shared>, device: &str, recorded: &state::Recorded) -> bool {
    match recorded {
        state::Recorded::New => {
            shared.sync.send(super::streams::Event::Controllers);
            true
        }
        state::Recorded::Known => true,
        state::Recorded::Unsaved(error) => {
            tracing::error!(%device, %error, "cannot save the agent state; refusing a device not on its record yet");
            false
        }
        state::Recorded::KnownUnsaved(error) => {
            tracing::error!(%device, %error, "cannot save the agent state; letting in a device already on its record");
            true
        }
    }
}

/// Entries a direct hello taught (already applied to the state): acted on as
/// any chain advance, every connection's route table swept.
pub(in crate::agent) fn learnt_from_direct(shared: &Arc<Shared>, learnt: &[Entry]) {
    if !learnt.is_empty() {
        tracing::info!(n = learnt.len(), "caught up on the device chain from a direct hello");
        chain_moved(shared, learnt, None);
    }
}

/// Whether `incoming` is a device's signaling stream for direct paths.
pub(in crate::agent) fn is_signaling(incoming: &Incoming) -> bool {
    incoming.class == StreamClass::Mux && RequestHead::decode(&incoming.meta).is_ok_and(|h| h.path == path::SIGNAL_PATH)
}

/// Refuse a signaling stream (direct paths off, or a channel without paths).
pub(in crate::agent) async fn refuse_signaling(incoming: Incoming) {
    let head = ResponseHead {
        status: 403,
        headers: Vec::new(),
    };
    if let Ok(bytes) = head.encode() {
        let _ = incoming
            .stream
            .send(&rh_proto::bridge::message(rh_proto::bridge::Kind::Head, &bytes))
            .await;
    }
    incoming.stream.close(rh_proto::bridge::CLOSE_FORBIDDEN);
}

/// The chain advanced by `entries` on a connection (from the hub, or a
/// device's hello on the hub or a relay): close this connection's channels of
/// devices that may no longer control this machine at once, then
/// [`chain_moved`]. Whatever the source: a removal learnt from a hello
/// closes the removed device's channel too.
pub(in crate::agent) fn chain_advanced(
    shared: &Arc<Shared>,
    entries: &[Entry],
    routes: &mut Routes,
    out: &mpsc::UnboundedSender<Frame>,
    text: &mpsc::UnboundedSender<String>,
) {
    drop_removed(shared, routes, out);
    if !chain_moved(shared, entries, Some(text)) {
        routes.clear();
    }
}

/// The chain advanced by `entries`, wherever they came from — the hub, a
/// device's hello on the hub or a relay, a direct hello, with the hub link up
/// or down: saved; every route table told to sweep
/// ([`Shared::chain_version`]: the hub connection and each relay connection
/// close the channels of devices that may no longer control this machine);
/// the account changes raised; this machine's own removal acted on; the mail
/// that waited for it tried again (on `text`, else the hub link's when it is
/// up). `false` when the chain removed this machine.
pub(in crate::agent) fn chain_moved(shared: &Arc<Shared>, entries: &[Entry], text: Option<&mpsc::UnboundedSender<String>>) -> bool {
    shared.save();
    shared.chain_version.send_modify(|v| *v += 1);
    // The status's `controllers` says which chain it is about: a new record.
    shared.sync.send(super::streams::Event::Controllers);
    alert_account_changes(shared, entries);
    if shared.state.lock().expect("agent state").removed_in_chain() {
        on_removed(shared, "its device chain says so");
        return false;
    }
    // A renewal of this device's own identity key: to its staged seed (the
    // next sign-in takes it), or to keys it does not hold, or may hold only
    // in a staged file that does not read (the link stops).
    if matches!(
        super::rotation::on_chain(shared),
        super::rotation::Verdict::Foreign { .. } | super::rotation::Verdict::Unreadable
    ) {
        return false;
    }
    let hub = shared.hub_text.lock().expect("hub text").clone();
    if let Some(text) = text.or(hub.as_ref()) {
        retry_mail(shared, text);
    }
    true
}

/// An enrollment the link completes (the hub assigned the id; a device has
/// just added the machine): exactly with this machine's keys, or it is
/// refused — an alarm, and the link stops.
pub(in crate::agent) fn check_enrolled_keys(shared: &Arc<Shared>) -> bool {
    let (sign, dh) = (shared.identity.sign_public(), shared.identity.dh_public());
    let (machine, ok) = {
        let st = shared.state.lock().expect("agent state");
        (st.machine.clone(), st.keys_match(&sign, &dh))
    };
    if ok {
        tracing::info!(%machine, "enrolled: the device chain names this machine with its keys");
        return true;
    }
    raise_alarm(
        shared,
        format!("the device chain added {machine} with other keys than this device's; refusing it"),
    );
    shared.cancel.cancel();
    false
}

pub(in crate::agent) fn on_frame(
    shared: &Arc<Shared>,
    frame: Frame,
    routes: &mut Routes,
    pending: &mut Pending,
    out: &mpsc::UnboundedSender<Frame>,
    text: &mpsc::UnboundedSender<String>,
) {
    match frame {
        Frame::Chain { entries, .. } => {
            let Ok(entries) = entries.iter().map(|e| Entry::decode(e)).collect::<Result<Vec<_>, _>>() else {
                tracing::warn!("undecodable chain entries from the hub");
                return;
            };
            let (was_enrolled, absorbed) = {
                let mut st = shared.state.lock().expect("agent state");
                let was = st.enrolled();
                (was, st.absorb(&entries, now_ms()))
            };
            let now_enrolled = shared.state.lock().expect("agent state").enrolled();
            if !was_enrolled && now_enrolled && !check_enrolled_keys(shared) {
                return;
            }
            // Everything that took effect is acted on — saved, removed
            // devices' channels closed, every change alerted — even when the
            // batch stopped short: a join followed by a spoiled entry must not
            // land silently. The first batch at enrollment is the history that
            // let this machine in: `news` is empty for it.
            if absorbed.applied > 0 {
                tracing::info!(n = absorbed.applied, "device chain advanced");
                chain_advanced(shared, &absorbed.news, routes, out, text);
            }
            match absorbed.error {
                None => {}
                Some(e) if e.is_conflict() => {
                    raise_alarm(
                        shared,
                        format!("the hub served a device chain that conflicts with this device's ({e}); refusing it"),
                    );
                }
                Some(ChainError::TooEarly { seq, ready_ms }) => {
                    *shared.held_until.lock().expect("held") = Some(ready_ms);
                    tracing::warn!(
                        seq,
                        ready_ms,
                        "a recovered device's self-activation arrived before this machine's own day since its join; holding it (and what follows) until then"
                    );
                }
                Some(error) => tracing::warn!(%error, "refused chain entries"),
            }
        }
        Frame::Attest { attestation, .. } => {
            if let Ok(att) = Attestation::decode(&attestation) {
                if take_attestation(shared, att) {
                    shared.save();
                }
            }
        }
        Frame::Ack { .. } | Frame::Conflict { .. } => shared.sync.send(super::streams::Event::Frame(frame)),
        frame => on_channel_frame(shared, frame, routes, pending, out, text),
    }
}

/// Take an attestation the hub relayed: every open channel's lapse notes
/// what it says first ([`ChannelLapse::note`]), then the state keeps it as
/// its device's newest — so a word that counted for a channel is in that
/// channel's lapse before a later word from the same admin, one that does
/// not count, replaces it in the state. Whether the state took it.
pub(in crate::agent) fn take_attestation(shared: &Shared, att: Attestation) -> bool {
    let mut st = shared.state.lock().expect("agent state");
    let now = now_ms();
    shared.lapses.lock().expect("lapses").retain(|weak| match weak.upgrade() {
        Some(lapse) => {
            lapse.lock().expect("channel lapse").note(&st, &att, now);
            true
        }
        None => false,
    });
    st.attest(att)
}

/// A channel's frame — from the hub (while it still relays) or from a relay
/// ([`super::relays`]): `OPEN` (the device's first message comes next),
/// `DATA`, `CREDIT`, `CLOSE`. Routes are per connection, and so are the
/// channels opening: at most [`MAX_PENDING`], none for longer than
/// [`PENDING_FOR`].
pub(in crate::agent) fn on_channel_frame(
    shared: &Arc<Shared>,
    frame: Frame,
    routes: &mut Routes,
    pending: &mut Pending,
    out: &mpsc::UnboundedSender<Frame>,
    text: &mpsc::UnboundedSender<String>,
) {
    match frame {
        Frame::Open { ch, peer } => {
            let now = Instant::now();
            expire_pending(pending, out, now);
            if pending.len() >= MAX_PENDING && !pending.contains_key(&ch) {
                refuse(out, ch, 4409, "too many channels opening");
            } else {
                pending.insert(ch, (peer, now));
            }
        }
        Frame::Data { ch, payload } => {
            if let Some((_, _, route)) = routes.get(&ch) {
                let _ = route.send(Frame::Data { ch, payload });
            } else if let Some((device, _)) = pending.remove(&ch) {
                accept(shared, ch, device, &payload, routes, out, text);
            }
        }
        Frame::Credit { ch, bytes } => {
            if let Some((_, _, route)) = routes.get(&ch) {
                let _ = route.send(Frame::Credit { ch, bytes });
            }
        }
        Frame::Close { ch, code, reason } => {
            pending.remove(&ch);
            if let Some((_, _, route)) = routes.remove(&ch) {
                let _ = route.send(Frame::Close { ch, code, reason });
            }
        }
        _ => {}
    }
}

/// Refuse the channels that opened [`PENDING_FOR`] before `now` and sent no
/// first message since.
pub(in crate::agent) fn expire_pending(pending: &mut Pending, out: &mpsc::UnboundedSender<Frame>, now: Instant) {
    pending.retain(|ch, (_, opened)| {
        let waiting = now.saturating_duration_since(*opened) < PENDING_FOR;
        if !waiting {
            refuse(out, *ch, 4408, "no first message in time");
        }
        waiting
    });
}

pub(in crate::agent) fn refuse(out: &mpsc::UnboundedSender<Frame>, ch: u32, code: u16, reason: &str) {
    let _ = out.send(Frame::Close {
        ch,
        code,
        reason: reason.into(),
    });
}

/// A device's first handshake message on `ch`.
pub(in crate::agent) fn accept(
    shared: &Arc<Shared>,
    ch: u32,
    device: String,
    message: &[u8],
    routes: &mut Routes,
    out: &mpsc::UnboundedSender<Frame>,
    text: &mpsc::UnboundedSender<String>,
) {
    let now = now_ms();
    let (outcome, learnt, recorded, lapse) = match admit(shared, &device, Hello::Channel(message), now, |st, _| {
        // The channel's control lapse, seeded and registered while the state
        // is still this admission's: an attestation arriving from here on
        // reaches it ([`take_attestation`]), and none can come in between.
        let lapse = Arc::new(Mutex::new(ChannelLapse::new(&device)));
        lapse.lock().expect("channel lapse").update(st, now);
        let mut lapses = shared.lapses.lock().expect("lapses");
        lapses.retain(|weak| weak.strong_count() > 0);
        lapses.push(Arc::downgrade(&lapse));
        lapse
    }) {
        Ok(admitted) => admitted,
        Err(NoChain) => return refuse(out, ch, 4503, "this device has not been added to the account yet"),
    };
    #[cfg(test)]
    if let Some(hook) = shared.after_admission.lock().expect("hook").take() {
        hook();
    }
    if !learnt.is_empty() {
        tracing::info!(applied = learnt.len(), "caught up on the device chain from a hello");
        chain_advanced(shared, &learnt, routes, out, text);
    }
    match outcome {
        Ok(Accepted::Open { reply, transport, hello, .. }) => {
            // Saved above (a device not on record yet gets in only once it is).
            if recorded.as_ref().is_some_and(|r| !let_in(shared, &device, r)) {
                return refuse(out, ch, AGENT_UNSAVED, "this device cannot save its record of who connects to it: check its disk");
            }
            let _ = out.send(Frame::Data { ch, payload: reply });
            let (tx, rx) = mpsc::unbounded_channel();
            // The key it opened with: a renewal of the device's identity
            // closes the channel ([`drop_removed`]).
            let key = transport.remote_static();
            routes.insert(ch, (device.clone(), key, tx));
            // A device that speaks sequenced frames gets them (and may ask
            // for a direct path); an older one gets the plain channel.
            let paths = path::client_wants_paths(&hello.client);
            // A device that reads streams named per key epoch: from now on
            // (remembered) the agent names them so.
            if rh_proto::stream::reads_epoch_names(&hello.client) {
                shared.sync.send(super::streams::Event::ReadsEpochNames);
            }
            let relay_hash = transport.handshake_hash();
            let session = if paths {
                Session::with_paths(ch, *transport, Role::Responder, PathOptions::default(), now_us())
            } else {
                Session::new(ch, *transport, Role::Responder)
            };
            let (channel, mut incoming): (Channel, _) = Channel::spawn(session, rx, out.clone());
            let loopback = shared.loopback.clone();
            // Taken after the chain's version is: a change landing in between
            // is one the channel's task sees (and its lapse is worked out
            // again on any chain moved since, [`ChannelLapse::holds`]).
            let mut moved = shared.chain_version.subscribe();
            let lapse = lapse.expect("an open channel's lapse, registered at its admission");
            let local: Arc<dyn bridge::Local> = Arc::new(AgentLocal::new(shared.clone(), lapse.clone()));
            let direct = (paths && shared.direct).then(|| {
                let (account, machine) = {
                    let st = shared.state.lock().expect("agent state");
                    (st.account.clone(), st.machine.clone())
                };
                Arc::new(AgentDirect {
                    admit: Arc::new(LinkAdmit { shared: shared.clone() }),
                    timeout: super::direct::ATTEMPT_TIMEOUT,
                    opts: super::direct::options(),
                    peers: shared.peers.clone(),
                    binding: Binding {
                        account,
                        device: device.clone(),
                        machine,
                    },
                    relay_hash,
                })
            });
            tracing::info!(%device, ch, paths, direct = direct.is_some(), "channel open");
            let shared = shared.clone();
            tokio::spawn(async move {
                // Control lapses on an open channel too (local-aggregator
                // §3.4, [`ChannelLapse`]): looked at when it is due, whenever
                // the chain moves (an admin whose word kept it fresh may be
                // one no longer), on every stream, and again when a request
                // is dispatched ([`bridge::Standing::controls`]). A lapse
                // ends the channel — its terminals, its direct path —
                // however long the hub keeps the connection up and a
                // removal to itself.
                let holds = || {
                    let st = shared.state.lock().expect("agent state");
                    lapse.lock().expect("channel lapse").holds(&st, now_ms())
                };
                let mut vouched = holds();
                loop {
                    if !vouched {
                        close_lapsed(&channel, &device, ch);
                        break;
                    }
                    let wait = lapse_wait(lapse.lock().expect("channel lapse").at(), now_ms());
                    let stream = tokio::select! {
                        next = incoming.recv() => match next {
                            Some(stream) => stream,
                            None => break,
                        },
                        changed = moved.changed() => {
                            if changed.is_err() {
                                break;
                            }
                            vouched = holds();
                            continue;
                        }
                        () = tokio::time::sleep(wait) => {
                            vouched = holds();
                            continue;
                        }
                    };
                    // Every stream re-checks that the device is still in the
                    // account: a removal that lands mid-channel stops new
                    // requests at once, whatever closes the channel.
                    // Chain v3: and that this machine still accepts control
                    // from it (an accept list changed, an admin demoted,
                    // this machine stopped running AI). And that its control
                    // has not lapsed, on the chain as it is now. Host
                    // identity renewal: and that the key it opened with is
                    // still the one the chain names for it.
                    let (current, fresh) = {
                        let st = shared.state.lock().expect("agent state");
                        let current = st
                            .chain
                            .as_ref()
                            .is_some_and(|c| c.may_control(&device, &st.machine).is_ok() && c.holds_named_key(&device, &key));
                        (current, lapse.lock().expect("channel lapse").holds(&st, now_ms()))
                    };
                    if !current {
                        tracing::info!(%device, ch, "a stream from a device that may no longer control this machine; closing its channel");
                        break;
                    }
                    if !fresh {
                        close_lapsed(&channel, &device, ch);
                        break;
                    }
                    if is_signaling(&stream) {
                        match &direct {
                            Some(ctx) => {
                                tracing::info!(%device, ch, "a device asks for a direct path");
                                tokio::spawn(rh_proto::direct::serve(stream.stream, channel.clone(), ctx.clone()));
                            }
                            None => {
                                tokio::spawn(refuse_signaling(stream));
                            }
                        }
                        continue;
                    }
                    tokio::spawn(bridge::serve(stream, device.clone(), loopback.clone(), local.clone()));
                }
                // The channel ended: its lapse leaves the registry (as any other
                // ended one's still listed), not only when an attestation comes.
                drop(local);
                drop(lapse);
                shared.lapses.lock().expect("lapses").retain(|weak| weak.strong_count() > 0);
            });
        }
        Ok(Accepted::NeedEntries { reply, from, .. }) => {
            tracing::info!(%device, from, "a device has seen more of the chain than this agent; asking for entries");
            let _ = out.send(Frame::Data { ch, payload: reply });
            refuse(out, ch, 4409, "send chain entries");
        }
        Ok(Accepted::Stale { reply, newest_ms, .. }) => {
            tracing::info!(%device, ?newest_ms, "refused a channel: nobody has vouched for this agent's chain head recently");
            let _ = out.send(Frame::Data { ch, payload: reply });
            refuse(out, ch, CHANNEL_STALE, "stale chain view: attest and retry");
        }
        Err(error) => {
            if error.is_alarm() {
                raise_alarm(
                    shared,
                    format!("device {device}'s view of the device chain conflicts with this device's ({error})"),
                );
            } else {
                tracing::info!(%device, %error, "refused a handshake");
            }
            refuse(out, ch, 4401, "handshake refused");
        }
    }
}

