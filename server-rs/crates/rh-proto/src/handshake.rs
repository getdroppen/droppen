//! What the two Noise handshake payloads carry, and the order an agent checks
//! things in before it lets a device in.
//!
//! # The payloads
//!
//! - **Hello** (device → agent, in the first message): the device's chain view
//!   `(seq, head)`, the last few chain entries it has (so an agent the hub has
//!   kept behind can catch up from the device it is talking to), and opaque
//!   client metadata. Deliberately **no request**: the first message is
//!   replayable and open to key-compromise impersonation, so anything that
//!   changes state waits for the first transport message, which only the holder
//!   of the device's static key can produce.
//! - **Reply** (agent → device): whether the device was accepted, the agent's
//!   own chain view, from which seq it needs entries if it is behind, and the
//!   entries the *device* lacks (up to [`MAX_HELLO_ENTRIES`] after its view), so
//!   a device the hub keeps behind learns of a removal or a pending recovery
//!   join from the agent, not only from the hub. The device merges them
//!   (signatures and rules checked as for any entry) and compares views: a
//!   different hash at a seq both have is a fork.
//!
//! # What an agent does with a first message ([`accept`])
//!
//! 1. Decrypt it under the binding `account | device | machine`.
//! 2. Apply the hello's chain entries it lacks (signatures and rules checked as
//!    for any entry). An entry that conflicts with what it has is a **fork** —
//!    refuse, and alert every device.
//! 3. If the device has seen further than the agent can now reach, reply
//!    "send entries from N" and stop: the agent will not authorise anyone
//!    against a view a device has proved stale.
//! 4. Authorise the device's static key against the (updated) chain, and
//!    check that this host accepts control from it
//!    ([`crate::chain::ChainState::may_control`], "谁可以控制我"): an admin
//!    device always; any other device — another host included — only when an
//!    admin has said so for this host. A device never controls itself.
//!
//! 5. Require the agent's view to be **fresh**
//!    ([`crate::chain::ChainState::freshness`]): an admin device must have
//!    vouched for exactly this head within [`FRESHNESS_MAX_AGE_MS`] (seven
//!    days: control; management requests inside the channel are held to
//!    [`MANAGE_FRESHNESS_MS`] by the agent). A view nobody has vouched for is
//!    refused with [`ReplyStatus::Stale`], so the device knows to send an
//!    attestation and try again; an attestation from beyond the agent's view
//!    is answered like a hello that is: "send entries".
//! 6. Only then consult the replay guard, so only authorised devices consume it.
//!
//! A hub that withholds a removal from an agent therefore loses as soon as any
//! device that saw the removal connects. Until then a removed device that
//! shares the agent's stale view could get in — which is what the freshness
//! gate bounds: some full device must have vouched for the agent's head
//! recently, and every channel is gated on it, not only privileged requests.
//!
//! The agent's [`ChainClock`] is its own live clock ([`AgentSide::clock`]):
//! [`accept`] refuses a history clock, sets the clock to the handshake's time,
//! and the chain records first sightings in it as it applies entries.
//!
//! The M0 re-review's must-fixes are in: the reply carries the agent's chain
//! suffix (above); [`ChainState::freshness`] no longer counts a just-recovered
//! device, nor a sole device's own attestation right after it removed the
//! others; and a removal clears the recovery key the removed device set or
//! came back with ([`crate::chain`]).

use thiserror::Error;

use crate::chain::{Attestation, ChainClock, ChainError, ChainState, Entry, Freshness};
use crate::codec::{DecodeError, Reader};
use crate::keys::DhKeypair;
use crate::noise::{Binding, ChannelError, Initiator, RekeyPolicy, ReplayGuard, Responder, Transport};
use crate::path::DirectBinding;

