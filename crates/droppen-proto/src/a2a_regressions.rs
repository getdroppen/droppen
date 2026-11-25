//! The A2A proxy on the device chain (`docs/a2a.md` §7.1), rule by rule: the
//! a2a flag (bit 2) never goes with host or admin, it is fixed at the join,
//! an A2A proxy signs and attests nothing, `SetA2aAccept` (kind 23) is an
//! admin's and names only current A2A proxies of a current host, a removal
//! drops a proxy from every list, a renewal pause holds its calls — and the
//! rule the lane rests on: an A2A proxy controls no host (`All` does not take
//! it in, no `SetAccept` list may name it), so no machine key is ever sealed
//! to it ([`ChainState::hosts_admitting`] is empty).
//!
//! Fixture: `v3_regressions::with_second_host` (`dev-0` admin, `dev-2` a plain
//! device, `m-1` and `h-2` hosts, `dev-1` removed) plus two A2A proxies, `a-1`
//! and `a-2`, where a test needs them.

use ed25519_dalek::SigningKey;

use crate::chain::tests::{fixture, h, Fixture};
use crate::chain::*;
use crate::handshake::{self, AgentSide, HandshakeError};
use crate::keys::DhKeypair;
use crate::noise::{RekeyPolicy, ReplayGuard};
use crate::v3_regressions::{extra, mk_commit, pak, with_second_host};

fn by0() -> Signer {
    Signer::Device("dev-0".into())
}

fn device(id: &str, keys: &(SigningKey, DhKeypair), flags: Flags) -> DeviceV3 {
    DeviceV3 {
        id: id.into(),
        sign_key: keys.0.verifying_key().to_bytes(),
        dh_key: keys.1.public(),
        flags,
    }
}

fn flags(host: bool, admin: bool, a2a: bool) -> Flags {
    Flags { host, admin, a2a }
}

/// The keys of A2A proxy `a-1` (`a-2`: `extra(41)`).
pub(crate) fn proxy_keys(i: u64) -> (SigningKey, DhKeypair) {
    extra(40 + i)
}

fn push(state: &mut ChainState, signer: Signer, key: &SigningKey, action: Action) -> Result<Entry, ChainError> {
    let entry = state.draft(signer, action, 20_000 + state.seq()).sign(key);
    state.apply(&entry, &mut h())?;
    Ok(entry)
}

fn try_push(state: &ChainState, signer: Signer, key: &SigningKey, action: Action) -> Result<(), ChainError> {
    let entry = state.draft(signer, action, 20_000 + state.seq()).sign(key);
    state.clone().apply(&entry, &mut h())
}

fn rule(seq: u64, reason: &'static str) -> Result<(), ChainError> {
    Err(ChainError::NotAllowed { seq, reason })
}

fn accept(host: &str, ids: &[&str]) -> Action {
    Action::SetA2aAccept {
        host: host.into(),
        devices: ids.iter().map(|s| s.to_string()).collect(),
    }
}

/// `with_second_host` plus, from dev-0: A2A proxies `a-1` and `a-2`; `m-1`
/// takes A2A calls from `a-1`, `h-2` from both.
pub(crate) fn with_proxies(f: &Fixture) -> (ChainState, Vec<Entry>) {
    let (mut state, mut entries) = with_second_host(f);
    let k0 = &f.devices[0].0;
    for action in [
        Action::AddDevice3(device("a-1", &proxy_keys(0), Flags::A2A_DEVICE)),
        Action::AddDevice3(device("a-2", &proxy_keys(1), Flags::A2A_DEVICE)),
        accept("m-1", &["a-1"]),
        accept("h-2", &["a-1", "a-2"]),
    ] {
        entries.push(push(&mut state, by0(), k0, action).unwrap());
    }
    (state, entries)
}

