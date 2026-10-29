//! A relay reads its configuration by the `DROPPEN_RELAY_*` names only: the
//! names from before the rename to Droppen, `RH_RELAY_*`, are ignored, and
//! nothing is said about them. Checked on the binary, with nothing in its
//! environment but what each case sets; every case stops on a missing or bad
//! value before listening.

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

/// Everything a relay needs to get as far as `DROPPEN_RELAY_TRUST_PROXY`,
/// under the new names.
fn new_names(key: &str, keys: &str) -> Vec<(&'static str, String)> {
    vec![
        ("DROPPEN_RELAY_ORIGIN", "ws://127.0.0.1:1".to_string()),
        ("DROPPEN_RELAY_ID", "sin-1".to_string()),
        ("DROPPEN_RELAY_KEY", key.to_string()),
        ("DROPPEN_RELAY_TICKET_KEYS", keys.to_string()),
    ]
}

/// `vars` with each `DROPPEN_RELAY_X` as `RH_RELAY_X`.
fn under_old_names(vars: &[(&'static str, String)]) -> Vec<(String, String)> {
    vars.iter().map(|(name, value)| (name.replacen("DROPPEN_RELAY_", "RH_RELAY_", 1), value.clone())).collect()
}

fn pairs(vars: &[(impl AsRef<str>, String)]) -> Vec<(&str, &str)> {
    vars.iter().map(|(k, v)| (k.as_ref(), v.as_str())).collect()
}

#[test]
fn the_old_names_configure_nothing() {
    let (key, keys) = (hex(&[8; 32]), format!("1:{}", hex(&[9; 32])));
    let old = under_old_names(&new_names(&key, &keys));
    let (ok, _, said) = relay(&["serve"], &pairs(&old));
    // It stops at the first thing it needs, which only an old name gave.
    assert!(!ok && said.contains("DROPPEN_RELAY_ORIGIN is required"), "{said}");
    assert!(!said.contains("RH_RELAY_") && !said.contains("deprecated"), "{said}");
}

#[test]
fn an_old_name_beside_the_new_ones_is_ignored() {
    let (key, keys) = (hex(&[8; 32]), format!("1:{}", hex(&[9; 32])));
    let env = new_names(&key, &keys);
    let env = pairs(&env);
    // The old name's bad value is not read: it stops at the next new name's.
    let (ok, _, said) = relay(&["serve"], &[&env[..], &[("RH_RELAY_MAX_CONNS", "lots"), ("DROPPEN_RELAY_DRAIN_SECS", "soon")]].concat());
    assert!(!ok && said.contains("DROPPEN_RELAY_DRAIN_SECS: a number") && !said.contains("MAX_CONNS: a number"), "{said}");
    // Set both: the new name's value is the one read.
    let (ok, _, said) = relay(&["serve"], &[&env[..], &[("DROPPEN_RELAY_MAX_CONNS", "lots"), ("RH_RELAY_MAX_CONNS", "10")]].concat());
    assert!(!ok && said.contains("DROPPEN_RELAY_MAX_CONNS: a number"), "{said}");
    assert!(!said.contains("RH_RELAY_") && !said.contains("deprecated"), "{said}");
}

/// A key under the old name is no key: without `DROPPEN_RELAY_KEY` or a
/// state directory, the relay says what to set rather than start.
#[test]
fn the_old_key_name_is_not_a_key() {
    let (key, keys) = (hex(&[8; 32]), format!("1:{}", hex(&[9; 32])));
    let mut env = new_names(&key, &keys);
    env.retain(|(name, _)| *name != "DROPPEN_RELAY_KEY");
    let env = [pairs(&env), vec![("RH_RELAY_KEY", key.as_str())]].concat();
    let (ok, _, said) = relay(&["serve"], &env);
    assert!(!ok && said.contains("set DROPPEN_RELAY_KEY"), "{said}");
}

#[test]
fn with_nothing_set_it_says_the_new_name() {
    let (ok, _, said) = relay(&["serve"], &[]);
    assert!(!ok && said.contains("DROPPEN_RELAY_ORIGIN is required"), "{said}");
    // A new key is printed under the new name.
    let (ok, stdout, _) = relay(&["keygen"], &[]);
    assert!(ok && stdout.starts_with("DROPPEN_RELAY_KEY=") && stdout.trim().len() == "DROPPEN_RELAY_KEY=".len() + 64, "{stdout}");
}
