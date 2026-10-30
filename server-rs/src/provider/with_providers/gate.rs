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
/// from `server-rs` (`src/…`, `crates/<crate>/src/…`), what it says, its code
/// built outside tests ([`Code::of`]), and whether that code can reach the
/// provider base at all ([`ManifestExt::sees_the_base`]). A file that is a
/// test module's has no code.
struct Source {
    path: String,
    text: String,
    code: Code,
    sees_base: bool,
}

impl Source {
    /// Every source file of the server and of every crate of its workspace,
    /// but the gate's own and this one. Wherever the feed and the drive
    /// live, the scans read them as code that can reach the base.
    fn all() -> Vec<Self> {
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
        let read: Vec<(PathBuf, String, (Code, Vec<String>))> = paths
            .into_iter()
            .map(|path| {
                let text = std::fs::read_to_string(&path).expect("source");
                let code = Code::of(&text).unwrap_or_else(|error| {
                    let at = error.span().start();
                    panic!("{}:{}:{}: {error}", path.display(), at.line, at.column + 1)
                });
                (path, text, code)
            })
            .collect();
        // The modules declared for tests alone, each a file and what is
        // under it, from the directory its declaring file's modules are in.
        let mut tests = Vec::new();
        for (path, _, (_, modules)) in &read {
            let dir = match path.file_name().and_then(|name| name.to_str()) {
                Some("mod.rs" | "lib.rs" | "main.rs") => path.parent().expect("a parent").to_path_buf(),
                _ => path.with_extension(""),
            };
            tests.extend(modules.iter().map(|module| dir.join(module)));
        }
        let sources: Vec<Self> = read
            .into_iter()
            .map(|(path, text, (code, _))| {
                let test = tests.iter().any(|module| path.starts_with(module) || path == module.with_extension("rs"));
                let path = path.strip_prefix(manifest).expect("under server-rs").display().to_string();
                let sees_base = path.strip_prefix("crates/").and_then(|rest| rest.split_once('/')).is_none_or(|(name, _)| {
                    let manifest = std::fs::read_to_string(manifest.join("crates").join(name).join("Cargo.toml")).expect("its manifest");
                    manifest.parse::<toml::Table>().expect("a manifest").sees_the_base()
                });
                Self {
                    path,
                    text,
                    code: if test { Code::default() } else { code },
                    sees_base,
                }
            })
            .filter(|source| !source.is_gate_or_scan())
            .collect();
        for module in ["feed/mod.rs", "droppen-drive/src/lib.rs"] {
            assert!(
                sources.iter().any(|source| source.path.ends_with(module) && !source.code.0.is_empty() && source.sees_base),
                "the scans read {module} as code that can reach the base"
            );
        }
        sources
    }

    /// Whether this is the spawn gate's own source or these scans'.
    fn is_gate_or_scan(&self) -> bool {
        ["crates/droppen-provider/src/gate.rs", "src/provider/with_providers/gate.rs"].contains(&self.path.as_str())
    }

    /// Whether this is the provider registry's source.
    fn is_registry(&self) -> bool {
        self.path == "crates/droppen-provider/src/registry.rs"
    }

    /// Whether this is the providers' own: the base's or an
    /// implementation's, each a `droppen-provider*` crate.
    fn in_provider(&self) -> bool {
        self.path.starts_with("crates/droppen-provider")
    }
}

/// What the scans read of a crate's manifest.
trait ManifestExt {
    /// Whether the crate can name the provider base's items: whether its
    /// `[dependencies]`, per target too but not its dev- or
    /// build-dependencies, have `droppen-provider`, however spelt (a table of
    /// its own, or renamed with `package`). One that cannot has fields and
    /// names of its own that only sound like the base's (the agent's
    /// `drafts` are its stream tasks).
    fn sees_the_base(&self) -> bool;
}