/// Bit 2 is a2a; bit 3 and up are unknown and do not decode. Every
/// combination of the three decodes (the rules refuse the bad ones), and an
/// `AddDevice3` with the flag round-trips.
#[test]
fn a1_the_a2a_flag_is_bit_2_and_every_higher_bit_is_refused() {
    for b in 0..=u8::MAX {
        match Flags::from_byte(b) {
            Ok(f) => {
                assert!(b < 8, "{b:#b} decodes");
                assert_eq!(f.to_byte(), b);
                assert_eq!(f.a2a, b & 0b100 != 0);
            }
            Err(_) => assert!(b >= 8, "{b:#b} is refused"),
        }
    }
    assert_eq!(Flags::A2A, 0b100);
    assert_eq!(Flags::A2A_DEVICE.to_byte(), 0b100);
    let f = fixture();
    let (state, entries) = with_proxies(&f);
    let add = &entries[7];
    assert!(matches!(&add.draft.action, Action::AddDevice3(d) if d.flags == Flags::A2A_DEVICE));
    assert_eq!(&Entry::decode(&add.encode()).unwrap(), add);
    let mut bytes = add.encode();
    let at = bytes.len() - 64 - 1;
    assert_eq!(bytes[at], Flags::A2A);
    bytes[at] = 0b1000 | Flags::A2A;
    assert!(matches!(Entry::decode(&bytes), Err(ChainError::Decode(_))));
    let a1 = state.device("a-1").unwrap();
    assert!(a1.a2a && !a1.host && !a1.admin && a1.is_current_a2a());
    assert_eq!(a1.flags(), Flags::A2A_DEVICE);
    assert!(state.a2a_device("a-1").is_some() && state.a2a_device("dev-2").is_none() && state.a2a_device("m-1").is_none());
}

/// `A2A_PLAIN`: an A2A proxy is never a host nor an admin — not at its join,
/// whichever entry adds it, nor later. A passkey adds none. No entry changes
/// the flag afterwards: setting host or admin is refused, clearing them is
/// no change.
#[test]
fn a2_an_a2a_device_is_never_a_host_nor_an_admin_and_its_flag_is_fixed() {
    let f = fixture();
    let ak = f.ak.verifying_key().to_bytes();
    // Genesis3: the first device is an admin, so with a2a it is refused.
    let g = |fl: Flags| ChainState::genesis(&genesis3_draft("acct-1", ak, device("d-1", &extra(42), fl), None, 1).sign(&f.ak), None);
    assert_eq!(g(flags(false, true, true)).err(), rule(0, rules::A2A_PLAIN).err());
    assert_eq!(
        g(flags(true, true, true)).err(),
        rule(0, rules::HOST_NOT_ADMIN).err(),
        "host with admin is the older rule"
    );
    assert!(matches!(g(Flags::A2A_DEVICE), Err(ChainError::NotAllowed { reason, .. }) if reason == "the first device must be an admin device"));
    g(flags(false, true, false)).unwrap();

    let (state, _) = with_proxies(&f);
    let k0 = &f.devices[0].0;
    let seq = state.seq() + 1;
    let add = |fl: Flags| Action::AddDevice3(device("a-x", &extra(43), fl));
    assert_eq!(try_push(&state, by0(), k0, add(flags(true, false, true))), rule(seq, rules::A2A_PLAIN));
    assert_eq!(try_push(&state, by0(), k0, add(flags(false, true, true))), rule(seq, rules::A2A_PLAIN));
    assert_eq!(
        try_push(&state, by0(), k0, add(flags(true, true, true))),
        rule(seq, rules::HOST_NOT_ADMIN)
    );
    try_push(&state, by0(), k0, add(Flags::A2A_DEVICE)).unwrap();

    // RecoveryJoin3: the recovered device is an admin, so never an A2A proxy.
    let rj = |fl: Flags| Action::RecoveryJoin3(device("dev-r", &extra(43), fl));
    assert_eq!(
        try_push(&state, Signer::Recovery, &f.recovery.signing, rj(flags(false, true, true))),
        rule(seq, rules::A2A_PLAIN)
    );
    assert_eq!(
        try_push(&state, Signer::Recovery, &f.recovery.signing, rj(Flags::A2A_DEVICE)),
        rule(seq, rules::RECOVERED_ADMIN)
    );

    // A passkey adds a device with no flag at all: never an A2A proxy.
    let mut slotted = state.clone();
    let p = pak(&f, "p-1", 800, 1, true);
    push(&mut slotted, by0(), k0, Action::PasskeySlotSet(p.slot.clone())).unwrap();
    assert_eq!(
        try_push(&slotted, Signer::Passkey("p-1".into()), &p.signing, add(Flags::A2A_DEVICE)),
        rule(slotted.seq() + 1, rules::PASSKEY_PLAIN)
    );
    try_push(&slotted, Signer::Passkey("p-1".into()), &p.signing, add(Flags::NONE)).unwrap();

    // Later: no entry gives it host or admin, none takes the flag away.
    let set_admin = |admin| Action::SetAdmin { id: "a-1".into(), admin };
    let set_host = |host| Action::SetHost { id: "a-1".into(), host };
    assert_eq!(try_push(&state, by0(), k0, set_admin(true)), rule(seq, rules::A2A_PLAIN));
    assert_eq!(try_push(&state, by0(), k0, set_host(true)), rule(seq, rules::A2A_PLAIN));
    assert_eq!(try_push(&state, by0(), k0, set_admin(false)), rule(seq, rules::NO_CHANGE));
    assert_eq!(try_push(&state, by0(), k0, set_host(false)), rule(seq, rules::NO_CHANGE));
    // Even in a batch that first makes it something else: nothing does.
    assert!(state.device("a-1").unwrap().a2a);
}

