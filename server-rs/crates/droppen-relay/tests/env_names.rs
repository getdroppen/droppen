//! The rename to Droppen: a relay reads `DROPPEN_RELAY_*` first
//! and the old `RH_RELAY_*` names when those are unset, logging each old name
//! it used once and that it stops working in v0.2.135. Checked on the binary,
//! with nothing in its environment but what each case sets; every case stops
//! on a bad value before listening.

use rh_relay::config::hex;

/// `droppen-relay <args>` with only `env`: whether it succeeded, its stdout
/// and everything it said.
fn relay(args: &[&str], env: &[(&str, &str)]) -> (bool, String, String) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_droppen-relay"))
        .args(args)
        .env_clear()
        .env("RUST_LOG", "warn")
        .envs(env.iter().copied())
        .output()
        .expect("run droppen-relay");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let said = format!("{stdout}{}", String::from_utf8_lossy(&out.stderr));
    (out.status.success(), stdout, said)
}

/// A relay's settings under the old names, every one it needs to get as far
/// as `RH_RELAY_MAX_CONNS`.
fn old_names(key: &str, keys: &str) -> Vec<(&'static str, String)> {
    vec![
        ("RH_RELAY_ORIGIN", "ws://127.0.0.1:1".to_string()),
        ("RH_RELAY_ID", "sin-1".to_string()),
        ("RH_RELAY_KEY", key.to_string()),
        ("RH_RELAY_TICKET_KEYS", keys.to_string()),
    ]
}

fn deprecations(said: &str, old: &str) -> usize {
    said.lines().filter(|l| l.contains(&format!("{old} is deprecated"))).count()
}

#[test]
fn the_old_names_still_configure_a_relay_and_each_is_logged_once() {
    let (key, keys) = (hex(&[8; 32]), format!("1:{}", hex(&[9; 32])));
    let env = old_names(&key, &keys);
    let mut env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    env.push(("RH_RELAY_MAX_CONNS", "lots"));
    let (ok, _, said) = relay(&["serve"], &env);
    // Past the origin, the id, the ticket keys and the key: each was read
    // under its old name, or it would have stopped there.
    assert!(!ok && said.contains("DROPPEN_RELAY_MAX_CONNS: a number"), "{said}");
    for old in ["RH_RELAY_ORIGIN", "RH_RELAY_ID", "RH_RELAY_KEY", "RH_RELAY_TICKET_KEYS", "RH_RELAY_MAX_CONNS"] {
        assert_eq!(deprecations(&said, old), 1, "{old}: {said}");
    }
    assert!(
        said.contains("RH_RELAY_KEY is deprecated: add DROPPEN_RELAY_KEY with the same value; RH_RELAY_KEY stops working in v0.2.135"),
        "{said}"
    );
}

#[test]
fn the_new_name_wins_over_the_old_one() {
    let (key, keys) = (hex(&[8; 32]), format!("1:{}", hex(&[9; 32])));
    let env = [
        ("DROPPEN_RELAY_ORIGIN", "ws://127.0.0.1:1"),
        ("DROPPEN_RELAY_ID", "sin-1"),
        ("DROPPEN_RELAY_KEY", key.as_str()),
        ("DROPPEN_RELAY_TICKET_KEYS", keys.as_str()),
    ];
    let (ok, _, said) = relay(&["serve"], &[&env[..], &[("DROPPEN_RELAY_MAX_CONNS", "lots"), ("RH_RELAY_MAX_CONNS", "10")]].concat());
    assert!(!ok && said.contains("DROPPEN_RELAY_MAX_CONNS: a number"), "{said}");
    assert!(
        said.contains(
            "DROPPEN_RELAY_MAX_CONNS and the deprecated RH_RELAY_MAX_CONNS are both set, to different values: DROPPEN_RELAY_MAX_CONNS is used; remove RH_RELAY_MAX_CONNS (it stops working in v0.2.135)"
        ),
        "{said}"
    );
    // The other way round: the new name's 10 parses, and it stops at the next.
    let (ok, _, said) = relay(
        &["serve"],
        &[&env[..], &[("DROPPEN_RELAY_MAX_CONNS", "10"), ("RH_RELAY_MAX_CONNS", "lots"), ("DROPPEN_RELAY_DRAIN_SECS", "soon")]].concat(),
    );
    assert!(!ok && said.contains("DROPPEN_RELAY_DRAIN_SECS: a number") && !said.contains("MAX_CONNS: a number"), "{said}");
    // Only new names: nothing deprecated.
    let (ok, _, said) = relay(&["serve"], &[&env[..], &[("DROPPEN_RELAY_MAX_CONNS", "lots")]].concat());
    assert!(!ok && said.contains("DROPPEN_RELAY_MAX_CONNS: a number") && !said.contains("deprecated"), "{said}");
}

#[test]
fn with_neither_name_set_it_says_the_new_one() {
    let (ok, _, said) = relay(&["serve"], &[]);
    assert!(!ok && said.contains("DROPPEN_RELAY_ORIGIN is required") && !said.contains("deprecated"), "{said}");
    // A new key is printed under the new name.
    let (ok, stdout, _) = relay(&["keygen"], &[]);
    assert!(ok && stdout.starts_with("DROPPEN_RELAY_KEY=") && stdout.trim().len() == "DROPPEN_RELAY_KEY=".len() + 64, "{stdout}");
}