impl ManifestExt for toml::Table {
    fn sees_the_base(&self) -> bool {
        let has = |deps: Option<&toml::Value>| {
            deps.and_then(toml::Value::as_table).is_some_and(|deps| {
                deps.iter()
                    .any(|(name, spec)| name == "droppen-provider" || spec.get("package").and_then(toml::Value::as_str) == Some("droppen-provider"))
            })
        };
        let targets = self.get("target").and_then(toml::Value::as_table);
        has(self.get("dependencies")) || targets.is_some_and(|targets| targets.values().any(|target| has(target.get("dependencies"))))
    }
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

/// The tokens of a file's code built outside tests, or of a pattern to find
/// in one.
#[derive(Clone, Debug, Default, PartialEq)]
struct Code(Vec<Tok>);

impl Code {
    /// `text`'s code built outside tests, and the modules it declares for
    /// its tests alone (`#[cfg(test)] mod name;`), whose files are theirs:
    /// each as a path from the directory its declaring file's modules are in
    /// (`probe/plant` for one inside an inline `mod probe`). Every item, impl
    /// or trait item, field, variant, field value, arm, statement and list
    /// element under a test-only `cfg` ([`AttrsExt::tests_only`]) is
    /// dropped; a macro's body is kept as it is written, `cfg(test)` and all.
    fn of(text: &str) -> syn::Result<(Self, Vec<String>)> {
        let mut file = syn::parse_file(text)?;
        let mut out = TestsOut::default();
        out.visit_file_mut(&mut file);
        let mut code = Self::default();
        code.push(file.into_token_stream());
        Ok((code, out.modules))
    }

    /// A pattern: words separated by spaces, each an identifier, a bracket,
    /// or punctuation one character a token (`::` is two).
    fn pattern(text: &str) -> Self {
        Self(
            text.split_whitespace()
                .flat_map(|word| match word {
                    "(" | "[" | "{" => vec![Tok::Open(word.chars().next().expect("a bracket"))],
                    ")" | "]" | "}" => vec![Tok::Close(word.chars().next().expect("a bracket"))],
                    _ if word.chars().all(|c| c.is_alphanumeric() || c == '_') => vec![Tok::Ident(word.to_string())],
                    _ => word.chars().map(Tok::Punct).collect(),
                })
                .collect(),
        )
    }

    /// `stream`'s tokens added, groups opened and closed around theirs.
    fn push(&mut self, stream: TokenStream) {
        for tree in stream {
            match tree {
                TokenTree::Ident(ident) => self.0.push(Tok::Ident(ident.to_string().trim_start_matches("r#").to_string())),
                TokenTree::Punct(punct) => self.0.push(Tok::Punct(punct.as_char())),
                TokenTree::Literal(_) => self.0.push(Tok::Literal),
                TokenTree::Group(group) => {
                    let brackets = match group.delimiter() {
                        Delimiter::Parenthesis => Some(('(', ')')),
                        Delimiter::Bracket => Some(('[', ']')),
                        Delimiter::Brace => Some(('{', '}')),
                        Delimiter::None => None,
                    };
                    self.0.extend(brackets.map(|(open, _)| Tok::Open(open)));
                    self.push(group.stream());
                    self.0.extend(brackets.map(|(_, close)| Tok::Close(close)));
                }
            }
        }
    }

    /// Where `pattern` starts.
    fn find<'a>(&'a self, pattern: &'a Self) -> impl Iterator<Item = usize> + 'a {
        (0..self.0.len()).filter(move |&at| self.0[at..].starts_with(&pattern.0))
    }

    /// Whether `pattern` is right at `at`.
    fn at(&self, at: usize, pattern: &Self) -> bool {
        self.0.get(at..).is_some_and(|rest| rest.starts_with(&pattern.0))
    }

    /// Whether it names the identifier `name`.
    fn names(&self, name: &str) -> bool {
        self.0.contains(&Tok::Ident(name.to_string()))
    }

    /// Whether it reads the field `name` off a value: `.name`, not a call
    /// of a method `name`.
    fn reads(&self, name: &str) -> bool {
        let field = Self::pattern(&format!(". {name}"));
        // Bound before it is returned: as the tail expression, the iterator
        // would outlive `field` (edition 2021's temporaries, E0597).
        let read = self.find(&field).any(|at| self.0.get(at + field.0.len()) != Some(&Tok::Open('(')));
        read
    }