/// The most chain entries a hello carries.
pub const MAX_HELLO_ENTRIES: usize = 16;
/// The most client or server metadata a payload carries.
pub const MAX_INFO: usize = 1024;
/// How recently an admin device must have vouched for an agent's chain head
/// for the agent to open a channel at all: control's window (seven days).
pub const FRESHNESS_MAX_AGE_MS: u64 = crate::chain::CONTROL_FRESHNESS_MS;
/// How recently an admin device must have vouched for it before the agent
/// takes a management request inside a channel (twelve hours).
pub const MANAGE_FRESHNESS_MS: u64 = crate::chain::MANAGE_FRESHNESS_MS;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum HandshakeError {
    #[error(transparent)]
    Channel(#[from] ChannelError),
    #[error(transparent)]
    Chain(#[from] ChainError),
    #[error(transparent)]
    Decode(#[from] DecodeError),
    #[error("the device is not a current, active device of this account")]
    Unauthorized,
    /// The device is current, but this host does not accept control from it
    /// (or it is this host itself).
    #[error("this host does not accept control from the device: {0}")]
    NotAccepted(String),
    #[error("the agent did not accept the handshake ({status:?}, entries from {need_from})")]
    Refused { status: ReplyStatus, need_from: u64 },
    #[error("an agent must verify the chain with its own live clock, not a history clock")]
    HistoryClock,
}

impl HandshakeError {
    /// Whether this is the kind of failure every device should be told about.
    pub fn is_alarm(&self) -> bool {
        matches!(
            self,
            HandshakeError::Chain(ChainError::Fork { .. }) | HandshakeError::Chain(ChainError::Rollback { .. })
        )
    }
}

/// The first message's payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloBody {
    pub chain_seq: u64,
    pub chain_head: [u8; 32],
    pub entries: Vec<Entry>,
    pub client: Vec<u8>,
}

impl HelloBody {
    /// The hello for a device whose verified view is `chain`.
    pub fn for_chain(chain: &ChainState, client: &[u8]) -> Self {
        Self {
            chain_seq: chain.seq(),
            chain_head: chain.head(),
            entries: chain.suffix(MAX_HELLO_ENTRIES).to_vec(),
            client: client.to_vec(),
        }
    }

    /// `u64 seq | head | u16 n | (u16 len | entry)×n | u16 len | client`.
    pub fn encode(&self) -> Result<Vec<u8>, DecodeError> {
        if self.entries.len() > MAX_HELLO_ENTRIES || self.client.len() > MAX_INFO {
            return Err(DecodeError::TooLarge {
                what: "hello",
                len: self.entries.len().max(self.client.len()),
                max: MAX_INFO,
            });
        }
        let mut out = Vec::new();
        out.extend_from_slice(&self.chain_seq.to_be_bytes());
        out.extend_from_slice(&self.chain_head);
        out.extend_from_slice(&(self.entries.len() as u16).to_be_bytes());
        for entry in &self.entries {
            let bytes = entry.encode();
            out.extend_from_slice(
                &u16::try_from(bytes.len())
                    .map_err(|_| DecodeError::Invalid("chain entry too long"))?
                    .to_be_bytes(),
            );
            out.extend_from_slice(&bytes);
        }
        out.extend_from_slice(&(self.client.len() as u16).to_be_bytes());
        out.extend_from_slice(&self.client);
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, HandshakeError> {
        let mut r = Reader::new(bytes);
        let chain_seq = r.u64()?;
        let chain_head = r.array()?;
        let count = usize::from(r.u16()?);
        if count > MAX_HELLO_ENTRIES {
            return Err(DecodeError::TooLarge {
                what: "hello entries",
                len: count,
                max: MAX_HELLO_ENTRIES,
            }
            .into());
        }
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let len = usize::from(r.u16()?);
            entries.push(Entry::decode(r.bytes(len)?)?);
        }
        let len = usize::from(r.u16()?);
        if len > MAX_INFO {
            return Err(DecodeError::TooLarge {
                what: "client info",
                len,
                max: MAX_INFO,
            }
            .into());
        }
        let client = r.bytes(len)?.to_vec();
        r.finish()?;
        Ok(Self {
            chain_seq,
            chain_head,
            entries,
            client,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ReplyStatus {
    Accepted = 0,
    /// The agent is behind the device; resend with entries from `need_from`.
    NeedEntries = 1,
    /// No full device has vouched for the agent's chain head recently: send
    /// an attestation ([`crate::frame::Frame::Attest`]) and try again.
    Stale = 2,
}

/// The reply's payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyBody {
    pub status: ReplyStatus,
    pub chain_seq: u64,
    pub chain_head: [u8; 32],
    pub need_from: u64,
    pub server: Vec<u8>,
    /// The agent's entries after the device's view (at most [`MAX_HELLO_ENTRIES`]).
    pub entries: Vec<Entry>,
}

impl ReplyBody {
    /// `u8 status | u64 seq | head | u64 need_from | u16 len | server | u16 n | (u16 len | entry)×n`.
    pub fn encode(&self) -> Result<Vec<u8>, DecodeError> {
        if self.server.len() > MAX_INFO || self.entries.len() > MAX_HELLO_ENTRIES {
            return Err(DecodeError::TooLarge {
                what: "reply",
                len: self.server.len().max(self.entries.len()),
                max: MAX_INFO,
            });
        }
        let mut out = vec![self.status as u8];
        out.extend_from_slice(&self.chain_seq.to_be_bytes());
        out.extend_from_slice(&self.chain_head);
        out.extend_from_slice(&self.need_from.to_be_bytes());
        out.extend_from_slice(&(self.server.len() as u16).to_be_bytes());
        out.extend_from_slice(&self.server);
        out.extend_from_slice(&(self.entries.len() as u16).to_be_bytes());
        for entry in &self.entries {
            let bytes = entry.encode();
            out.extend_from_slice(
                &u16::try_from(bytes.len())
                    .map_err(|_| DecodeError::Invalid("chain entry too long"))?
                    .to_be_bytes(),
            );
            out.extend_from_slice(&bytes);
        }
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, HandshakeError> {
        let mut r = Reader::new(bytes);
        let status = match r.u8()? {
            0 => ReplyStatus::Accepted,
            1 => ReplyStatus::NeedEntries,
            2 => ReplyStatus::Stale,
            other => {
                return Err(DecodeError::Unknown {
                    what: "reply status",
                    value: other,
                }
                .into())
            }
        };
        let chain_seq = r.u64()?;
        let chain_head = r.array()?;
        let need_from = r.u64()?;
        let len = usize::from(r.u16()?);
        if len > MAX_INFO {
            return Err(DecodeError::TooLarge {
                what: "server info",
                len,
                max: MAX_INFO,
            }
            .into());
        }
        let server = r.bytes(len)?.to_vec();
        let count = usize::from(r.u16()?);
        if count > MAX_HELLO_ENTRIES {
            return Err(DecodeError::TooLarge {
                what: "reply entries",
                len: count,
                max: MAX_HELLO_ENTRIES,
            }
            .into());
        }
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let len = usize::from(r.u16()?);
            entries.push(Entry::decode(r.bytes(len)?)?);
        }
        r.finish()?;
        Ok(Self {
            status,
            chain_seq,
            chain_head,
            need_from,
            server,
            entries,
        })
    }
}

/// Everything an agent brings to a handshake.
pub struct AgentSide<'a> {
    pub chain: &'a mut ChainState,
    /// The agent's own live clock, persisted with its first sightings. Never
    /// [`ChainClock::history`]: [`accept`] refuses one.
    pub clock: &'a mut ChainClock,
    pub guard: &'a mut ReplayGuard,
    pub key: &'a DhKeypair,
    pub machine: &'a str,
    pub policy: RekeyPolicy,
    pub server_info: &'a [u8],
    /// The newest attestation the agent holds from each device, as relayed by
    /// the hub. Every channel requires one of them to make the view fresh.
    pub attestations: &'a [Attestation],
    /// [`FRESHNESS_MAX_AGE_MS`] unless a test says otherwise.
    pub freshness_max_age_ms: u64,
}

/// What [`accept`] decided. `applied` is always how many chain entries the
/// hello taught the agent — persist them whatever the outcome.
pub enum Accepted {
    /// Send `reply`; the channel is open.
    Open {
        reply: Vec<u8>,
        transport: Box<Transport>,
        hello: HelloBody,
        applied: usize,
    },
    /// Send `reply`, then close: the device must resend with older entries.
    NeedEntries { reply: Vec<u8>, from: u64, applied: usize },
    /// Send `reply`, then close: nobody has vouched for the agent's head
    /// recently (`newest_ms` is the newest usable attestation, if any).
    Stale {
        reply: Vec<u8>,
        newest_ms: Option<u64>,
        applied: usize,
    },
}

/// The agent's handling of a first message from device `device` (the id the
/// hub's `OPEN` named). See the module docs for the order of checks.
pub fn accept(agent: AgentSide<'_>, device: &str, message: &[u8], now_ms: u64) -> Result<Accepted, HandshakeError> {
    let binding = Binding {
        account: agent.chain.account().into(),
        device: device.into(),
        machine: agent.machine.into(),
    };
    let prologue = binding.prologue()?;
    accept_with_prologue(agent, device, &prologue, message, now_ms)
}

/// A device's first message on a direct path ([`crate::path`]): admitted by
/// exactly the rules of [`accept`] — chain catch-up, the device's place in the
/// chain, the freshness gate, the replay guard — under the direct prologue,
/// which binds it to the hub channel it grew from and to both DTLS
/// certificates. `bind` must name this agent's account and machine and the
/// device the hub channel was authorised for.
pub fn accept_direct(agent: AgentSide<'_>, device: &str, bind: &DirectBinding, message: &[u8], now_ms: u64) -> Result<Accepted, HandshakeError> {
    if bind.binding.account != agent.chain.account() || bind.binding.machine != agent.machine || bind.binding.device != device {
        return Err(HandshakeError::Unauthorized);
    }
    let prologue = bind.prologue()?;
    accept_with_prologue(agent, device, &prologue, message, now_ms)
}

fn accept_with_prologue(agent: AgentSide<'_>, device: &str, prologue: &[u8], message: &[u8], now_ms: u64) -> Result<Accepted, HandshakeError> {
    if !agent.clock.is_live() {
        return Err(HandshakeError::HistoryClock);
    }
    // One identity per device (chain v3): a host that also controls must
    // never be let in by itself — no reflection of its own first message.
    if device == agent.machine {
        return Err(HandshakeError::NotAccepted("a device does not control itself".into()));
    }
    agent.clock.set_now(now_ms);
    let mut responder = Responder::with_prologue(agent.key, prologue)?;
    let hello = responder.read_hello(message)?;
    let body = HelloBody::decode(&hello.payload)?;

    // A fork is an alarm every device hears about — but only from a device
    // whose key the chain names: the hub (or anyone) can send a hello under
    // any device id and chain view, and must not be able to raise alarms at
    // will (M2 re-review N3). From anyone else a conflict is a refusal.
    let named = |chain: &ChainState| chain.device_record(device).is_some_and(|d| d.info.dh_key == hello.initiator_static);
    let fork = |chain: &ChainState, seq: u64| -> HandshakeError {
        if named(chain) {
            ChainError::Fork { seq }.into()
        } else {
            HandshakeError::Unauthorized
        }
    };
    // Catch up from what the device knows; a conflict is a fork.
    let mut applied = 0;
    for entry in &body.entries {
        if entry.seq() > agent.chain.seq() + 1 {
            // A gap: the hello does not reach back far enough. Ask below.
            break;
        }
        match agent.chain.merge(std::slice::from_ref(entry), agent.clock) {
            Ok(n) => applied += n,
            // A recovered device's self-activation the agent may not accept yet:
            // it stays behind there, and says so below.
            Err(ChainError::TooEarly { .. }) => break,
            Err(ChainError::Fork { seq }) => return Err(fork(agent.chain, seq)),
            Err(e) => return Err(e.into()),
        }
    }
    let device_seq = body.chain_seq;
    let reply_with = |responder: Responder, status: ReplyStatus, need_from: u64, chain: &ChainState| -> Result<Vec<u8>, HandshakeError> {
        let body = ReplyBody {
            status,
            chain_seq: chain.seq(),
            chain_head: chain.head(),
            need_from,
            server: agent.server_info.to_vec(),
            entries: suffix_after(chain, device_seq),
        };
        Ok(responder.write_reply(&body.encode()?, agent.policy, now_ms)?.0)
    };
    match agent.chain.hash_at(body.chain_seq) {
        Some(hash) if hash != body.chain_head => return Err(fork(agent.chain, body.chain_seq)),
        Some(_) => {}
        None => {
            let from = agent.chain.seq() + 1;
            let reply = reply_with(responder, ReplyStatus::NeedEntries, from, agent.chain)?;
            return Ok(Accepted::NeedEntries { reply, from, applied });
        }
    }

    agent
        .chain
        .authorize_device(device, &hello.initiator_static)
        .map_err(|_| HandshakeError::Unauthorized)?;
    agent
        .chain
        .may_control(device, agent.machine)
        .map_err(|e| HandshakeError::NotAccepted(e.to_string()))?;
    match agent.chain.freshness(device, agent.attestations, now_ms, agent.freshness_max_age_ms)? {
        Freshness::Fresh => {}
        Freshness::Behind { .. } => {
            let from = agent.chain.seq() + 1;
            let reply = reply_with(responder, ReplyStatus::NeedEntries, from, agent.chain)?;
            return Ok(Accepted::NeedEntries { reply, from, applied });
        }
        Freshness::Stale { newest_ms } => {
            let reply = reply_with(responder, ReplyStatus::Stale, 0, agent.chain)?;
            return Ok(Accepted::Stale { reply, newest_ms, applied });
        }
    }
    agent.guard.check(device, &hello, now_ms)?;
    let reply = ReplyBody {
        status: ReplyStatus::Accepted,
        chain_seq: agent.chain.seq(),
        chain_head: agent.chain.head(),
        need_from: 0,
        server: agent.server_info.to_vec(),
        entries: suffix_after(agent.chain, device_seq),
    };
    let (reply, transport) = responder.write_reply(&reply.encode()?, agent.policy, now_ms)?;
    Ok(Accepted::Open {
        reply,
        transport: Box::new(transport),
        hello: body,
        applied,
    })
}

/// The entries of `chain` after seq `seen` (at most [`MAX_HELLO_ENTRIES`]).
fn suffix_after(chain: &ChainState, seen: u64) -> Vec<Entry> {
    let from = usize::try_from(seen.saturating_add(1)).unwrap_or(usize::MAX);
    chain.entries().iter().skip(from).take(MAX_HELLO_ENTRIES).cloned().collect()
}

/// The device's first message, carrying its chain view.
pub fn start(
    chain: &ChainState,
    device_key: &DhKeypair,
    device: &str,
    machine: &str,
    now_ms: u64,
    client: &[u8],
) -> Result<(Initiator, Vec<u8>), HandshakeError> {
    if device == machine {
        return Err(HandshakeError::NotAccepted("a device does not control itself".into()));
    }
    let machine_key = chain.machine(machine).ok_or(HandshakeError::Unauthorized)?.info.dh_key;
    let binding = Binding {
        account: chain.account().into(),
        device: device.into(),
        machine: machine.into(),
    };
    let mut initiator = Initiator::new(device_key, &machine_key, &binding)?;
    let hello = initiator.write_hello(now_ms, &HelloBody::for_chain(chain, client).encode()?)?;
    Ok((initiator, hello))
}

/// The device's first message on a direct path, under `bind`'s prologue
/// ([`crate::path::DirectBinding`]). The reply is read with [`finish`].
pub fn start_direct(
    chain: &ChainState,
    device_key: &DhKeypair,
    bind: &DirectBinding,
    now_ms: u64,
    client: &[u8],
) -> Result<(Initiator, Vec<u8>), HandshakeError> {
    if bind.binding.device == bind.binding.machine {
        return Err(HandshakeError::NotAccepted("a device does not control itself".into()));
    }
    let machine_key = chain.machine(&bind.binding.machine).ok_or(HandshakeError::Unauthorized)?.info.dh_key;
    if bind.binding.account != chain.account() {
        return Err(HandshakeError::Unauthorized);
    }
    let mut initiator = Initiator::with_prologue(device_key, &machine_key, &bind.prologue()?)?;
    let hello = initiator.write_hello(now_ms, &HelloBody::for_chain(chain, client).encode()?)?;
    Ok((initiator, hello))
}

/// The device's handling of the agent's reply: merge the entries the agent
/// sent that the device lacked (into `chain`, on the device's own `clock`),
/// check the views agree, then see whether it was let in. Entries are merged
/// whatever the status — a refusal can be exactly how a device learns it was
/// removed or that someone is recovering into the account. A fork between the
/// two views is an error to raise loudly.
pub fn finish(
    initiator: Initiator,
    chain: &mut ChainState,
    clock: &mut ChainClock,
    reply: &[u8],
    policy: RekeyPolicy,
    now_ms: u64,
) -> Result<(Transport, ReplyBody), HandshakeError> {
    let (transport, payload) = initiator.read_reply(reply, policy, now_ms)?;
    let body = ReplyBody::decode(&payload)?;
    clock.set_now(now_ms);
    for entry in &body.entries {
        if entry.seq() > chain.seq() + 1 {
            break;
        }
        match chain.merge(std::slice::from_ref(entry), clock) {
            Ok(_) => {}
            Err(e @ ChainError::Fork { .. }) => return Err(e.into()),
            // A self-activation this device may not accept yet, or an entry it
            // refuses: it stays where it is (the hub serves the rest).
            Err(_) => break,
        }
    }
    if let Some(hash) = chain.hash_at(body.chain_seq) {
        if hash != body.chain_head {
            return Err(ChainError::Fork { seq: body.chain_seq }.into());
        }
    }
    if body.status != ReplyStatus::Accepted {
        return Err(HandshakeError::Refused {
            status: body.status,
            need_from: body.need_from,
        });
    }
    Ok((transport, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::tests::{fixture, h, sample_chain};
    use crate::chain::{Action, Signer};

    /// An agent's side with a live clock and the given attestations.
    struct Agent {
        clock: ChainClock,
        guard: ReplayGuard,
        attestations: Vec<Attestation>,
    }

    impl Agent {
        fn new(attestations: Vec<Attestation>, per_device: usize) -> Self {
            Self {
                // Enrolled after the sample chain's five entries: its removal
                // (seq 4) is history, not one dev-0 made just now.
                clock: ChainClock::live(0, 5),
                guard: ReplayGuard::new(60_000, 60_000, per_device, 0),
                attestations,
            }
        }

        fn side<'a>(&'a mut self, chain: &'a mut ChainState, key: &'a DhKeypair) -> AgentSide<'a> {
            AgentSide {
                chain,
                clock: &mut self.clock,
                guard: &mut self.guard,
                key,
                machine: "m-1",
                policy: RekeyPolicy::default(),
                server_info: b"v0.3",
                attestations: &self.attestations,
                freshness_max_age_ms: FRESHNESS_MAX_AGE_MS,
            }
        }
    }

    /// dev-0 is the sample chain's only active full device, so its own
    /// attestation is what makes a view fresh.
    fn vouch(chain: &ChainState, f: &crate::chain::tests::Fixture, time_ms: u64) -> Vec<Attestation> {
        vec![Attestation::create(chain, "dev-0", &f.devices[0].0, time_ms)]
    }

    #[test]
    fn hello_and_reply_bodies_round_trip_and_refuse_junk() {
        let f = fixture();
        let chain = ChainState::verify(&sample_chain(&f), None, &mut h()).unwrap();
        let body = HelloBody::for_chain(&chain, b"web/1");
        assert_eq!(HelloBody::decode(&body.encode().unwrap()).unwrap(), body);
        let mut junk = body.encode().unwrap();
        junk.push(0);
        assert!(HelloBody::decode(&junk).is_err());
        assert!(
            HelloBody::decode(br#"{"rpc":"POST /api/terminals/1/input"}"#).is_err(),
            "a hello is not a request"
        );
        for status in [ReplyStatus::NeedEntries, ReplyStatus::Stale] {
            let reply = ReplyBody {
                status,
                chain_seq: 3,
                chain_head: [1; 32],
                need_from: 4,
                server: vec![],
                entries: vec![],
            };
            assert_eq!(ReplyBody::decode(&reply.encode().unwrap()).unwrap(), reply);
        }
    }

    /// §3.4 rule 10 (大小上限): [`MAX_HELLO_ENTRIES`] renewal pauses of the
    /// most bytes the chain takes fill a hello and a reply beside the most
    /// info, within what a handshake message carries; one fills a `CHAIN`
    /// frame; an entry past a u16 length is refused, never cut.
    #[test]
    fn the_largest_renewal_pauses_fit_every_transport() {
        use crate::chain::{Draft, PausePhase, MAX_PAUSE_ENTRY_LEN, VERSION_3};
        use crate::frame::Frame;
        let f = fixture();
        let entry = |ids: &[String]| {
            Draft {
                version: VERSION_3,
                account: "acct-1".into(),
                seq: 9,
                prev: [0; 32],
                created_ms: 1,
                signer: Signer::Device("dev-0".into()),
                action: Action::RenewalPause {
                    op: [1; 16],
                    phase: PausePhase::Start,
                    removals: vec![("m-1".into(), ids.to_vec())],
                },
            }
            .sign(&f.devices[0].0)
        };
        let mut ids: Vec<String> = Vec::new();
        while entry(&ids).encode().len() + 2 * 66 <= MAX_PAUSE_ENTRY_LEN {
            ids.push(format!("{:064}", ids.len()));
        }
        let rest = MAX_PAUSE_ENTRY_LEN - entry(&ids).encode().len() - 4;
        ids.push("a".repeat(rest / 2));
        ids.push("b".repeat(rest - rest / 2));
        let largest = entry(&ids);
        assert_eq!(largest.encode().len(), MAX_PAUSE_ENTRY_LEN);

        let hello = HelloBody {
            chain_seq: 9,
            chain_head: [2; 32],
            entries: vec![largest.clone(); MAX_HELLO_ENTRIES],
            client: vec![7; MAX_INFO],
        };
        let bytes = hello.encode().unwrap();
        assert!(bytes.len() <= crate::noise::MAX_HELLO_PAYLOAD, "{} bytes", bytes.len());
        assert_eq!(HelloBody::decode(&bytes).unwrap(), hello);
        let reply = ReplyBody {
            status: ReplyStatus::Accepted,
            chain_seq: 9,
            chain_head: [2; 32],
            need_from: 0,
            server: vec![7; MAX_INFO],
            entries: vec![largest.clone(); MAX_HELLO_ENTRIES],
        };
        let bytes = reply.encode().unwrap();
        assert!(bytes.len() <= crate::noise::MAX_HELLO_PAYLOAD, "{} bytes", bytes.len());
        assert_eq!(ReplyBody::decode(&bytes).unwrap(), reply);
        let frame = Frame::Chain {
            ch: 0,
            entries: vec![largest.encode()],
        };
        assert_eq!(Frame::decode(&frame.encode().unwrap()).unwrap(), frame);

        // Past a u16 length: refused, never cut.
        let mut huge = largest.clone();
        let Action::RenewalPause { removals, .. } = &mut huge.draft.action else { unreachable!() };
        removals[0].1 = (0..1100).map(|i| format!("{i:064}")).collect();
        assert!(huge.encode().len() > usize::from(u16::MAX));
        let hello = HelloBody {
            entries: vec![huge.clone()],
            ..hello
        };
        assert!(hello.encode().is_err());
        assert!(ReplyBody { entries: vec![huge.clone()], ..reply }.encode().is_err());
        assert!(Frame::Chain { ch: 0, entries: vec![huge.encode()] }.encode().is_err());
        // No count is clamped: one past u16::MAX does not encode at all.
        let mut wide = largest.draft.clone();
        let Action::RenewalPause { removals, .. } = &mut wide.action else { unreachable!() };
        *removals = vec![("m-1".into(), vec!["d".into()]); 0x10000];
        assert!(std::panic::catch_unwind(|| wide.encode()).is_err(), "a count past u16::MAX is refused, never cut");
    }

    #[test]
    fn a_device_and_an_agent_with_the_same_view_open_a_channel() {
        let f = fixture();
        let entries = sample_chain(&f);
        let device_view = ChainState::verify(&entries, None, &mut h()).unwrap();
        let mut agent_view = device_view.clone();
        let mut agent = Agent::new(vouch(&device_view, &f, 900), 8);
        let (initiator, hello) = start(&device_view, &f.devices[0].1, "dev-0", "m-1", 1_000, b"web").unwrap();
        let Accepted::Open { reply, mut transport, .. } = accept(agent.side(&mut agent_view, &f.machine.1), "dev-0", &hello, 1_000).unwrap() else {
            panic!("accepted")
        };
        let (mut device, body) = finish(
            initiator,
            &mut device_view.clone(),
            &mut ChainClock::live(0, 5),
            &reply,
            RekeyPolicy::default(),
            1_000,
        )
        .unwrap();
        assert_eq!(body.server, b"v0.3");
        assert_eq!(transport.open(&device.seal(1_001, b"now a request").unwrap()).unwrap(), b"now a request");
        // The same first message again is a replay.
        assert!(matches!(
            accept(agent.side(&mut agent_view, &f.machine.1), "dev-0", &hello, 1_500),
            Err(HandshakeError::Channel(ChannelError::Replay))
        ));
    }

    #[test]
    fn an_agent_refuses_to_verify_with_a_history_clock() {
        let f = fixture();
        let device_view = ChainState::verify(&sample_chain(&f), None, &mut h()).unwrap();
        let mut agent_view = device_view.clone();
        let mut agent = Agent::new(vouch(&device_view, &f, 900), 8);
        agent.clock = ChainClock::history();
        let (_, hello) = start(&device_view, &f.devices[0].1, "dev-0", "m-1", 1_000, b"").unwrap();
        assert_eq!(
            accept(agent.side(&mut agent_view, &f.machine.1), "dev-0", &hello, 1_000).err(),
            Some(HandshakeError::HistoryClock)
        );
    }

    #[test]
    fn every_channel_needs_a_recent_attestation_of_the_agents_head() {
        let f = fixture();
        let device_view = ChainState::verify(&sample_chain(&f), None, &mut h()).unwrap();
        for (attestations, now) in [(vec![], 1_000), (vouch(&device_view, &f, 1_000), 1_000 + FRESHNESS_MAX_AGE_MS + 1)] {
            let mut agent_view = device_view.clone();
            let mut agent = Agent::new(attestations, 8);
            let (initiator, hello) = start(&device_view, &f.devices[0].1, "dev-0", "m-1", now, b"").unwrap();
            let Accepted::Stale { reply, applied, .. } = accept(agent.side(&mut agent_view, &f.machine.1), "dev-0", &hello, now).unwrap() else {
                panic!("a view nobody vouched for is stale")
            };
            assert_eq!(applied, 0);
            assert!(agent.guard.snapshot().is_empty(), "a refused hello does not spend the guard");
            assert_eq!(
                finish(
                    initiator,
                    &mut device_view.clone(),
                    &mut ChainClock::live(0, 5),
                    &reply,
                    RekeyPolicy::default(),
                    now
                )
                .err(),
                Some(HandshakeError::Refused {
                    status: ReplyStatus::Stale,
                    need_from: 0
                })
            );
        }
    }

    #[test]
    fn a_lagging_agent_catches_up_from_the_device_or_asks_for_more() {
        let f = fixture();
        let entries = sample_chain(&f);
        let device_view = ChainState::verify(&entries, None, &mut h()).unwrap();
        // Two entries behind: the hello's suffix covers them.
        let mut agent_view = ChainState::verify(&entries[..3], None, &mut h()).unwrap();
        let mut agent = Agent::new(vouch(&device_view, &f, 900), 8);
        let (_, hello) = start(&device_view, &f.devices[0].1, "dev-0", "m-1", 1_000, b"").unwrap();
        let Accepted::Open { applied, .. } = accept(agent.side(&mut agent_view, &f.machine.1), "dev-0", &hello, 1_000).unwrap() else {
            panic!("caught up")
        };
        assert_eq!(applied, 2);
        assert_eq!(agent_view.checkpoint(), device_view.checkpoint());
        assert_eq!(
            agent.clock.first_seen().keys().copied().collect::<Vec<_>>(),
            vec![3, 4],
            "the chain recorded when the agent first saw each entry"
        );

        // Further behind than a hello reaches: grow the device's chain past the suffix.
        let mut long = device_view.clone();
        for i in 0..MAX_HELLO_ENTRIES {
            let id = format!("m-x{i}");
            let m = crate::chain::MachineInfo {
                id: id.clone(),
                sign_key: f.machine.0.verifying_key().to_bytes(),
                dh_key: f.machine.1.public(),
            };
            long.apply(
                &long.draft(Signer::Device("dev-0".into()), Action::AddMachine(m), 0).sign(&f.devices[0].0),
                &mut h(),
            )
            .unwrap();
        }
        let mut agent_view = ChainState::verify(&entries[..2], None, &mut h()).unwrap();
        let mut agent = Agent::new(vouch(&long, &f, 1_900), 8);
        let (initiator, hello) = start(&long, &f.devices[0].1, "dev-0", "m-1", 2_000, b"").unwrap();
        let Accepted::NeedEntries { reply, from, applied } = accept(agent.side(&mut agent_view, &f.machine.1), "dev-0", &hello, 2_000).unwrap()
        else {
            panic!("needs entries")
        };
        assert_eq!((from, applied), (2, 0));
        assert_eq!(
            finish(
                initiator,
                &mut long.clone(),
                &mut ChainClock::live(0, 5),
                &reply,
                RekeyPolicy::default(),
                2_000
            )
            .err(),
            Some(HandshakeError::Refused {
                status: ReplyStatus::NeedEntries,
                need_from: 2
            })
        );
    }

    #[test]
    fn need_entries_reports_what_the_hello_already_taught_the_agent() {
        // The hello covers the agent's gap but the device attests to a head
        // beyond even that (an attestation relayed from the hub): the agent
        // keeps what it learnt and still asks for more.
        let f = fixture();
        let entries = sample_chain(&f);
        let device_view = ChainState::verify(&entries, None, &mut h()).unwrap();
        let mut ahead = device_view.clone();
        ahead
            .apply(
                &ahead
                    .draft(Signer::Device("dev-0".into()), Action::RemoveMachine("m-1".into()), 9)
                    .sign(&f.devices[0].0),
                &mut h(),
            )
            .unwrap();
        let mut agent_view = ChainState::verify(&entries[..3], None, &mut h()).unwrap();
        let mut agent = Agent::new(vouch(&ahead, &f, 900), 8);
        let (_, hello) = start(&device_view, &f.devices[0].1, "dev-0", "m-1", 1_000, b"").unwrap();
        let Accepted::NeedEntries { from, applied, .. } = accept(agent.side(&mut agent_view, &f.machine.1), "dev-0", &hello, 1_000).unwrap() else {
            panic!("behind the attestation")
        };
        assert_eq!((from, applied), (5, 2));
        assert_eq!(agent_view.seq(), 4, "the two entries from the hello stay applied");
    }

    /// M2 re-review N3: the hub can send a hello under any device id with any
    /// chain view; a conflict it fakes is a refusal, not an alarm to every
    /// device. Only a hello under a key the chain names raises one.
    #[test]
    fn only_a_device_the_chain_names_can_raise_a_fork_alarm() {
        let f = fixture();
        let entries = sample_chain(&f);
        let mut agent_view = ChainState::verify(&entries, None, &mut h()).unwrap();
        // A view that disagrees at seq 4, under a key no device has.
        let mut forged = ChainState::verify(&entries[..4], None, &mut h()).unwrap();
        forged
            .apply(
                &forged
                    .draft(
                        Signer::Device("dev-0".into()),
                        Action::AddDevice(crate::chain::tests::device_info(&f, 3, true)),
                        9,
                    )
                    .sign(&f.devices[0].0),
                &mut h(),
            )
            .unwrap();
        let stranger = DhKeypair::from_secret(crate::testing::bytes32(77));
        let mut agent = Agent::new(vec![], 8);
        for id in ["dev-0", "dev-9"] {
            let (_, hello) = start(&forged, &stranger, id, "m-1", 1_000, b"").unwrap();
            let err = accept(agent.side(&mut agent_view, &f.machine.1), id, &hello, 1_000).err().unwrap();
            assert_eq!(err, HandshakeError::Unauthorized, "{id}");
            assert!(!err.is_alarm());
        }
        // From dev-0's own key, the same conflict is the alarm it is.
        let (_, hello) = start(&forged, &f.devices[0].1, "dev-0", "m-1", 1_000, b"").unwrap();
        assert!(accept(agent.side(&mut agent_view, &f.machine.1), "dev-0", &hello, 1_000)
            .err()
            .unwrap()
            .is_alarm());
    }

    #[test]
    fn a_fork_between_the_views_is_an_alarm_on_both_sides() {
        let f = fixture();
        let entries = sample_chain(&f);
        let device_view = ChainState::verify(&entries, None, &mut h()).unwrap();
        let mut forked = ChainState::verify(&entries[..4], None, &mut h()).unwrap();
        forked
            .apply(
                &forked
                    .draft(Signer::Device("dev-0".into()), Action::RemoveMachine("m-1".into()), 9)
                    .sign(&f.devices[0].0),
                &mut h(),
            )
            .unwrap();
        // The agent's view diverges from the device's at seq 4.
        let mut agent = Agent::new(vec![], 8);
        let (_, hello) = start(&device_view, &f.devices[0].1, "dev-0", "m-1", 1_000, b"").unwrap();
        let err = accept(agent.side(&mut forked, &f.machine.1), "dev-0", &hello, 1_000).err().unwrap();
        assert!(err.is_alarm(), "{err:?}");
    }

    #[test]
    fn an_unauthorised_device_gets_no_reply_and_no_replay_slot() {
        let f = fixture();
        let entries = sample_chain(&f);
        let view = ChainState::verify(&entries, None, &mut h()).unwrap();
        let mut agent = Agent::new(vouch(&view, &f, 900), 1);
        // dev-1 was removed at seq 4; it still has its key and an old view.
        let stale = ChainState::verify(&entries[..4], None, &mut h()).unwrap();
        let (_, hello) = start(&stale, &f.devices[1].1, "dev-1", "m-1", 1_000, b"").unwrap();
        let mut agent_view = view.clone();
        assert_eq!(
            accept(agent.side(&mut agent_view, &f.machine.1), "dev-1", &hello, 1_000).err(),
            Some(HandshakeError::Unauthorized)
        );
        assert!(agent.guard.snapshot().is_empty(), "the guard was never touched");
    }

    /// The hub channel's transports for dev-0 and a direct binding grown from it.
    fn relay_and_binding(f: &crate::chain::tests::Fixture, view: &ChainState, agent: &mut Agent) -> (Transport, Transport, DirectBinding) {
        let mut agent_view = view.clone();
        let (initiator, hello) = start(view, &f.devices[0].1, "dev-0", "m-1", 1_000, b"web/1 paths/1").unwrap();
        let Accepted::Open { reply, transport, hello, .. } = accept(agent.side(&mut agent_view, &f.machine.1), "dev-0", &hello, 1_000).unwrap()
        else {
            panic!("accepted")
        };
        assert!(crate::path::client_wants_paths(&hello.client));
        let (device, _) = finish(
            initiator,
            &mut view.clone(),
            &mut ChainClock::live(0, 5),
            &reply,
            RekeyPolicy::default(),
            1_000,
        )
        .unwrap();
        let bind = DirectBinding {
            binding: Binding {
                account: view.account().into(),
                device: "dev-0".into(),
                machine: "m-1".into(),
            },
            relay_hash: device.handshake_hash(),
            nonce: [7; 32],
            device_fp: [8; 32],
            agent_fp: [9; 32],
        };
        (device, *transport, bind)
    }

    #[test]
    fn a_direct_path_is_admitted_like_a_hub_channel_and_has_its_own_keys() {
        let f = fixture();
        let view = ChainState::verify(&sample_chain(&f), None, &mut h()).unwrap();
        let mut agent = Agent::new(vouch(&view, &f, 900), 8);
        let (mut relay_device, mut relay_agent, bind) = relay_and_binding(&f, &view, &mut agent);
        assert_eq!(relay_device.handshake_hash(), relay_agent.handshake_hash());
        let (initiator, hello) = start_direct(&view, &f.devices[0].1, &bind, 2_000, b"web/1 paths/1").unwrap();
        let mut agent_view = view.clone();
        let Accepted::Open { reply, mut transport, .. } =
            accept_direct(agent.side(&mut agent_view, &f.machine.1), "dev-0", &bind, &hello, 2_000).unwrap()
        else {
            panic!("the direct handshake is admitted")
        };
        let (mut direct, _) = finish(
            initiator,
            &mut view.clone(),
            &mut ChainClock::live(0, 5),
            &reply,
            RekeyPolicy::default(),
            2_000,
        )
        .unwrap();
        assert_ne!(direct.handshake_hash(), relay_device.handshake_hash());
        assert_eq!(
            transport.open(&direct.seal(2_001, b"over the direct path").unwrap()).unwrap(),
            b"over the direct path"
        );
        // No key is shared between the paths: a message sealed for one does not open on the other.
        let sealed = relay_device.seal(2_002, b"hub").unwrap();
        assert!(transport.open(&sealed).is_err());
        assert_eq!(relay_agent.open(&sealed).unwrap(), b"hub", "the hub channel still opens its own");
        // The same direct hello again is a replay.
        assert!(matches!(
            accept_direct(agent.side(&mut agent_view, &f.machine.1), "dev-0", &bind, &hello, 2_100),
            Err(HandshakeError::Channel(ChannelError::Replay))
        ));
    }

    #[test]
    fn a_direct_handshake_bound_to_another_certificate_or_channel_fails() {
        let f = fixture();
        let view = ChainState::verify(&sample_chain(&f), None, &mut h()).unwrap();
        let mut agent = Agent::new(vouch(&view, &f, 900), 64);
        let (_, _, bind) = relay_and_binding(&f, &view, &mut agent);
        // Each side's view of the binding differs in one part: the device saw a
        // different agent certificate (a swapped SDP), the agent a different
        // device certificate, another attempt's nonce, another hub channel.
        let tampered = [
            DirectBinding {
                agent_fp: [0xee; 32],
                ..bind.clone()
            },
            DirectBinding {
                device_fp: [0xee; 32],
                ..bind.clone()
            },
            DirectBinding {
                nonce: [0xee; 32],
                ..bind.clone()
            },
            DirectBinding {
                relay_hash: [0xee; 32],
                ..bind.clone()
            },
        ];
        for (i, device_side) in tampered.iter().enumerate() {
            let (_, hello) = start_direct(&view, &f.devices[0].1, device_side, 2_000 + i as u64, b"").unwrap();
            let mut agent_view = view.clone();
            assert_eq!(
                accept_direct(agent.side(&mut agent_view, &f.machine.1), "dev-0", &bind, &hello, 2_000 + i as u64).err(),
                Some(HandshakeError::Channel(ChannelError::Decrypt)),
                "{device_side:?}"
            );
        }
        // A binding that names another device than the one the hub channel was for.
        let (_, hello) = start_direct(&view, &f.devices[0].1, &bind, 3_000, b"").unwrap();
        let mut agent_view = view.clone();
        assert_eq!(
            accept_direct(agent.side(&mut agent_view, &f.machine.1), "dev-2", &bind, &hello, 3_000).err(),
            Some(HandshakeError::Unauthorized)
        );
        // The hub channel's own prologue does not open a direct hello either.
        let mut agent_view = view.clone();
        assert!(accept(agent.side(&mut agent_view, &f.machine.1), "dev-0", &hello, 3_000).is_err());
    }

    #[test]
    fn a_removed_or_stale_device_gets_no_direct_path() {
        let f = fixture();
        let entries = sample_chain(&f);
        let view = ChainState::verify(&entries, None, &mut h()).unwrap();
        // dev-1 was removed at seq 4: with its old view and its key it asks for a direct path.
        let old = ChainState::verify(&entries[..4], None, &mut h()).unwrap();
        let bind = DirectBinding {
            binding: Binding {
                account: view.account().into(),
                device: "dev-1".into(),
                machine: "m-1".into(),
            },
            relay_hash: [1; 32],
            nonce: [2; 32],
            device_fp: [3; 32],
            agent_fp: [4; 32],
        };
        let mut agent = Agent::new(vouch(&view, &f, 900), 8);
        let (_, hello) = start_direct(&old, &f.devices[1].1, &bind, 1_000, b"").unwrap();
        let mut agent_view = view.clone();
        assert_eq!(
            accept_direct(agent.side(&mut agent_view, &f.machine.1), "dev-1", &bind, &hello, 1_000).err(),
            Some(HandshakeError::Unauthorized)
        );
        assert!(agent.guard.snapshot().is_empty());

        // A current device, but nobody has vouched for the agent's head lately.
        let bind0 = DirectBinding {
            binding: Binding {
                device: "dev-0".into(),
                ..bind.binding.clone()
            },
            ..bind
        };
        for (attestations, now) in [(vec![], 1_000), (vouch(&view, &f, 1_000), 1_000 + FRESHNESS_MAX_AGE_MS + 1)] {
            let mut agent = Agent::new(attestations, 8);
            let (initiator, hello) = start_direct(&view, &f.devices[0].1, &bind0, now, b"").unwrap();
            let mut agent_view = view.clone();
            let Accepted::Stale { reply, .. } = accept_direct(agent.side(&mut agent_view, &f.machine.1), "dev-0", &bind0, &hello, now).unwrap()
            else {
                panic!("stale")
            };
            assert!(matches!(
                finish(
                    initiator,
                    &mut view.clone(),
                    &mut ChainClock::live(0, 5),
                    &reply,
                    RekeyPolicy::default(),
                    now
                ),
                Err(HandshakeError::Refused {
                    status: ReplyStatus::Stale,
                    ..
                })
            ));
        }
    }
}
