//! The spawn gate and the runs, as the server's sources use them. Each
//! source file is parsed (`syn`), what is built for tests alone is dropped,
//! and the rest is read token by token: a comment is no token, and a string
//! or a doc comment is a literal, which no scan matches.

use std::path::{Path, PathBuf};

use proc_macro2::{Delimiter, TokenStream, TokenTree};
use quote::ToTokens;
use syn::punctuated::Punctuated;
use syn::visit_mut::{self, VisitMut};
use syn::{Attribute, Expr, ForeignItem, ImplItem, Item, Meta, Stmt, Token, TraitItem};

/// One source file of the server or of a crate of its workspace: its path
/// from `server-rs` (`src/…`, `crates/<crate>/src/…`), what it says, the
/// tokens of its code built outside tests ([`code`]), and whether that code
/// can reach the provider base at all ([`sees_the_base`]). A file that is a
/// test module's has no tokens.
struct Source {
    path: String,
    text: String,
    code: Vec<Tok>,
    sees_base: bool,
}

/// Whether a crate with this manifest can name the provider base's items:
/// whether its `[dependencies]`, per target too but not its dev- or
/// build-dependencies, have `droppen-provider`, however spelt (a table of its
/// own, or renamed with `package`). One that cannot has fields and names of
/// its own that only sound like the base's (the agent's `drafts` are its
/// stream tasks).
fn sees_the_base(manifest: &str) -> bool {
    let manifest: toml::Table = manifest.parse().expect("a manifest");
    let has = |deps: Option<&toml::Value>| {
        deps.and_then(toml::Value::as_table).is_some_and(|deps| {
            deps.iter()
                .any(|(name, spec)| name == "droppen-provider" || spec.get("package").and_then(toml::Value::as_str) == Some("droppen-provider"))
        })
    };
    let targets = manifest.get("target").and_then(toml::Value::as_table);
    has(manifest.get("dependencies")) || targets.is_some_and(|targets| targets.values().any(|target| has(target.get("dependencies"))))
}

/// Every source file of the server and of every crate of its workspace, but
/// the gate's own and this one. Wherever the feed and the drive live, the
/// scans read them as code that can reach the base.
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
    let crates = std::fs::read_dir(manifest.join("crates")).expect("the workspace's crates").flatten();
    for dir in std::iter::once(manifest.join("src")).chain(crates.map(|entry| entry.path().join("src"))) {
        if dir.is_dir() {
            walk(&dir, &mut paths);
        }
    }
    let read: Vec<(PathBuf, String, (Vec<Tok>, Vec<String>))> = paths
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path).expect("source");
            let code = code(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
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
    let sources: Vec<Source> = read
        .into_iter()
        .map(|(path, text, (code, _))| {
            let test = tests.iter().any(|module| path.starts_with(module) || path == module.with_extension("rs"));
            let path = path.strip_prefix(manifest).expect("under server-rs").display().to_string();
            let sees_base = path.strip_prefix("crates/").and_then(|rest| rest.split_once('/')).is_none_or(|(name, _)| {
                sees_the_base(&std::fs::read_to_string(manifest.join("crates").join(name).join("Cargo.toml")).expect("its manifest"))
            });
            Source {
                path,
                text,
                code: if test { Vec::new() } else { code },
                sees_base,
            }
        })
        .filter(|source| !gate_or_scan(&source.path))
        .collect();
    for module in ["feed/mod.rs", "droppen-drive/src/lib.rs"] {
        assert!(
            sources.iter().any(|source| source.path.ends_with(module) && !source.code.is_empty() && source.sees_base),
            "the scans read {module} as code that can reach the base"
        );
    }
    sources
}

/// Whether `path` is the spawn gate's own source or these scans'.
fn gate_or_scan(path: &str) -> bool {
    ["crates/droppen-provider/src/gate.rs", "src/provider/with_providers/gate.rs"].contains(&path)
}

/// Whether `path` is the provider registry's source.
fn registry(path: &str) -> bool {
    path == "crates/droppen-provider/src/registry.rs"
}

/// Whether `path` is the providers' own: the base's, or an implementation's,
/// in the server or in a `droppen-provider*` crate.
fn in_provider(path: &str) -> bool {
    path.starts_with("src/provider/") || path.starts_with("crates/droppen-provider")
}

