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
    println!("cargo::rustc-check-cfg=cfg(web_dist_missing)");
    println!("cargo::rerun-if-changed=../web/dist/index.html");

    // Watched always, read only when needed.
    rerun_if_changed(&["DROPPEN_ALLOW_MISSING_WEB_DIST"]);
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../web/dist/index.html");
    if dist.exists() {
        return;
    }
    if build_var("DROPPEN_ALLOW_MISSING_WEB_DIST").as_deref() != Some("1") {
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