    /// How it reaches the runs: each of [`RUN_NAMES`] it names, each of
    /// [`RUN_FIELDS`] it reads off a value (`.journal`), and `RunHost { … }`
    /// where it takes a host apart (`let RunHost { journal, .. } = host;`),
    /// which reads its fields without a `.`; `impl … RunHost {` and
    /// `for RunHost {` do not.
    fn runs_reached(&self) -> Vec<String> {
        let named = RUN_NAMES.into_iter().filter(|name| self.names(name)).map(str::to_string);
        let read = RUN_FIELDS.into_iter().filter(|name| self.reads(name)).map(|name| format!(".{name}"));
        let host = Self::pattern("RunHost {");
        let apart = self
            .find(&host)
            .any(|at| !matches!(at.checked_sub(1).map(|before| &self.0[before]), Some(Tok::Ident(word)) if word == "impl" || word == "for"))
            .then(|| "RunHost { … }".to_string());
        named.chain(read).chain(apart).collect()
    }

    /// Whether it makes a spawn gate: `SpawnGate::new`, however spaced and
    /// whatever it is reached through.
    fn makes_a_gate(&self) -> bool {
        let new = Self::pattern("SpawnGate :: new");
        // Bound before it is returned, as in `reads`.
        let makes = self.find(&new).next().is_some();
        makes
    }

    /// What the scans find in it: how it reaches the runs, the gate's names
    /// it says, and whether it makes a gate.
    fn found(&self) -> Vec<String> {
        let gate = GATE_NAMES.into_iter().filter(|name| self.names(name)).map(str::to_string);
        let makes = self.makes_a_gate().then(|| "SpawnGate::new".to_string());
        self.runs_reached().into_iter().chain(gate).chain(makes).collect()
    }
}

/// What a `cfg` and a `path` say of the node they are on.
trait AttrsExt {
    /// Whether they hold a `cfg` built in tests alone: `cfg(test)`, or
    /// `cfg(all(…, test, …))`, which needs it. `cfg(any(test, feature = …))`
    /// is built outside tests too.
    fn tests_only(&self) -> bool;

    /// Whether they give a module's file a `#[path]` of its own.
    fn has_path(&self) -> bool;
}

impl AttrsExt for [Attribute] {
    fn tests_only(&self) -> bool {
        fn needs_test(meta: &Meta) -> bool {
            let nested = |list: &syn::MetaList| list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated).ok();
            match meta {
                Meta::Path(path) => path.is_ident("test"),
                Meta::List(list) if list.path.is_ident("all") => nested(list).is_some_and(|all| all.iter().any(needs_test)),
                Meta::List(list) if list.path.is_ident("any") => nested(list).is_some_and(|any| !any.is_empty() && any.iter().all(needs_test)),
                _ => false,
            }
        }
        self.iter().any(|attr| attr.path().is_ident("cfg") && attr.parse_args::<Meta>().is_ok_and(|meta| needs_test(&meta)))
    }

    fn has_path(&self) -> bool {
        self.iter().any(|attr| attr.path().is_ident("path"))
    }
}

/// A node a `cfg` can drop: its attributes.
trait Attrs {
    fn attrs(&self) -> &[Attribute];
}

/// [`Attrs`] for each variant of an enum of nodes.
macro_rules! attrs_by_variant {
    ($type:ident; $($variant:ident),+) => {
        impl Attrs for $type {
            fn attrs(&self) -> &[Attribute] {
                match self {
                    $($type::$variant(node) => &node.attrs,)+
                    _ => &[],
                }
            }
        }
    };
}
attrs_by_variant!(Item; Const, Enum, ExternCrate, Fn, ForeignMod, Impl, Macro, Mod, Static, Struct, Trait, TraitAlias, Type, Union, Use);
attrs_by_variant!(ImplItem; Const, Fn, Type, Macro);
attrs_by_variant!(TraitItem; Const, Fn, Type, Macro);
attrs_by_variant!(ForeignItem; Fn, Static, Type, Macro);
attrs_by_variant!(
    Expr; Array, Assign, Async, Await, Binary, Block, Break, Call, Cast, Closure, Const, Continue, Field, ForLoop, Group, If, Index, Infer,
    Let, Lit, Loop, Macro, Match, MethodCall, Paren, Path, Range, RawAddr, Reference, Repeat, Return, Struct, Try, TryBlock, Tuple, Unary,
    Unsafe, While, Yield
);

