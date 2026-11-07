//! The server's version and the commit it was built from, for
//! `SERVER_VERSION` and `RUNTIME_VERSION`. Both name the server's build, not
//! this crate's: this crate's own `CARGO_PKG_VERSION` is its `0.1.0`, so the
//! version is read from the server's manifest, and the commit from its tree.

use std::path::Path;

fn main() {
    server_version();
    build_identity();
}

/// The server package's version (`server-rs/Cargo.toml`), as
/// `DROPPEN_SERVER_VERSION`: what `CARGO_PKG_VERSION` says when the server
/// itself is compiled. The release tooling bumps that one line, so it stays
/// the only place the version is written.
fn server_version() {
    println!("cargo::rerun-if-changed=../../Cargo.toml");
    let manifest = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml")).expect("the server's manifest");
    let mut section = "";
    let version = manifest
        .lines()
        .find_map(|line| {
            if line.starts_with('[') {
                section = line.trim();
                return None;
            }
            (section == "[package]").then(|| line.strip_prefix("version = \"")?.strip_suffix('"')).flatten()
        })
        .expect("the server's [package] version");
    println!("cargo::rustc-env=DROPPEN_SERVER_VERSION={version}");
}

/// Stamp the build with the commit it came from, as `DROPPEN_BUILD_REV`.
///
/// Self-update needs two builds of the same crate version to be *distinguishable*
/// — `0.1.0` replaced by `0.1.0` is the normal case during development, and a
/// version that does not change is a banner that never appears. So the crate
/// version is joined with a short commit hash, and the whole thing is the
/// server's identity.
///
/// Three ways this can come out, in order of preference:
///
/// * `DROPPEN_BUILD_REV` already in the environment — how CI passes a revision in
///   when the build context has no usable `.git`, or `git` is not in the
///   builder image;
/// * `git rev-parse --short HEAD`, with `-dirty` appended when the tree has
///   uncommitted changes, for a developer build;
/// * empty, for a source tarball with neither. The version is then the bare
///   crate version, which is honest: there is nothing more to say about it.
///
/// The `rerun-if` lines matter more than they look. Without them cargo would
/// keep a stale revision baked into an otherwise fresh binary, and a self-update
/// that swapped one commit for another would report no change at all.
fn build_identity() {
    for path in ["../../../.git/HEAD", "../../../.git/index"] {
        if Path::new(env!("CARGO_MANIFEST_DIR")).join(path).exists() {
            println!("cargo::rerun-if-changed={path}");
        }
    }

    rerun_if_changed(&["DROPPEN_BUILD_REV"]);
    let rev = build_var("DROPPEN_BUILD_REV").unwrap_or_else(git_rev);
    println!("cargo::rustc-env=DROPPEN_BUILD_REV={}", rev.trim());
}

/// Rebuild when any of `names` changes.
fn rerun_if_changed(names: &[&str]) {
    for name in names {
        println!("cargo::rerun-if-env-changed={name}");
    }
}

/// A build variable from the environment: trimmed, and empty counts as unset.
fn build_var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// The short commit this tree is on, marked `-dirty` when it has been edited.
fn git_rev() -> String {
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .args(args)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let Some(rev) = run(&["rev-parse", "--short", "HEAD"]).filter(|rev| !rev.is_empty()) else {
        return String::new();
    };
    // `--quiet` makes this an exit code rather than a diff, so a large working
    // tree does not slow every build down.
    match run(&["diff", "--quiet", "HEAD", "--"]) {
        Some(_) => rev,
        None => format!("{rev}-dirty"),
    }
}
