//! Chain v3's security checklist (`docs/local-aggregator.md` §3.6), item by
//! item: each test runs the attack the item names against the v3 rules and
//! asserts that it fails, and that the honest path next to it works. The
//! numbers in the test names are the checklist's.
//!
//! Fixture: the v2 sample chain (`dev-0` full → admin, `dev-2` limited → a
//! plain device, `m-1` a machine → a host that is not an admin, `dev-1`
//! removed), extended with v3 entries where a test needs them.

use ed25519_dalek::SigningKey;

use crate::chain::tests::{fixture, h, sample_chain, Fixture};
use crate::chain::*;
use crate::handshake::{self, Accepted, AgentSide, HandshakeError};
use crate::keys::DhKeypair;
use crate::noise::{RekeyPolicy, ReplayGuard};
use crate::testing::bytes32;

const DAY: u64 = 24 * 60 * 60 * 1000;
const HOUR: u64 = 60 * 60 * 1000;

/// Extra devices for v3 tests: index `i` has seed `700 + 2i` / `701 + 2i`.
pub(crate) fn extra(i: u64) -> (SigningKey, DhKeypair) {
    (
        SigningKey::from_bytes(&bytes32(700 + 2 * i)),
        DhKeypair::from_secret(bytes32(701 + 2 * i)),
    )
}

pub(crate) fn dev3(id: &str, keys: &(SigningKey, DhKeypair), host: bool, admin: bool) -> DeviceV3 {
    DeviceV3 {
        id: id.into(),
        sign_key: keys.0.verifying_key().to_bytes(),
        dh_key: keys.1.public(),
        flags: Flags { host, admin },
    }
}

fn by0() -> Signer {
    Signer::Device("dev-0".into())
}

/// Sign `action` as `signer` with `key` and apply it.
fn push(state: &mut ChainState, signer: Signer, key: &SigningKey, action: Action) -> Result<Entry, ChainError> {
    let entry = state.draft(signer, action, 10_000 + state.seq()).sign(key);
    state.apply(&entry, &mut h())?;
    Ok(entry)
}

fn try_push(state: &ChainState, signer: Signer, key: &SigningKey, action: Action) -> Result<(), ChainError> {
    let entry = state.draft(signer, action, 10_000 + state.seq()).sign(key);
    state.clone().apply(&entry, &mut h())
}

pub(crate) fn mk_commit(host: &str, epoch: u32, key: &[u8; 32]) -> KeyCommits {
    KeyCommits {
        user: None,
        machines: vec![(
            host.into(),
            KeyCommit {
                epoch,
                hash: machine_key_commit("acct-1", host, epoch, key),
            },
        )],
    }
}

/// The sample chain plus, from dev-0: a second host `h-2` (not an admin) with
/// its first machine key — the first v3 entries.
pub(crate) fn with_second_host(f: &Fixture) -> (ChainState, Vec<Entry>) {
    let mut entries = sample_chain(f);
    let mut state = ChainState::verify(&entries, None, &mut h()).unwrap();
    let key = &f.devices[0].0;
    entries.push(push(&mut state, by0(), key, Action::AddDevice3(dev3("h-2", &extra(0), true, false))).unwrap());
    entries.push(push(&mut state, by0(), key, Action::KeyEpochs(mk_commit("h-2", 0, &[0x22; 32]))).unwrap());
    (state, entries)
}

pub(crate) struct Pak {
    pub signing: SigningKey,
    pub slot: PasskeySlot,
}

/// A passkey slot `p-1` for acct-1 (its proof over the fixture's `AK`).
pub(crate) fn pak(f: &Fixture, slot: &str, seed: u64, cred: u8, self_activate: bool) -> Pak {
    let signing = SigningKey::from_bytes(&bytes32(seed));
    let key = signing.verifying_key().to_bytes();
    let ak = f.ak.verifying_key().to_bytes();
    let (cred_hash, salt_nonce) = ([cred; 32], [cred ^ 0x55; 32]);
    let proof = crate::sig::sign(
        &signing,
        crate::sig::PASSKEY_PROOF,
        &passkey_proof_body("acct-1", &ak, &key, slot, &cred_hash, &salt_nonce),
    );
    Pak {
        signing,
        slot: PasskeySlot {
            slot: slot.into(),
            key,
            proof,
            cred_hash,
            salt_nonce,
            grants: 0,
            self_activate,
        },
    }
}

// ---------------------------------------------------------------- 1. the v2 mapping

/// Item 1: a v2 chain maps onto the v3 state deterministically — a full
/// device is an admin, a limited one a plain device, a machine a host that is
/// not an admin, a recovered device a pending admin — and the v2 views
/// (`device`, `machine`) say what they said before. (Byte-identical replay of
/// the v2 vectors is `vectors::rust_vectors_are_current`; the same mapping in
/// TypeScript is checked against `chain.v3.state` there.)
#[test]
fn c1_a_v2_chain_maps_onto_devices_with_flags() {
    let f = fixture();
    let state = ChainState::verify(&sample_chain(&f), None, &mut h()).unwrap();
    let flags = |id: &str| state.device_record(id).map(|d| (d.host, d.admin, d.status));
    assert_eq!(flags("dev-0"), Some((false, true, DeviceStatus::Active)));
    assert_eq!(flags("dev-1"), Some((false, true, DeviceStatus::Removed)));
    assert_eq!(flags("dev-2"), Some((false, false, DeviceStatus::Active)));
    assert_eq!(flags("m-1"), Some((true, false, DeviceStatus::Active)));
    assert!(state.machine("m-1").is_some() && state.machine("dev-0").is_none());
    assert!(state.device("m-1").is_some(), "a host is a device too");
    assert!(state.full_device_key("m-1").is_err(), "but not an admin");
    assert!(!state.is_v3());
    // A recovered device is a pending admin.
    let mut s = state.clone();
    let join = s
        .draft(Signer::Recovery, Action::RecoveryJoin(crate::chain::tests::recovered_device(&f)), 6_000)
        .sign(&f.recovery.signing);
    s.apply(&join, &mut h()).unwrap();
    let r = s.device_record("dev-r").unwrap();
    assert!(r.admin && !r.host && r.via_recovery && matches!(r.status, DeviceStatus::Pending { .. }));
}

// ---------------------------------------------------------------- 2. version confusion

/// Item 2: a v3 entry cannot be read as v2 nor the other way round (the
/// version byte is signed, and each version has its own actions and signers);
/// unknown flag bits, non-canonical booleans and lists are refused; and no v2
/// entry follows the first v3 one.
#[test]
fn c2_versions_cannot_be_confused_and_encodings_are_canonical() {
    let f = fixture();
    let (state, entries) = with_second_host(&f);
    let add = entries[5].clone();
    let keyed = entries[6].clone();
    assert!(add.is_v3() && keyed.is_v3());
    // AddDevice3 relabelled as v2: kind 12 does not exist there.
    let mut bytes = add.encode();
    bytes[0] = VERSION;
    assert!(matches!(Entry::decode(&bytes), Err(ChainError::Decode(_))));
    // A shared kind (KeyEpochs) relabelled from 3 to 2: it decodes, and is refused.
    let mut bytes = keyed.encode();
    bytes[0] = VERSION;
    let relabelled = Entry::decode(&bytes).unwrap();
    let base = ChainState::verify(&entries[..6], None, &mut h()).unwrap();
    assert!(base.clone().apply(&relabelled, &mut h()).is_err());
    // And from 2 to 3 on a v2 chain: the signature covered the "2".
    let mut v2 = ChainState::verify(&sample_chain(&f), None, &mut h()).unwrap();
    let keyed2 = v2.draft(by0(), Action::KeyEpochs(mk_commit("m-1", 0, &[7; 32])), 1).sign(&f.devices[0].0);
    assert_eq!(keyed2.draft.version, VERSION, "a v2 chain writes shared kinds as v2");
    let mut bytes = keyed2.encode();
    bytes[0] = VERSION_3;
    let relabelled = Entry::decode(&bytes).unwrap();
    assert!(matches!(v2.clone().apply(&relabelled, &mut h()), Err(ChainError::BadSignature { .. })));
    v2.apply(&keyed2, &mut h()).unwrap();
    // A v2 AddDevice relabelled as v3: kind 2 does not exist there.
    let mut bytes = sample_chain(&f)[1].encode();
    bytes[0] = VERSION_3;
    assert!(matches!(Entry::decode(&bytes), Err(ChainError::Decode(_))));
    // Unknown versions.
    for v in [0, 1, 4, 255] {
        let mut bytes = add.encode();
        bytes[0] = v;
        assert_eq!(Entry::decode(&bytes), Err(ChainError::Version(v)));
    }
    // A passkey signer does not exist in v2.
    let pk = pak(&f, "p-1", 800, 1, true);
    let mut v2_by_pak = state.draft(Signer::Passkey("p-1".into()), Action::ApproveDevice("x".into()), 1);
    v2_by_pak.version = VERSION;
    assert!(matches!(Entry::decode(&v2_by_pak.sign(&pk.signing).encode()), Err(ChainError::Decode(_))));
    // Flag bits beyond host and admin; a boolean that is not 0 or 1.
    let mut bytes = add.encode();
    let flags_at = bytes.len() - 64 - 1;
    assert_eq!(bytes[flags_at], Flags::HOST);
    bytes[flags_at] = 0b100 | Flags::HOST;
    assert!(matches!(Entry::decode(&bytes), Err(ChainError::Decode(_))));
    let set = state
        .draft(
            by0(),
            Action::SetAdmin {
                id: "dev-2".into(),
                admin: true,
            },
            1,
        )
        .sign(&f.devices[0].0);
    let mut bytes = set.encode();
    let bool_at = bytes.len() - 64 - 1;
    bytes[bool_at] = 2;
    assert!(matches!(Entry::decode(&bytes), Err(ChainError::Decode(_))));
    // An accept list out of order, repeated, or empty.
    for ids in [vec!["dev-2", "dev-0"], vec!["dev-0", "dev-0"], vec![]] {
        let e = state
            .draft(
                by0(),
                Action::SetAccept {
                    host: "m-1".into(),
                    from: Accept::Devices(ids.iter().map(|s| s.to_string()).collect()),
                },
                1,
            )
            .sign(&f.devices[0].0);
        assert!(matches!(Entry::decode(&e.encode()), Err(ChainError::Decode(_))), "{ids:?}");
    }
    // A draft whose version does not fit its action.
    let mut wrong = state.draft(
        by0(),
        Action::SetAdmin {
            id: "dev-2".into(),
            admin: true,
        },
        1,
    );
    wrong.version = VERSION;
    assert_eq!(
        state.clone().apply(&wrong.sign(&f.devices[0].0), &mut h()),
        Err(ChainError::Version(VERSION))
    );
    // After the first v3 entry, no v2 entry: neither a v2-only action...
    assert!(state.is_v3() && state.v3_from() == Some(5));
    assert!(matches!(
        try_push(&state, by0(), &f.devices[0].0, Action::RemoveMachine("m-1".into())),
        Err(ChainError::Version(_)) | Err(ChainError::NotAllowed { .. })
    ));
    // ...nor a shared one written as v2.
    let mut v2 = state.draft(by0(), Action::KeyEpochs(mk_commit("h-2", 1, &[1; 32])), 1);
    v2.version = VERSION;
    assert!(matches!(
        state.clone().apply(&v2.sign(&f.devices[0].0), &mut h()),
        Err(ChainError::NotAllowed { .. })
    ));
    // Every v3 entry round-trips.
    for e in &entries {
        assert_eq!(&Entry::decode(&e.encode()).unwrap(), e);
    }
}

// ---------------------------------------------------------------- 3. who signs

/// Item 3: anything signed by a device that is not an active admin is
/// refused — a plain device, a host that is not an admin, a removed or
/// pending device; `RSK` signs only its two entries, `AK` only genesis
/// (`Genesis3` included), a passkey slot only its own two.
#[test]
fn c3_only_active_admins_sign_and_the_special_keys_stay_in_their_lanes() {
    let f = fixture();
    let (state, _) = with_second_host(&f);
    let h2 = extra(0);
    let any = || Action::AddDevice3(dev3("d-new", &extra(5), false, true));
    // A plain device, a non-admin host (v3 and v2 views), a removed device.
    assert!(matches!(
        try_push(&state, Signer::Device("dev-2".into()), &f.devices[2].0, any()),
        Err(ChainError::NotAllowed { .. })
    ));
    assert!(matches!(
        try_push(&state, Signer::Device("h-2".into()), &h2.0, any()),
        Err(ChainError::NotAllowed { .. })
    ));
    assert!(matches!(
        try_push(
            &state,
            Signer::Device("m-1".into()),
            &f.machine.0,
            Action::SetAdmin {
                id: "m-1".into(),
                admin: true
            }
        ),
        Err(ChainError::NotAllowed { .. })
    ));
    assert!(matches!(
        try_push(&state, Signer::Device("dev-1".into()), &f.devices[1].0, any()),
        Err(ChainError::UnknownId { .. })
    ));
    // In a v2 chain a machine is not a signer at all.
    let v2 = ChainState::verify(&sample_chain(&f), None, &mut h()).unwrap();
    assert!(matches!(
        try_push(&v2, Signer::Device("m-1".into()), &f.machine.0, Action::RemoveDevice("dev-2".into())),
        Err(ChainError::UnknownId { .. })
    ));
    // A pending (recovered) admin signs nothing yet.
    let mut pending = state.clone();
    push(
        &mut pending,
        Signer::Recovery,
        &f.recovery.signing,
        Action::RecoveryJoin3(dev3("dev-r", &extra(6), false, true)),
    )
    .unwrap();
    assert!(matches!(
        try_push(&pending, Signer::Device("dev-r".into()), &extra(6).0, any()),
        Err(ChainError::UnknownId { .. })
    ));
    // A key that is not the claimed signer's.
    assert!(matches!(
        try_push(&state, by0(), &f.devices[2].0, any()),
        Err(ChainError::BadSignature { .. })
    ));
    // RSK: only RecoveryJoin(3) and ActivateRecovered.
    for action in [
        any(),
        Action::SetAdmin {
            id: "dev-2".into(),
            admin: true,
        },
        Action::RemoveDevice3("dev-2".into()),
    ] {
        assert!(matches!(
            try_push(&state, Signer::Recovery, &f.recovery.signing, action),
            Err(ChainError::NotAllowed { .. })
        ));
    }
    // A recovered device must be an admin (it may be a host).
    assert!(matches!(
        try_push(
            &state,
            Signer::Recovery,
            &f.recovery.signing,
            Action::RecoveryJoin3(dev3("dev-r", &extra(6), true, false))
        ),
        Err(ChainError::NotAllowed { .. })
    ));
    // ...and, while hosts cannot be admins (review D1), not a host either.
    assert_eq!(
        try_push(
            &state,
            Signer::Recovery,
            &f.recovery.signing,
            Action::RecoveryJoin3(dev3("dev-r", &extra(6), true, true)),
        ),
        Err(ChainError::NotAllowed {
            seq: 7,
            reason: rules::HOST_NOT_ADMIN
        })
    );
    try_push(
        &state,
        Signer::Recovery,
        &f.recovery.signing,
        Action::RecoveryJoin3(dev3("dev-r", &extra(6), false, true)),
    )
    .unwrap();
    // AK: genesis only, and no second genesis of either version.
    assert!(matches!(
        try_push(&state, Signer::Account, &f.ak, any()),
        Err(ChainError::NotAllowed { .. })
    ));
    assert!(matches!(
        try_push(
            &state,
            Signer::Account,
            &f.ak,
            Action::Genesis3 {
                account_key: f.ak.verifying_key().to_bytes(),
                device: dev3("dev-g", &extra(7), false, true),
                recovery_key: None,
            }
        ),
        Err(ChainError::LateGenesis { .. })
    ));
    // A v3 genesis needs an admin device.
    let ak = f.ak.verifying_key().to_bytes();
    let g = genesis3_draft("acct-3", ak, dev3("dev-g", &extra(7), true, false), None, 1).sign(&f.ak);
    assert!(matches!(ChainState::genesis(&g, None), Err(ChainError::NotAllowed { .. })));
    let g = genesis3_draft("acct-3", ak, dev3("dev-g", &extra(7), false, true), None, 1).sign(&f.ak);
    let g3 = ChainState::genesis(&g, None).unwrap();
    assert!(g3.is_v3() && g3.admin("dev-g").is_some());
}

// ---------------------------------------------------------------- 4. no self-grants, the last admin, batches

