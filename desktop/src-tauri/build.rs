fn main() {
    // `cargo build` / `cargo test` of a checkout that has never run build.sh:
    // Tauri insists the sidecar exists, so a debug build gets a placeholder that
    // says what to do. A release build (`cargo tauri build`, which build.sh
    // drives after placing the real server) never does — a missing sidecar
    // there is the error it should be.
    if std::env::var("PROFILE").as_deref() == Ok("debug") {
        let target = std::env::var("TARGET").expect("TARGET");
        let path = std::path::Path::new("binaries").join(format!("droppen-server-{target}"));
        if !path.exists() {
            use std::os::unix::fs::PermissionsExt;
            std::fs::create_dir_all("binaries").expect("binaries/");
            std::fs::write(
                &path,
                "#!/bin/sh\necho 'placeholder: run desktop/build.sh, or put the server here as binaries/droppen-server-<target>' >&2\nexit 1\n",
            )
            .expect("placeholder sidecar");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
    }
    check_identity();
    // The app's own commands, declared so they need a permission: only the
    // bundled start-up page is granted them (capabilities/main.json), never the
    // server origin the window later shows.
    let manifest = tauri_build::AppManifest::new().commands(&["startup_status", "retry_start", "open_logs"]);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest)).expect("tauri build script");
}

/// The bundle identifier this build gets — `tauri.conf.json`, the platform's
/// `tauri.macos.conf.json` if there is one, then `TAURI_CONFIG` (what
/// `cargo tauri build --config …` merges in, e.g. `tauri.e2e.conf.json`) —
/// must be the identity the app is compiled with (`core/src/identity.rs`:
/// `RELEASE`, or `E2E` with the `e2e` feature). The app refuses to start when
/// they differ, so a mismatch would ship an app that exits on every launch,
/// and the updater would install it everywhere: a build error instead. Only a
/// debug build with the `e2e` feature and no `--config` (`cargo test
/// --features e2e`, which never runs the app) gets a warning.
fn check_identity() {
    use serde_json::Value;

    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=tauri.macos.conf.json");
    println!("cargo:rerun-if-changed=../core/src/identity.rs");
    println!("cargo:rerun-if-env-changed=TAURI_CONFIG");

    let read = |path: &str| -> Option<Value> {
        let text = std::fs::read_to_string(path).ok()?;
        Some(serde_json::from_str(&text).unwrap_or_else(|error| panic!("{path}: {error}")))
    };
    let merged = std::env::var("TAURI_CONFIG")
        .ok()
        .map(|text| serde_json::from_str::<Value>(&text).unwrap_or_else(|error| panic!("TAURI_CONFIG: {error}")));
    let identifier = [read("tauri.conf.json"), read("tauri.macos.conf.json"), merged]
        .into_iter()
        .flatten()
        .filter_map(|config| config.get("identifier").and_then(Value::as_str).map(str::to_string))
        .last()
        .expect("tauri.conf.json names an identifier");

    let e2e = std::env::var_os("CARGO_FEATURE_E2E").is_some();
    let identity = if e2e { "E2E" } else { "RELEASE" };
    let source = std::fs::read_to_string("../core/src/identity.rs").expect("../core/src/identity.rs");
    let expected = bundle_id_of(&source, identity).unwrap_or_else(|| panic!("no bundle_id for {identity} in core/src/identity.rs"));
    if identifier == expected {
        return;
    }
    let message = format!(
        "the bundle identifier is {identifier}, but the app is compiled as {expected} (core/src/identity.rs, {identity}): it would refuse to start{}",
        if e2e { "; an end-to-end build takes --config src-tauri/tauri.e2e.conf.json (desktop/build.sh --e2e)" } else { "" }
    );
    let release = std::env::var("PROFILE").as_deref() == Ok("release");
    if e2e && !release && std::env::var_os("TAURI_CONFIG").is_none() {
        println!("cargo:warning={message}");
    } else {
        panic!("{message}");
    }
}

/// `bundle_id: "…"` of `pub const <name>: Identity = Identity { … };`.
fn bundle_id_of(source: &str, name: &str) -> Option<String> {
    let start = source.find(&format!("pub const {name}: Identity = Identity {{"))?;
    let block = &source[start..start + source[start..].find("};")?];
    let line = block.lines().find_map(|line| line.trim().strip_prefix("bundle_id: \""))?;
    Some(line.strip_suffix("\",")?.to_string())
}