// ------------------------------------------------------------ the code

/// A token of a file's code as the scans match it: an identifier, a
/// punctuation character, a bracket that opens or closes a group, or a
/// literal, whose text no scan matches.
#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Punct(char),
    Open(char),
    Close(char),
    Literal,
}

/// The tokens of `text`'s code built outside tests, and the modules it
/// declares for its tests alone (`#[cfg(test)] mod name;`), whose files are
/// theirs. Every item, impl or trait item, field, variant, field value, arm,
/// statement and list element under a test-only `cfg` ([`tests_only`]) is
/// dropped; a macro's body is kept as it is written, `cfg(test)` and all.
fn code(text: &str) -> syn::Result<(Vec<Tok>, Vec<String>)> {
    let mut file = syn::parse_file(text)?;
    let mut out = TestsOut::default();
    out.visit_file_mut(&mut file);
    let mut tokens = Vec::new();
    flatten(file.into_token_stream(), &mut tokens);
    Ok((tokens, out.modules))
}

/// `stream`'s tokens, groups opened and closed around theirs.
fn flatten(stream: TokenStream, out: &mut Vec<Tok>) {
    for tree in stream {
        match tree {
            TokenTree::Ident(ident) => out.push(Tok::Ident(ident.to_string().trim_start_matches("r#").to_string())),
            TokenTree::Punct(punct) => out.push(Tok::Punct(punct.as_char())),
            TokenTree::Literal(_) => out.push(Tok::Literal),
            TokenTree::Group(group) => {
                let brackets = match group.delimiter() {
                    Delimiter::Parenthesis => Some(('(', ')')),
                    Delimiter::Bracket => Some(('[', ']')),
                    Delimiter::Brace => Some(('{', '}')),
                    Delimiter::None => None,
                };
                out.extend(brackets.map(|(open, _)| Tok::Open(open)));
                flatten(group.stream(), out);
                out.extend(brackets.map(|(_, close)| Tok::Close(close)));
            }
        }
    }
}

/// `pattern`'s tokens: words separated by spaces, each an identifier, a
/// bracket, or punctuation one character a token (`::` is two).
fn pattern(pattern: &str) -> Vec<Tok> {
    pattern
        .split_whitespace()
        .flat_map(|word| match word {
            "(" | "[" | "{" => vec![Tok::Open(word.chars().next().expect("a bracket"))],
            ")" | "]" | "}" => vec![Tok::Close(word.chars().next().expect("a bracket"))],
            _ if word.chars().all(|c| c.is_alphanumeric() || c == '_') => vec![Tok::Ident(word.to_string())],
            _ => word.chars().map(Tok::Punct).collect(),
        })
        .collect()
}

/// Where in `code` the tokens of `pattern` ([`pattern`]) start.
fn find<'a>(code: &'a [Tok], pattern: &'a [Tok]) -> impl Iterator<Item = usize> + 'a {
    (0..code.len()).filter(move |&at| code[at..].starts_with(pattern))
}

/// Whether `code` names the identifier `name`.
fn names(code: &[Tok], name: &str) -> bool {
    code.contains(&Tok::Ident(name.to_string()))
}

/// Whether `code` reads the field `name` off a value: `.name`, not a call of
/// a method `name`.
fn reads(code: &[Tok], name: &str) -> bool {
    let field = pattern(&format!(". {name}"));
    let read = find(code, &field).any(|at| code.get(at + field.len()) != Some(&Tok::Open('(')));
    read
}

/// Whether `attrs` hold a `cfg` built in tests alone: `cfg(test)`, or
/// `cfg(all(…, test, …))`, which needs it. `cfg(any(test, feature = …))` is
/// built outside tests too, so it stays.
fn tests_only(attrs: &[Attribute]) -> bool {
    fn needs_test(meta: &Meta) -> bool {
        let nested = |list: &syn::MetaList| list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated).ok();
        match meta {
            Meta::Path(path) => path.is_ident("test"),
            Meta::List(list) if list.path.is_ident("all") => nested(list).is_some_and(|all| all.iter().any(needs_test)),
            Meta::List(list) if list.path.is_ident("any") => nested(list).is_some_and(|any| !any.is_empty() && any.iter().all(needs_test)),
            _ => false,
        }
    }
    attrs.iter().any(|attr| attr.path().is_ident("cfg") && attr.parse_args::<Meta>().is_ok_and(|meta| needs_test(&meta)))
}