/// Item 4: no device grants itself admin or host; the last admin can be
/// neither demoted nor removed; within a batch an admin that demoted itself
/// signs nothing more.
#[test]
fn c4_no_self_grants_the_last_admin_stays_and_a_batch_applies_in_order() {
    let f = fixture();
    let (state, _) = with_second_host(&f);
    let k0 = &f.devices[0].0;
    // dev-0 makes itself a host: refused; SetAdmin on itself is meaningless (it is one) and refused too.
    assert!(matches!(
        try_push(
            &state,
            by0(),
            k0,
            Action::SetHost {
                id: "dev-0".into(),
                host: true
            }
        ),
        Err(ChainError::NotAllowed { .. })
    ));
    assert!(matches!(
        try_push(
            &state,
            by0(),
            k0,
            Action::SetAdmin {
                id: "dev-0".into(),
                admin: true
            }
        ),
        Err(ChainError::NotAllowed { .. })
    ));
    // The sole admin can neither step down nor be removed.
    assert_eq!(
        try_push(
            &state,
            by0(),
            k0,
            Action::SetAdmin {
                id: "dev-0".into(),
                admin: false
            }
        ),
        Err(ChainError::LastAdmin { seq: 7 })
    );
    assert_eq!(
        try_push(&state, by0(), k0, Action::RemoveDevice3("dev-0".into())),
        Err(ChainError::LastAdmin { seq: 7 })
    );
    // With a second admin (dev-2 promoted by dev-0), dev-0 may step down...
    let mut s = state.clone();
    push(
        &mut s,
        by0(),
        k0,
        Action::SetAdmin {
            id: "dev-2".into(),
            admin: true,
        },
    )
    .unwrap();
    // ...but then, in the same batch, signs nothing more.
    let batch = [
        s.draft(
            by0(),
            Action::SetAdmin {
                id: "dev-0".into(),
                admin: false,
            },
            1,
        )
        .sign(k0),
        {
            let mut next = s.draft(
                by0(),
                Action::SetAdmin {
                    id: "dev-2".into(),
                    admin: false,
                },
                2,
            );
            next.seq += 1;
            next.prev = s
                .draft(
                    by0(),
                    Action::SetAdmin {
                        id: "dev-0".into(),
                        admin: false,
                    },
                    1,
                )
                .sign(k0)
                .hash();
            next.sign(k0)
        },
    ];
    let mut merged = s.clone();
    let result = merged.merge_partial(&batch, &mut h());
    assert_eq!(result.applied, 1);
    assert!(matches!(result.error, Some(ChainError::NotAllowed { .. })));
    assert!(merged.admin("dev-2").is_some() && merged.admin("dev-0").is_none());
    // And dev-2 now cannot be demoted by anyone: it is the last admin.
    assert_eq!(
        try_push(
            &merged,
            Signer::Device("dev-2".into()),
            &f.devices[2].0,
            Action::SetAdmin {
                id: "dev-2".into(),
                admin: false
            }
        ),
        Err(ChainError::LastAdmin { seq: 9 })
    );
    // A no-op is refused (it would alert about nothing).
    assert!(matches!(
        try_push(
            &state,
            by0(),
            k0,
            Action::SetHost {
                id: "m-1".into(),
                host: true
            }
        ),
        Err(ChainError::NotAllowed { .. })
    ));
}

// ---------------------------------------------------------------- 5. machine keys only for hosts

/// Item 5: machine keys are committed only for current hosts; a new host (or
/// one restarting hosting) gets a *new* machine key epoch in the very next
/// entry; a host that stops hosting has no current machine key, so nothing
/// it signs passes as a stream head (readers take the head key from
/// `machine()`), while its old epochs stay checkable for its old copies.
#[test]
fn c5_machine_keys_and_stream_authority_belong_to_hosts() {
    let f = fixture();
    let (state, _) = with_second_host(&f);
    let k0 = &f.devices[0].0;
    // Not a host: no machine key.
    assert!(matches!(
        try_push(&state, by0(), k0, Action::KeyEpochs(mk_commit("dev-2", 0, &[1; 32]))),
        Err(ChainError::UnknownId { .. })
    ));
    // A new host must be followed by its first key: anything else in between is refused.
    let mut s = state.clone();
    push(&mut s, by0(), k0, Action::AddDevice3(dev3("h-3", &extra(1), true, false))).unwrap();
    assert_eq!(s.host_key_due(), Some("h-3"));
    assert!(matches!(
        try_push(
            &s,
            by0(),
            k0,
            Action::SetAdmin {
                id: "dev-2".into(),
                admin: true
            }
        ),
        Err(ChainError::NotAllowed { .. })
    ));
    assert!(matches!(
        try_push(&s, by0(), k0, Action::KeyEpochs(mk_commit("m-1", 0, &[1; 32]))),
        Err(ChainError::NotAllowed { .. })
    ));
    push(&mut s, by0(), k0, Action::KeyEpochs(mk_commit("h-3", 0, &[3; 32]))).unwrap();
    assert_eq!(s.host_key_due(), None);
    // Stop hosting: no current machine, no new epochs, no control; the old epoch still checks.
    push(
        &mut s,
        by0(),
        k0,
        Action::SetHost {
            id: "h-2".into(),
            host: false,
        },
    )
    .unwrap();
    assert!(s.machine("h-2").is_none() && s.device("h-2").is_some());
    assert!(matches!(
        try_push(&s, by0(), k0, Action::KeyEpochs(mk_commit("h-2", 1, &[4; 32]))),
        Err(ChainError::UnknownId { .. })
    ));
    s.check_machine_key("h-2", 0, &[0x22; 32]).unwrap();
    assert!(matches!(s.may_control("dev-0", "h-2"), Err(ChainError::NotAccepted { .. })));
    // Restart hosting: the next entry must be epoch 1 (a new key), not epoch 0 again.
    push(
        &mut s,
        by0(),
        k0,
        Action::SetHost {
            id: "h-2".into(),
            host: true,
        },
    )
    .unwrap();
    assert!(matches!(
        try_push(&s, by0(), k0, Action::KeyEpochs(mk_commit("h-2", 0, &[0x22; 32]))),
        Err(ChainError::NotAllowed { .. })
    ));
    push(&mut s, by0(), k0, Action::KeyEpochs(mk_commit("h-2", 1, &[5; 32]))).unwrap();
    assert!(s.machine("h-2").is_some());
    // A stream head verifies only under a current host's key (what a reader looks up).
    let head_key = |chain: &ChainState, id: &str| chain.machine(id).map(|m| m.info.sign_key);
    assert_eq!(head_key(&s, "h-2"), Some(extra(0).0.verifying_key().to_bytes()));
    assert_eq!(head_key(&s, "dev-0"), None);
}

// ---------------------------------------------------------------- 8. freshness by operation

/// Item 8: control needs an admin's attestation within seven days,
/// management within twelve hours; only admins attest; a requester's own
/// attestation counts only when it is the sole admin.
#[test]
fn c8_freshness_follows_the_operation_and_only_admins_vouch() {
    let f = fixture();
    let (state, _) = with_second_host(&f);
    let t = 1_000_000;
    let by_admin = Attestation::create(&state, "dev-0", &f.devices[0].0, t);
    let fresh = |requester: &str, atts: &[Attestation], now: u64, op: Operation| state.freshness(requester, atts, now, op.max_age_ms()).unwrap();
    // A host that is not an admin, controlled six days on: fresh; eight days on: stale.
    let atts = std::slice::from_ref(&by_admin);
    assert_eq!(fresh("h-2", atts, t + 6 * DAY, Operation::Control), Freshness::Fresh);
    assert_eq!(
        fresh("h-2", atts, t + 8 * DAY, Operation::Control),
        Freshness::Stale { newest_ms: Some(t) }
    );
    // Management: eleven hours yes, thirteen no.
    assert_eq!(fresh("h-2", atts, t + 11 * HOUR, Operation::Manage), Freshness::Fresh);
    assert_eq!(
        fresh("h-2", atts, t + 13 * HOUR, Operation::Manage),
        Freshness::Stale { newest_ms: Some(t) }
    );
    // A host or a plain device attesting does not count, and is refused as such.
    for (id, key) in [("h-2", &extra(0).0), ("dev-2", &f.devices[2].0), ("m-1", &f.machine.0)] {
        let att = Attestation::create(&state, id, key, t);
        assert_eq!(
            state.check_attestation(&att),
            Err(ChainError::BadAttestation("not a current full device"))
        );
        assert_eq!(
            fresh("dev-0", std::slice::from_ref(&att), t, Operation::Control),
            Freshness::Stale { newest_ms: None }
        );
    }
    // dev-0, the sole admin, vouches for itself; once dev-2 is an admin too, it no longer does.
    assert_eq!(fresh("dev-0", atts, t, Operation::Manage), Freshness::Fresh);
    let mut two = state.clone();
    push(
        &mut two,
        by0(),
        &f.devices[0].0,
        Action::SetAdmin {
            id: "dev-2".into(),
            admin: true,
        },
    )
    .unwrap();
    let own = Attestation::create(&two, "dev-0", &f.devices[0].0, t);
    assert_eq!(
        two.freshness("dev-0", std::slice::from_ref(&own), t, MANAGE_FRESHNESS_MS),
        Ok(Freshness::Stale { newest_ms: None })
    );
    let other = Attestation::create(&two, "dev-2", &f.devices[2].0, t);
    assert_eq!(two.freshness("dev-0", &[own, other], t, MANAGE_FRESHNESS_MS), Ok(Freshness::Fresh));
}

/// Item 8 through the handshake an agent runs: a six-day-old attestation
/// opens a channel, an eight-day-old one does not.
#[test]
fn c8_an_agent_admits_on_a_week_old_attestation_but_not_an_older_one() {
    let f = fixture();
    let (state, _) = with_second_host(&f);
    let t = 50 * DAY;
    let att = Attestation::create(&state, "dev-0", &f.devices[0].0, t);
    for (age, open) in [(6 * DAY, true), (8 * DAY, false)] {
        let now = t + age;
        let (_, hello) = handshake::start(&state, &f.devices[0].1, "dev-0", "h-2", now, b"").unwrap();
        let mut chain = state.clone();
        let mut clock = ChainClock::live(now, chain.seq() + 1);
        let mut guard = ReplayGuard::new(60_000, 60_000, 16, 0);
        let side = AgentSide {
            chain: &mut chain,
            clock: &mut clock,
            guard: &mut guard,
            key: &extra(0).1,
            machine: "h-2",
            policy: RekeyPolicy::default(),
            server_info: b"",
            attestations: std::slice::from_ref(&att),
            freshness_max_age_ms: handshake::FRESHNESS_MAX_AGE_MS,
        };
        let accepted = handshake::accept(side, "dev-0", &hello, now).unwrap();
        assert_eq!(matches!(accepted, Accepted::Open { .. }), open, "{age}");
    }
}

// ---------------------------------------------------------------- 9. one key, four roles

/// Item 9: a host signs in to the Hub as an agent and as a device, and to a
/// relay as an agent and as a device, with one key; a signature in one role
/// is worthless in every other.
#[test]
fn c9_one_key_signing_in_four_roles_cannot_swap_them() {
    use crate::sig::{HubLogin, LoginRole};
    let key = &extra(0).0;
    let public = key.verifying_key().to_bytes();
    let roles = [
        LoginRole::Agent,
        LoginRole::DeviceSession,
        LoginRole::RelayAgent,
        LoginRole::RelayDevice,
        LoginRole::Device,
    ];
    let login = |role| HubLogin {
        role,
        account: "acct-1".into(),
        id: "h-2".into(),
        hub: "wss://hub.example".into(),
        nonce: [9; 32],
        time_ms: 5,
    };
    for signed in roles {
        let sig = login(signed).sign(key).unwrap();
        for checked in roles {
            assert_eq!(login(checked).verify(&public, &sig), signed == checked, "{signed:?} as {checked:?}");
        }
    }
}

// ---------------------------------------------------------------- 10. Noise with one static key in both roles

/// Item 10: a device never handshakes with itself (neither end lets it), and
/// with one static key used as initiator and responder, a first message is
/// bound to its responder by the prologue: one a host wrote to another host
/// is refused if reflected back to it or replayed to a third.
#[test]
fn c10_no_self_channels_and_no_reflection_with_one_key_in_both_roles() {
    let f = fixture();
    let (mut state, _) = with_second_host(&f);
    let k0 = &f.devices[0].0;
    // h-2 may control m-1 here (so the reflection, not the policy, is what fails).
    push(
        &mut state,
        by0(),
        k0,
        Action::SetAccept {
            host: "m-1".into(),
            from: Accept::Devices(vec!["h-2".into()]),
        },
    )
    .unwrap();
    let now = 5_000;
    let att = Attestation::create(&state, "dev-0", k0, now);
    let accept = |dh: &DhKeypair, me: &str, from: &str, hello: &[u8]| {
        let mut chain = state.clone();
        let mut clock = ChainClock::live(now, chain.seq() + 1);
        let mut guard = ReplayGuard::new(60_000, 60_000, 16, 0);
        handshake::accept(
            AgentSide {
                chain: &mut chain,
                clock: &mut clock,
                guard: &mut guard,
                key: dh,
                machine: me,
                policy: RekeyPolicy::default(),
                server_info: b"",
                attestations: std::slice::from_ref(&att),
                freshness_max_age_ms: handshake::FRESHNESS_MAX_AGE_MS,
            },
            from,
            hello,
            now,
        )
        .map(|a| matches!(a, Accepted::Open { .. }))
    };
    let h2 = extra(0);
    // Neither end starts or accepts a channel to itself.
    assert!(matches!(
        handshake::start(&state, &h2.1, "h-2", "h-2", now, b""),
        Err(HandshakeError::NotAccepted(_))
    ));
    let (_, to_m1) = handshake::start(&state, &h2.1, "h-2", "m-1", now, b"").unwrap();
    assert!(matches!(accept(&h2.1, "h-2", "h-2", &to_m1), Err(HandshakeError::NotAccepted(_))));
    // The honest channel h-2 → m-1 opens.
    assert_eq!(accept(&f.machine.1, "m-1", "h-2", &to_m1), Ok(true));
    // Reflected back to h-2 (as if from m-1), or replayed to h-3: the prologue does not match.
    assert!(matches!(accept(&h2.1, "h-2", "m-1", &to_m1), Err(HandshakeError::Channel(_))));
    let mut s3 = state.clone();
    push(&mut s3, by0(), k0, Action::AddDevice3(dev3("h-3", &extra(1), true, false))).unwrap();
    assert!(accept(&extra(1).1, "h-3", "h-2", &to_m1).is_err());
}

// ---------------------------------------------------------------- 17. passkey slots

fn with_slot(f: &Fixture, self_activate: bool) -> (ChainState, Pak) {
    let (mut state, _) = with_second_host(f);
    let p = pak(f, "p-1", 800, 1, self_activate);
    push(&mut state, by0(), &f.devices[0].0, Action::PasskeySlotSet(p.slot.clone())).unwrap();
    (state, p)
}

/// Item 17, registering: only an admin registers a slot, with a proof over
/// this account, root, slot, credential and salt; `grants` must be 0; at most
/// eight slots, one per credential, ids never reused.
#[test]
fn c17_a_passkey_slot_needs_an_admin_a_proof_and_grants_nothing() {
    let f = fixture();
    let (state, _) = with_second_host(&f);
    let k0 = &f.devices[0].0;
    let p = pak(&f, "p-1", 800, 1, true);
    // By a host that is not an admin: refused.
    assert!(matches!(
        try_push(&state, Signer::Device("h-2".into()), &extra(0).0, Action::PasskeySlotSet(p.slot.clone())),
        Err(ChainError::NotAllowed { .. })
    ));
    // grants ≠ 0: refused.
    assert!(matches!(
        try_push(&state, by0(), k0, Action::PasskeySlotSet(PasskeySlot { grants: 1, ..p.slot.clone() })),
        Err(ChainError::NotAllowed { .. })
    ));
    // A proof over anything else: refused (another account's, another slot's, another credential's, salt's, a zero proof).
    let other_account = {
        let ak = f.ak.verifying_key().to_bytes();
        let body = passkey_proof_body("acct-2", &ak, &p.slot.key, "p-1", &p.slot.cred_hash, &p.slot.salt_nonce);
        crate::sig::sign(&p.signing, crate::sig::PASSKEY_PROOF, &body)
    };
    for bad in [
        PasskeySlot {
            proof: other_account,
            ..p.slot.clone()
        },
        PasskeySlot {
            slot: "p-2".into(),
            ..p.slot.clone()
        },
        PasskeySlot {
            cred_hash: [9; 32],
            ..p.slot.clone()
        },
        PasskeySlot {
            salt_nonce: [9; 32],
            ..p.slot.clone()
        },
        PasskeySlot {
            proof: [0; 64],
            ..p.slot.clone()
        },
    ] {
        assert!(matches!(
            try_push(&state, by0(), k0, Action::PasskeySlotSet(bad)),
            Err(ChainError::NotAllowed { .. })
        ));
    }
    let mut weak = p.slot.clone();
    weak.key = [0; 32];
    weak.key[0] = 1;
    assert!(matches!(
        try_push(&state, by0(), k0, Action::PasskeySlotSet(weak)),
        Err(ChainError::WeakKey { .. })
    ));
    // One slot per credential; ids never reused; at most eight.
    let mut s = state.clone();
    push(&mut s, by0(), k0, Action::PasskeySlotSet(p.slot.clone())).unwrap();
    assert_eq!(s.slot("p-1").unwrap().set_by, "dev-0");
    assert!(matches!(
        try_push(&s, by0(), k0, Action::PasskeySlotSet(pak(&f, "p-2", 801, 1, true).slot)),
        Err(ChainError::NotAllowed { .. })
    ));
    push(&mut s, by0(), k0, Action::PasskeySlotRemove("p-1".into())).unwrap();
    assert!(matches!(
        try_push(&s, by0(), k0, Action::PasskeySlotSet(p.slot.clone())),
        Err(ChainError::DuplicateId { .. })
    ));
    assert!(matches!(
        try_push(&s, by0(), k0, Action::PasskeySlotRemove("p-1".into())),
        Err(ChainError::UnknownSlot { .. })
    ));
    for i in 0..8u8 {
        push(
            &mut s,
            by0(),
            k0,
            Action::PasskeySlotSet(pak(&f, &format!("p-x{i}"), 810 + u64::from(i), 20 + i, true).slot),
        )
        .unwrap();
    }
    assert!(matches!(
        try_push(&s, by0(), k0, Action::PasskeySlotSet(pak(&f, "p-y", 830, 40, true).slot)),
        Err(ChainError::NotAllowed { .. })
    ));
}