/// An A2A proxy signs nothing (it is no admin), attests nothing (its
/// attestation does not count for freshness) and delivers no key.
#[test]
fn a3_an_a2a_device_signs_attests_and_delivers_nothing() {
    let f = fixture();
    let (state, _) = with_proxies(&f);
    let a1 = proxy_keys(0);
    let seq = state.seq() + 1;
    for action in [
        accept("m-1", &["a-1", "a-2"]),
        Action::AddDevice3(device("a-x", &extra(43), Flags::A2A_DEVICE)),
        Action::KeyEpochs(mk_commit("m-1", 0, &[1; 32])),
        Action::RemoveDevice3("a-2".into()),
    ] {
        assert_eq!(
            try_push(&state, Signer::Device("a-1".into()), &a1.0, action),
            rule(seq, rules::ONLY_ADMIN_SIGNS)
        );
    }
    let att = Attestation::create(&state, "a-1", &a1.0, 5_000);
    assert!(matches!(state.check_attestation(&att), Err(ChainError::BadAttestation(_))));
    assert!(matches!(
        state.freshness("m-1", &[att], 5_000, CONTROL_FRESHNESS_MS),
        Ok(Freshness::Stale { newest_ms: None })
    ));
    assert!(state.full_device_key("a-1").is_err(), "no key sealed by it opens");
}

