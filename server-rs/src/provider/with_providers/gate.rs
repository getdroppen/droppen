//! The spawn gate and the runs, as the server's sources use them.

use std::path::{Path, PathBuf};

/// One source file of the server, of the provider base or of a provider's
/// crate: what it says, and its code as these scans read it ([`code`]). A
/// file that is a test module's has none.
struct Source {
    path: String,
    text: String,
    code: String,
}

/// Every source file of the server, of the provider base and of the ACP
/// provider's crate, but the gate's own and this one.
fn server_sources() -> Vec<Source> {
    fn walk(dir: &Path, found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read_dir").flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                found.push(path);
            }
        }
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut paths = Vec::new();
    for dir in [manifest.join("src"), manifest.join("crates/droppen-provider/src"), manifest.join("crates/droppen-provider-acp/src")] {
        if dir.is_dir() {
            walk(&dir, &mut paths);
        }
    }
    let read: Vec<(PathBuf, String, (String, Vec<String>))> = paths
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path).expect("source");
            let code = code(&text);
            (path, text, code)
        })
        .collect();
    // The modules declared for tests alone, each a file and what is under it.
    let mut tests = Vec::new();
    for (path, _, (_, modules)) in &read {
        let dir = match path.file_name().and_then(|name| name.to_str()) {
            Some("mod.rs" | "lib.rs" | "main.rs") => path.parent().expect("a parent").to_path_buf(),
            _ => path.with_extension(""),
        };
        tests.extend(modules.iter().map(|module| dir.join(module)));
    }
    read.into_iter()
        .filter(|(path, _, _)| !gate_or_scan(path))
        .map(|(path, text, (code, _))| {
            let test = tests.iter().any(|module| path.starts_with(module) || path == module.with_extension("rs"));
            Source {
                path: path.display().to_string(),
                text,
                code: if test { String::new() } else { code },
            }
        })
        .collect()
}

/// Whether `path` is the spawn gate's own source, the test probe that hands
/// out its gateless permit, or these scans'.
fn gate_or_scan(path: &Path) -> bool {
    ["provider/gate.rs", "droppen-provider/src/gate.rs", "droppen-provider/src/testing.rs", "with_providers/gate.rs"].iter().any(|own| path.ends_with(own))
}

/// Whether `path` is the provider registry's source.
fn registry(path: &str) -> bool {
    path.ends_with("provider/registry.rs") || path.ends_with("droppen-provider/src/registry.rs")
}

/// Whether `path` is the providers' own: the base's, or an implementation's,
/// in the server or in a crate of its own.
fn in_provider(path: &str) -> bool {
    path.contains("/src/provider/") || path.contains("/crates/droppen-provider/") || path.contains("/crates/droppen-provider-acp/")
}

/// Whether `c` can be part of an identifier.
fn ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Where in `code` the identifier `name` is, as a whole identifier.
fn whole<'a>(code: &'a str, name: &'a str) -> impl Iterator<Item = usize> + 'a {
    code.match_indices(name)
        .map(|(at, _)| at)
        .filter(move |&at| !code[..at].ends_with(ident) && !code[at + name.len()..].starts_with(ident))
}

