//! The release this relay is, for `uplink::VERSION`: the version the Hub's
//! directory shows for it.
//!
//! This crate's own `CARGO_PKG_VERSION` is its `0.1.0`, which says nothing
//! about the release. The release is the server package's version
//! (`crates/droppen-server/Cargo.toml`, the one line the release tooling
//! bumps), the same one `droppen-server --version` reports and
//! `droppen-http`'s `build.rs` reads. The relay is built from the whole
//! workspace (its Dockerfile copies `crates/`), so that manifest is there.

use std::path::Path;

fn main() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../droppen-server/Cargo.toml");
    let version = std::fs::read_to_string(&manifest).ok().and_then(|text| package_version(&text));
    let version = match version {
        Some(version) => {
            println!("cargo::rerun-if-changed=../droppen-server/Cargo.toml");
            version
        }
        // This crate built without the rest of the workspace beside it: say
        // so, and tell the Hub what the crate knows rather than nothing.
        None => {
            println!("cargo::warning=the server's manifest was not found at {}: the relay will report this crate's version", manifest.display());
            std::env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION")
        }
    };
    println!("cargo::rustc-env=DROPPEN_RELAY_RELEASE_VERSION={version}");
}

/// The `version` of a manifest's `[package]`.
fn package_version(manifest: &str) -> Option<String> {
    let mut section = "";
    manifest.lines().find_map(|line| {
        if line.starts_with('[') {
            section = line.trim();
            return None;
        }
        (section == "[package]").then(|| line.strip_prefix("version = \"")?.strip_suffix('"')).flatten().map(str::to_string)
    })
}