/// [`Attrs`] for the nodes that hold theirs in a field.
macro_rules! attrs_field {
    ($($type:ty),+) => {
        $(impl Attrs for $type {
            fn attrs(&self) -> &[Attribute] {
                &self.attrs
            }
        })+
    };
}
attrs_field!(syn::Field, syn::Variant, syn::FieldValue, syn::Arm);

impl Attrs for Stmt {
    fn attrs(&self) -> &[Attribute] {
        match self {
            Stmt::Local(local) => &local.attrs,
            Stmt::Item(item) => item.attrs(),
            Stmt::Expr(expr, _) => expr.attrs(),
            Stmt::Macro(mac) => &mac.attrs,
        }
    }
}

/// A list without the elements built for tests alone.
trait WithoutTests {
    fn without_tests(&self) -> Self;
}

impl<T: Attrs + Clone, P: Clone + Default> WithoutTests for Punctuated<T, P> {
    fn without_tests(&self) -> Self {
        self.iter().filter(|node| !node.attrs().tests_only()).cloned().collect()
    }
}

/// What drops the nodes built for tests alone, noting the test modules
/// that are files of their own.
#[derive(Default)]
struct TestsOut {
    /// The inline modules around the node being visited, outermost first:
    /// their names, and whether each has a `#[path]`.
    within: Vec<(String, bool)>,
    modules: Vec<String>,
}

impl TestsOut {
    /// `items` without those built for tests alone. A test module that is a
    /// file of its own is noted by its path through the inline modules
    /// around it. Under a `#[path]`, its own or one around it, its file is
    /// not where that path says: it is not noted, and its file stays read.
    fn keep_items(&mut self, items: &mut Vec<Item>) {
        let pathed = self.within.iter().any(|(_, path)| *path);
        let around: Vec<&str> = self.within.iter().map(|(name, _)| name.as_str()).collect();
        let modules = &mut self.modules;
        items.retain(|item| {
            let test = item.attrs().tests_only();
            if let (true, Item::Mod(module)) = (test, item) {
                if module.content.is_none() && !pathed && !module.attrs.has_path() {
                    let name = module.ident.to_string();
                    modules.push(around.iter().copied().chain([name.as_str()]).collect::<Vec<_>>().join("/"));
                }
            }
            !test
        });
    }
}

impl VisitMut for TestsOut {
    fn visit_file_mut(&mut self, file: &mut syn::File) {
        if file.attrs.tests_only() {
            file.items.clear();
        }
        self.keep_items(&mut file.items);
        visit_mut::visit_file_mut(self, file);
    }

    fn visit_item_mod_mut(&mut self, module: &mut syn::ItemMod) {
        self.within.push((module.ident.to_string(), module.attrs.has_path()));
        if let Some((_, items)) = &mut module.content {
            self.keep_items(items);
        }
        visit_mut::visit_item_mod_mut(self, module);
        self.within.pop();
    }

    fn visit_item_impl_mut(&mut self, block: &mut syn::ItemImpl) {
        block.items.retain(|item| !item.attrs().tests_only());
        visit_mut::visit_item_impl_mut(self, block);
    }

    fn visit_item_trait_mut(&mut self, block: &mut syn::ItemTrait) {
        block.items.retain(|item| !item.attrs().tests_only());
        visit_mut::visit_item_trait_mut(self, block);
    }

    fn visit_item_foreign_mod_mut(&mut self, block: &mut syn::ItemForeignMod) {
        block.items.retain(|item| !item.attrs().tests_only());
        visit_mut::visit_item_foreign_mod_mut(self, block);
    }