/// `SetA2aAccept`: an admin's; for a current host; names current A2A
/// proxies only, ascending, each once, at most 64, possibly none; never a
/// no-op; one encoding; v3 only.
#[test]
fn a4_set_a2a_accept_is_an_admins_for_a_host_naming_current_proxies() {
    let f = fixture();
    let (state, entries) = with_proxies(&f);
    let k0 = &f.devices[0].0;
    let seq = state.seq() + 1;
    assert_eq!(state.a2a_accept("m-1"), ["a-1"]);
    assert_eq!(state.a2a_accept("h-2"), ["a-1", "a-2"]);
    assert!(state.a2a_accept("dev-2").is_empty() && state.a2a_accept("nobody").is_empty());
    // Who signs: an active admin only.
    for (signer, key) in [
        ("dev-2", &f.devices[2].0),
        ("h-2", &extra(0).0),
        ("m-1", &f.machine.0),
        ("a-2", &proxy_keys(1).0),
    ] {
        assert_eq!(
            try_push(&state, Signer::Device(signer.into()), key, accept("m-1", &["a-1", "a-2"])),
            rule(seq, rules::ONLY_ADMIN_SIGNS),
            "{signer}"
        );
    }
    assert!(matches!(
        try_push(&state, Signer::Recovery, &f.recovery.signing, accept("m-1", &[])),
        Err(ChainError::NotAllowed { .. })
    ));
    let mut slotted = state.clone();
    let p = pak(&f, "p-1", 800, 1, true);
    push(&mut slotted, by0(), k0, Action::PasskeySlotSet(p.slot.clone())).unwrap();
    assert!(matches!(
        try_push(&slotted, Signer::Passkey("p-1".into()), &p.signing, accept("m-1", &[])),
        Err(ChainError::NotAllowed { .. })
    ));
    // For a current host.
    assert_eq!(
        try_push(&state, by0(), k0, accept("dev-2", &["a-1"])),
        rule(seq, rules::A2A_ACCEPT_NOT_HOST)
    );
    assert_eq!(
        try_push(&state, by0(), k0, accept("a-1", &["a-2"])),
        rule(seq, rules::A2A_ACCEPT_NOT_HOST)
    );
    assert_eq!(
        try_push(&state, by0(), k0, accept("dev-0", &["a-1"])),
        rule(seq, rules::A2A_ACCEPT_NOT_HOST)
    );
    assert!(matches!(
        try_push(&state, by0(), k0, accept("dev-1", &["a-1"])),
        Err(ChainError::UnknownId { .. })
    ));
    assert!(matches!(
        try_push(&state, by0(), k0, accept("nobody", &["a-1"])),
        Err(ChainError::UnknownId { .. })
    ));
    // Naming current A2A proxies only.
    for ids in [&["dev-2"][..], &["m-1"], &["h-2"], &["dev-0"], &["a-1", "dev-2"]] {
        assert_eq!(
            try_push(&state, by0(), k0, accept("m-1", ids)),
            rule(seq, rules::A2A_ACCEPT_NOT_A2A),
            "{ids:?}"
        );
    }
    for ids in [&["dev-1"][..], &["nobody"], &["a-1", "a-9"]] {
        assert!(
            matches!(try_push(&state, by0(), k0, accept("m-1", ids)), Err(ChainError::UnknownId { .. })),
            "{ids:?}"
        );
    }
    assert!(matches!(
        try_push(&state, by0(), k0, accept("m-1", &["a 1"])),
        Err(ChainError::BadId { .. })
    ));
    // Never a no-op: the same list, or none when it has none.
    assert_eq!(try_push(&state, by0(), k0, accept("m-1", &["a-1"])), rule(seq, rules::A2A_ACCEPT_SAME));
    assert_eq!(try_push(&state, by0(), k0, accept("dev-2", &[])), rule(seq, rules::A2A_ACCEPT_NOT_HOST));
    let mut none = state.clone();
    push(&mut none, by0(), k0, accept("m-1", &[])).unwrap();
    push(&mut none, by0(), k0, Action::KeyEpochs(mk_commit("m-1", 0, &[0x41; 32]))).unwrap();
    assert_eq!(
        try_push(&none, by0(), k0, accept("m-1", &[])),
        rule(none.seq() + 1, rules::A2A_ACCEPT_SAME)
    );
    // One encoding: ascending by bytes, each once, at most 64; none decodes.
    let base = state.draft(by0(), accept("m-1", &["a-1", "a-2"]), 1).sign(k0);
    assert_eq!(Entry::decode(&base.encode()).unwrap(), base);
    for ids in [vec!["a-2", "a-1"], vec!["a-1", "a-1"]] {
        let e = state.draft(by0(), accept("m-1", &ids), 1).sign(k0);
        assert!(matches!(Entry::decode(&e.encode()), Err(ChainError::Decode(_))), "{ids:?}");
    }
    let many = |n: usize| (0..n).map(|i| format!("a-{i:03}")).collect::<Vec<_>>();
    let e = state
        .draft(
            by0(),
            Action::SetA2aAccept {
                host: "m-1".into(),
                devices: many(MAX_ACCEPT_IDS + 1),
            },
            1,
        )
        .sign(k0);
    assert!(matches!(Entry::decode(&e.encode()), Err(ChainError::Decode(_))));
    let e = state
        .draft(
            by0(),
            Action::SetA2aAccept {
                host: "m-1".into(),
                devices: many(MAX_ACCEPT_IDS),
            },
            1,
        )
        .sign(k0);
    assert!(matches!(
        state.clone().apply(&Entry::decode(&e.encode()).unwrap(), &mut h()),
        Err(ChainError::UnknownId { .. })
    ));
    // v3 only: kind 23 does not exist in a v2 entry, and the draft is v3.
    assert_eq!(base.draft.version, VERSION_3);
    let mut bytes = base.encode();
    bytes[0] = VERSION;
    assert!(matches!(Entry::decode(&bytes), Err(ChainError::Decode(_))));
    assert_eq!(accept("m-1", &[]).kind(), 23);
    assert_eq!(accept("m-1", &[]).fixed_version(), Some(VERSION_3));
    // Dropping a proxy takes the host's next machine key epoch in the very
    // next entry (`HOST_KEY_NEXT`): a session it started could read the
    // host's key files. Adding one owes nothing.
    let mut s = state.clone();
    push(&mut s, by0(), k0, accept("h-2", &["a-2"])).unwrap();
    assert_eq!(s.host_key_due(), Some("h-2"));
    assert_eq!(
        try_push(&s, by0(), k0, accept("m-1", &["a-1", "a-2"])),
        rule(s.seq() + 1, rules::HOST_KEY_NEXT)
    );
    assert_eq!(
        try_push(&s, by0(), k0, Action::KeyEpochs(mk_commit("m-1", 0, &[0x42; 32]))),
        rule(s.seq() + 1, rules::HOST_KEY_NEXT),
        "another host's key does not cover it"
    );
    push(&mut s, by0(), k0, Action::KeyEpochs(mk_commit("h-2", 1, &[0x43; 32]))).unwrap();
    assert_eq!(s.host_key_due(), None);
    push(&mut s, by0(), k0, accept("h-2", &["a-1", "a-2"])).unwrap();
    assert_eq!(s.host_key_due(), None, "widening owes nothing");
    push(&mut s, by0(), k0, accept("h-2", &[])).unwrap();
    assert_eq!(s.host_key_due(), Some("h-2"));
    push(&mut s, by0(), k0, Action::KeyEpochs(mk_commit("h-2", 2, &[0x44; 32]))).unwrap();
    assert!(s.a2a_accept("h-2").is_empty());
    // A removal drops the proxy from the lists without an entry of the
    // host's: the chain owes nothing then (the web turns the keys over).
    let mut r = state.clone();
    push(&mut r, by0(), k0, Action::RemoveDevice3("a-2".into())).unwrap();
    assert_eq!(r.host_key_due(), None);
    // Every entry of the fixture round-trips.
    for e in &entries {
        assert_eq!(&Entry::decode(&e.encode()).unwrap(), e);
    }
}

