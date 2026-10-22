use std::path::Path;

/// The `embed` feature is default-on so a release build always ships the SPA.
/// `rust-embed`'s derive, however, fails the build outright when its folder is
/// missing — which would make `cargo check` impossible on a fresh clone that has
/// not run `pnpm --filter @remote-hand/web build` yet. So the presence of
/// `web/dist` is decided here and handed to the crate as a cfg: present means a
/// real embed, absent means a stub service that explains what to run.
///
/// Absent is a *deliberate* state, not a default one. A `cargo build --release`
/// that quietly produces a binary serving "run pnpm build" instead of the
/// application is the kind of thing that is only discovered after it ships, so
/// the missing directory fails the build unless `DROPPEN_ALLOW_MISSING_WEB_DIST=1`
/// says the caller meant it. The root `pnpm dev:server`, `pnpm gen:api` and
/// `pnpm test` scripts set it; `pnpm build:server` deliberately does not.
fn main() {
    build_identity();

    println!("cargo::rustc-check-cfg=cfg(web_dist_missing)");
    println!("cargo::rerun-if-changed=../web/dist/index.html");

    // Watched always, read (and an old name warned about) only when needed.
    rerun_if_changed(&["DROPPEN_ALLOW_MISSING_WEB_DIST", "RH_ALLOW_MISSING_WEB_DIST"]);
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../web/dist/index.html");
    if dist.exists() {
        return;
    }
    if build_env("DROPPEN_ALLOW_MISSING_WEB_DIST", "RH_ALLOW_MISSING_WEB_DIST").as_deref() != Some("1") {
        panic!(
            "web/dist/index.html is missing, so this binary would serve a placeholder page \
             instead of the SPA. Run `pnpm build:web` first, or set \
             DROPPEN_ALLOW_MISSING_WEB_DIST=1 for a deliberately server-only build."
        );
    }
    println!("cargo::rustc-cfg=web_dist_missing");
    println!(
        "cargo::warning=web/dist not found; the SPA will not be embedded. \
         Run `pnpm --filter @remote-hand/web build` and rebuild."
    );
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
    for path in ["../.git/HEAD", "../.git/index"] {
        if Path::new(env!("CARGO_MANIFEST_DIR")).join(path).exists() {
            println!("cargo::rerun-if-changed={path}");
        }
    }

    rerun_if_changed(&["DROPPEN_BUILD_REV", "RH_BUILD_REV"]);
    let rev = build_env("DROPPEN_BUILD_REV", "RH_BUILD_REV").unwrap_or_else(git_rev);
    println!("cargo::rustc-env=DROPPEN_BUILD_REV={}", rev.trim());
}

/// Rebuild when any of `names` changes.
fn rerun_if_changed(names: &[&str]) {
    for name in names {
        println!("cargo::rerun-if-env-changed={name}");
    }
}

/// A build variable under its name since the rename to Droppen, else under
/// `old`, the one it had before: trimmed, empty
/// is unset, and the old name — or both set to different values, when the new
/// one wins — is a build warning. The server's own reader, `crate::env`, does
/// the same for the variables it reads when it runs.
fn build_env(name: &str, old: &str) -> Option<String> {
    let get = |n: &str| std::env::var(n).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    match (get(name), get(old)) {
        (None, Some(was)) => {
            println!("cargo::warning={old} is deprecated: rename it {name} (the old name is still read, for now)");
            Some(was)
        }
        (Some(value), Some(was)) if was != value => {
            println!("cargo::warning={name} and the deprecated {old} are both set, to different values: {name} is used; remove {old}");
            Some(value)
        }
        (value, _) => value,
    }
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
