//! The bridge's route allow-list (`droppen_agent::bridge::ROUTES`) against
//! every route the server registers: read off the router it serves and off
//! its source, which only the server has.

use droppen_agent::bridge::{classify, refusal, Refusal, RouteClass, Standing, View, LOCAL_PREFIX, ROUTES};
use droppen_agent::testing::plugin_route;

const ADMIN: Standing = Standing {
    admin: true,
    view: View::Fresh,
    controls: true,
};

/// `(METHOD, path template)` of every route the server's source
/// registers on its router: `.get("/api/…", handler)` and the other
/// methods (a path constant resolved in its own file, or else in the one
/// other file that defines it, as the guard's crate defines
/// `auth::RETURN_ROUTE` for cloud's handler), and
/// `upload_route("/api/…", …)` (a POST). The undocumented ones — the
/// websockets — are only found this way.
fn registered_in_source() -> std::collections::BTreeSet<(String, String)> {
    let call = regex::Regex::new(r#"\.(get|post|put|patch|delete|head)\(\s*(?:"(/api/[A-Za-z0-9_./{}-]*)"|([A-Z][A-Z0-9_]*))\s*,"#).unwrap();
    let upload = regex::Regex::new(r#"upload_route\(\s*"(/api/[A-Za-z0-9_./{}-]*)""#).unwrap();
    let constant = regex::Regex::new(r#"const ([A-Z][A-Z0-9_]*): &str = "(/api/[^"]*)";"#).unwrap();
    let mut files = Vec::new();
    // The server's sources, and those of the crates it is made of that
    // register routes.
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut dirs = vec![
        manifest.join("src"),
        manifest.join("crates/droppen-supervisor/src"),
        manifest.join("crates/droppen-auth/src"),
        manifest.join("crates/droppen-files/src"),
        manifest.join("crates/droppen-workbench/src"),
        manifest.join("crates/droppen-plugins/src"),
        manifest.join("crates/droppen-accounts/src"),
        manifest.join("crates/droppen-setup/src"),
        manifest.join("crates/droppen-sessions/src"),
        manifest.join("crates/droppen-agent/src"),
        manifest.join("crates/droppen-cloud/src"),
        manifest.join("crates/droppen-notify/src"),
        manifest.join("crates/droppen-provider/src"),
        manifest.join("crates/droppen-provider-acp/src"),
        manifest.join("crates/droppen-drive/src"),
    ];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            files.push(std::fs::read_to_string(&path).unwrap());
        }
    }
    // A route constant every file can name: one defined in exactly one file.
    let mut shared: std::collections::BTreeMap<String, Option<String>> = std::collections::BTreeMap::new();
    for text in &files {
        for d in constant.captures_iter(text) {
            shared
                .entry(d[1].to_string())
                .and_modify(|seen| *seen = None)
                .or_insert_with(|| Some(d[2].to_string()));
        }
    }
    let mut found = std::collections::BTreeSet::new();
    for text in &files {
        for c in call.captures_iter(text) {
            let route = match (c.get(2), c.get(3)) {
                (Some(literal), _) => literal.as_str().to_string(),
                (None, Some(name)) => {
                    let def = regex::Regex::new(&format!(r#"const {}: &str = "([^"]*)";"#, name.as_str())).unwrap();
                    match (def.captures(text), shared.get(name.as_str())) {
                        (Some(d), _) => d[1].to_string(),
                        (None, Some(Some(route))) => route.clone(),
                        // Not a route constant (`.get(KEY, …)` on a map).
                        _ => continue,
                    }
                }
                _ => unreachable!(),
            };
            if route.starts_with("/api/") {
                found.insert((c[1].to_ascii_uppercase(), route));
            }
        }
        for c in upload.captures_iter(text) {
            found.insert(("POST".to_string(), c[1].to_string()));
        }
    }
    found
}

/// The routes a device may reach are an allow-list.
/// Every route the server registers — read off the router it serves
/// (its OpenAPI document) and off the source (which also finds the
/// undocumented websockets) — is classified in [`ROUTES`]; a new route
/// fails here until it is, and is refused to every device meanwhile. The
/// exception is the plugin routes, which are local-only
/// (`docs/plugins/runtime.md` §5) and refused by [`plugin_route`] rather
/// than classified: a new one needs no entry here.
#[tokio::test]
async fn every_route_the_server_registers_is_classified() {
    use std::collections::BTreeSet;
    let home = tempfile::tempdir().unwrap();
    let documented = super::test_support::documented_routes(home.path()).await;
    let registered = registered_in_source();
    assert!(documented.len() > 80, "{} documented routes", documented.len());
    let unseen: Vec<_> = documented.difference(&registered).collect();
    assert!(
        unseen.is_empty(),
        "the source scan does not see these routes the router serves: {unseen:?}"
    );
    let classified: BTreeSet<(String, String)> = ROUTES.iter().map(|(m, p, _)| (m.to_string(), p.to_string())).collect();
    assert_eq!(classified.len(), ROUTES.len(), "a route classified twice");
    let unclassified: Vec<_> = registered
        .union(&documented)
        .filter(|r| !classified.contains(*r) && !plugin_route(&r.1))
        .collect();
    assert!(unclassified.is_empty(), "classify these in bridge::ROUTES: {unclassified:?}");
    let stale: Vec<_> = classified
        .iter()
        .filter(|(_, p)| !p.starts_with(LOCAL_PREFIX))
        .filter(|r| !registered.contains(*r))
        .collect();
    assert!(stale.is_empty(), "bridge::ROUTES lists routes the server does not register: {stale:?}");
    // Each, as a device sends it, is judged by its class — never refused as unlisted.
    for (method, template) in &registered {
        let sent: Vec<&str> = template.split('/').map(|s| if s.starts_with('{') { "x-1" } else { s }).collect();
        let sent = sent.join("/");
        let judged = refusal(method, &sent, ADMIN);
        assert_ne!(judged, Some(Refusal::Unlisted), "{method} {sent}");
        let expected = if plugin_route(&sent) || classify(method, &sent) == Some(RouteClass::Never) {
            Some(Refusal::NotFromCloud)
        } else {
            None
        };
        assert_eq!(judged, expected, "{method} {sent}");
    }
}