/// `may_call_a2a`: a current A2A proxy calls any current host. Nobody else —
/// not an admin, not a host, not a plain device — and the old accept lists
/// no longer gate calls.
#[test]
fn a5_an_a2a_proxy_may_call_every_current_host() {
    let f = fixture();
    let (state, _) = with_proxies(&f);
    state.may_call_a2a("a-1", "m-1").unwrap();
    state.may_call_a2a("a-1", "h-2").unwrap();
    state.may_call_a2a("a-2", "h-2").unwrap();
    state.may_call_a2a("a-2", "m-1").unwrap();
    for (d, host) in [
        ("dev-0", "m-1"),
        ("dev-2", "m-1"),
        ("h-2", "m-1"),
        ("m-1", "h-2"),
        ("a-1", "dev-2"),
        ("a-1", "a-2"),
        ("a-1", "dev-1"),
    ] {
        assert!(matches!(state.may_call_a2a(d, host), Err(ChainError::NotAccepted { .. })), "{d} → {host}");
    }
    assert_eq!(state.may_call_a2a("a-1", "a-1"), Err(ChainError::SelfControl));
    assert!(matches!(state.may_call_a2a("dev-1", "m-1"), Err(ChainError::UnknownDevice(_))));
    assert!(matches!(state.may_call_a2a("nobody", "m-1"), Err(ChainError::UnknownDevice(_))));
    let ids = |it: Vec<&DeviceRecord>| it.into_iter().map(|d| d.info.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(state.hosts_accepting_a2a("a-1").collect()), ["h-2", "m-1"]);
    assert_eq!(ids(state.hosts_accepting_a2a("a-2").collect()), ["h-2", "m-1"]);
    assert!(state.hosts_accepting_a2a("dev-0").next().is_none());
    // A host that stops running AI takes no A2A call; running AI again takes
    // calls at once, its list being empty notwithstanding.
    let k0 = &f.devices[0].0;
    let mut s = state.clone();
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
    assert!(s.a2a_accept("h-2").is_empty());
    assert!(s.may_call_a2a("a-1", "h-2").is_err());
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
    push(&mut s, by0(), k0, Action::KeyEpochs(mk_commit("h-2", 1, &[0x23; 32]))).unwrap();
    assert!(s.a2a_accept("h-2").is_empty());
    s.may_call_a2a("a-1", "h-2").unwrap();
    s.may_call_a2a("a-1", "m-1").unwrap();
}