    fn visit_item_enum_mut(&mut self, item: &mut syn::ItemEnum) {
        item.variants = item.variants.without_tests();
        visit_mut::visit_item_enum_mut(self, item);
    }

    fn visit_fields_named_mut(&mut self, fields: &mut syn::FieldsNamed) {
        fields.named = fields.named.without_tests();
        visit_mut::visit_fields_named_mut(self, fields);
    }

    fn visit_fields_unnamed_mut(&mut self, fields: &mut syn::FieldsUnnamed) {
        fields.unnamed = fields.unnamed.without_tests();
        visit_mut::visit_fields_unnamed_mut(self, fields);
    }

    fn visit_expr_struct_mut(&mut self, expr: &mut syn::ExprStruct) {
        expr.fields = expr.fields.without_tests();
        visit_mut::visit_expr_struct_mut(self, expr);
    }

    fn visit_expr_match_mut(&mut self, expr: &mut syn::ExprMatch) {
        expr.arms.retain(|arm| !arm.attrs().tests_only());
        visit_mut::visit_expr_match_mut(self, expr);
    }

    fn visit_expr_array_mut(&mut self, expr: &mut syn::ExprArray) {
        expr.elems = expr.elems.without_tests();
        visit_mut::visit_expr_array_mut(self, expr);
    }

    fn visit_expr_tuple_mut(&mut self, expr: &mut syn::ExprTuple) {
        expr.elems = expr.elems.without_tests();
        visit_mut::visit_expr_tuple_mut(self, expr);
    }

    fn visit_expr_call_mut(&mut self, expr: &mut syn::ExprCall) {
        expr.args = expr.args.without_tests();
        visit_mut::visit_expr_call_mut(self, expr);
    }

    fn visit_expr_method_call_mut(&mut self, expr: &mut syn::ExprMethodCall) {
        expr.args = expr.args.without_tests();
        visit_mut::visit_expr_method_call_mut(self, expr);
    }

    fn visit_block_mut(&mut self, block: &mut syn::Block) {
        block.stmts.retain(|stmt| !stmt.attrs().tests_only());
        visit_mut::visit_block_mut(self, block);
    }
}

// ------------------------------------------------------------ the scans

/// The names of a run's entry, of what publishes, renames or withdraws a
/// run, and of where the runs' journal lies (`runs_file`, which only the base
/// opens), and the host's fields that hold the runs' journal and drafts.
const RUN_NAMES: [&str; 6] = ["publish_run", "withdraw_run", "rename_run", "running_run", "RunEntry", "runs_file"];
const RUN_FIELDS: [&str; 2] = ["journal", "drafts"];

/// The names that take the spawn gate, mint a permit, build a lease, or
/// borrow the gate from the registry.
const GATE_NAMES: [&str; 4] = ["SpawnGate", "SpawnPermit", "SessionLease", "provider_with_gate"];

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
    let found: Vec<String> = Source::all()
        .into_iter()
        .filter(|source| source.path != PROBE && says(&source.text))
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
    let sources = Source::all();
    assert!(
        sources.iter().any(|source| source.is_registry() && source.code.makes_a_gate()),
        "the provider registry makes the gate"
    );
    let found: Vec<&str> = sources
        .iter()
        .filter(|source| !source.is_registry() && source.code.makes_a_gate())
        .map(|source| source.path.as_str())
        .collect();
    assert!(found.is_empty(), "a spawn gate made outside the provider registry: {found:?}");
}

