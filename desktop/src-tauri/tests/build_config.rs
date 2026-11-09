//! `build_config.rs`: how `build.rs` finds the bundle identifier a build gets,
//! which it holds to the one the app is compiled with.

#[path = "../build_config.rs"]
mod build_config;

use build_config::{bundle_id_of, identifier, platform_config};
use serde_json::{json, Value};

/// The targets releases are built for.
const RELEASE_TARGETS: [&str; 4] = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
];

fn config(name: &str) -> Option<Value> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(name);
    let text = std::fs::read_to_string(&path).ok()?;
    Some(serde_json::from_str(&text).unwrap_or_else(|error| panic!("{name}: {error}")))
}

/// What `build.rs` merges for `target`: `tauri.conf.json`, the platform's own
/// file if there is one, then `extra` (`--config`).
fn merged(target: &str, extra: Option<Value>) -> Vec<Value> {
    [config("tauri.conf.json"), config(platform_config(target)), extra]
        .into_iter()
        .flatten()
        .collect()
}

fn compiled_as(identity: &str) -> String {
    let source = include_str!("../../core/src/identity.rs");
    bundle_id_of(source, identity).unwrap_or_else(|| panic!("no bundle_id for {identity} in core/src/identity.rs"))
}

#[test]
fn each_target_merges_its_own_platform_file() {
    assert_eq!(platform_config("aarch64-apple-darwin"), "tauri.macos.conf.json");
    assert_eq!(platform_config("x86_64-apple-darwin"), "tauri.macos.conf.json");
    assert_eq!(platform_config("universal-apple-darwin"), "tauri.macos.conf.json");
    assert_eq!(platform_config("x86_64-unknown-linux-gnu"), "tauri.linux.conf.json");
    assert_eq!(platform_config("aarch64-unknown-linux-gnu"), "tauri.linux.conf.json");
    assert_eq!(platform_config("x86_64-pc-windows-msvc"), "tauri.windows.conf.json");
}

#[test]
fn the_last_file_that_names_an_identifier_wins() {
    let base = json!({ "identifier": "app.droppen.desktop", "productName": "Droppen" });
    let platform = json!({ "identifier": "app.droppen.desktop.linux" });
    let none = json!({ "bundle": { "targets": ["deb"] } });
    assert_eq!(identifier(std::slice::from_ref(&base)).as_deref(), Some("app.droppen.desktop"));
    assert_eq!(
        identifier(&[base.clone(), platform.clone()]).as_deref(),
        Some("app.droppen.desktop.linux")
    );
    assert_eq!(
        identifier(&[base.clone(), platform, none.clone()]).as_deref(),
        Some("app.droppen.desktop.linux")
    );
    assert_eq!(identifier(&[base, none.clone()]).as_deref(), Some("app.droppen.desktop"));
    assert_eq!(identifier(&[none]), None);
}

#[test]
fn every_release_target_is_built_as_the_release_identity() {
    let release = compiled_as("RELEASE");
    for target in RELEASE_TARGETS {
        assert_eq!(identifier(&merged(target, None)).as_deref(), Some(release.as_str()), "{target}");
    }
}

#[test]
fn every_end_to_end_build_is_built_as_its_own_identity() {
    // What `desktop/build.sh --e2e` passes as `--config`.
    let e2e = compiled_as("E2E");
    for target in RELEASE_TARGETS {
        let configs = merged(target, config("tauri.e2e.conf.json"));
        assert_eq!(identifier(&configs).as_deref(), Some(e2e.as_str()), "{target}");
    }
}

#[test]
fn linux_has_a_platform_file_and_it_bundles_what_linux_installs() {
    let linux = config("tauri.linux.conf.json").expect("tauri.linux.conf.json");
    assert_eq!(linux["bundle"]["targets"], json!(["deb", "appimage"]));
    // No `@2x` icon: Tauri installs one as the scale-2 icon of its size
    // (`hicolor/256x256@2`, 512 pixels), not as the 256-pixel one.
    let icons = linux["bundle"]["icon"].as_array().expect("bundle.icon");
    assert!(icons.iter().all(|icon| !icon.as_str().expect("a path").contains("@2x")), "{icons:?}");
    for icon in icons {
        let name = icon.as_str().expect("a path");
        let png = std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(name)).unwrap_or_else(|error| panic!("{name}: {error}"));
        // An `NxN.png` is N pixels square (the PNG header's width and height).
        let size = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().expect("4 bytes"));
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n") && png.len() > 24, "{name} is not a PNG");
        if let Some((width, height)) = name.strip_prefix("icons/").and_then(|file| file.strip_suffix(".png")?.split_once('x')) {
            assert_eq!(
                (size(16).to_string(), size(20).to_string()),
                (width.to_string(), height.to_string()),
                "{name}"
            );
        }
    }
}