/// The rule the lane rests on: an A2A proxy never controls a host — `All`
/// does not take it in, and no `SetAccept` list may name it
/// ([`rules::A2A_NOT_CONTROLLER`]: a name written but ignored would mislead) —
/// so it is never among the devices a host's machine keys are sealed to
/// (`hosts_admitting` is empty). An agent's admission refuses its handshake
/// (it fails closed until the agent knows `A2aOnly`).
#[test]
fn a6_an_a2a_device_controls_nothing_and_is_sealed_no_machine_key() {
    let f = fixture();
    let (mut state, _) = with_proxies(&f);
    let k0 = &f.devices[0].0;
    let set = |host: &str, from: Accept| Action::SetAccept { host: host.into(), from };
    let list = |ids: &[&str]| Accept::Devices(ids.iter().map(|s| s.to_string()).collect());
    push(&mut state, by0(), k0, set("m-1", Accept::All)).unwrap();
    // A list never names it, alone or beside a device that does control.
    let seq = state.seq() + 1;
    for ids in [&["a-1"][..], &["a-1", "a-2", "dev-2"], &["a-2", "dev-2"]] {
        assert_eq!(
            try_push(&state, by0(), k0, set("h-2", list(ids))),
            rule(seq, rules::A2A_NOT_CONTROLLER),
            "{ids:?}"
        );
    }
    // Each id in turn; an unknown one, or the host itself, as before.
    assert!(matches!(
        try_push(&state, by0(), k0, set("h-2", list(&["a-9"]))),
        Err(ChainError::UnknownId { .. })
    ));
    assert_eq!(try_push(&state, by0(), k0, set("h-2", list(&["h-2"]))), rule(seq, rules::ACCEPT_SELF));
    push(&mut state, by0(), k0, set("h-2", list(&["dev-2"]))).unwrap();
    assert_eq!(state.host_key_due(), None);
    for host in ["m-1", "h-2"] {
        for d in ["a-1", "a-2"] {
            assert!(matches!(state.may_control(d, host), Err(ChainError::NotAccepted { .. })), "{d} → {host}");
        }
        state.may_control("dev-2", host).unwrap();
    }
    let hosts = |d: &str| state.hosts_admitting(d).map(|h| h.info.id.clone()).collect::<Vec<_>>();
    assert!(
        hosts("a-1").is_empty() && hosts("a-2").is_empty(),
        "no machine key is ever sealed to an A2A proxy"
    );
    assert_eq!(hosts("dev-2"), ["h-2", "m-1"]);
    assert_eq!(hosts("dev-0"), ["h-2", "m-1"]);
    // Narrowing `All` to the others it let in owes no new key (the proxies
    // were never among them); shutting out one that could control does.
    let mut s = state.clone();
    push(&mut s, by0(), k0, set("m-1", list(&["dev-2", "h-2"]))).unwrap();
    assert_eq!(s.host_key_due(), None);
    push(&mut s, by0(), k0, set("m-1", list(&["dev-2"]))).unwrap();
    assert_eq!(s.host_key_due(), Some("m-1"), "h-2 was shut out");
    // Removing an A2A proxy owes no machine key either.
    let mut s = state.clone();
    push(&mut s, by0(), k0, Action::RemoveDevice3("a-1".into())).unwrap();
    assert_eq!(s.host_key_due(), None);

    // `accept` refuses its channel: it is not control (`accept_with_a2a` lets
    // it in `A2aOnly`, `handshake::tests`).
    let now = 5_000;
    let att = Attestation::create(&state, "dev-0", k0, now);
    let a1 = proxy_keys(0);
    let (_, hello) = handshake::start(&state, &a1.1, "a-1", "m-1", now, b"").unwrap();
    let mut view = state.clone();
    let mut clock = ChainClock::live(now, view.seq() + 1);
    let mut guard = ReplayGuard::new(60_000, 60_000, 16, 0);
    let accepted = handshake::accept(
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
        "a-1",
        &hello,
        now,
    );
    assert!(matches!(accepted, Err(HandshakeError::NotAccepted(_))));
}