/// The attributes of each kind of node a `cfg` can drop, by variant.
macro_rules! attrs_of {
    ($name:ident, $type:ident; $($variant:ident),+) => {
        fn $name(node: &$type) -> &[Attribute] {
            match node {
                $($type::$variant(node) => &node.attrs,)+
                _ => &[],
            }
        }
    };
}
attrs_of!(item_attrs, Item; Const, Enum, ExternCrate, Fn, ForeignMod, Impl, Macro, Mod, Static, Struct, Trait, TraitAlias, Type, Union, Use);
attrs_of!(impl_item_attrs, ImplItem; Const, Fn, Type, Macro);
attrs_of!(trait_item_attrs, TraitItem; Const, Fn, Type, Macro);
attrs_of!(foreign_item_attrs, ForeignItem; Fn, Static, Type, Macro);
attrs_of!(
    expr_attrs, Expr; Array, Assign, Async, Await, Binary, Block, Break, Call, Cast, Closure, Const, Continue, Field, ForLoop, Group, If,
    Index, Infer, Let, Lit, Loop, Macro, Match, MethodCall, Paren, Path, Range, RawAddr, Reference, Repeat, Return, Struct, Try, TryBlock,
    Tuple, Unary, Unsafe, While, Yield
);

/// A statement's attributes.
fn stmt_attrs(stmt: &Stmt) -> &[Attribute] {
    match stmt {
        Stmt::Local(local) => &local.attrs,
        Stmt::Item(item) => item_attrs(item),
        Stmt::Expr(expr, _) => expr_attrs(expr),
        Stmt::Macro(mac) => &mac.attrs,
    }
}

/// `list` without the elements `attrs` says are for tests alone.
fn kept<T: Clone, P: Clone + Default>(list: &Punctuated<T, P>, attrs: impl Fn(&T) -> &[Attribute]) -> Punctuated<T, P> {
    list.iter().filter(|node| !tests_only(attrs(node))).cloned().collect()
}

/// What drops the nodes built for tests alone, noting the test modules
/// that are files of their own.
#[derive(Default)]
struct TestsOut {
    modules: Vec<String>,
}

impl TestsOut {
    fn keep_items(&mut self, items: &mut Vec<Item>) {
        let modules = &mut self.modules;
        items.retain(|item| {
            let test = tests_only(item_attrs(item));
            if let (true, Item::Mod(module)) = (test, item) {
                if module.content.is_none() {
                    modules.push(module.ident.to_string());
                }
            }
            !test
        });
    }
}

impl VisitMut for TestsOut {
    fn visit_file_mut(&mut self, file: &mut syn::File) {
        if tests_only(&file.attrs) {
            file.items.clear();
        }
        self.keep_items(&mut file.items);
        visit_mut::visit_file_mut(self, file);
    }

    fn visit_item_mod_mut(&mut self, module: &mut syn::ItemMod) {
        if let Some((_, items)) = &mut module.content {
            self.keep_items(items);
        }
        visit_mut::visit_item_mod_mut(self, module);
    }

    fn visit_item_impl_mut(&mut self, block: &mut syn::ItemImpl) {
        block.items.retain(|item| !tests_only(impl_item_attrs(item)));
        visit_mut::visit_item_impl_mut(self, block);
    }

    fn visit_item_trait_mut(&mut self, block: &mut syn::ItemTrait) {
        block.items.retain(|item| !tests_only(trait_item_attrs(item)));
        visit_mut::visit_item_trait_mut(self, block);
    }

    fn visit_item_foreign_mod_mut(&mut self, block: &mut syn::ItemForeignMod) {
        block.items.retain(|item| !tests_only(foreign_item_attrs(item)));
        visit_mut::visit_item_foreign_mod_mut(self, block);
    }

    fn visit_item_enum_mut(&mut self, item: &mut syn::ItemEnum) {
        item.variants = kept(&item.variants, |variant| &variant.attrs);
        visit_mut::visit_item_enum_mut(self, item);
    }