/// Item 17, signing: `Passkey(slot)` signs only a flag-less pending join and
/// that device's activation — never a flagged device, never anything else;
/// a newer join replaces the waiting one.
#[test]
fn c17_a_passkey_signs_only_its_own_plain_pending_join_and_its_activation() {
    let f = fixture();
    let (state, p) = with_slot(&f, true);
    let by_pk = || Signer::Passkey("p-1".into());
    for flags in [(true, false), (false, true), (true, true)] {
        assert!(matches!(
            try_push(&state, by_pk(), &p.signing, Action::AddDevice3(dev3("d-p", &extra(2), flags.0, flags.1))),
            Err(ChainError::NotAllowed { .. })
        ));
    }
    for action in [
        Action::SetAdmin {
            id: "dev-2".into(),
            admin: true,
        },
        Action::RemoveDevice3("dev-0".into()),
        Action::ApproveDevice("dev-2".into()),
        Action::KeyEpochs(mk_commit("h-2", 1, &[1; 32])),
        Action::PasskeySlotRemove("p-1".into()),
    ] {
        assert!(matches!(
            try_push(&state, by_pk(), &p.signing, action),
            Err(ChainError::NotAllowed { .. })
        ));
    }
    // Another slot's key, or a slot that does not exist: refused.
    assert!(matches!(
        try_push(&state, by_pk(), &f.devices[0].0, Action::AddDevice3(dev3("d-p", &extra(2), false, false))),
        Err(ChainError::BadSignature { .. })
    ));
    assert!(matches!(
        try_push(
            &state,
            Signer::Passkey("p-9".into()),
            &p.signing,
            Action::AddDevice3(dev3("d-p", &extra(2), false, false))
        ),
        Err(ChainError::UnknownSlot { .. })
    ));
    // The plain join: pending, via p-1, neither host nor admin; it cannot control anything yet.
    let mut s = state.clone();
    push(&mut s, by_pk(), &p.signing, Action::AddDevice3(dev3("d-p", &extra(2), false, false))).unwrap();
    let r = s.device_record("d-p").unwrap();
    assert!(!r.host && !r.admin && r.via_passkey.as_deref() == Some("p-1"));
    assert!(s.authorize_device("d-p", &extra(2).1.public()).is_err());
    assert_eq!(s.pending_passkey_joins(), vec![("d-p", 8)]);
    // A second join replaces it.
    push(&mut s, by_pk(), &p.signing, Action::AddDevice3(dev3("d-q", &extra(3), false, false))).unwrap();
    assert_eq!(s.device_record("d-p").unwrap().status, DeviceStatus::Removed);
    assert_eq!(s.pending_passkey_joins(), vec![("d-q", 9)]);
    assert!(s.slot("p-1").is_some(), "a replacement does not void the slot");
    // A device does not sign a passkey activation; RSK does not activate a passkey join.
    assert!(matches!(
        try_push(&s, by0(), &f.devices[0].0, Action::PasskeyActivate("d-q".into())),
        Err(ChainError::NotAllowed { .. })
    ));
    assert!(matches!(
        try_push(&s, Signer::Recovery, &f.recovery.signing, Action::ActivateRecovered("d-q".into())),
        Err(ChainError::NotAllowed { .. })
    ));
    // An admin approves it and makes it an admin in the same batch: the signer is not the grantee.
    let mut approved = s.clone();
    push(&mut approved, by0(), &f.devices[0].0, Action::ApproveDevice("d-q".into())).unwrap();
    push(
        &mut approved,
        by0(),
        &f.devices[0].0,
        Action::SetAdmin {
            id: "d-q".into(),
            admin: true,
        },
    )
    .unwrap();
    assert!(approved.admin("d-q").is_some());
}

/// Item 17, activating: 24 hours by the verifier's own first sighting of the
/// join; only the slot that added the device; only a slot that allows it.
#[test]
fn c17_self_activation_waits_a_day_on_the_verifiers_clock_and_needs_its_slot() {
    let f = fixture();
    let (state, p) = with_slot(&f, true);
    let by_pk = || Signer::Passkey("p-1".into());
    let join = state
        .draft(by_pk(), Action::AddDevice3(dev3("d-p", &extra(2), false, false)), 1)
        .sign(&p.signing);
    let mut joined = state.clone();
    joined.apply(&join, &mut h()).unwrap();
    let activate = joined.draft(by_pk(), Action::PasskeyActivate("d-p".into()), 2).sign(&p.signing);
    // A live verifier first sees the join at t = 1000.
    let mut clock = ChainClock::live(1_000, state.seq() + 1);
    let mut live = state.clone();
    live.apply(&join, &mut clock).unwrap();
    clock.set_now(1_000 + PASSKEY_DELAY_MS - 1);
    assert!(matches!(live.apply(&activate, &mut clock), Err(ChainError::TooEarly { .. })));
    clock.set_now(1_000 + PASSKEY_DELAY_MS);
    live.apply(&activate, &mut clock).unwrap();
    let r = live.device("d-p").unwrap();
    assert!(!r.admin && !r.host, "a passkey never brings admin");
    // A verifier that starts later waits from its own first sighting, not the entry's timestamp.
    let mut late = ChainClock::live(9_000_000, state.seq() + 2);
    let mut view = ChainState::verify(&[sample_chain(&f), joined.entries()[5..].to_vec()].concat(), None, &mut late).unwrap();
    assert!(matches!(view.apply(&activate, &mut late), Err(ChainError::TooEarly { .. })));
    // A slot that does not allow self-activation, a removed slot, another slot: refused.
    let (no_self, p2) = with_slot(&fixture(), false);
    let mut ns = no_self.clone();
    push(&mut ns, by_pk(), &p2.signing, Action::AddDevice3(dev3("d-p", &extra(2), false, false))).unwrap();
    assert!(matches!(
        try_push(&ns, by_pk(), &p2.signing, Action::PasskeyActivate("d-p".into())),
        Err(ChainError::NotAllowed { .. })
    ));
    let mut removed = joined.clone();
    push(&mut removed, by0(), &f.devices[0].0, Action::PasskeySlotRemove("p-1".into())).unwrap();
    assert!(matches!(
        try_push(&removed, by_pk(), &p.signing, Action::PasskeyActivate("d-p".into())),
        Err(ChainError::UnknownSlot { .. })
    ));
    let mut two = joined.clone();
    let p3 = pak(&f, "p-3", 803, 3, true);
    push(&mut two, by0(), &f.devices[0].0, Action::PasskeySlotSet(p3.slot.clone())).unwrap();
    assert!(matches!(
        try_push(&two, Signer::Passkey("p-3".into()), &p3.signing, Action::PasskeyActivate("d-p".into())),
        Err(ChainError::NotAllowed { .. })
    ));
}

/// Item 17, voiding: removing the device that joined with a slot, or the admin
/// that registered it, voids the slot (its waiting devices can then only be
/// approved or removed by an admin), as removing them clears the recovery key.
#[test]
fn c17_removing_who_computed_a_slots_prf_voids_the_slot() {
    let f = fixture();
    let (state, p) = with_slot(&f, true);
    let by_pk = || Signer::Passkey("p-1".into());
    let k0 = &f.devices[0].0;
    // The device that joined with it.
    let mut s = state.clone();
    push(&mut s, by_pk(), &p.signing, Action::AddDevice3(dev3("d-p", &extra(2), false, false))).unwrap();
    push(&mut s, by0(), k0, Action::RemoveDevice3("d-p".into())).unwrap();
    assert!(s.slot("p-1").is_none());
    // The admin that registered it (dev-2 promoted, registers, is removed): the slot goes, a waiting device stays waiting.
    let mut s = state.clone();
    push(
        &mut s,
        by0(),
        k0,
        Action::SetAdmin {
            id: "dev-2".into(),
            admin: true,
        },
    )
    .unwrap();
    let q = pak(&f, "p-2", 802, 2, true);
    push(
        &mut s,
        Signer::Device("dev-2".into()),
        &f.devices[2].0,
        Action::PasskeySlotSet(q.slot.clone()),
    )
    .unwrap();
    push(
        &mut s,
        Signer::Passkey("p-2".into()),
        &q.signing,
        Action::AddDevice3(dev3("d-q", &extra(3), false, false)),
    )
    .unwrap();
    push(&mut s, by0(), k0, Action::RemoveDevice3("dev-2".into())).unwrap();
    assert!(s.slot("p-2").is_none() && s.slot("p-1").is_some());
    assert!(matches!(
        try_push(&s, Signer::Passkey("p-2".into()), &q.signing, Action::PasskeyActivate("d-q".into())),
        Err(ChainError::UnknownSlot { .. })
    ));
    push(&mut s, by0(), k0, Action::ApproveDevice("d-q".into())).unwrap();
    assert!(s.device("d-q").is_some());
}

// ---------------------------------------------------------------- 谁可以控制我 (S4)

/// "Who may control me": a host accepts control from admin devices by
/// default; another host (or any other device) only once an admin says so,
/// and only that host; an admin's word, nobody else's; never itself; never a
/// removed device.
#[test]
fn s4_a_host_accepts_control_only_from_admins_unless_an_admin_allows_more() {
    let f = fixture();
    let (state, _) = with_second_host(&f);
    let k0 = &f.devices[0].0;
    // Defaults.
    state.may_control("dev-0", "m-1").unwrap();
    state.may_control("dev-0", "h-2").unwrap();
    for (from, to) in [("h-2", "m-1"), ("m-1", "h-2"), ("dev-2", "m-1")] {
        assert!(
            matches!(state.may_control(from, to), Err(ChainError::NotAccepted { .. })),
            "{from} → {to}"
        );
    }
    assert_eq!(state.may_control("m-1", "m-1"), Err(ChainError::SelfControl));
    assert!(state.may_control("dev-0", "dev-2").is_err(), "not a host");
    assert!(state.may_control("dev-1", "m-1").is_err(), "removed");
    // Only an admin sets a policy; not the host for itself, not the device that wants in.
    for (signer, key) in [("m-1", &f.machine.0), ("h-2", &extra(0).0), ("dev-2", &f.devices[2].0)] {
        assert!(matches!(
            try_push(
                &state,
                Signer::Device(signer.into()),
                key,
                Action::SetAccept {
                    host: "m-1".into(),
                    from: Accept::All
                }
            ),
            Err(ChainError::NotAllowed { .. })
        ));
    }
    // The policy names current devices other than the host, and must change something.
    let set = |ids: &[&str]| Action::SetAccept {
        host: "m-1".into(),
        from: Accept::Devices(ids.iter().map(|s| s.to_string()).collect()),
    };
    assert!(matches!(try_push(&state, by0(), k0, set(&["m-1"])), Err(ChainError::NotAllowed { .. })));
    assert!(matches!(try_push(&state, by0(), k0, set(&["dev-1"])), Err(ChainError::UnknownId { .. })));
    assert!(matches!(try_push(&state, by0(), k0, set(&["nobody"])), Err(ChainError::UnknownId { .. })));
    assert!(matches!(
        try_push(
            &state,
            by0(),
            k0,
            Action::SetAccept {
                host: "m-1".into(),
                from: Accept::Admins
            }
        ),
        Err(ChainError::NotAllowed { .. })
    ));
    assert!(matches!(
        try_push(
            &state,
            by0(),
            k0,
            Action::SetAccept {
                host: "dev-2".into(),
                from: Accept::All
            }
        ),
        Err(ChainError::NotAllowed { .. })
    ));
    // m-1 lets h-2 in: h-2 → m-1 only; the reverse and dev-2 still not.
    let mut s = state.clone();
    push(&mut s, by0(), k0, set(&["h-2"])).unwrap();
    s.may_control("h-2", "m-1").unwrap();
    assert!(s.may_control("m-1", "h-2").is_err());
    assert!(s.may_control("dev-2", "m-1").is_err());
    // Everyone, then back to admins only.
    push(
        &mut s,
        by0(),
        k0,
        Action::SetAccept {
            host: "m-1".into(),
            from: Accept::All,
        },
    )
    .unwrap();
    s.may_control("dev-2", "m-1").unwrap();
    // Narrowing: the host's next machine key epoch must come with it (D2).
    push(
        &mut s,
        by0(),
        k0,
        Action::SetAccept {
            host: "m-1".into(),
            from: Accept::Admins,
        },
    )
    .unwrap();
    assert_eq!(s.host_key_due(), Some("m-1"));
    push(&mut s, by0(), k0, Action::KeyEpochs(mk_commit("m-1", 0, &[0x31; 32]))).unwrap();
    assert!(s.may_control("dev-2", "m-1").is_err() && s.may_control("h-2", "m-1").is_err());
    // A listed device, once removed, is out.
    push(&mut s, by0(), k0, set(&["h-2"])).unwrap();
    push(&mut s, by0(), k0, Action::RemoveDevice3("h-2".into())).unwrap();
    assert!(s.may_control("h-2", "m-1").is_err());
}

/// The same through the agent's admission: a host's channel to another host
/// is refused by default (4403-class `NotAccepted`) and opens once allowed;
/// a v2 limited device, which old agents let in, is refused by default too.
#[test]
fn s4_an_agent_refuses_host_to_host_control_until_an_admin_allows_it() {
    let f = fixture();
    let (state, _) = with_second_host(&f);
    let now = 5_000;
    let open = |chain: &ChainState, from: &str, dh: &DhKeypair| {
        let att = Attestation::create(chain, "dev-0", &f.devices[0].0, now);
        let (_, hello) = handshake::start(chain, dh, from, "m-1", now, b"").unwrap();
        let mut view = chain.clone();
        let mut clock = ChainClock::live(now, view.seq() + 1);
        let mut guard = ReplayGuard::new(60_000, 60_000, 16, 0);
        handshake::accept(
            AgentSide {
                chain: &mut view,
                clock: &mut clock,
                guard: &mut guard,
                key: &f.machine.1,
                machine: "m-1",
                policy: RekeyPolicy::default(),
                server_info: b"",
                attestations: std::slice::from_ref(&att),
                freshness_max_age_ms: handshake::FRESHNESS_MAX_AGE_MS,
            },
            from,
            &hello,
            now,
        )
        .map(|a| matches!(a, Accepted::Open { .. }))
    };
    assert!(matches!(open(&state, "h-2", &extra(0).1), Err(HandshakeError::NotAccepted(_))));
    assert!(matches!(open(&state, "dev-2", &f.devices[2].1), Err(HandshakeError::NotAccepted(_))));
    assert_eq!(open(&state, "dev-0", &f.devices[0].1), Ok(true));
    let mut allowed = state.clone();
    push(
        &mut allowed,
        by0(),
        &f.devices[0].0,
        Action::SetAccept {
            host: "m-1".into(),
            from: Accept::Devices(vec!["h-2".into()]),
        },
    )
    .unwrap();
    assert_eq!(open(&allowed, "h-2", &extra(0).1), Ok(true));
}

// ---------------------------------------------------------------- 6. UK never to a non-admin; MK only from an admin

/// Item 6 (protocol half): a key sealed by a device that is not an admin is
/// refused whatever it is (a host cannot hand out `UK`s or plant `MK`s), and a
/// host key sealed for a plain device opens only from an admin, only for the
/// key the chain committed for that host.
#[test]
fn c6_only_admins_deliver_keys_and_host_keys_are_bound_to_their_host() {
    use crate::wrap::{open_host_key, open_user_key, seal_host_key, seal_to, HostKeyContext, Purpose, SealContext, WrapError};
    let f = fixture();
    let (state, _) = with_second_host(&f);
    let rng = &mut crate::testing::rng(7);
    let dev2 = &f.devices[2].1;
    // m-1 (a host, not an admin) seals a "UK" to dev-2: refused.
    let uk_ctx = SealContext {
        purpose: Purpose::UserKey,
        account: "acct-1".into(),
        sender: "m-1".into(),
        recipient: "dev-2".into(),
        epoch: 0,
    };
    let sealed = seal_to(&f.machine.1, &dev2.public(), &uk_ctx, &[1; 32], rng).unwrap();
    assert!(matches!(
        open_user_key(dev2, "dev-2", &state, &uk_ctx, &sealed),
        Err(WrapError::UnknownSender(_))
    ));
    // dev-0 seals h-2's MK epoch 0 for dev-2: opens; for another host, epoch, recipient or sender: does not.
    let ctx = HostKeyContext {
        account: "acct-1".into(),
        sender: "dev-0".into(),
        recipient: "dev-2".into(),
        host: "h-2".into(),
        epoch: 0,
    };
    let sealed = seal_host_key(&f.devices[0].1, &dev2.public(), &ctx, &[0x22; 32], rng).unwrap();
    assert_eq!(*open_host_key(dev2, "dev-2", &state, &ctx, &sealed).unwrap(), vec![0x22; 32]);
    for bad in [
        HostKeyContext {
            host: "m-1".into(),
            ..ctx.clone()
        },
        HostKeyContext { epoch: 1, ..ctx.clone() },
    ] {
        assert!(open_host_key(dev2, "dev-2", &state, &bad, &sealed).is_err());
    }
    assert!(matches!(
        open_host_key(dev2, "dev-0", &state, &ctx, &sealed),
        Err(WrapError::WrongRecipient(_))
    ));
    // A key the chain did not commit, even from the admin: refused.
    let planted = seal_host_key(&f.devices[0].1, &dev2.public(), &ctx, &[0x99; 32], rng).unwrap();
    assert!(matches!(
        open_host_key(dev2, "dev-2", &state, &ctx, &planted),
        Err(WrapError::Uncommitted(_))
    ));
    // From the host itself (not an admin): refused.
    let by_host = HostKeyContext {
        sender: "h-2".into(),
        ..ctx.clone()
    };
    let sealed = seal_host_key(&extra(0).1, &dev2.public(), &by_host, &[0x22; 32], rng).unwrap();
    assert!(matches!(
        open_host_key(dev2, "dev-2", &state, &by_host, &sealed),
        Err(WrapError::UnknownSender(_))
    ));
    // The mail blob carries the host and round-trips.
    let blob = crate::wrap::encode_host_key_mail("h-2", &sealed);
    assert_eq!(crate::wrap::decode_host_key_mail(&blob).unwrap(), ("h-2".to_string(), sealed));
}