/// A removal drops an A2A proxy from every host's list — the lists say who
/// may call now — and a removed proxy is named by no new list.
#[test]
fn a7_removing_an_a2a_device_drops_it_from_every_list() {
    let f = fixture();
    let (mut state, _) = with_proxies(&f);
    let k0 = &f.devices[0].0;
    push(&mut state, by0(), k0, Action::RemoveDevice3("a-2".into())).unwrap();
    assert_eq!(state.a2a_accept("h-2"), ["a-1"]);
    assert_eq!(state.a2a_accept("m-1"), ["a-1"]);
    assert!(state.may_call_a2a("a-2", "h-2").is_err());
    let seq = state.seq() + 1;
    assert_eq!(try_push(&state, by0(), k0, accept("h-2", &["a-1"])), rule(seq, rules::A2A_ACCEPT_SAME));
    assert!(matches!(
        try_push(&state, by0(), k0, accept("h-2", &["a-1", "a-2"])),
        Err(ChainError::UnknownId { .. })
    ));
    push(&mut state, by0(), k0, Action::RemoveDevice3("a-1".into())).unwrap();
    assert!(state.a2a_accept("h-2").is_empty() && state.a2a_accept("m-1").is_empty());
    assert_eq!(
        try_push(&state, by0(), k0, accept("m-1", &[])),
        rule(state.seq() + 1, rules::A2A_ACCEPT_SAME)
    );
    // A host removed takes its own list along.
    let (mut s, _) = with_proxies(&f);
    push(&mut s, by0(), k0, Action::RemoveDevice3("h-2".into())).unwrap();
    assert!(s.a2a_accept("h-2").is_empty());
    assert_eq!(s.a2a_accept("m-1"), ["a-1"]);
}

/// A renewal pause holds an A2A proxy's calls as it holds control (§3.4 rule
/// 10): while an open pause names the proxy for a host, `may_call_a2a`
/// refuses it, whatever the host's list says; the end gives it back. The list
/// itself is never touched.
#[test]
fn a8_a_renewal_pause_holds_a2a_calls_as_it_holds_control() {
    let f = fixture();
    let (mut state, _) = with_proxies(&f);
    let k0 = &f.devices[0].0;
    let op = [0xa2; 16];
    push(
        &mut state,
        by0(),
        k0,
        Action::RenewalPause {
            op,
            phase: PausePhase::Start,
            removals: vec![("m-1".into(), vec!["a-1".into()])],
        },
    )
    .unwrap();
    assert_eq!(state.paused_edges("m-1"), ["a-1"]);
    assert!(matches!(state.may_call_a2a("a-1", "m-1"), Err(ChainError::NotAccepted { .. })));
    assert_eq!(state.a2a_accept("m-1"), ["a-1"], "the list stays as it was");
    state.may_call_a2a("a-1", "h-2").unwrap();
    let ids = |st: &ChainState, d: &str| st.hosts_accepting_a2a(d).map(|h| h.info.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(&state, "a-1"), ["h-2"]);
    // Ended (completed or cancelled), the call comes back.
    for phase in [PausePhase::Completed, PausePhase::Cancelled] {
        let mut s = state.clone();
        push(&mut s, by0(), k0, Action::RenewalPause { op, phase, removals: vec![] }).unwrap();
        s.may_call_a2a("a-1", "m-1").unwrap();
        assert_eq!(ids(&s, "a-1"), ["h-2", "m-1"]);
    }
}