/// The spawn gate reaches the providers alone. Outside them (the
/// `droppen-provider*` crates), no production code of the
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
    let sources = Source::all();
    let mut found = Vec::new();
    for source in &sources {
        if !source.in_provider() && source.sees_base {
            found.extend(GATE_NAMES.into_iter().filter(|name| source.code.names(name)).map(|name| format!("{}: {name}", source.path)));
        } else if !source.is_registry() && source.path != ROUTES && source.code.names(LEND) {
            found.push(format!("{}: {LEND}", source.path));
        }
    }
    assert!(found.is_empty(), "the spawn gate reached outside the providers: {found:?}");

    let routes = &sources.iter().find(|source| source.path == ROUTES).expect("the ACP agent routes").code;
    let lend = Code::pattern(LEND);
    let lends: Vec<usize> = routes.find(&lend).collect();
    assert_eq!(lends.len(), 1, "the ACP agent routes are lent the gate in one place");
    assert!(
        routes.at(lends[0] + 1, &Code::pattern(":: < crate :: Acp > ( )")),
        "the ACP agent routes are lent the gate as the ACP provider"
    );
    // The calls the routes make that take the gate: the ACP runs' methods
    // whose first argument is one.
    let signature = Code::pattern("( & self , gate : & SpawnGate");
    let gated: Vec<String> = sources
        .iter()
        .filter(|source| source.path.starts_with("crates/droppen-provider-acp/src/"))
        .flat_map(|source| {
            let tokens = &source.code.0;
            source
                .code
                .find(&signature)
                .filter_map(|at| match tokens.get(at.checked_sub(2)?..at) {
                    Some([Tok::Ident(word), Tok::Ident(name)]) if word == "fn" => Some(name.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let handed = Code::pattern("( state . gate ( ) ?");
    let mut calls = 0;
    for name in &gated {
        let call = Code::pattern(&format!(". {name}"));
        for at in routes.find(&call) {
            calls += 1;
            assert!(routes.at(at + call.0.len(), &handed), "an ACP agent route calls {name} with a gate of its own");
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
    let sources = Source::all();
    let (inside, outside): (Vec<&Source>, Vec<&Source>) = sources.iter().partition(|source| source.in_provider());
    for name in RUN_NAMES {
        assert!(inside.iter().any(|source| source.code.names(name)), "the providers use {name}");
    }
    for name in RUN_FIELDS {
        assert!(inside.iter().any(|source| source.code.reads(name)), "the providers read .{name}");
    }
    let found: Vec<String> = outside
        .iter()
        .filter(|source| source.sees_base)
        .flat_map(|source| source.code.runs_reached().into_iter().map(|how| format!("{}: {how}", source.path)))
        .collect();
    assert!(found.is_empty(), "a run reached outside the providers: {found:?}");
}

/// Production code after or around something built for tests alone is read,
/// whatever the shape of the test-only part; the test-only part is not.
/// Each case is a reach the scans must see. The first eighteen are a
/// review's plants, as written; seven of them hid their reach from the scans
/// the server had before these. The review's nineteenth, E1, is with the
/// code that must not be read.
#[test]
fn the_scans_read_past_a_file_s_tests() {
    let found_in = |text: &str| Code::of(text).expect("parses").0.found();
    let cases: &[(&str, &str, &str)] = &[
        ("A1", "fn plant(h: &RunHost) -> S {\n    S {\n        #[cfg(test)]\n        steps,\n        journal: h.journal.clone(),\n    }\n}\n", ".journal"),
        ("A2", "fn plant(h: &RunHost, a: u8, b: u8) -> S {\n    S {\n        #[cfg(test)]\n        probe: a < b,\n        journal: h.journal.clone(),\n    }\n}\n", ".journal"),
        ("B1", "fn plant(r: Run, p: bool) {\n    match r {\n        #[cfg(test)]\n        other if p => drop(other),\n        _ => r.host.withdraw_run(Source::Codex, \"id\"),\n    }\n}\n", "withdraw_run"),
        ("B2", "fn plant(r: &Run, a: u8, b: u8) {\n    match (a, b) {\n        #[cfg(test)]\n        (0, 0) => {\n            probe();\n        }\n        (_, _) => r.host.withdraw_run(Source::Codex, \"id\"),\n    }\n}\n", "withdraw_run"),
        ("B3", "fn plant(r: &Run, s: &Kind) {\n    match s {\n        #[cfg(test)]\n        Kind::Probe => {\n            probe();\n        }\n        &Kind::Live => r.host.withdraw_run(Source::Codex, \"id\"),\n        _ => {}\n    }\n}\n", "withdraw_run"),
        ("B4", "fn plant(h: &RunHost, s: &str) {\n    match s {\n        #[cfg(test)]\n        \"probe\" => {\n            probe();\n        }\n        \"live\" => drop(h.drafts.len()),\n        _ => {}\n    }\n}\n", ".drafts"),
        ("B5", "fn plant(r: &Run, k: Kind) {\n    match k {\n        #[cfg(test)]\n        probe_pat!() => probe(),\n        Kind::Live => r.host.withdraw_run(Source::Codex, \"id\"),\n        _ => {}\n    }\n}\n", "withdraw_run"),
        ("C1", "fn plant(h: &RunHost) {\n    #[cfg(test)]\n    probe();\n    let _d = h.drafts.len();\n}\n", ".drafts"),
        ("C2", "async fn plant(h: &RunHost) {\n    #[cfg(test)]\n    tokio::select! { _ = a() => {}, _ = b() => {} }\n    *h.drafts.lock().unwrap() = Default::default();\n}\n", ".drafts"),
        ("D1", "fn plant(h: &RunHost, row: Row) {\n    #[cfg(test)]\n    let probe = Probe { a: 1, b: 2 };\n    h.journal.append(row);\n}\n", ".journal"),
        ("D2", "fn plant(r: &Run) {\n    #[cfg(test)]\n    let f = |a, b| a + b;\n    r.host.withdraw_run(Source::Codex, \"id\");\n}\n", "withdraw_run"),
        ("E2", "#[cfg(any(test, feature = \"x\"))]\nfn plant(r: &Run) {\n    r.host.withdraw_run(Source::Codex, \"id\");\n}\n", "withdraw_run"),
        ("F1", "fn plant(h: &RunHost) { let _ = h.drafts.len(); }\n#[cfg(test)]\nfn t() {}", ".drafts"),
        ("F2", "fn plant(h: &RunHost) {\n    #[cfg(test)]\n    probe();\n    let _ = h.drafts.len(); }", ".drafts"),
        ("G1", "fn plant(a: u8, b: u8) {\n    match (a, b) {\n        #[cfg(test)]\n        (0, 0) => {\n            probe();\n        }\n        (_, _) => drop(droppen_provider::SpawnGate::new()),\n    }\n}\n", "SpawnGate::new"),
        ("H1", "fn plant(h: &RunHost) -> S {\n    S {\n        #[cfg(test)]\n        probe: r#\"}, {\"#, // }\n        journal: h.journal.clone(), /* , } */\n    }\n}\n", ".journal"),
        ("H2", "fn plant(h: &RunHost) -> S {\n    S {\n        #[cfg(test)]\n        probe: b'}',\n        journal: h.journal.clone(),\n    }\n}\n", ".journal"),
        ("H3", "fn plant(h: &RunHost) {\n    let RunHost { drafts, .. } = h;\n}\n", "RunHost { … }"),
        // More shapes: an arm after a block, by each token it can start
        // with, and other test-only items before production code.
        ("a slice arm", "fn f(h: &H, x: &[u8]) -> usize { match x { #[cfg(test)] [] => { gone(); } [a, ..] => h.drafts.len(), } }", ".drafts"),
        ("a negative arm", "fn f(h: &H, x: i8) -> usize { match x { #[cfg(test)] 0 => { gone(); } -1 => h.journal.len(), _ => 0 } }", ".journal"),
        ("a `|` arm", "fn f(r: &R, x: X) { match x { #[cfg(test)] X::A => { gone(); } | X::B | X::C => r.host.publish_run(), } }", "publish_run"),
        ("a generic field", "struct S { #[cfg(test)] steps: Mutex<HashMap<String, Vec<Step>>>, entry: RunEntry }", "RunEntry"),
        ("a block", "fn f(h: &H) { #[cfg(test)] { gone(); } h.host.running_run(Source::Claude, \"id\"); }", "running_run"),
        ("a statement", "fn f(h: &H) { #[cfg(test)] self.host.step(\"a\", Source::Acp, None); h.host.rename_run(a, b); }", "rename_run"),
        ("a thread-local", "#[cfg(test)] thread_local! { static GONE: u8 = 0; } fn f(p: SpawnPermit) {}", "SpawnPermit"),
        ("an impl block", "#[cfg(test)] impl Probe { fn gone() {} } fn f(l: SessionLease) {}", "SessionLease"),
        ("a use", "#[cfg(test)] use x::y; fn f(r: &R) { r.provider_with_gate::<P>(); }", "provider_with_gate"),
    ];
    for (what, text, reach) in cases {
        assert!(found_in(text).contains(&reach.to_string()), "{what}: {reach} is read in\n{text}\nfound {:?}", found_in(text));
    }
    // What is built for tests alone, a comment, a string or a doc is not
    // read. The review's E1 is here: `cfg(all(test, unix))` needs `test`,
    // so it is built for tests alone.
    let quiet = [
        ("E1", "#[cfg(all(test, unix))]\nfn plant(r: &Run) {\n    r.host.withdraw_run(Source::Codex, \"id\");\n}\n"),
        ("a test function", "#[cfg(test)] fn f(h: &H) { h.journal.len(); }"),
        ("a test statement", "fn f(h: &H) { #[cfg(test)] h.journal.len(); }"),
        ("comments and strings", "// h.journal and SpawnGate::new\nfn f() { let s = \"h.journal\"; }"),
        ("a doc", "/// `h.journal`, `RunHost { .. }`\nfn f() {}"),
        ("impls of a host", "impl Trait for RunHost {} impl RunHost { fn f(&self) {} }"),
        ("a method named like a field", "fn f(s: &S) { s.drafts(); }"),
    ];
    for (what, text) in quiet {
        assert!(found_in(text).is_empty(), "{what}: nothing is read in\n{text}\nfound {:?}", found_in(text));
    }
    // A test module that is a file of its own is noted by its path through
    // the inline modules around it, and not at all under a `#[path]`: its
    // file is then not where its name says, so it stays read.
    let modules = |text: &str| Code::of(text).expect("parses").1;
    assert_eq!(modules("#[cfg(test)] mod elsewhere; #[cfg(test)] mod inline {} mod kept;"), ["elsewhere"]);
    assert_eq!(modules("mod plant; mod probe { #[cfg(test)] mod plant; }"), ["probe/plant"]);
    assert!(modules("#[path = \"plant.rs\"] mod live; #[cfg(test)] #[path = \"probe.rs\"] mod plant;").is_empty());
    assert!(modules("#[path = \"elsewhere\"] mod probe { #[cfg(test)] mod plant; }").is_empty());
}

/// A crate depends on the base however its manifest spells it, per target
/// too; a dev- or build-dependency is no dependency of its code.
#[test]
fn a_manifest_says_whether_its_crate_sees_the_base() {
    let sees = |manifest: &str| manifest.parse::<toml::Table>().expect("a manifest").sees_the_base();
    let on = [
        "[dependencies]\ndroppen-provider = { path = \"../droppen-provider\" }\n",
        "[dependencies.droppen-provider]\npath = \"../droppen-provider\"\n",
        "[dependencies]\nbase = { package = \"droppen-provider\", path = \"../droppen-provider\" }\n",
        "[target.'cfg(unix)'.dependencies]\ndroppen-provider = { path = \"../droppen-provider\" }\n",
    ];
    for manifest in on {
        assert!(sees(manifest), "sees the base:\n{manifest}");
    }
    let off = [
        "[dev-dependencies]\ndroppen-provider = { path = \"../droppen-provider\" }\n",
        "[build-dependencies]\ndroppen-provider = { path = \"../droppen-provider\" }\n",
        "[dependencies]\ndroppen-sessions = { path = \"../droppen-sessions\" }\n",
    ];
    for manifest in off {
        assert!(!sees(manifest), "does not see the base:\n{manifest}");
    }
}