/// Which of `names` `code` names.
fn named(code: &str, names: &[&'static str]) -> Vec<&'static str> {
    names.iter().copied().filter(|name| whole(code, name).next().is_some()).collect()
}

/// Which of the fields `names` `code` reads off a value (`.name`, not a
/// call of a method `name`).
fn fields(code: &str, names: &[&'static str]) -> Vec<&'static str> {
    names
        .iter()
        .copied()
        .filter(|name| whole(code, name).any(|at| code[..at].ends_with('.') && !code[at + name.len()..].trim_start().starts_with('(')))
        .collect()
}

/// The attribute of an item, a statement or a field there for tests alone.
const TESTS_ONLY: &str = "#[cfg(test)]";

/// What `text` says as code, and the modules it declares for its tests
/// alone (`#[cfg(test)] mod name;`), whose files are theirs. Its comments
/// go, and so does every `#[cfg(test)]` item, statement or field, from the
/// attribute through the `;`, `,` or `}` that ends it; what follows one
/// stays. String and character literals are read as such, so a brace or a
/// `//` in one is not counted.
fn code(text: &str) -> (String, Vec<String>) {
    let mut code = String::with_capacity(text.len());
    let mut modules = Vec::new();
    let mut at = 0;
    while at < text.len() {
        if text[at..].starts_with(TESTS_ONLY) {
            let end = tests_only_end(text, at + TESTS_ONLY.len());
            modules.extend(declared_module(&code_of(&text[at + TESTS_ONLY.len()..end])));
            at = end;
            continue;
        }
        let end = token_end(text, at);
        if !text[at..].starts_with("//") && !text[at..].starts_with("/*") {
            code.push_str(&text[at..end]);
        }
        at = end;
    }
    (code, modules)
}

/// [`code`], without the modules.
fn code_of(text: &str) -> String {
    code(text).0
}

/// The module `item` declares in a file of its own (`mod name;`), if that
/// is what it is.
fn declared_module(item: &str) -> Option<String> {
    let item = item.trim_start();
    let item = item.strip_prefix("pub").map_or(item, |rest| {
        let rest = rest.trim_start();
        rest.strip_prefix('(').and_then(|rest| rest.split_once(')')).map_or(rest, |(_, rest)| rest)
    });
    let name = item.trim_start().strip_prefix("mod")?.strip_suffix(';')?;
    if !name.starts_with(char::is_whitespace) {
        return None;
    }
    let name = name.trim();
    (name.chars().all(ident) && !name.is_empty()).then(|| name.to_string())
}

/// Where the token at `at` in `text` ends: a comment, a string or character
/// literal, or else one character.
fn token_end(text: &str, at: usize) -> usize {
    let rest = &text[at..];
    let one = at + rest.chars().next().map_or(1, char::len_utf8);
    if rest.starts_with("//") {
        return at + rest.find('\n').unwrap_or(rest.len());
    }
    if rest.starts_with("/*") {
        // Block comments nest.
        let mut depth = 0;
        let mut i = 0;
        while i < rest.len() {
            if rest[i..].starts_with("/*") {
                depth += 1;
                i += 2;
            } else if rest[i..].starts_with("*/") {
                depth -= 1;
                i += 2;
                if depth == 0 {
                    return at + i;
                }
            } else {
                i += rest[i..].chars().next().map_or(1, char::len_utf8);
            }
        }
        return text.len();
    }
    // A raw string: `r"…"`, `r#"…"#`, `br"…"`, closed by a `"` and as many
    // `#` as it opens with.
    let raw = rest.strip_prefix("br").or_else(|| rest.strip_prefix('r')).filter(|_| !text[..at].ends_with(ident));
    if let Some(after) = raw {
        let hashes = after.len() - after.trim_start_matches('#').len();
        if after[hashes..].starts_with('"') {
            let open = rest.len() - after.len() + hashes + 1;
            let close = format!("\"{}", "#".repeat(hashes));
            return rest[open..].find(&close).map_or(text.len(), |end| at + open + end + close.len());
        }
    }
    if rest.starts_with('"') {
        let mut chars = rest.char_indices().skip(1);
        while let Some((i, c)) = chars.next() {
            match c {
                '\\' => {
                    chars.next();
                }
                '"' => return at + i + 1,
                _ => {}
            }
        }
        return text.len();
    }
    if rest.starts_with('\'') {
        // A character literal, or else a lifetime or a label.
        let mut chars = rest.char_indices().skip(1);
        return match chars.next() {
            // An escape: `\` and the character after it, then up to the `'`.
            Some((_, '\\')) => rest.get(3..).and_then(|tail| tail.find('\'')).map_or(text.len(), |end| at + 3 + end + 1),
            Some(_) => match chars.next() {
                Some((i, '\'')) => at + i + 1,
                _ => one,
            },
            None => one,
        };
    }
    one
}

/// Where the item, statement or field whose attribute ends at `from` in
/// `text` ends: past the `;` or the `}` of its body that ends an item or a
/// statement, or past the `,` or `;` that ends a field, a variant or an
/// arm. A field is `name: Type`, its angle brackets counted; a variant, a
/// path or a struct starts with a capital.
fn tests_only_end(text: &str, from: usize) -> usize {
    let mut cut = text.len().min(from + 400);
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let head = code_of(&text[from..cut]);
    let mut head = head.trim_start();
    while head.starts_with("#[") {
        head = head.split_once(']').map_or("", |(_, rest)| rest).trim_start();
    }
    let unscoped = head.strip_prefix("pub").map_or(head, |rest| {
        let rest = rest.trim_start();
        rest.strip_prefix('(').and_then(|rest| rest.split_once(')')).map_or(rest, |(_, rest)| rest)
    });
    let word: String = unscoped.trim_start().chars().take_while(|&c| ident(c)).collect();
    let after = unscoped.trim_start()[word.len()..].trim_start();
    let declared = !word.is_empty() && after.starts_with(':') && !after.starts_with("::");
    let listed = declared || word.starts_with(char::is_uppercase);
    let mut depth = 0usize;
    let mut at = from;
    while at < text.len() {
        let end = token_end(text, at);
        let previous = text[..at].chars().next_back();
        match &text[at..end] {
            "(" | "[" | "{" => depth += 1,
            "<" if declared => depth += 1,
            ">" if declared && !matches!(previous, Some('-' | '=')) => depth = depth.saturating_sub(1),
            ")" | "]" | "}" if depth == 0 => return at,
            ")" | "]" => depth -= 1,
            "}" => {
                depth -= 1;
                if depth == 0 && !listed {
                    return end;
                }
            }
            ";" if depth == 0 => return end,
            "," if depth == 0 && listed => return end,
            _ => {}
        }
        at = end;
    }
    text.len()
}

/// The permit that takes no gate starts nothing in the server: only the
/// shim's own end-to-end test, outside `src/`, may use it.
#[test]
fn nothing_in_the_server_starts_a_cli_without_the_gate() {
    // The name as a whole identifier, so a call spelt
    // `without_a_session ()` or passed as a function is found
    // too, and a test named `…_without_a_session_…` is not.
    const NAME: &str = "without_a_session";
    let found: Vec<String> = server_sources()
        .into_iter()
        .filter(|source| whole(&source.text, NAME).next().is_some())
        .map(|source| source.path)
        .collect();
    assert!(found.is_empty(), "a CLI started without the spawn gate: {found:?}");
}

/// Only the provider registry makes a spawn gate, so the gate a provider
/// is lent is the only one there is: a provider that made its own and took
/// that instead would start a CLI beside one the registry's gate keeps
/// out. A file's tests make theirs as they like.
#[test]
fn only_the_provider_registry_makes_a_spawn_gate() {
    // `SpawnGate::new` as a path, however spaced and whatever it is
    // reached through, in a file's code: its tests and comments aside.
    let makes = |code: &str| {
        whole(code, "SpawnGate").any(|at| {
            let rest = code[at + "SpawnGate".len()..].trim_start();
            rest.strip_prefix("::").is_some_and(|rest| {
                let rest = rest.trim_start();
                rest.starts_with("new") && !rest["new".len()..].starts_with(ident)
            })
        })
    };
    let sources = server_sources();
    assert!(
        sources.iter().any(|source| registry(&source.path) && makes(&source.code)),
        "the provider registry makes the gate"
    );
    let found: Vec<&str> = sources
        .iter()
        .filter(|source| !registry(&source.path) && makes(&source.code))
        .map(|source| source.path.as_str())
        .collect();
    assert!(found.is_empty(), "a spawn gate made outside the provider registry: {found:?}");
}

/// The spawn gate reaches the providers alone. Outside them (the server's
/// `provider`, the base's crate and the ACP provider's), no production code
/// names the gate, a permit or a lease, so none can take the gate, mint a
/// permit or build a lease. Inside them, the ACP provider's
/// agent routes, which start and stop its CLI outside a run, are lent the
/// gate through `provider_with_gate`, which nothing else names but the
/// registry that defines it: they reach it through `AcpAgentState::gate`,
/// and every call of theirs that takes the gate is handed it from there.
/// A file's tests aside.
#[test]
fn only_the_providers_reach_the_spawn_gate() {
    const NAMES: [&str; 4] = ["SpawnGate", "SpawnPermit", "SessionLease", "provider_with_gate"];
    const LEND: &str = "provider_with_gate";
    let sources = server_sources();
    let mut found = Vec::new();
    for source in &sources {
        if !in_provider(&source.path) {
            found.extend(named(&source.code, &NAMES).into_iter().map(|name| format!("{}: {name}", source.path)));
        } else if !registry(&source.path) && !source.path.ends_with("droppen-provider-acp/src/agents.rs") && whole(&source.code, LEND).next().is_some() {
            found.push(format!("{}: {LEND}", source.path));
        }
    }
    assert!(found.is_empty(), "the spawn gate reached outside the providers: {found:?}");

    let routes = sources.iter().find(|source| source.path.ends_with("droppen-provider-acp/src/agents.rs")).expect("the ACP agent routes");
    let lends: Vec<usize> = whole(&routes.code, LEND).collect();
    assert_eq!(lends.len(), 1, "the ACP agent routes are lent the gate in one place");
    let lent = routes.code[lends[0] + LEND.len()..].split_whitespace().collect::<String>();
    assert!(lent.starts_with("::<crate::Acp>()"), "the ACP agent routes are lent the gate as the ACP provider");
    // The calls the routes make that take the gate: the ACP runs' methods
    // whose first argument is one.
    let gated: Vec<String> = sources
        .iter()
        .filter(|source| source.path.contains("/droppen-provider-acp/src/"))
        .flat_map(|source| {
            let code = source.code.split_whitespace().collect::<Vec<_>>().join(" ");
            code.match_indices("(&self, gate: &SpawnGate")
                .filter_map(|(at, _)| code[..at].rsplit_once("fn ").map(|(_, name)| name.trim().to_string()))
                .collect::<Vec<_>>()
        })
        .collect();
    let mut calls = 0;
    for name in &gated {
        for at in whole(&routes.code, name) {
            if routes.code[..at].ends_with('.') {
                calls += 1;
                let argument = routes.code[at + name.len()..].split_whitespace().collect::<String>();
                assert!(argument.starts_with("(state.gate()?"), "an ACP agent route calls {name} with a gate of its own");
            }
        }
    }
    assert!(calls > 0, "the ACP agent routes change agents under the gate: {gated:?}");
}

/// A run's entry and its row in the journal are its driver's, and so is
/// the draft of its turn: outside the providers, no production code publishes,
/// renames or withdraws a run, holds one's entry, or reaches the host's run
/// journal or drafts, so none can flip a run's state or forge a journal
/// row. The feeds watch a draft through the provider registry.
#[test]
fn only_the_providers_keep_the_runs() {
    const NAMES: [&str; 5] = ["publish_run", "withdraw_run", "rename_run", "running_run", "RunEntry"];
    const FIELDS: [&str; 2] = ["journal", "drafts"];
    let sources = server_sources();
    let (inside, outside): (Vec<&Source>, Vec<&Source>) = sources.iter().partition(|source| in_provider(&source.path));
    for name in NAMES {
        assert!(inside.iter().any(|source| !named(&source.code, &[name]).is_empty()), "the providers use {name}");
    }
    for name in FIELDS {
        assert!(inside.iter().any(|source| !fields(&source.code, &[name]).is_empty()), "the providers read .{name}");
    }
    let found: Vec<String> = outside
        .iter()
        .flat_map(|source| {
            let names = named(&source.code, &NAMES).into_iter().map(str::to_string);
            let fields = fields(&source.code, &FIELDS).into_iter().map(|name| format!(".{name}"));
            names.chain(fields).map(|name| format!("{}: {name}", source.path)).collect::<Vec<_>>()
        })
        .collect();
    assert!(found.is_empty(), "a run reached outside the providers: {found:?}");
}

/// The scans read code as the compiler does: a `#[cfg(test)]` item,
/// statement or field goes, and the code after it stays; so do strings
/// with braces in them.
#[test]
fn the_scans_read_past_a_file_s_tests() {
    let text = r##"
fn kept() { let brace = "}"; let raw = r#"{"#; let c = '}'; }
#[cfg(test)]
mod tests {
    fn gone() { SpawnGate::new(); }
}
#[cfg(test)]
mod elsewhere;
struct Host {
    #[cfg(test)]
    steps: Mutex<HashMap<String, Vec<Step>>>,
    kept: u8,
}
fn also_kept<'a>(x: &'a str) {
    #[cfg(test)]
    self.host.step("a", Source::Acp, None);
    after();
}
#[cfg(test)]
impl Probe { fn gone_too() {} }
// SpawnGate::new in a comment
fn last() {}
"##;
    let (code, modules) = code(text);
    for kept in ["fn kept()", r#"let brace = "}";"#, r##"r#"{"#"##, "'}'", "struct Host {", "kept: u8,", "fn also_kept<'a>", "after();", "fn last() {}"] {
        assert!(code.contains(kept), "{kept} is kept:\n{code}");
    }
    for gone in ["mod tests", "gone()", "mod elsewhere", "steps", "self.host.step", "Probe", "gone_too", "SpawnGate"] {
        assert!(!code.contains(gone), "{gone} is gone:\n{code}");
    }
    assert_eq!(modules, ["elsewhere"]);
}