// ---------------------------------------------------------------- 16. migration

/// Item 16: the v2 vectors in `vectors/rust.json` were written by the code
/// before chain v3 (their bytes are unchanged since): the v3 code verifies
/// them to the very same hashes and head, and pins the same genesis and
/// account fingerprint — an account switched to the v3 readers is the same
/// account to every verifier, with no entry written.
#[test]
fn c16_a_chain_from_before_v3_verifies_to_the_same_head_genesis_and_pin() {
    let text = std::fs::read_to_string(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vectors/rust.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let c = &v["chain"];
    let hexv = |x: &serde_json::Value| crate::codec::hex::decode(x.as_str().unwrap());
    let entries: Vec<Entry> = c["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| Entry::decode(&hexv(e)).unwrap())
        .collect();
    assert!(entries.iter().all(|e| !e.is_v3()), "a chain from before v3");
    let pin: [u8; 16] = hexv(&c["accountFingerprint"]).try_into().unwrap();
    let root = TrustRoot::from_fingerprint(pin);
    let state = ChainState::verify_rooted(&entries, &root, &mut h()).unwrap();
    for (entry, hash) in entries.iter().zip(c["hashes"].as_array().unwrap()) {
        assert_eq!(entry.hash().to_vec(), hexv(hash));
        assert_eq!(
            Entry::decode(&entry.encode()).unwrap().encode(),
            entry.encode(),
            "re-encodes byte for byte"
        );
    }
    assert_eq!(state.head().to_vec(), hexv(&c["state"]["head"]));
    assert_eq!(state.seq(), c["state"]["seq"].as_u64().unwrap());
    assert_eq!(root.pinned_to(&state).genesis_hash, Some(entries[0].hash()));
    assert!(!state.is_v3());
}

/// Once a v3 entry lands, a reader that knows only v2 — every release before
/// the first v3 reader decoded `version != 2` as an error — cannot follow the
/// chain past it, and nothing ever writes v2 after it: readers cannot be
/// rolled back below the first release that reads v3 (local-aggregator §3.5).
#[test]
fn c16_once_a_v3_entry_lands_readers_cannot_go_back_below_v3() {
    let f = fixture();
    let (state, entries) = with_second_host(&f);
    // What a v2-only reader did with every entry: refuse any version but 2.
    let v2_reader = |e: &Entry| -> Result<(), u8> {
        match e.encode()[0] {
            VERSION => Ok(()),
            other => Err(other),
        }
    };
    let first_v3 = state.v3_from().unwrap() as usize;
    assert!(entries[..first_v3].iter().all(|e| v2_reader(e).is_ok()));
    assert_eq!(v2_reader(&entries[first_v3]), Err(VERSION_3));
    // And after it every entry is v3, even the kinds both versions share.
    let next = state.draft(by0(), Action::KeyEpochs(mk_commit("h-2", 1, &[9; 32])), 1);
    assert_eq!(next.version, VERSION_3);
}

// ---------------------------------------------------------------- review of chain v3

/// Review blocker 2 / D1: an admin host never attests, so it used to make
/// the admin browser's channel stale (two admins: the browser's own word no
/// longer counts) and count as an admin that keeps the last real one from
/// stepping down. Hosts cannot be admins now, by every route in.
#[test]
fn r_d1_a_host_never_manages_the_account() {
    let f = fixture();
    let (state, _) = with_second_host(&f);
    let k0 = &f.devices[0].0;
    let not_admin = Err(ChainError::NotAllowed {
        seq: 7,
        reason: rules::HOST_NOT_ADMIN,
    });
    // The reproduction: SetAdmin{h-2, true}, then the admin's channel to h-2.
    assert_eq!(
        try_push(
            &state,
            by0(),
            k0,
            Action::SetAdmin {
                id: "h-2".into(),
                admin: true
            }
        ),
        not_admin
    );
    let now = 5_000;
    let att = Attestation::create(&state, "dev-0", k0, now);
    let (_, hello) = handshake::start(&state, &f.devices[0].1, "dev-0", "h-2", now, b"").unwrap();
    let mut chain = state.clone();
    let mut clock = ChainClock::live(now, chain.seq() + 1);
    let mut guard = ReplayGuard::new(60_000, 60_000, 16, 0);
    let opened = handshake::accept(
        AgentSide {
            chain: &mut chain,
            clock: &mut clock,
            guard: &mut guard,
            key: &extra(0).1,
            machine: "h-2",
            policy: RekeyPolicy::default(),
            server_info: b"",
            attestations: std::slice::from_ref(&att),
            freshness_max_age_ms: handshake::FRESHNESS_MAX_AGE_MS,
        },
        "dev-0",
        &hello,
        now,
    )
    .unwrap();
    assert!(matches!(opened, Accepted::Open { .. }), "the sole admin's own word keeps counting");
    // Added as both, or made a host while an admin: refused too.
    assert_eq!(
        try_push(&state, by0(), k0, Action::AddDevice3(dev3("h-3", &extra(1), true, true))),
        not_admin
    );
    let mut two = state.clone();
    push(
        &mut two,
        by0(),
        k0,
        Action::SetAdmin {
            id: "dev-2".into(),
            admin: true,
        },
    )
    .unwrap();
    assert_eq!(
        try_push(
            &two,
            by0(),
            k0,
            Action::SetHost {
                id: "dev-2".into(),
                host: true
            }
        ),
        Err(ChainError::NotAllowed {
            seq: 8,
            reason: rules::HOST_NOT_ADMIN
        })
    );
    // Genesis browser A, a computer added as an admin: refused, so A stays the
    // last admin and cannot turn itself off.
    let ak = f.ak.verifying_key().to_bytes();
    let g = genesis3_draft("acct-3", ak, dev3("d-a", &extra(7), false, true), None, 1).sign(&f.ak);
    let mut a = ChainState::genesis(&g, None).unwrap();
    let ka = &extra(7).0;
    let by_a = || Signer::Device("d-a".into());
    assert!(matches!(
        try_push(&a, by_a(), ka, Action::AddDevice3(dev3("m-1", &extra(8), true, true))),
        Err(ChainError::NotAllowed {
            reason: rules::HOST_NOT_ADMIN,
            ..
        })
    ));
    push(&mut a, by_a(), ka, Action::AddDevice3(dev3("m-1", &extra(8), true, false))).unwrap();
    push(&mut a, by_a(), ka, Action::KeyEpochs(mk_commit("m-1", 0, &[8; 32]))).unwrap();
    assert!(matches!(
        try_push(
            &a,
            by_a(),
            ka,
            Action::SetAdmin {
                id: "d-a".into(),
                admin: false
            }
        ),
        Err(ChainError::LastAdmin { .. })
    ));
    // A genesis device that is both: refused.
    let g = genesis3_draft("acct-4", ak, dev3("d-b", &extra(7), true, true), None, 1).sign(&f.ak);
    assert_eq!(
        ChainState::genesis(&g, None),
        Err(ChainError::NotAllowed {
            seq: 0,
            reason: rules::HOST_NOT_ADMIN
        })
    );
}

/// Review D2: narrowing whom a host accepts takes a new machine key epoch for
/// it in the next entry; widening does not; stopping hosting drops the policy.
#[test]
fn r_d2_machine_keys_follow_the_accept_policy() {
    let f = fixture();
    let (state, _) = with_second_host(&f);
    let k0 = &f.devices[0].0;
    let devs = |ids: &[&str]| Action::SetAccept {
        host: "h-2".into(),
        from: Accept::Devices(ids.iter().map(|s| s.to_string()).collect()),
    };
    let mut s = state.clone();
    push(&mut s, by0(), k0, devs(&["dev-2", "m-1"])).unwrap();
    assert_eq!(s.host_key_due(), None, "widening");
    push(&mut s, by0(), k0, devs(&["m-1"])).unwrap();
    assert_eq!(s.host_key_due(), Some("h-2"), "dev-2 is out: a new MK");
    assert_eq!(
        try_push(
            &s,
            by0(),
            k0,
            Action::SetAdmin {
                id: "dev-2".into(),
                admin: true
            }
        ),
        Err(ChainError::NotAllowed {
            seq: 9,
            reason: rules::HOST_KEY_NEXT
        })
    );
    push(&mut s, by0(), k0, Action::KeyEpochs(mk_commit("h-2", 1, &[5; 32]))).unwrap();
    // All → a list, or All → admins only: narrowing too.
    push(
        &mut s,
        by0(),
        k0,
        Action::SetAccept {
            host: "h-2".into(),
            from: Accept::All,
        },
    )
    .unwrap();
    assert_eq!(s.host_key_due(), None);
    push(&mut s, by0(), k0, devs(&["dev-2"])).unwrap();
    assert_eq!(s.host_key_due(), Some("h-2"));
    push(&mut s, by0(), k0, Action::KeyEpochs(mk_commit("h-2", 2, &[6; 32]))).unwrap();
    // Stopping hosting drops the policy: restarting starts from admins only.
    push(
        &mut s,
        by0(),
        k0,
        Action::SetHost {
            id: "h-2".into(),
            host: false,
        },
    )
    .unwrap();
    assert_eq!(s.accept_policy("h-2"), Accept::Admins);
    push(
        &mut s,
        by0(),
        k0,
        Action::SetHost {
            id: "h-2".into(),
            host: true,
        },
    )
    .unwrap();
    push(&mut s, by0(), k0, Action::KeyEpochs(mk_commit("h-2", 3, &[7; 32]))).unwrap();
    assert!(s.may_control("dev-2", "h-2").is_err());
}

/// Review finding 10: each rule on its own, by its reason.
#[test]
fn r_10_each_set_admin_rule_fails_on_its_own() {
    let f = fixture();
    let (state, _) = with_second_host(&f);
    let k0 = &f.devices[0].0;
    let reason = |r: Result<(), ChainError>| match r {
        Err(ChainError::NotAllowed { reason, .. }) => reason,
        other => panic!("{other:?}"),
    };
    // Self-grant is judged before the no-op rule (an admin granting itself is both).
    assert_eq!(
        reason(try_push(
            &state,
            by0(),
            k0,
            Action::SetAdmin {
                id: "dev-0".into(),
                admin: true
            }
        )),
        rules::SELF_ADMIN
    );
    // A no-op on another device.
    let mut two = state.clone();
    push(
        &mut two,
        by0(),
        k0,
        Action::SetAdmin {
            id: "dev-2".into(),
            admin: true,
        },
    )
    .unwrap();
    assert_eq!(
        reason(try_push(
            &two,
            by0(),
            k0,
            Action::SetAdmin {
                id: "dev-2".into(),
                admin: true
            }
        )),
        rules::NO_CHANGE
    );
    assert_eq!(
        reason(try_push(
            &state,
            by0(),
            k0,
            Action::SetAdmin {
                id: "dev-2".into(),
                admin: false
            }
        )),
        rules::NO_CHANGE
    );
    // On a pending device: no such current device.
    let mut pending = state.clone();
    push(
        &mut pending,
        Signer::Recovery,
        &f.recovery.signing,
        Action::RecoveryJoin3(dev3("dev-r", &extra(6), false, true)),
    )
    .unwrap();
    assert!(matches!(
        try_push(
            &pending,
            by0(),
            k0,
            Action::SetAdmin {
                id: "dev-r".into(),
                admin: false
            }
        ),
        Err(ChainError::UnknownId { .. })
    ));
    // A demoted admin's passkey slots go with its standing.
    let mut slotted = state.clone();
    push(
        &mut slotted,
        by0(),
        k0,
        Action::SetAdmin {
            id: "dev-2".into(),
            admin: true,
        },
    )
    .unwrap();
    let p = pak(&f, "p-1", 800, 1, true);
    push(
        &mut slotted,
        Signer::Device("dev-2".into()),
        &f.devices[2].0,
        Action::PasskeySlotSet(p.slot.clone()),
    )
    .unwrap();
    push(
        &mut slotted,
        by0(),
        k0,
        Action::SetAdmin {
            id: "dev-2".into(),
            admin: false,
        },
    )
    .unwrap();
    assert!(slotted.slot("p-1").is_none());
}

/// Review blocker 3 / D4: the recovery words bring a device back into a v3
/// account with a v3 entry (a v2 one is refused after v3).
#[test]
fn r_d4_recovery_works_on_a_v3_chain() {
    let f = fixture();
    let (state, _) = with_second_host(&f);
    let v2 = state
        .draft(Signer::Recovery, Action::RecoveryJoin(crate::chain::tests::recovered_device(&f)), 1)
        .sign(&f.recovery.signing);
    assert_eq!(
        state.clone().apply(&v2, &mut h()),
        Err(ChainError::NotAllowed {
            seq: 7,
            reason: rules::V2_AFTER_V3
        })
    );
    let mut s = state.clone();
    push(
        &mut s,
        Signer::Recovery,
        &f.recovery.signing,
        Action::RecoveryJoin3(dev3("dev-r", &extra(6), false, true)),
    )
    .unwrap();
    push(&mut s, by0(), &f.devices[0].0, Action::ApproveDevice("dev-r".into())).unwrap();
    assert!(s.admin("dev-r").is_some());
}

/// Re-review of chain v3, D1 extended: a host that stops running AI is still
/// a device whose agent kept its key in a file, so it never becomes an admin
/// — else the browser's attestation reads stale again and, once the browser
/// demotes itself, the only admin is a key in a 0600 file. A v2 machine that
/// never had a key committed is no different.
#[test]
fn r_d1_a_device_that_ran_ai_never_manages_the_account() {
    let f = fixture();
    let (mut state, _) = with_second_host(&f);
    let k0 = &f.devices[0].0;
    push(
        &mut state,
        by0(),
        k0,
        Action::SetHost {
            id: "h-2".into(),
            host: false,
        },
    )
    .unwrap();
    assert!(state.device_record("h-2").is_some_and(|d| !d.host && d.ran_ai));
    let promote = |id: &str| Action::SetAdmin { id: id.into(), admin: true };
    assert_eq!(
        try_push(&state, by0(), k0, promote("h-2")),
        Err(ChainError::NotAllowed {
            seq: state.seq() + 1,
            reason: rules::RAN_AI_NOT_ADMIN,
        })
    );
    // m-1 (a v2 machine) stopped likewise.
    push(
        &mut state,
        by0(),
        k0,
        Action::SetHost {
            id: "m-1".into(),
            host: false,
        },
    )
    .unwrap();
    assert_eq!(
        try_push(&state, by0(), k0, promote("m-1")),
        Err(ChainError::NotAllowed {
            seq: state.seq() + 1,
            reason: rules::RAN_AI_NOT_ADMIN,
        })
    );
    // A device that never ran AI still can be promoted.
    assert_eq!(try_push(&state, by0(), k0, promote("dev-2")), Ok(()));
    // And the last admin stays the browser.
    assert_eq!(
        try_push(
            &state,
            by0(),
            k0,
            Action::SetAdmin {
                id: "dev-0".into(),
                admin: false
            }
        ),
        Err(ChainError::LastAdmin { seq: state.seq() + 1 })
    );
}

/// Write gate, must-fix 1: an admin that set the recovery words, or came
/// back with them, knows `RK`; demoted, it keeps that knowledge, and every
/// later user key is sealed for the recovery words — so the key stops
/// counting, as on its removal. Demoting an admin that never knew them
/// leaves the key alone.
#[test]
fn w1_demoting_the_admin_that_knows_the_recovery_words_clears_them() {
    let f = fixture();
    let (mut state, _) = with_second_host(&f);
    let k0 = &f.devices[0].0;
    let ak = f.ak.verifying_key().to_bytes();
    let w = extra(10);
    let p = extra(11);
    push(&mut state, by0(), k0, Action::AddDevice3(dev3("d-w", &w, false, true))).unwrap();
    push(&mut state, by0(), k0, Action::AddDevice3(dev3("d-p", &p, false, true))).unwrap();
    let rsk = SigningKey::from_bytes(&bytes32(810));
    push(
        &mut state,
        Signer::Device("d-w".into()),
        &w.0,
        Action::SetRecoveryKey(Some(RecoveryKey::new(&rsk, "acct-1", &ak))),
    )
    .unwrap();
    // Demoting an admin that never knew the words: they still count.
    push(
        &mut state,
        by0(),
        k0,
        Action::SetAdmin {
            id: "d-p".into(),
            admin: false,
        },
    )
    .unwrap();
    assert_eq!(state.recovery_key(), Some(rsk.verifying_key().to_bytes()));
    // Demoting the admin that set them: they stop counting.
    push(
        &mut state,
        by0(),
        k0,
        Action::SetAdmin {
            id: "d-w".into(),
            admin: false,
        },
    )
    .unwrap();
    assert_eq!(state.recovery_key(), None);
    let join = state
        .draft(Signer::Recovery, Action::RecoveryJoin3(dev3("d-v", &extra(12), false, true)), 20_000)
        .sign(&rsk);
    assert!(matches!(state.clone().apply(&join, &mut h()), Err(ChainError::NotAllowed { .. })));
    // A device that came back with the words, approved, then demoted: likewise.
    let rsk2 = SigningKey::from_bytes(&bytes32(811));
    push(
        &mut state,
        by0(),
        k0,
        Action::SetRecoveryKey(Some(RecoveryKey::new(&rsk2, "acct-1", &ak))),
    )
    .unwrap();
    let back = state
        .draft(Signer::Recovery, Action::RecoveryJoin3(dev3("d-r", &extra(13), false, true)), 21_000)
        .sign(&rsk2);
    state.apply(&back, &mut h()).unwrap();
    push(&mut state, by0(), k0, Action::ApproveDevice("d-r".into())).unwrap();
    assert!(state.recovery_key().is_some());
    push(
        &mut state,
        by0(),
        k0,
        Action::SetAdmin {
            id: "d-r".into(),
            admin: false,
        },
    )
    .unwrap();
    assert_eq!(state.recovery_key(), None);
}

/// The v2 chain the recovery-key rules must leave alone: dev-r comes back
/// with the words, is approved, the owner sets new words, and dev-r is
/// removed the v2 way — then, in a second account state, the new words'
/// setter is gone too. Its state, as JSON, is frozen below.
pub(crate) fn v2_removal_chain() -> (ChainState, Vec<Entry>) {
    let f = fixture();
    let mut entries = sample_chain(&f);
    let mut state = ChainState::verify(&entries, None, &mut h()).unwrap();
    let k0 = &f.devices[0].0;
    let ak = f.ak.verifying_key().to_bytes();
    let join = state
        .draft(Signer::Recovery, Action::RecoveryJoin(crate::chain::tests::recovered_device(&f)), 6_000)
        .sign(&f.recovery.signing);
    state.apply(&join, &mut h()).unwrap();
    entries.push(join);
    entries.push(push(&mut state, by0(), k0, Action::ApproveDevice("dev-r".into())).unwrap());
    let rsk2 = SigningKey::from_bytes(&bytes32(820));
    let new_words = Action::SetRecoveryKey(Some(RecoveryKey::new(&rsk2, "acct-1", &ak)));
    entries.push(push(&mut state, by0(), k0, new_words).unwrap());
    entries.push(push(&mut state, by0(), k0, Action::RemoveDevice("dev-r".into())).unwrap());
    assert!(entries.iter().all(|e| !e.is_v3()), "a v2 chain");
    (state, entries)
}

/// Write gate, should-fix: the recovery-key rule changed for v3 only — a v2
/// chain with removals replays to exactly the state it did before (its JSON
/// was recorded on the code before the change), or old and new verifiers
/// would split.
#[test]
fn w2_a_v2_chain_with_removals_replays_exactly_as_before() {
    let (state, _) = v2_removal_chain();
    let json = crate::vectors::v3_state_json(&state, &["m-1"]).to_string();
    assert_eq!(json, V2_REMOVAL_STATE, "{json}");
    assert_eq!(state.recovery_key(), None, "v2 keeps its rule: the removed device came back with words");
}

/// Recorded before the v3-only rule: v2's removal of a device
/// that came back with the words voids the account's words, whichever.
const V2_REMOVAL_STATE: &str = r#"{"accept":[{"from":{"kind":"admins"},"host":"m-1"}],"admins":["dev-0"],"devices":[{"activatedSeq":0,"addedSeq":0,"admin":true,"host":false,"id":"dev-0","status":"active","viaPasskey":null,"viaRecovery":false},{"activatedSeq":1,"addedSeq":1,"admin":true,"host":false,"id":"dev-1","status":"removed","viaPasskey":null,"viaRecovery":false},{"activatedSeq":3,"addedSeq":3,"admin":false,"host":false,"id":"dev-2","status":"active","viaPasskey":null,"viaRecovery":false},{"activatedSeq":6,"addedSeq":5,"admin":true,"host":false,"id":"dev-r","status":"removed","viaPasskey":null,"viaRecovery":true},{"activatedSeq":2,"addedSeq":2,"admin":false,"host":true,"id":"m-1","status":"active","viaPasskey":null,"viaRecovery":false}],"head":"c275baf73d2604f37e294c5c11d23afd83db4f54a83d6bdf4ad816e36a56e823","machineKeyEpochs":{"m-1":0},"machines":["m-1"],"recoveryKey":null,"seq":8,"slots":[],"userKeyEpochs":0,"v3From":null}"#;

/// Write gate, should-fix: demoting or removing (v3) a device that came back
/// with the recovery words voids them only while they are still the
/// account's words — words the owner set since, which it never knew, stand.
#[test]
fn w3_only_the_words_a_device_knows_go_with_it() {
    let f = fixture();
    let k0 = &f.devices[0].0;
    let ak = f.ak.verifying_key().to_bytes();
    let rsk2 = SigningKey::from_bytes(&bytes32(830));
    let rsk3 = SigningKey::from_bytes(&bytes32(831));
    // d-r comes back with RSK2 and is approved.
    let back_with_rsk2 = || {
        let (mut state, _) = with_second_host(&f);
        push(
            &mut state,
            by0(),
            k0,
            Action::SetRecoveryKey(Some(RecoveryKey::new(&rsk2, "acct-1", &ak))),
        )
        .unwrap();
        let join = state
            .draft(Signer::Recovery, Action::RecoveryJoin3(dev3("d-r", &extra(14), false, true)), 20_000)
            .sign(&rsk2);
        state.apply(&join, &mut h()).unwrap();
        push(&mut state, by0(), k0, Action::ApproveDevice("d-r".into())).unwrap();
        state
    };
    let demote = Action::SetAdmin {
        id: "d-r".into(),
        admin: false,
    };
    let remove = Action::RemoveDevice3("d-r".into());
    for action in [demote.clone(), remove.clone()] {
        // The owner sets RSK3 afterwards: it stands.
        let mut state = back_with_rsk2();
        push(
            &mut state,
            by0(),
            k0,
            Action::SetRecoveryKey(Some(RecoveryKey::new(&rsk3, "acct-1", &ak))),
        )
        .unwrap();
        push(&mut state, by0(), k0, action.clone()).unwrap();
        assert_eq!(state.recovery_key(), Some(rsk3.verifying_key().to_bytes()), "{action:?}");
        // Still RSK2, the words it came back with: they go.
        let mut state = back_with_rsk2();
        push(&mut state, by0(), k0, action.clone()).unwrap();
        assert_eq!(state.recovery_key(), None, "{action:?}");
    }
}

// ---------------------------------------------------------------- S1-4. the local install as a device

/// S1-4: whose machine keys a device may hold, and whom it may reach, is one
/// list — the hosts that admit it (`hosts_admitting`): every other current
/// host for an admin; for anyone else (a host included: the local install)
/// only the hosts whose accept policy names it or all; never itself, never a
/// host that stopped running AI or was removed, nothing once it is removed.
#[test]
fn s14_the_hosts_a_device_may_reach_follow_the_accept_policy() {
    let f = fixture();
    let (mut state, _) = with_second_host(&f);
    let k0 = &f.devices[0].0;
    let ids = |s: &ChainState, d: &str| s.hosts_admitting(d).map(|r| r.info.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(&state, "dev-0"), ["h-2", "m-1"], "an admin reaches every host");
    assert!(ids(&state, "m-1").is_empty(), "by default a host admits admins only");
    assert!(ids(&state, "dev-2").is_empty());
    assert!(ids(&state, "dev-1").is_empty(), "removed");
    assert!(ids(&state, "nobody").is_empty());
    // h-2 lets m-1 (the local install, a host that is not an admin) in.
    push(
        &mut state,
        by0(),
        k0,
        Action::SetAccept {
            host: "h-2".into(),
            from: Accept::Devices(vec!["m-1".into()]),
        },
    )
    .unwrap();
    assert_eq!(ids(&state, "m-1"), ["h-2"]);
    assert!(ids(&state, "dev-2").is_empty(), "a list names whom it admits");
    assert_eq!(ids(&state, "h-2"), Vec::<String>::new(), "m-1 still admits admins only");
    // m-1 lets everyone in: h-2 reaches m-1 — never itself.
    push(
        &mut state,
        by0(),
        k0,
        Action::SetAccept {
            host: "m-1".into(),
            from: Accept::All,
        },
    )
    .unwrap();
    assert_eq!(ids(&state, "h-2"), ["m-1"]);
    assert_eq!(ids(&state, "dev-2"), ["m-1"]);
    // A host that stops running AI is reached by nobody, and itself still reaches h-2.
    push(
        &mut state,
        by0(),
        k0,
        Action::SetHost {
            id: "m-1".into(),
            host: false,
        },
    )
    .unwrap();
    assert!(ids(&state, "h-2").is_empty());
    assert_eq!(ids(&state, "m-1"), ["h-2"], "still a device: it controls what admits it");
    // Removed: nothing.
    push(&mut state, by0(), k0, Action::RemoveDevice3("m-1".into())).unwrap();
    assert!(ids(&state, "m-1").is_empty());
    assert_eq!(ids(&state, "dev-0"), ["h-2"]);
}

/// S1-4: the local install is a host that is not an admin. An admin seals it
/// another host's committed machine key (purpose 5's sealed part) to its one
/// identity — the machine key it enrolled with — and it opens it; the same
/// bytes do not open as the host's own machine key (purpose 2), nor as a key
/// for the host it is, and nothing it seals itself opens anywhere.
#[test]
fn s14_a_host_opens_another_hosts_key_an_admin_sealed_to_its_one_identity() {
    use crate::wrap::{open_host_key, open_machine_key, seal_host_key, HostKeyContext, Purpose, SealContext, SealedKey, WrapError};
    let f = fixture();
    let (state, _) = with_second_host(&f);
    let rng = &mut crate::testing::rng(41);
    let m1 = &f.machine.1;
    let ctx = HostKeyContext {
        account: "acct-1".into(),
        sender: "dev-0".into(),
        recipient: "m-1".into(),
        host: "h-2".into(),
        epoch: 0,
    };
    let sealed = seal_host_key(&f.devices[0].1, &m1.public(), &ctx, &[0x22; 32], rng).unwrap();
    assert_eq!(*open_host_key(m1, "m-1", &state, &ctx, &sealed).unwrap(), vec![0x22; 32]);
    // The same sealed bytes, read as m-1's own machine key: another context, refused.
    let own = SealContext {
        purpose: Purpose::MachineKey,
        account: "acct-1".into(),
        sender: "dev-0".into(),
        recipient: "m-1".into(),
        epoch: 0,
    };
    assert!(open_machine_key(m1, "m-1", &state, &own, &sealed).is_err());
    // Through the mail blob, unchanged.
    let blob = crate::wrap::encode_host_key_mail("h-2", &sealed);
    let (host, back): (String, SealedKey) = crate::wrap::decode_host_key_mail(&blob).unwrap();
    assert_eq!(host, "h-2");
    assert_eq!(
        *open_host_key(m1, "m-1", &state, &HostKeyContext { host, ..ctx.clone() }, &back).unwrap(),
        vec![0x22; 32]
    );
    // A host never hands out keys: m-1 sealing h-2's key for dev-2 opens nowhere.
    let by_m1 = HostKeyContext {
        sender: "m-1".into(),
        recipient: "dev-2".into(),
        ..ctx.clone()
    };
    let sealed = seal_host_key(m1, &f.devices[2].1.public(), &by_m1, &[0x22; 32], rng).unwrap();
    assert!(matches!(
        open_host_key(&f.devices[2].1, "dev-2", &state, &by_m1, &sealed),
        Err(WrapError::UnknownSender(_))
    ));
}

// ---------------------------------------------------------------- host identity renewal

// `docs/host-identity-rotation.md` §3.4 and §8 items 1–3: `RotateHostKey`
// (kind 20) renews a current host's identity key. The fixture: `h-2` joins,
// a plain device `d-c` that it accepts control from is removed, and `h-2`
// renews its key twice (automatically, then confirmed on the device), each
// time with its next machine key epoch right after.

/// The keys `h-2` renews to: `extra(21)` first, `extra(22)` second.
pub(crate) const RENEWED: [u64; 2] = [21, 22];

/// A renewal of `host` to `new`, proved by `old` (the key that signs
/// `old_sig`: the host's current one, honestly) and by `new` against entry
/// `staged_seq` of `state`; the old keys in the proof are the ones `state`
/// names for the host.
pub(crate) fn renewal(
    state: &ChainState,
    host: &str,
    old: &SigningKey,
    new: &(SigningKey, DhKeypair),
    staged_seq: u64,
    nonce: [u8; 16],
    assurance: Assurance,
) -> HostRotation {
    let current = state.device_record(host).map(|d| d.info.clone()).unwrap_or(DeviceKeys {
        id: host.into(),
        sign_key: [0; 32],
        dh_key: [0; 32],
    });
    let mut r = HostRotation {
        host: host.into(),
        sign_key: new.0.verifying_key().to_bytes(),
        dh_key: new.1.public(),
        staged_seq,
        staged_head: state.hash_at(staged_seq).expect("an entry of this view"),
        nonce,
        assurance,
        old_sig: [0; 64],
        new_sig: [0; 64],
    };
    let body = r.body(state.account(), &current.sign_key, &current.dh_key);
    r.old_sig = crate::sig::sign(old, crate::sig::HOST_ROTATE, &body);
    r.new_sig = crate::sig::sign(&new.0, crate::sig::HOST_ROTATE, &body);
    r
}

/// `with_second_host`, then: a plain device `d-c` that `h-2` accepts control
/// from, its removal and `h-2`'s next machine key; `h-2` renewing to
/// `extra(21)` (automatic) and its next machine key; renewing again to
/// `extra(22)` (confirmed on the device, proved against the entry before the
/// head) and its next machine key. With each step's label and seq.
pub(crate) fn rotation_chain(f: &Fixture) -> (ChainState, Vec<Entry>, Vec<(String, u64)>) {
    let (mut state, mut entries) = with_second_host(f);
    let k0 = &f.devices[0].0;
    let mut steps = Vec::new();
    let mut step = |state: &mut ChainState, entries: &mut Vec<Entry>, label: &str, action: Action| {
        entries.push(push(state, by0(), k0, action).unwrap_or_else(|e| panic!("{label}: {e}")));
        steps.push((label.to_string(), state.seq()));
    };
    step(&mut state, &mut entries, "addController", Action::AddDevice3(dev3("d-c", &extra(20), false, false)));
    step(
        &mut state,
        &mut entries,
        "accept",
        Action::SetAccept {
            host: "h-2".into(),
            from: Accept::Devices(vec!["d-c".into()]),
        },
    );
    step(&mut state, &mut entries, "removeController", Action::RemoveDevice3("d-c".into()));
    step(&mut state, &mut entries, "removalKeys", Action::KeyEpochs(mk_commit("h-2", 1, &[0x23; 32])));
    let first = renewal(&state, "h-2", &extra(0).0, &extra(RENEWED[0]), state.seq(), [0x31; 16], Assurance::Auto);
    step(&mut state, &mut entries, "rotate", Action::RotateHostKey(first));
    step(&mut state, &mut entries, "rotateKey", Action::KeyEpochs(mk_commit("h-2", 2, &[0x24; 32])));
    let second = renewal(
        &state,
        "h-2",
        &extra(RENEWED[0]).0,
        &extra(RENEWED[1]),
        state.seq() - 1,
        [0x32; 16],
        Assurance::OnDevice,
    );
    step(&mut state, &mut entries, "rotateAgain", Action::RotateHostKey(second));
    step(&mut state, &mut entries, "rotateAgainKey", Action::KeyEpochs(mk_commit("h-2", 3, &[0x25; 32])));
    (state, entries, steps)
}

pub(crate) fn seq_of(steps: &[(String, u64)], label: &str) -> u64 {
    steps.iter().find(|(l, _)| l == label).map(|(_, s)| *s).expect("a step")
}

/// The chain up to and including step `label`, verified.
pub(crate) fn upto(entries: &[Entry], steps: &[(String, u64)], label: &str) -> ChainState {
    ChainState::verify(&entries[..=seq_of(steps, label) as usize], None, &mut h()).unwrap()
}

fn rule(seq: u64, reason: &'static str) -> Result<(), ChainError> {
    Err(ChainError::NotAllowed { seq, reason })
}

fn public(k: &(SigningKey, DhKeypair)) -> ([u8; 32], [u8; 32]) {
    (k.0.verifying_key().to_bytes(), k.1.public())
}

/// §3.4 effects: the id and everything the chain says about it stay — flags,
/// accept policy, machine key epochs, `ran_ai` — the old keys are retired
/// (`keys_at` before and after), the new ones are what a handshake and
/// `may_control` use, and the next machine key epoch comes next.
#[test]
fn r1_a_renewal_keeps_the_id_and_retires_the_old_keys() {
    let f = fixture();
    let (state, entries, steps) = rotation_chain(&f);
    let (old, first, second) = (extra(0), extra(RENEWED[0]), extra(RENEWED[1]));
    let rotate = seq_of(&steps, "rotate");
    let again = seq_of(&steps, "rotateAgain");
    let h2 = state.machine("h-2").expect("still a current host");
    assert_eq!((h2.info.sign_key, h2.info.dh_key), public(&second));
    assert!(h2.host && !h2.admin && h2.ran_ai && h2.is_active());
    assert_eq!((h2.added_seq, h2.activated_seq, h2.rotated_seq), (5, Some(5), Some(again)));
    assert_eq!(
        state.retired_keys("h-2"),
        &[
            RetiredKeys {
                sign_key: public(&old).0,
                dh_key: public(&old).1,
                until_seq: rotate,
                by_assured: false,
            },
            RetiredKeys {
                sign_key: public(&first).0,
                dh_key: public(&first).1,
                until_seq: again,
                by_assured: true,
            },
        ]
    );
    assert!(state.retired_keys("m-1").is_empty() && state.retired_keys("nobody").is_empty());
    assert_eq!(state.accept_policy("h-2"), Accept::Devices(vec!["d-c".into()]), "its policy stays");
    assert_eq!(state.machine_key_epochs("h-2"), 4, "its machine keys go on from where they were");
    state.check_machine_key("h-2", 1, &[0x23; 32]).unwrap();
    // keys_at: the old keys up to the entry before the renewal, the new ones from it on.
    let at = |seq| state.keys_at("h-2", seq).map(|k| (k.sign_key, k.dh_key));
    assert_eq!(at(4), None, "before it joined");
    assert_eq!(at(5), Some(public(&old)));
    assert_eq!(at(rotate - 1), Some(public(&old)));
    assert_eq!(at(rotate), Some(public(&first)));
    assert_eq!(at(again - 1), Some(public(&first)));
    assert_eq!(at(again), Some(public(&second)));
    assert_eq!(at(state.seq()), Some(public(&second)));
    assert_eq!(at(state.seq() + 1), None, "past this view");
    assert_eq!(
        state.keys_at("m-1", 2).map(|k| k.sign_key),
        Some(f.machine.0.verifying_key().to_bytes())
    );
    // ever_named: current, retired, any device's (removed ones too), either kind.
    for key in [
        public(&old).0,
        public(&old).1,
        public(&first).0,
        public(&second).1,
        public(&extra(20)).0,
        f.devices[1].1.public(),
    ] {
        assert!(state.ever_named(&key));
    }
    assert!(!state.ever_named(&public(&extra(23)).0) && !state.ever_named(&[7; 32]));
    // Only the new keys open a channel; control is unchanged.
    state.authorize_device("h-2", &second.1.public()).unwrap();
    for gone in [&old, &first] {
        assert!(state.authorize_device("h-2", &gone.1.public()).is_err());
    }
    state.may_control("dev-0", "h-2").unwrap();
    // Each renewal owes the host's next machine key epoch at once.
    let before = upto(&entries, &steps, "rotate");
    assert_eq!(before.host_key_due(), Some("h-2"));
    assert_eq!(
        try_push(&before, by0(), &f.devices[0].0, Action::RemoveDevice3("dev-2".into())),
        rule(rotate + 1, rules::HOST_KEY_NEXT)
    );
    assert_eq!(
        try_push(&before, by0(), &f.devices[0].0, Action::KeyEpochs(mk_commit("m-1", 0, &[1; 32]))),
        rule(rotate + 1, rules::HOST_KEY_NEXT),
        "another host's key does not pay it"
    );
    assert_eq!(state.host_key_due(), None);
    // Every entry round-trips, and the chain replays to the same state.
    for e in &entries {
        assert_eq!(&Entry::decode(&e.encode()).unwrap(), e);
    }
    assert_eq!(ChainState::verify(&entries, None, &mut h()).unwrap(), state);
}

/// Encoding: kind 20 exists in v3 only, its assurance is 0 or 1 and nothing
/// else, and a draft that says v2 is refused.
#[test]
fn r1_kind_20_is_v3_only_and_its_assurance_is_one_bit() {
    let f = fixture();
    let (_, entries, steps) = rotation_chain(&f);
    for label in ["rotate", "rotateAgain"] {
        let e = &entries[seq_of(&steps, label) as usize];
        assert_eq!((e.draft.version, e.draft.action.kind()), (VERSION_3, 20));
        let bytes = e.encode();
        // u8 assurance sits right before the two signatures (and the entry's own).
        let at = bytes.len() - 64 * 3 - 1;
        assert_eq!(bytes[at], u8::from(label == "rotateAgain"));
        for bad in [2, 3, 0x80, 0xff] {
            let mut b = bytes.clone();
            b[at] = bad;
            assert!(matches!(Entry::decode(&b), Err(ChainError::Decode(_))), "{label} {bad}");
        }
        let mut b = bytes.clone();
        b[0] = VERSION;
        assert!(matches!(Entry::decode(&b), Err(ChainError::Decode(_))), "kind 20 under version 2");
    }
    let prefix = upto(&entries, &steps, "removalKeys");
    let r = renewal(&prefix, "h-2", &extra(0).0, &extra(21), prefix.seq(), [1; 16], Assurance::Auto);
    let mut v2 = prefix.draft(by0(), Action::RotateHostKey(r), 1);
    assert_eq!(v2.version, VERSION_3, "kind 20 is drafted as v3");
    v2.version = VERSION;
    assert_eq!(
        prefix.clone().apply(&v2.sign(&f.devices[0].0), &mut h()),
        Err(ChainError::Version(VERSION))
    );
    // On a chain still at v2, kind 20 is drafted as v3 (and so would move the chain to v3).
    let v2_chain = ChainState::verify(&sample_chain(&f), None, &mut h()).unwrap();
    let r = renewal(&v2_chain, "m-1", &f.machine.0, &extra(21), v2_chain.seq(), [1; 16], Assurance::Auto);
    assert_eq!(v2_chain.draft(by0(), Action::RotateHostKey(r), 1).version, VERSION_3);
}

/// Rules 1–5 and 7 of §3.4, each failing on its own with its own reason.
#[test]
fn r1_each_renewal_rule_fails_on_its_own() {
    let f = fixture();
    let (_, entries, steps) = rotation_chain(&f);
    let k0 = &f.devices[0].0;
    let base = upto(&entries, &steps, "removalKeys");
    let seq = base.seq() + 1;
    let ok = |s: &ChainState, host: &str, old: &SigningKey| renewal(s, host, old, &extra(21), s.seq(), [9; 16], Assurance::Auto);
    let attempt = |s: &ChainState, r: HostRotation| try_push(s, by0(), k0, Action::RotateHostKey(r));
    // The honest ones go through.
    attempt(&base, ok(&base, "h-2", &extra(0).0)).unwrap();
    attempt(&base, ok(&base, "m-1", &f.machine.0)).unwrap();

    // Rule 2: a browser or phone, an admin, an unknown id, a pending device, a
    // removed host, a host that stopped.
    assert_eq!(attempt(&base, ok(&base, "dev-2", &f.devices[2].0)), rule(seq, rules::ROTATE_NOT_HOST));
    assert_eq!(attempt(&base, ok(&base, "dev-0", k0)), rule(seq, rules::ROTATE_NOT_HOST), "an admin");
    assert_eq!(attempt(&base, ok(&base, "h-9", k0)), rule(seq, rules::ROTATE_NOT_HOST), "nobody");
    let mut pending = base.clone();
    push(
        &mut pending,
        Signer::Recovery,
        &f.recovery.signing,
        Action::RecoveryJoin3(dev3("dev-r", &extra(4), false, true)),
    )
    .unwrap();
    assert_eq!(
        attempt(&pending, ok(&pending, "dev-r", &extra(4).0)),
        rule(seq + 1, rules::ROTATE_NOT_HOST),
        "a pending device"
    );
    let mut removed = base.clone();
    push(&mut removed, by0(), k0, Action::RemoveDevice3("m-1".into())).unwrap();
    assert_eq!(
        attempt(&removed, ok(&removed, "m-1", &f.machine.0)),
        rule(seq + 1, rules::ROTATE_NOT_HOST),
        "a removed host"
    );
    let mut stopped = base.clone();
    push(
        &mut stopped,
        by0(),
        k0,
        Action::SetHost {
            id: "m-1".into(),
            host: false,
        },
    )
    .unwrap();
    assert_eq!(
        attempt(&stopped, ok(&stopped, "m-1", &f.machine.0)),
        rule(seq + 1, rules::ROTATE_NOT_HOST),
        "a host that stopped"
    );

    // Rule 3: weak keys; the host's own current keys; another device's
    // (current or removed); a retired key.
    let mut identity = [0u8; 32];
    identity[0] = 1;
    let mut weak = ok(&base, "h-2", &extra(0).0);
    weak.sign_key = identity;
    assert_eq!(attempt(&base, weak), Err(ChainError::WeakKey { seq }));
    let mut weak = ok(&base, "h-2", &extra(0).0);
    weak.dh_key = [0; 32];
    assert_eq!(attempt(&base, weak), Err(ChainError::WeakKey { seq }));
    let to = |s: &ChainState, old: &SigningKey, new: (SigningKey, DhKeypair)| renewal(s, "h-2", old, &new, s.seq(), [9; 16], Assurance::Auto);
    let o = &extra(0).0;
    assert_eq!(attempt(&base, to(&base, o, extra(0))), rule(seq, rules::ROTATE_KEY_REUSED), "its current keys");
    let dev0 = (f.devices[0].0.clone(), DhKeypair::from_secret(bytes32(300)));
    assert_eq!(attempt(&base, to(&base, o, dev0)), rule(seq, rules::ROTATE_KEY_REUSED), "an admin's keys");
    let m1_dh = (SigningKey::from_bytes(&bytes32(990)), DhKeypair::from_secret(bytes32(401)));
    assert_eq!(attempt(&base, to(&base, o, m1_dh)), rule(seq, rules::ROTATE_KEY_REUSED), "another host's DH key");
    let removed_dev = (f.devices[1].0.clone(), DhKeypair::from_secret(bytes32(991)));
    assert_eq!(attempt(&base, to(&base, o, removed_dev)), rule(seq, rules::ROTATE_KEY_REUSED), "a removed device's key");
    let removed_ctl = (SigningKey::from_bytes(&bytes32(992)), extra(20).1);
    assert_eq!(attempt(&base, to(&base, o, removed_ctl)), rule(seq, rules::ROTATE_KEY_REUSED), "the removed controller's");
    let renewed = upto(&entries, &steps, "rotateKey");
    let n = &extra(21).0;
    assert_eq!(
        attempt(&renewed, to(&renewed, n, extra(0))),
        rule(renewed.seq() + 1, rules::ROTATE_KEY_REUSED),
        "back to a retired key"
    );
    let half = (SigningKey::from_bytes(&bytes32(993)), extra(0).1);
    assert_eq!(
        attempt(&renewed, to(&renewed, n, half)),
        rule(renewed.seq() + 1, rules::ROTATE_KEY_REUSED),
        "a retired DH key with a fresh sign key"
    );

    // Rule 4: a view at or past the entry, a head that is not the chain's.
    let mut ahead = ok(&base, "h-2", &extra(0).0);
    ahead.staged_seq = seq;
    assert_eq!(attempt(&base, ahead), rule(seq, rules::ROTATE_STAGED), "the entry itself");
    let mut far = ok(&base, "h-2", &extra(0).0);
    far.staged_seq = seq + 5;
    assert_eq!(attempt(&base, far), rule(seq, rules::ROTATE_STAGED));
    let mut forked = ok(&base, "h-2", &extra(0).0);
    forked.staged_head = [7; 32];
    assert_eq!(attempt(&base, forked), rule(seq, rules::ROTATE_STAGED), "another head");
    let mut shifted = ok(&base, "h-2", &extra(0).0);
    shifted.staged_seq -= 1;
    assert_eq!(attempt(&base, shifted), rule(seq, rules::ROTATE_STAGED), "the head of another entry");

    // Rule 5: no old signature, the admin's key for the host's, an old
    // signature by the retired key, a bad new one, the two swapped.
    let mut no_old = ok(&base, "h-2", &extra(0).0);
    no_old.old_sig = [0; 64];
    assert_eq!(attempt(&base, no_old), rule(seq, rules::ROTATE_PROOF));
    assert_eq!(attempt(&base, ok(&base, "h-2", k0)), rule(seq, rules::ROTATE_PROOF), "the admin's key");
    assert_eq!(
        attempt(
            &renewed,
            renewal(&renewed, "h-2", &extra(0).0, &extra(22), renewed.seq(), [9; 16], Assurance::Auto)
        ),
        rule(renewed.seq() + 1, rules::ROTATE_PROOF),
        "signed by the retired key"
    );
    let mut bad_new = ok(&base, "h-2", &extra(0).0);
    bad_new.new_sig[0] ^= 1;
    assert_eq!(attempt(&base, bad_new), rule(seq, rules::ROTATE_PROOF));
    let mut swapped = ok(&base, "h-2", &extra(0).0);
    std::mem::swap(&mut swapped.old_sig, &mut swapped.new_sig);
    assert_eq!(attempt(&base, swapped), rule(seq, rules::ROTATE_PROOF));
    // Every field is under both signatures.
    let honest = ok(&base, "h-2", &extra(0).0);
    let mut nonce = honest.clone();
    nonce.nonce[0] ^= 1;
    let mut assured = honest.clone();
    assured.assurance = Assurance::OnDevice;
    let mut view = honest.clone();
    view.staged_seq = 0;
    view.staged_head = base.hash_at(0).unwrap();
    let mut dh = honest.clone();
    dh.dh_key = extra(23).1.public();
    for (what, r) in [("nonce", nonce), ("assurance", assured), ("view", view), ("DH key", dh)] {
        assert_eq!(attempt(&base, r), rule(seq, rules::ROTATE_PROOF), "{what}");
    }

    // Rule 1: only an active admin signs it.
    let r = || Action::RotateHostKey(ok(&base, "h-2", &extra(0).0));
    assert_eq!(
        try_push(&base, Signer::Device("dev-2".into()), &f.devices[2].0, r()),
        rule(seq, rules::ONLY_ADMIN_SIGNS)
    );
    assert_eq!(
        try_push(&base, Signer::Device("h-2".into()), &extra(0).0, r()),
        rule(seq, rules::ONLY_ADMIN_SIGNS),
        "the host itself"
    );
    assert!(matches!(
        try_push(&base, Signer::Device("dev-1".into()), &f.devices[1].0, r()),
        Err(ChainError::UnknownId { .. })
    ));
    assert!(matches!(
        try_push(&base, Signer::Recovery, &f.recovery.signing, r()),
        Err(ChainError::NotAllowed { .. })
    ));

    // Rule 7: a host that owes its machine key epoch gets that first (a new
    // host; a narrowing) — the renewal waits for the next batch.
    let mut joined = base.clone();
    push(&mut joined, by0(), k0, Action::AddDevice3(dev3("h-3", &extra(24), true, false))).unwrap();
    assert_eq!(
        attempt(
            &joined,
            renewal(&joined, "h-3", &extra(24).0, &extra(21), joined.seq(), [9; 16], Assurance::Auto)
        ),
        rule(seq + 1, rules::HOST_KEY_NEXT)
    );
    let mut narrowed = base.clone();
    for from in [Accept::All, Accept::Admins] {
        push(&mut narrowed, by0(), k0, Action::SetAccept { host: "h-2".into(), from }).unwrap();
    }
    assert_eq!(narrowed.host_key_due(), Some("h-2"));
    assert_eq!(
        attempt(&narrowed, ok(&narrowed, "h-2", &extra(0).0)),
        rule(seq + 2, rules::HOST_KEY_NEXT)
    );
}

/// §5.7 attack regressions (§8 items 2–3): one admin alone cannot rebind a
/// host's id to keys of its choosing; an old proof cannot be replayed; a
/// host's identity cannot be collided with another device's key.
#[test]
fn r1_attacks_on_a_hosts_identity_fail() {
    let f = fixture();
    let (state, entries, steps) = rotation_chain(&f);
    let k0 = &f.devices[0].0;
    let base = upto(&entries, &steps, "removalKeys");
    let seq = base.seq() + 1;
    let thief = (SigningKey::from_bytes(&bytes32(994)), DhKeypair::from_secret(bytes32(995)));
    // 1. A stolen admin device rebinds h-2 to the thief's keys: it signs the
    //    entry and the new key's half, never the host's current key's.
    let mut rebind = renewal(&base, "h-2", &thief.0, &thief, base.seq(), [5; 16], Assurance::Auto);
    assert_eq!(
        try_push(&base, by0(), k0, Action::RotateHostKey(rebind.clone())),
        rule(seq, rules::ROTATE_PROOF)
    );
    rebind.old_sig = [0; 64];
    assert_eq!(
        try_push(&base, by0(), k0, Action::RotateHostKey(rebind.clone())),
        rule(seq, rules::ROTATE_PROOF),
        "no old signature at all"
    );
    let (old_sign, old_dh) = public(&extra(0));
    rebind.old_sig = crate::sig::sign(k0, crate::sig::HOST_ROTATE, &rebind.body("acct-1", &old_sign, &old_dh));
    assert_eq!(
        try_push(&base, by0(), k0, Action::RotateHostKey(rebind)),
        rule(seq, rules::ROTATE_PROOF),
        "the admin's own key is not the host's"
    );

    // 2. The hub (or anyone) replays a proof that was already used: at the
    //    next renewal, after the one after, and on a fork that took it.
    let Action::RotateHostKey(used) = entries[seq_of(&steps, "rotate") as usize].draft.action.clone() else {
        panic!("a renewal")
    };
    let renewed = upto(&entries, &steps, "rotateKey");
    assert_eq!(
        try_push(&renewed, by0(), k0, Action::RotateHostKey(used.clone())),
        rule(renewed.seq() + 1, rules::ROTATE_KEY_REUSED)
    );
    assert_eq!(
        try_push(&state, by0(), k0, Action::RotateHostKey(used.clone())),
        rule(state.seq() + 1, rules::ROTATE_KEY_REUSED),
        "after the second renewal too"
    );
    let mut fork = base.clone();
    push(&mut fork, by0(), k0, Action::RotateHostKey(used.clone())).unwrap();
    push(&mut fork, by0(), k0, Action::KeyEpochs(mk_commit("h-2", 2, &[1; 32]))).unwrap();
    assert_eq!(
        try_push(&fork, by0(), k0, Action::RotateHostKey(used)),
        rule(fork.seq() + 1, rules::ROTATE_KEY_REUSED),
        "a proof applies once"
    );

    // 3. A host's identity collided with another device's: dev-0's keys (both
    //    halves signing, as a holder of dev-0's key would), dev-2's DH key alone,
    //    m-1's sign key alone.
    let dev0 = (f.devices[0].0.clone(), DhKeypair::from_secret(bytes32(300)));
    let dev2_dh = (SigningKey::from_bytes(&bytes32(996)), DhKeypair::from_secret(bytes32(302)));
    let m1_sign = (f.machine.0.clone(), DhKeypair::from_secret(bytes32(997)));
    for (what, keys) in [("dev-0", dev0), ("dev-2's DH key", dev2_dh), ("m-1's sign key", m1_sign)] {
        let r = renewal(&base, "h-2", &extra(0).0, &keys, base.seq(), [6; 16], Assurance::Auto);
        assert_eq!(
            try_push(&base, by0(), k0, Action::RotateHostKey(r)),
            rule(seq, rules::ROTATE_KEY_REUSED),
            "{what}"
        );
    }
    // And the old keys open no channel as h-2 once it renewed.
    assert!(state.authorize_device("h-2", &extra(0).1.public()).is_err());
}

/// The helpers later phases use (§3.2, §3.3, §3.7): the DH confirmation MAC
/// from either side, the code, and the new key's acknowledgement.
#[test]
fn r1_the_confirmation_the_code_and_the_ack() {
    let f = fixture();
    let (_, entries, steps) = rotation_chain(&f);
    let Action::RotateHostKey(r) = &entries[seq_of(&steps, "rotate") as usize].draft.action else {
        panic!("a renewal")
    };
    let (old, new) = (extra(0), extra(RENEWED[0]));
    let (old_sign, old_dh) = public(&old);
    let body = r.body("acct-1", &old_sign, &old_dh);
    assert_eq!(body.len(), (2 + 6) + (2 + 3) + 8 + 32 * 5 + 16 + 1);
    assert!(r.verify("acct-1", &old_sign, &old_dh));
    let eph = DhKeypair::from_secret(bytes32(900));
    let mac = host_rotate_confirm(&new.1, &eph.public(), &body).unwrap();
    assert_eq!(mac, rotate_confirm_mac(&eph.agree(&new.1.public()).unwrap(), &body), "the same secret either side");
    assert!(check_rotate_confirm(&eph, &new.1.public(), &body, &mac));
    assert!(!check_rotate_confirm(&eph, &old.1.public(), &body, &mac), "the old DH key proves nothing");
    let mut other = body.clone();
    *other.last_mut().unwrap() = 1;
    assert!(!check_rotate_confirm(&eph, &new.1.public(), &other, &mac));
    assert_eq!(host_rotate_confirm(&new.1, &[0; 32], &body), None, "a low-order ephemeral");
    assert!(!check_rotate_confirm(&eph, &[0; 32], &body, &mac));
    // The code: 8 characters of base32 in two groups of four, from the body alone.
    let code = rotate_sas(&body);
    assert_eq!(code.len(), 9);
    assert_eq!(&code[4..5], "-");
    assert!(code.chars().all(|c| c == '-' || c.is_ascii_uppercase() || ('2'..='7').contains(&c)));
    assert_ne!(rotate_sas(&other), code);
    // The ack: the new key over the account, host, proof hash and this login's nonce.
    let nonce = bytes32(901);
    let ack = sign_rotate_ack(&new.0, "acct-1", "h-2", &body, &nonce);
    let new_sign = new.0.verifying_key().to_bytes();
    assert!(verify_rotate_ack(&new_sign, "acct-1", "h-2", &body, &nonce, &ack));
    assert!(!verify_rotate_ack(&new_sign, "acct-1", "h-2", &body, &bytes32(902), &ack), "another login");
    assert!(!verify_rotate_ack(&new_sign, "acct-1", "m-1", &body, &nonce, &ack));
    assert!(!verify_rotate_ack(&new_sign, "acct-2", "h-2", &body, &nonce, &ack));
    assert!(!verify_rotate_ack(&new_sign, "acct-1", "h-2", &other, &nonce, &ack));
    assert!(!verify_rotate_ack(&old_sign, "acct-1", "h-2", &body, &nonce, &ack));
    // An ack is no proof signature, and a proof signature is no ack.
    assert!(!crate::sig::verify(
        &new_sign,
        crate::sig::HOST_ROTATE,
        &rotate_ack_body("acct-1", "h-2", &body, &nonce),
        &ack
    ));
    assert!(!verify_rotate_ack(&new_sign, "acct-1", "h-2", &body, &nonce, &r.new_sig));
}

/// A key a renewal retired never names a device again (lead decision on
/// R1): not added by an admin or a passkey, not enrolled as a host, not back
/// with the recovery words — sign key or DH key, the first renewal's or the
/// second's. A key a *current* device holds is not refused (chains from
/// before may name one twice), and no chain from before holds a retired key,
/// so every one of them replays as it did.
#[test]
fn r1_a_retired_key_never_names_a_device_again() {
    let f = fixture();
    let (state, entries, steps) = rotation_chain(&f);
    let k0 = &f.devices[0].0;
    let seq = state.seq() + 1;
    let (old, first) = (extra(0), extra(RENEWED[0]));
    for key in [public(&old).0, public(&old).1, public(&first).0, public(&first).1] {
        assert!(state.is_retired_key(&key));
    }
    assert!(!state.is_retired_key(&public(&extra(RENEWED[1])).0), "h-2's current key is not retired");
    assert!(!state.is_retired_key(&f.devices[1].1.public()), "nor a removed device's");
    let fresh_dh = || DhKeypair::from_secret(bytes32(980));
    let fresh_sign = || SigningKey::from_bytes(&bytes32(981));
    let with_sign = |k: &SigningKey| (k.clone(), fresh_dh());
    let with_dh = |d: &DhKeypair| (fresh_sign(), DhKeypair::from_secret(*d.secret()));
    let add = |id: &str, keys: &(SigningKey, DhKeypair), host: bool| Action::AddDevice3(dev3(id, keys, host, false));
    for (what, action) in [
        ("the first retired sign key", add("d-k", &with_sign(&old.0), false)),
        ("the first retired DH key", add("d-k", &with_dh(&old.1), false)),
        ("the second retired sign key", add("d-k", &with_sign(&first.0), false)),
        ("the second retired DH key", add("d-k", &with_dh(&first.1), false)),
        ("a host enrolled with them", add("h-k", &first, true)),
    ] {
        assert_eq!(try_push(&state, by0(), k0, action), rule(seq, rules::KEY_RETIRED), "{what}");
    }
    try_push(&state, by0(), k0, add("d-k", &(fresh_sign(), fresh_dh()), false)).expect("fresh keys go in");
    // Back with the recovery words, or joining with a passkey: refused the same way.
    assert_eq!(
        try_push(
            &state,
            Signer::Recovery,
            &f.recovery.signing,
            Action::RecoveryJoin3(dev3("d-r", &with_dh(&old.1), false, true))
        ),
        rule(seq, rules::KEY_RETIRED)
    );
    let mut slotted = state.clone();
    let p = pak(&f, "p-1", 800, 1, true);
    push(&mut slotted, by0(), k0, Action::PasskeySlotSet(p.slot.clone())).unwrap();
    assert_eq!(
        try_push(
            &slotted,
            Signer::Passkey("p-1".into()),
            &p.signing,
            Action::AddDevice3(dev3("d-p", &with_sign(&first.0), false, false))
        ),
        rule(seq + 1, rules::KEY_RETIRED)
    );
    // Before the renewal that retired it, the same key was only a current
    // device's: a second device naming it is what chains always allowed.
    let before = upto(&entries, &steps, "removalKeys");
    try_push(&before, by0(), k0, add("d-k", &with_dh(&old.1), false)).expect("a current device's key is not refused");
    // A refused join changes nothing: the state is as it was.
    let mut same = state.clone();
    assert!(push(&mut same, by0(), k0, add("d-k", &with_sign(&old.0), false)).is_err());
    assert_eq!(same, state);
}

/// Security review: a pending join made with a host's key
/// before its renewal does not come alive after it — not approved by an
/// admin, not activated with the recovery words, not activated by its passkey
/// (a thief with the old key and a synced passkey would otherwise join early
/// and activate late). Without the renewal in between, each activation goes
/// through as before.
#[test]
fn r1b_a_retired_key_never_activates_a_device() {
    let f = fixture();
    let (_, entries, steps) = rotation_chain(&f);
    let k0 = &f.devices[0].0;
    let rsk = &f.recovery.signing;
    let base = upto(&entries, &steps, "removalKeys");
    let renew = |s: &mut ChainState| {
        let r = renewal(s, "h-2", &extra(0).0, &extra(RENEWED[0]), s.seq(), [9; 16], Assurance::Auto);
        push(s, by0(), k0, Action::RotateHostKey(r)).unwrap();
        let epoch = s.machine_key_epochs("h-2");
        push(s, by0(), k0, Action::KeyEpochs(mk_commit("h-2", epoch, &[0x26; 32]))).unwrap();
    };
    // Back with the recovery words, with h-2's keys (a copy of its seed), while they were current.
    let mut joined = base.clone();
    push(
        &mut joined,
        Signer::Recovery,
        rsk,
        Action::RecoveryJoin3(dev3("d-k0", &extra(0), false, true)),
    )
    .expect("a current device's keys are not refused");
    try_push(&joined, by0(), k0, Action::ApproveDevice("d-k0".into())).expect("approved before the renewal");
    try_push(&joined, Signer::Recovery, rsk, Action::ActivateRecovered("d-k0".into())).expect("activated before it");
    let mut renewed = joined.clone();
    renew(&mut renewed);
    let seq = renewed.seq() + 1;
    assert_eq!(
        try_push(&renewed, by0(), k0, Action::ApproveDevice("d-k0".into())),
        rule(seq, rules::KEY_RETIRED)
    );
    assert_eq!(
        try_push(&renewed, Signer::Recovery, rsk, Action::ActivateRecovered("d-k0".into())),
        rule(seq, rules::KEY_RETIRED)
    );
    assert!(matches!(renewed.device_record("d-k0").unwrap().status, DeviceStatus::Pending { .. }));
    // Joined with a passkey, the same.
    let p = pak(&f, "p-1", 800, 1, true);
    let by_pk = || Signer::Passkey("p-1".into());
    let mut slotted = base.clone();
    push(&mut slotted, by0(), k0, Action::PasskeySlotSet(p.slot.clone())).unwrap();
    push(&mut slotted, by_pk(), &p.signing, Action::AddDevice3(dev3("d-k0", &extra(0), false, false))).unwrap();
    try_push(&slotted, by_pk(), &p.signing, Action::PasskeyActivate("d-k0".into())).expect("activated before the renewal");
    renew(&mut slotted);
    assert_eq!(
        try_push(&slotted, by_pk(), &p.signing, Action::PasskeyActivate("d-k0".into())),
        rule(slotted.seq() + 1, rules::KEY_RETIRED)
    );
    assert_eq!(
        try_push(&slotted, by0(), k0, Action::ApproveDevice("d-k0".into())),
        rule(slotted.seq() + 1, rules::KEY_RETIRED),
        "nor does an admin's approval bring it in"
    );
    // A pending device whose keys were never retired is approved as before.
    let mut other = renewed.clone();
    push(
        &mut other,
        Signer::Recovery,
        rsk,
        Action::RecoveryJoin3(dev3("d-ok", &extra(25), false, true)),
    )
    .unwrap();
    try_push(&other, by0(), k0, Action::ApproveDevice("d-ok".into())).expect("fresh keys are approved");
}

/// §3.8 「证据：退役钥签的争议声明」: the key a renewal retired signs a
/// statement bound to that renewal's hash, and a verifier checks it against
/// its own chain — no clocks to compare. Only that entry, that hash, that new
/// key, and the key that entry retired make one that verifies.
#[test]
fn r1c_a_dispute_names_the_renewal_by_its_hash() {
    let f = fixture();
    let (state, entries, steps) = rotation_chain(&f);
    let (rotate, again) = (seq_of(&steps, "rotate"), seq_of(&steps, "rotateAgain"));
    let (old, first, second) = (extra(0), extra(RENEWED[0]), extra(RENEWED[1]));
    let hash = |seq: u64| entries[seq as usize].hash();
    let sign = |key: &SigningKey, host: &str, seq: u64, entry_hash: [u8; 32], new_sign: [u8; 32]| {
        sign_rotate_dispute(key, "acct-1", host, seq, &entry_hash, &new_sign)
    };
    let new_of = |k: &(SigningKey, DhKeypair)| k.0.verifying_key().to_bytes();
    let body = rotate_dispute_body("acct-1", "h-2", rotate, &hash(rotate), &new_of(&first));
    assert_eq!(body.len(), (2 + 6) + (2 + 3) + 8 + 32 + 32);
    let honest = sign(&old.0, "h-2", rotate, hash(rotate), new_of(&first));
    assert!(state.verify_rotate_dispute("h-2", rotate, &honest));
    // The second renewal is disputed by the key it retired (the first renewal's), and only by it.
    let second_stmt = sign(&first.0, "h-2", again, hash(again), new_of(&second));
    assert!(state.verify_rotate_dispute("h-2", again, &second_stmt));
    assert!(!state.verify_rotate_dispute("h-2", again, &sign(&old.0, "h-2", again, hash(again), new_of(&second))));
    assert!(!state.verify_rotate_dispute("h-2", again, &honest), "one renewal's statement for another");
    assert!(!state.verify_rotate_dispute("h-2", rotate, &second_stmt));
    // By the new key, another device, for another host, over another hash or new key, elsewhere in the chain.
    let k0 = &f.devices[0].0;
    for (what, sig) in [
        ("the new key", sign(&first.0, "h-2", rotate, hash(rotate), new_of(&first))),
        ("another device", sign(k0, "h-2", rotate, hash(rotate), new_of(&first))),
        ("another hash", sign(&old.0, "h-2", rotate, [7; 32], new_of(&first))),
        ("another new key", sign(&old.0, "h-2", rotate, hash(rotate), [7; 32])),
        ("another account", sign_rotate_dispute(&old.0, "acct-2", "h-2", rotate, &hash(rotate), &new_of(&first))),
        ("another seq in the body", sign(&old.0, "h-2", rotate + 1, hash(rotate), new_of(&first))),
    ] {
        assert!(!state.verify_rotate_dispute("h-2", rotate, &sig), "{what}");
    }
    assert!(!state.verify_rotate_dispute("m-1", rotate, &sign(&old.0, "m-1", rotate, hash(rotate), new_of(&first))), "another host");
    let keys = seq_of(&steps, "rotateKey");
    assert!(
        !state.verify_rotate_dispute("h-2", keys, &sign(&first.0, "h-2", keys, hash(keys), new_of(&first))),
        "an entry that is not a renewal"
    );
    for seq in [0, state.seq() + 1, u64::MAX] {
        assert!(!state.verify_rotate_dispute("h-2", seq, &honest), "seq {seq}");
    }
    // A view without the renewal cannot check it; one with it can, however far it went on.
    let before = upto(&entries, &steps, "removalKeys");
    assert!(!before.verify_rotate_dispute("h-2", rotate, &honest));
    assert!(upto(&entries, &steps, "rotate").verify_rotate_dispute("h-2", rotate, &honest));
    // A statement is no other signature, and no other signature is a statement.
    assert!(!crate::sig::verify(&new_of(&old), crate::sig::HOST_ROTATE, &body, &honest));
    let Action::RotateHostKey(r) = &entries[rotate as usize].draft.action else {
        panic!("a renewal")
    };
    assert!(!state.verify_rotate_dispute("h-2", rotate, &r.old_sig));
}

/// §3.4 rule 9: the account's renewal policy is on the chain. Only an
/// active admin sets it; while it is on, a renewal that was not confirmed on
/// the device is refused, an assured one goes through; turned off, automatic
/// renewals go through again. Kind 21 exists in v3 only, its byte is 0 or 1.
#[test]
fn r1d_the_renewal_policy_asks_for_renewals_confirmed_on_the_device() {
    let f = fixture();
    let (_, entries, steps) = rotation_chain(&f);
    let k0 = &f.devices[0].0;
    let mut state = upto(&entries, &steps, "removalKeys");
    assert!(!state.always_assured() && state.renewal_policy_seq().is_none(), "off until an admin turns it on");
    let policy = |on: bool| Action::SetRenewalPolicy { always_assured: on };
    // Only an active admin sets it: not a plain device, not the host, not the recovery words.
    let seq = state.seq() + 1;
    assert_eq!(
        try_push(&state, Signer::Device("dev-2".into()), &f.devices[2].0, policy(true)),
        rule(seq, rules::ONLY_ADMIN_SIGNS)
    );
    assert_eq!(
        try_push(&state, Signer::Device("h-2".into()), &extra(0).0, policy(true)),
        rule(seq, rules::ONLY_ADMIN_SIGNS)
    );
    assert!(matches!(
        try_push(&state, Signer::Recovery, &f.recovery.signing, policy(true)),
        Err(ChainError::NotAllowed { .. })
    ));
    let on = push(&mut state, by0(), k0, policy(true)).unwrap();
    assert!(state.always_assured() && state.renewal_policy_seq() == Some(on.seq()));
    let again = push(&mut state, by0(), k0, policy(true)).expect("setting it again is allowed");
    assert_eq!(state.renewal_policy_seq(), Some(again.seq()));
    // While on: assurance 0 refused, for that reason alone; assurance 1 taken.
    let auto = renewal(&state, "h-2", &extra(0).0, &extra(RENEWED[0]), state.seq(), [9; 16], Assurance::Auto);
    assert_eq!(
        try_push(&state, by0(), k0, Action::RotateHostKey(auto.clone())),
        rule(state.seq() + 1, rules::ROTATE_NEEDS_ASSURANCE)
    );
    let mut off_view = state.clone();
    push(&mut off_view, by0(), k0, policy(false)).unwrap();
    let auto_later = renewal(&off_view, "h-2", &extra(0).0, &extra(RENEWED[0]), off_view.seq(), [9; 16], Assurance::Auto);
    try_push(&off_view, by0(), k0, Action::RotateHostKey(auto_later)).expect("off again: automatic renewals go through");
    let assured = renewal(&state, "h-2", &extra(0).0, &extra(RENEWED[0]), state.seq(), [9; 16], Assurance::OnDevice);
    push(&mut state, by0(), k0, Action::RotateHostKey(assured)).unwrap();
    let epoch = state.machine_key_epochs("h-2");
    push(&mut state, by0(), k0, Action::KeyEpochs(mk_commit("h-2", epoch, &[0x27; 32]))).unwrap();
    assert!(state.retired_keys("h-2")[0].by_assured);
    // The other rules still come first: a bad proof is refused as one.
    let mut bad_proof = renewal(&state, "h-2", &extra(RENEWED[0]).0, &extra(RENEWED[1]), state.seq(), [9; 16], Assurance::Auto);
    bad_proof.old_sig = [0; 64];
    assert_eq!(
        try_push(&state, by0(), k0, Action::RotateHostKey(bad_proof)),
        rule(state.seq() + 1, rules::ROTATE_PROOF)
    );
    // Off: automatic renewals again.
    let off = push(&mut state, by0(), k0, policy(false)).unwrap();
    assert!(!state.always_assured() && state.renewal_policy_seq() == Some(off.seq()));
    let auto = renewal(&state, "h-2", &extra(RENEWED[0]).0, &extra(RENEWED[1]), state.seq(), [9; 16], Assurance::Auto);
    push(&mut state, by0(), k0, Action::RotateHostKey(auto)).unwrap();
    // Encoding: kind 21, one byte 0 or 1; nothing else decodes, nor kind 21 under v2.
    for (e, byte) in [(&on, 1u8), (&off, 0u8)] {
        let bytes = e.encode();
        assert_eq!((e.draft.version, e.draft.action.kind()), (VERSION_3, 21));
        assert_eq!(&bytes[bytes.len() - 66..bytes.len() - 64], &[21, byte]);
        assert_eq!(&Entry::decode(&bytes).unwrap(), e);
        for bad in [2u8, 0x80, 0xff] {
            let mut b = bytes.clone();
            let at = b.len() - 65;
            b[at] = bad;
            assert!(matches!(Entry::decode(&b), Err(ChainError::Decode(_))), "{bad}");
        }
        let mut b = bytes.clone();
        b[0] = VERSION;
        assert!(matches!(Entry::decode(&b), Err(ChainError::Decode(_))), "kind 21 under version 2");
    }
    // A draft that says v2 is refused for its version; replay gives the same state.
    let base = upto(&entries, &steps, "removalKeys");
    let mut v2 = base.draft(by0(), policy(true), 1);
    assert_eq!(v2.version, VERSION_3);
    v2.version = VERSION;
    assert_eq!(base.clone().apply(&v2.sign(k0), &mut h()), Err(ChainError::Version(VERSION)));
    assert_eq!(ChainState::verify(state.entries(), None, &mut h()).unwrap(), state);
}

/// §3.4 rule 10 (overlay, after the final review): a renewal pause narrows
/// the effective accept of its hosts — base minus every open pause's
/// removals; never an admin (D1) — without touching the base; `may_control`,
/// `authorize_control` and `hosts_admitting` follow it; a hand `SetAccept`
/// during the pause changes only the base; completion (by any admin) lifts
/// it; a pause whose starter was demoted stays in force; removals are
/// canonical and the entry fits every transport.
#[test]
fn r1e_a_renewal_pause_overlays_the_accept_policy_and_leaves_the_base_alone() {
    let f = fixture();
    let (_, entries, steps) = rotation_chain(&f);
    let k0 = &f.devices[0].0;
    let mut st = upto(&entries, &steps, "removalKeys");
    let (a, b) = ([0xa1u8; 16], [0xb2u8; 16]);
    let removal = |hosts: &[(&str, &[&str])]| -> Vec<(String, Vec<String>)> {
        hosts.iter().map(|(h, ids)| (h.to_string(), ids.iter().map(|i| i.to_string()).collect())).collect()
    };
    let pause = |op: [u8; 16], phase: PausePhase, removals: Vec<(String, Vec<String>)>| Action::RenewalPause { op, phase, removals };
    let ids = |ids: &[&str]| Accept::Devices(ids.iter().map(|i| i.to_string()).collect());
    let accept = |host: &str, from: Accept| Action::SetAccept { host: host.into(), from };
    let ok = |s: &ChainState, d: &str, host: &str| s.may_control(d, host).is_ok();
    push(&mut st, by0(), k0, accept("m-1", Accept::All)).unwrap();
    push(&mut st, by0(), k0, accept("h-2", ids(&["dev-2", "m-1"]))).unwrap();
    push(&mut st, by0(), k0, Action::AddDevice3(dev3("d-a", &extra(26), false, true))).unwrap();
    let ka = &extra(26).0;
    let by_a = || Signer::Device("d-a".into());
    assert!(ok(&st, "h-2", "m-1") && ok(&st, "m-1", "h-2") && ok(&st, "dev-2", "m-1") && ok(&st, "dev-0", "m-1"));

    // The pause: h-2 stops accepting m-1; m-1 stops accepting dev-2 and h-2 (hosts, and each host's ids, in byte order).
    let start = push(&mut st, by0(), k0, pause(a, PausePhase::Start, removal(&[("h-2", &["m-1"]), ("m-1", &["dev-2", "h-2"])]))).unwrap();
    assert!(!ok(&st, "h-2", "m-1") && !ok(&st, "m-1", "h-2") && !ok(&st, "dev-2", "m-1"), "the paused edges are refused");
    assert!(
        matches!(st.may_control("dev-2", "m-1"), Err(ChainError::NotAccepted { reason, .. }) if reason == "control is paused while identity keys are renewed")
    );
    assert!(ok(&st, "dev-2", "h-2") && ok(&st, "dev-0", "m-1") && ok(&st, "d-a", "m-1"), "the rest stands, admins always");
    assert!(st.authorize_control("h-2", &extra(0).1.public(), "m-1").is_err());
    assert_eq!(st.hosts_admitting("h-2").map(|d| d.info.id.clone()).collect::<Vec<_>>(), Vec::<String>::new());
    assert_eq!((st.base_accept("m-1"), st.paused_edges("m-1")), (Accept::All, vec!["dev-2".to_string(), "h-2".to_string()]));
    assert_eq!(st.paused_edges("h-2"), vec!["m-1".to_string()]);
    assert_eq!(
        st.open_pauses(),
        vec![OpenPause {
            op: a,
            start_seq: start.seq(),
            signer: "dev-0".into(),
            removals: removal(&[("h-2", &["m-1"]), ("m-1", &["dev-2", "h-2"])]),
        }]
    );
    // A hand SetAccept during the pause changes only the base; the pause still holds.
    push(&mut st, by0(), k0, accept("m-1", ids(&["dev-2", "h-2"]))).unwrap();
    assert_eq!(st.base_accept("m-1"), ids(&["dev-2", "h-2"]));
    assert!(!ok(&st, "h-2", "m-1"), "listed in the base, paused all the same");
    let epoch = st.machine_key_epochs("m-1");
    push(&mut st, by0(), k0, Action::KeyEpochs(mk_commit("m-1", epoch, &[0x43; 32]))).unwrap();
    // Another admin completes it: the overlay lifts, the base as it now is stands.
    push(&mut st, by_a(), ka, pause(a, PausePhase::Completed, vec![])).unwrap();
    assert!(st.open_pauses().is_empty() && st.paused_edges("m-1").is_empty());
    assert!(ok(&st, "h-2", "m-1") && ok(&st, "m-1", "h-2") && ok(&st, "dev-2", "m-1") && ok(&st, "dev-0", "m-1"));

    // A pause whose starter is demoted stays in force; another admin ends it.
    push(&mut st, by_a(), ka, pause(b, PausePhase::Start, removal(&[("m-1", &["dev-2"])]))).unwrap();
    push(&mut st, by0(), k0, Action::SetAdmin { id: "d-a".into(), admin: false }).unwrap();
    assert_eq!(st.paused_edges("m-1"), vec!["dev-2".to_string()]);
    assert!(!ok(&st, "dev-2", "m-1"));
    assert_eq!(
        try_push(&st, by_a(), ka, pause(b, PausePhase::Cancelled, vec![])),
        rule(st.seq() + 1, rules::ONLY_ADMIN_SIGNS),
        "a demoted starter ends nothing"
    );
    let cancelled = push(&mut st, by0(), k0, pause(b, PausePhase::Cancelled, vec![])).unwrap();
    assert!(ok(&st, "dev-2", "m-1"));
    assert_eq!(st.pause(&b).and_then(|p| p.end), Some((PausePhase::Cancelled, cancelled.seq())));
    // Overlapping pauses: an edge comes back only when every pause naming it ended.
    let mut two = st.clone();
    push(&mut two, by0(), k0, pause([0xd1; 16], PausePhase::Start, removal(&[("m-1", &["h-2"])]))).unwrap();
    push(&mut two, by0(), k0, pause([0xd2; 16], PausePhase::Start, removal(&[("m-1", &["dev-2", "h-2"])]))).unwrap();
    push(&mut two, by0(), k0, pause([0xd1; 16], PausePhase::Completed, vec![])).unwrap();
    assert_eq!(two.paused_edges("m-1"), vec!["dev-2".to_string(), "h-2".to_string()]);
    push(&mut two, by0(), k0, pause([0xd2; 16], PausePhase::Completed, vec![])).unwrap();
    assert!(two.paused_edges("m-1").is_empty());
    // A device made admin while paused stays paused until the pause ends: a pause only ever narrows.
    let mut promoted = st.clone();
    push(&mut promoted, by0(), k0, pause([0xd3; 16], PausePhase::Start, removal(&[("m-1", &["dev-2"])]))).unwrap();
    push(&mut promoted, by0(), k0, Action::SetAdmin { id: "dev-2".into(), admin: true }).unwrap();
    assert!(!ok(&promoted, "dev-2", "m-1"), "an admin now, paused all the same");
    push(&mut promoted, by0(), k0, pause([0xd3; 16], PausePhase::Completed, vec![])).unwrap();
    assert!(ok(&promoted, "dev-2", "m-1"));

    // The rules, each on its own.
    let seq = st.seq() + 1;
    let attempt = |s: &ChainState, action: Action| try_push(s, by0(), k0, action);
    let c = [0xc1u8; 16];
    assert_eq!(attempt(&st, pause(a, PausePhase::Start, removal(&[("m-1", &["dev-2"])]))), rule(seq, rules::PAUSE_OP_REUSED));
    for removals in [
        vec![],
        removal(&[("dev-2", &["m-1"])]),
        removal(&[("dev-1", &["m-1"])]),
        removal(&[("nobody", &["m-1"])]),
        removal(&[("m-1", &["dev-2"]), ("m-1", &["h-2"])]),
        removal(&[("m-1", &["dev-2"]), ("h-2", &["dev-2"])]),
        (0..257).map(|i| (format!("h-{i}"), vec!["dev-2".to_string()])).collect(),
    ] {
        assert_eq!(attempt(&st, pause(c, PausePhase::Start, removals.clone())), rule(seq, rules::PAUSE_HOSTS), "{removals:?}");
    }
    let mut pending = st.clone();
    push(&mut pending, Signer::Recovery, &f.recovery.signing, Action::RecoveryJoin3(dev3("dev-r", &extra(4), false, true))).unwrap();
    assert_eq!(
        attempt(&pending, pause(c, PausePhase::Start, removal(&[("dev-r", &["m-1"])]))),
        rule(seq + 1, rules::PAUSE_HOSTS),
        "a pending device"
    );
    let many: Vec<String> = (0..513).map(|i| format!("d-{i}")).collect();
    assert_eq!(
        attempt(&st, pause(c, PausePhase::Start, vec![("h-2".into(), many.clone()), ("m-1".into(), many)])),
        rule(seq, rules::PAUSE_EDGES)
    );
    for removals in [
        removal(&[("m-1", &[])]),
        removal(&[("m-1", &["m-1"])]),
        removal(&[("m-1", &["nobody"])]),
        removal(&[("m-1", &["dev-2", "dev-2"])]),
        removal(&[("m-1", &["h-2", "dev-2"])]),
        removal(&[("m-1", &["dev-0"])]),
    ] {
        assert_eq!(attempt(&st, pause(c, PausePhase::Start, removals.clone())), rule(seq, rules::PAUSE_IDS), "{removals:?}");
    }
    // The whole entry fits every transport: one byte past MAX_PAUSE_ENTRY_LEN is refused for that alone; at it, the
    // ids' own rule speaks.
    let sized = |len: usize| {
        let entry = |ids: &[String]| st.draft(by0(), pause(c, PausePhase::Start, vec![("m-1".into(), ids.to_vec())]), 0).sign(k0);
        let mut ids: Vec<String> = Vec::new();
        while entry(&ids).encode().len() + 2 * 66 <= len {
            ids.push(format!("x{:063}", ids.len()));
        }
        let rest = len - entry(&ids).encode().len() - 4;
        ids.push("y".repeat(rest / 2));
        ids.push("z".repeat(rest - rest / 2));
        assert_eq!(entry(&ids).encode().len(), len);
        pause(c, PausePhase::Start, vec![("m-1".into(), ids)])
    };
    assert_eq!(attempt(&st, sized(MAX_PAUSE_ENTRY_LEN + 1)), rule(seq, rules::PAUSE_TOO_LARGE));
    assert_eq!(attempt(&st, sized(MAX_PAUSE_ENTRY_LEN)), rule(seq, rules::PAUSE_IDS));
    attempt(&st, pause(c, PausePhase::Start, removal(&[("m-1", &["dev-1"])]))).expect("a removed device is a device the chain named");
    assert_eq!(attempt(&st, pause(a, PausePhase::Completed, vec![])), rule(seq, rules::PAUSE_NOT_OPEN), "twice");
    assert_eq!(attempt(&st, pause([0xcc; 16], PausePhase::Cancelled, vec![])), rule(seq, rules::PAUSE_NOT_OPEN), "unknown");
    let mut open = st.clone();
    push(&mut open, by0(), k0, pause(c, PausePhase::Start, removal(&[("m-1", &["dev-2"])]))).unwrap();
    assert_eq!(
        attempt(&open, pause(c, PausePhase::Completed, removal(&[("m-1", &["dev-2"])]))),
        rule(seq + 1, rules::PAUSE_END_REMOVALS)
    );
    let start_c = || pause(c, PausePhase::Start, removal(&[("m-1", &["dev-2"])]));
    assert_eq!(
        try_push(&st, Signer::Device("dev-2".into()), &f.devices[2].0, start_c()),
        rule(seq, rules::ONLY_ADMIN_SIGNS)
    );
    assert!(matches!(
        try_push(&st, Signer::Recovery, &f.recovery.signing, start_c()),
        Err(ChainError::NotAllowed { .. })
    ));
    let mut slotted = st.clone();
    let p = pak(&f, "p-1", 800, 1, true);
    push(&mut slotted, by0(), k0, Action::PasskeySlotSet(p.slot.clone())).unwrap();
    assert!(matches!(
        try_push(&slotted, Signer::Passkey("p-1".into()), &p.signing, start_c()),
        Err(ChainError::NotAllowed { .. })
    ));

    // Encoding: kind 22, op, phase, the removals; nothing else decodes, nor kind 22 under v2.
    for e in [&start, &cancelled] {
        assert_eq!((e.draft.version, e.draft.action.kind()), (VERSION_3, 22));
        assert_eq!(&Entry::decode(&e.encode()).unwrap(), e);
    }
    let bytes = cancelled.encode();
    let at = bytes.len() - 64 - 2 - 1;
    assert_eq!(bytes[at], 2);
    for bad in [3u8, 0x80, 0xff] {
        let mut x = bytes.clone();
        x[at] = bad;
        assert!(matches!(Entry::decode(&x), Err(ChainError::Decode(_))), "{bad}");
    }
    let mut v2 = bytes.clone();
    v2[0] = VERSION;
    assert!(matches!(Entry::decode(&v2), Err(ChainError::Decode(_))), "kind 22 under version 2");
    let mut draft = st.draft(by0(), start_c(), 1);
    assert_eq!(draft.version, VERSION_3);
    draft.version = VERSION;
    assert_eq!(st.clone().apply(&draft.sign(k0), &mut h()), Err(ChainError::Version(VERSION)));
    for removals in [
        (0..257).map(|i| (format!("h-{i}"), vec!["d".to_string()])).collect::<Vec<_>>(),
        vec![("m-1".to_string(), (0..1025).map(|i| format!("d-{i}")).collect())],
    ] {
        let e = st.draft(by0(), pause(c, PausePhase::Start, removals), 1).sign(k0);
        assert!(matches!(Entry::decode(&e.encode()), Err(ChainError::Decode(_))), "too large to decode");
    }
    assert_eq!(ChainState::verify(st.entries(), None, &mut h()).unwrap(), st);
}