    fn visit_fields_named_mut(&mut self, fields: &mut syn::FieldsNamed) {
        fields.named = kept(&fields.named, |field| &field.attrs);
        visit_mut::visit_fields_named_mut(self, fields);
    }

    fn visit_fields_unnamed_mut(&mut self, fields: &mut syn::FieldsUnnamed) {
        fields.unnamed = kept(&fields.unnamed, |field| &field.attrs);
        visit_mut::visit_fields_unnamed_mut(self, fields);
    }

    fn visit_expr_struct_mut(&mut self, expr: &mut syn::ExprStruct) {
        expr.fields = kept(&expr.fields, |field| &field.attrs);
        visit_mut::visit_expr_struct_mut(self, expr);
    }

    fn visit_expr_match_mut(&mut self, expr: &mut syn::ExprMatch) {
        expr.arms.retain(|arm| !tests_only(&arm.attrs));
        visit_mut::visit_expr_match_mut(self, expr);
    }

    fn visit_expr_array_mut(&mut self, expr: &mut syn::ExprArray) {
        expr.elems = kept(&expr.elems, expr_attrs);
        visit_mut::visit_expr_array_mut(self, expr);
    }

    fn visit_expr_tuple_mut(&mut self, expr: &mut syn::ExprTuple) {
        expr.elems = kept(&expr.elems, expr_attrs);
        visit_mut::visit_expr_tuple_mut(self, expr);
    }

    fn visit_expr_call_mut(&mut self, expr: &mut syn::ExprCall) {
        expr.args = kept(&expr.args, expr_attrs);
        visit_mut::visit_expr_call_mut(self, expr);
    }

    fn visit_expr_method_call_mut(&mut self, expr: &mut syn::ExprMethodCall) {
        expr.args = kept(&expr.args, expr_attrs);
        visit_mut::visit_expr_method_call_mut(self, expr);
    }

    fn visit_block_mut(&mut self, block: &mut syn::Block) {
        block.stmts.retain(|stmt| !tests_only(stmt_attrs(stmt)));
        visit_mut::visit_block_mut(self, block);
    }
}

// ------------------------------------------------------------ the runs

/// The names of a run's entry and of what publishes, renames or withdraws a
/// run, and the host's fields that hold the runs' journal and drafts.
const RUN_NAMES: [&str; 5] = ["publish_run", "withdraw_run", "rename_run", "running_run", "RunEntry"];
const RUN_FIELDS: [&str; 2] = ["journal", "drafts"];

/// How `code` reaches the runs: each of [`RUN_NAMES`] it names, each of
/// [`RUN_FIELDS`] it reads off a value (`.journal`), and `RunHost { … }`
/// where it takes a host apart (`let RunHost { journal, .. } = host;`), which
/// reads its fields without a `.`; `impl … RunHost {` and `for RunHost {` do
/// not.
fn runs_reached(code: &[Tok]) -> Vec<String> {
    let named = RUN_NAMES.into_iter().filter(|name| names(code, name)).map(str::to_string);
    let read = RUN_FIELDS.into_iter().filter(|name| reads(code, name)).map(|name| format!(".{name}"));
    let apart = find(code, &pattern("RunHost {"))
        .any(|at| !matches!(at.checked_sub(1).map(|before| &code[before]), Some(Tok::Ident(word)) if word == "impl" || word == "for"))
        .then(|| "RunHost { … }".to_string());
    named.chain(read).chain(apart).collect()
}

// ------------------------------------------------------------ the scans

/// The permit that takes no gate starts nothing in the server: only the
/// test probe that hands it out (`droppen_provider::testing`) names it, and
/// the shim's own end-to-end test, outside `src/`, uses it. Read in the
/// whole text, tests and comments too.
#[test]
fn nothing_in_the_server_starts_a_cli_without_the_gate() {
    const NAME: &str = "without_a_session";
    const PROBE: &str = "crates/droppen-provider/src/testing.rs";
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    // The name as a whole identifier, so a call spelt `without_a_session ()`
    // or passed as a function is found too, and a test named
    // `…_without_a_session_…` is not.
    let says = |text: &str| text.match_indices(NAME).any(|(at, _)| !text[..at].ends_with(ident) && !text[at + NAME.len()..].starts_with(ident));
    let found: Vec<String> = server_sources()
        .into_iter()
        .filter(|source| source.path != PROBE && says(&source.text))
        .map(|source| source.path)
        .collect();
    assert!(found.is_empty(), "a CLI started without the spawn gate: {found:?}");
}

/// Whether `code` makes a spawn gate: `SpawnGate::new`, however spaced and
/// whatever it is reached through.
fn makes_a_gate(code: &[Tok]) -> bool {
    find(code, &pattern("SpawnGate :: new")).next().is_some()
}

/// Only the provider registry makes a spawn gate, so the gate a provider
/// is lent is the only one there is: a provider that made its own and took
/// that instead would start a CLI beside one the registry's gate keeps
/// out. A file's tests make theirs as they like.
#[test]
fn only_the_provider_registry_makes_a_spawn_gate() {
    let sources = server_sources();
    assert!(
        sources.iter().any(|source| registry(&source.path) && makes_a_gate(&source.code)),
        "the provider registry makes the gate"
    );
    let found: Vec<&str> = sources
        .iter()
        .filter(|source| !registry(&source.path) && makes_a_gate(&source.code))
        .map(|source| source.path.as_str())
        .collect();
    assert!(found.is_empty(), "a spawn gate made outside the provider registry: {found:?}");
}

/// The names that take the spawn gate, mint a permit, build a lease, or
/// borrow the gate from the registry.
const GATE_NAMES: [&str; 4] = ["SpawnGate", "SpawnPermit", "SessionLease", "provider_with_gate"];

/// The spawn gate reaches the providers alone. Outside them (the server's
/// `provider` and the `droppen-provider*` crates), no production code of the
/// server or of a crate that depends on the base names the gate, a permit or
/// a lease, so none can take the gate, mint a permit or build a lease.
/// Inside them, the ACP provider's agent routes, which start and stop its
/// CLI outside a run, are lent the gate through `provider_with_gate`, which
/// nothing else names but the registry that defines it: they reach it
/// through `AcpAgentState::gate`, and every call of theirs that takes the
/// gate is handed it from there. A file's tests aside.
#[test]
fn only_the_providers_reach_the_spawn_gate() {
    const LEND: &str = "provider_with_gate";
    const ROUTES: &str = "crates/droppen-provider-acp/src/agents.rs";
    let sources = server_sources();
    let mut found = Vec::new();
    for source in &sources {
        if !in_provider(&source.path) && source.sees_base {
            found.extend(GATE_NAMES.into_iter().filter(|name| names(&source.code, name)).map(|name| format!("{}: {name}", source.path)));
        } else if !registry(&source.path) && source.path != ROUTES && names(&source.code, LEND) {
            found.push(format!("{}: {LEND}", source.path));
        }
    }
    assert!(found.is_empty(), "the spawn gate reached outside the providers: {found:?}");

    let routes = sources.iter().find(|source| source.path == ROUTES).expect("the ACP agent routes");
    let lend = pattern(LEND);
    let lends: Vec<usize> = find(&routes.code, &lend).collect();
    assert_eq!(lends.len(), 1, "the ACP agent routes are lent the gate in one place");
    assert!(
        routes.code[lends[0] + 1..].starts_with(&pattern(":: < crate :: Acp > ( )")),
        "the ACP agent routes are lent the gate as the ACP provider"
    );
    // The calls the routes make that take the gate: the ACP runs' methods
    // whose first argument is one.
    let signature = pattern("( & self , gate : & SpawnGate");
    let gated: Vec<String> = sources
        .iter()
        .filter(|source| source.path.starts_with("crates/droppen-provider-acp/src/"))
        .flat_map(|source| {
            find(&source.code, &signature)
                .filter_map(|at| match source.code.get(at.checked_sub(2)?..at) {
                    Some([Tok::Ident(word), Tok::Ident(name)]) if word == "fn" => Some(name.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let handed = pattern("( state . gate ( ) ?");
    let mut calls = 0;
    for name in &gated {
        let call = pattern(&format!(". {name}"));
        for at in find(&routes.code, &call) {
            calls += 1;
            assert!(routes.code[at + call.len()..].starts_with(&handed), "an ACP agent route calls {name} with a gate of its own");
        }
    }
    assert!(calls > 0, "the ACP agent routes change agents under the gate: {gated:?}");
}

/// A run's entry and its row in the journal are its driver's, and so is
/// the draft of its turn: outside the providers, no production code
/// publishes, renames or withdraws a run, holds one's entry, or reaches the
/// host's run journal or drafts, so none can flip a run's state or forge a
/// journal row. The feeds watch a draft through the provider registry. The
/// server and every crate that depends on the base are read, so code that
/// moves out of the server is still held to this.
#[test]
fn only_the_providers_keep_the_runs() {
    let sources = server_sources();
    let (inside, outside): (Vec<&Source>, Vec<&Source>) = sources.iter().partition(|source| in_provider(&source.path));
    for name in RUN_NAMES {
        assert!(inside.iter().any(|source| names(&source.code, name)), "the providers use {name}");
    }
    for name in RUN_FIELDS {
        assert!(inside.iter().any(|source| reads(&source.code, name)), "the providers read .{name}");
    }
    let found: Vec<String> = outside
        .iter()
        .filter(|source| source.sees_base)
        .flat_map(|source| runs_reached(&source.code).into_iter().map(|how| format!("{}: {how}", source.path)))
        .collect();
    assert!(found.is_empty(), "a run reached outside the providers: {found:?}");
}

/// What the scans find in `text`, read as a file's code: how it reaches the
/// runs, the gate's names it says, and whether it makes a gate.
fn found_in(text: &str) -> Vec<String> {
    let (code, _) = code(text).expect("parses");
    let gate = GATE_NAMES.into_iter().filter(|name| names(&code, name)).map(str::to_string);
    let makes = makes_a_gate(&code).then(|| "SpawnGate::new".to_string());
    runs_reached(&code).into_iter().chain(gate).chain(makes).collect()
}

/// Production code after or around something built for tests alone is read,
/// whatever the shape of the test-only part; the test-only part is not.
/// Each case is a reach the scans must see.
#[test]
fn the_scans_read_past_a_file_s_tests() {
    let cases: &[(&str, &str, &str)] = &[
        // A field value whose test-only neighbour has a `<` in it.
        ("a comparison in a test-only field value", "fn f(h: &RunHost) -> S { S { #[cfg(test)] probe: a < b, journal: h.journal.clone() } }", ".journal"),
        // A block-bodied test-only arm, then an arm of each shape.
        ("an arm after a block: a tuple", "fn f(h: &H, x: X) -> usize { match x { #[cfg(test)] Probe => { gone(); } (a, b) => h.journal.len(), } }", ".journal"),
        ("an arm after a block: a reference", "fn f(r: &R, x: &X) { match x { #[cfg(test)] Probe => { gone(); } &y => r.host.withdraw_run(Source::Codex, y), } }", "withdraw_run"),
        ("an arm after a block: a slice", "fn f(h: &H, x: &[u8]) -> usize { match x { #[cfg(test)] [] => { gone(); } [a, ..] => h.drafts.len(), } }", ".drafts"),
        ("an arm after a block: a literal", "fn f(h: &H, x: u8) -> usize { match x { #[cfg(test)] 0 => { gone(); } 1 => h.journal.len(), _ => 0 } }", ".journal"),
        ("an arm after a block: a negative literal", "fn f(h: &H, x: i8) -> usize { match x { #[cfg(test)] 0 => { gone(); } -1 => h.journal.len(), _ => 0 } }", ".journal"),
        ("an arm after a block: a leading `|`", "fn f(r: &R, x: X) { match x { #[cfg(test)] X::A => { gone(); } | X::B | X::C => r.host.publish_run(), } }", "publish_run"),
        ("an arm whose pattern is a macro", "fn f(r: &R, x: X) { match x { #[cfg(test)] probe_pat!() => gone(), _ => r.host.withdraw_run(Source::Codex, \"id\"), } }", "withdraw_run"),
        ("a statement after a test-only path macro", "async fn f(h: &H) { #[cfg(test)] tokio::select! { _ = a => {} } let n = *h.drafts.count(); }", ".drafts"),
        // The cases the scans read before.
        ("a shorthand field value", "fn f(h: &RunHost) -> S { S { #[cfg(test)] steps, journal: h.journal.clone() } }", ".journal"),
        ("an arm with a guard", "fn f(r: Run) { match r { #[cfg(test)] other if p => drop(other), _ => r.host.withdraw_run(Source::Codex, \"id\"), } }", "withdraw_run"),
        ("a host taken apart", "fn f(h: &RunHost) { let RunHost { drafts, .. } = h; }", "RunHost { … }"),
        ("a field with a generic type", "struct S { #[cfg(test)] steps: Mutex<HashMap<String, Vec<Step>>>, entry: RunEntry }", "RunEntry"),
        ("a statement", "fn f(h: &H) { #[cfg(test)] self.host.step(\"a\", Source::Acp, None); h.host.rename_run(a, b); }", "rename_run"),
        ("a block", "fn f(h: &H) { #[cfg(test)] { gone(); } h.host.running_run(Source::Claude, \"id\"); }", "running_run"),
        ("a thread-local", "#[cfg(test)] thread_local! { static GONE: u8 = 0; } fn f(p: SpawnPermit) {}", "SpawnPermit"),
        ("an impl block", "#[cfg(test)] impl Probe { fn gone() {} } fn f(l: SessionLease) {}", "SessionLease"),
        ("a test module, then code", "#[cfg(test)] mod tests { fn gone() {} } fn f() { let g = SpawnGate::new(); }", "SpawnGate::new"),
        ("a use", "#[cfg(test)] use x::y; fn f(r: &R) { r.provider_with_gate::<P>(); }", "provider_with_gate"),
    ];
    for (what, text, reach) in cases {
        assert!(found_in(text).contains(&reach.to_string()), "{what}: {reach} is read in\n{text}\nfound {:?}", found_in(text));
    }
    // What is built for tests alone, a comment, a string or a doc is not
    // read; `cfg(any(test, …))` is built outside tests too, and is.
    let quiet = [
        "#[cfg(test)] fn f(h: &H) { h.journal.len(); }",
        "#[cfg(all(test, unix))] fn f(h: &H) { h.journal.len(); }",
        "fn f(h: &H) { #[cfg(test)] h.journal.len(); }",
        "// h.journal and SpawnGate::new\nfn f() { let s = \"h.journal\"; }",
        "/// `h.journal`, `RunHost { .. }`\nfn f() {}",
        "impl Trait for RunHost {} impl RunHost { fn f(&self) {} }",
        "fn f(s: &S) { s.drafts(); }",
    ];
    for text in quiet {
        assert!(found_in(text).is_empty(), "nothing is read in\n{text}\nfound {:?}", found_in(text));
    }
    assert_eq!(found_in("#[cfg(any(test, feature = \"x\"))] fn f(h: &H) { h.journal.len(); }"), [".journal"]);
    let (_, modules) = code("#[cfg(test)] mod elsewhere; #[cfg(test)] mod inline {} mod kept;").expect("parses");
    assert_eq!(modules, ["elsewhere"]);
}

/// A crate depends on the base however its manifest spells it, per target
/// too; a dev- or build-dependency is no dependency of its code.
#[test]
fn a_manifest_says_whether_its_crate_sees_the_base() {
    let on = [
        "[dependencies]\ndroppen-provider = { path = \"../droppen-provider\" }\n",
        "[dependencies.droppen-provider]\npath = \"../droppen-provider\"\n",
        "[dependencies]\nbase = { package = \"droppen-provider\", path = \"../droppen-provider\" }\n",
        "[target.'cfg(unix)'.dependencies]\ndroppen-provider = { path = \"../droppen-provider\" }\n",
    ];
    for manifest in on {
        assert!(sees_the_base(manifest), "sees the base:\n{manifest}");
    }
    let off = [
        "[dev-dependencies]\ndroppen-provider = { path = \"../droppen-provider\" }\n",
        "[build-dependencies]\ndroppen-provider = { path = \"../droppen-provider\" }\n",
        "[dependencies]\ndroppen-sessions = { path = \"../droppen-sessions\" }\n",
    ];
    for manifest in off {
        assert!(!sees_the_base(manifest), "does not see the base:\n{manifest}");
    }
}
