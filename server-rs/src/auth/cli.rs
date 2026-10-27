//! `remote-hand-server user add|list|passwd|rm|mark-app`, `setup-code` and
//! `open`.
//!
//! The bootstrap problem — "who creates the first account?" — has three
//! answers. This one is the shell: anyone who can run this binary already has
//! the operating-system account it would be running as, and therefore already
//! has everything the API could give them, so `user add` asks for no code. The
//! second is the same proof made to the running server: `open` gets a one-time
//! link over a socket only that user can reach ([`super::local_open`]) and
//! opens the page as the device's owner, who may make the first account there.
//! The third is the setup page ([`super::setup`]), for a device somebody is
//! setting up from another one: it takes the one-time code the server keeps,
//! which `setup-code` shows.
//!
//! Consequently these subcommands take **no instance lock**. They write one
//! instance's accounts and (for `passwd` and `rm`) that same instance's session
//! store, both of which a running server may be reading — a race whose worst
//! outcome is a lost login in the second it takes to type a new password, and
//! whose alternative is refusing to reset a password while a server is up.
//!
//! # Which instance
//!
//! Accounts are per-instance ([`super::users`]), so every one of these
//! subcommands has to name one, and naming the wrong one has two failure modes
//! worth keeping apart. Adding an account to an instance nobody is running is
//! merely useless. *Not* adding it to the instance somebody is running is a
//! server left open by a command that reported success — so the instance is
//! named in every line of output, and a defaulted instance with no directory on
//! this machine is a refusal rather than a guess.
//!
//! The default is the instance `$APP_INSTANCE` names, which is the same setting
//! a server is started with: `APP_INSTANCE=bob remote-hand-server user add bob`
//! and the server on bob's port agree about which accounts they mean because
//! they read the same value. `--instance <name>` says it outright, for a shell
//! that has no such variable exported.

use std::time::Duration;

use crate::sessions::Paths;

use super::sessions::SessionsStore;
use super::users::{UserError, UsersStore};

/// `println!`, in the name this binary was run as: the commands a message
/// suggests read `droppen …` when it was run as `droppen`
/// ([`crate::program::Name::rewrite`]).
macro_rules! say {
    ($($arg:tt)*) => {
        println!("{}", crate::program::Name::invoked().rewrite(&format!($($arg)*)))
    };
}

/// The TTL a session store built here is given. Irrelevant — nothing minted from
/// the CLI — but the type asks for one.
const NOMINAL_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// What to print when the subcommand does not parse.
pub const USAGE: &str = "\
USAGE:
    remote-hand-server user add <name> [--app]  [--instance <instance>]
    remote-hand-server user list [--app]        [--instance <instance>]
    remote-hand-server user passwd <name>       [--instance <instance>]
    remote-hand-server user rm <name> [--yes]   [--instance <instance>]
    remote-hand-server user mark-app <name>     [--instance <instance>]

A name is 1-32 characters of a-z, 0-9, '.', '_' or '-'. Passwords are read
twice from the terminal without echo, or once from stdin when it is not a
terminal (`printf '%s\\n' \"$PW\" | remote-hand-server user add ada`).

Accounts belong to one instance and let that one server in: they live in
~/.local/share/remote-hand/instances/<instance>/users.json. Without
--instance the instance is the one $APP_INSTANCE names — the same setting the
server is started with — or `default`. Once an instance has an account, every
request to it needs a login, from this device too. Until then it is in its
setup state: `remote-hand-server open` on this device opens its page signed in
as the device's owner, and anywhere else the first account is made on the page
with the one-time code its server keeps (`remote-hand-server setup-code`), or
here with `user add`, which needs no code.

Removing the last user account asks for --yes: the instance goes back to its
setup state, and every browser signed in with an account is signed out.

--app makes (or, with list, lists) an app-internal account: the Droppen
desktop app's own login, with a password nobody is shown. It signs the app's
window in, and is not a user account — it is left out of `user list` and does
not end the setup state. `mark-app` turns an existing account into one.";

/// What to print when `open` does not parse.
pub const OPEN_USAGE: &str = "\
USAGE:
    remote-hand-server open [--print] [--instance <instance>]

Opens the instance's page in this device's browser. While the instance has no
user account, it opens it signed in as this device's owner: it asks the
running server — over a socket only this operating-system user can reach — for
a one-time link that works once, within a minute. With --print, or with no
browser to open, it prints that link instead; if opening a browser fails, it
prints the link and exits with an error. Once the instance has an account, it
opens the login page.";

/// What to print when `setup-code` does not parse.
pub const SETUP_CODE_USAGE: &str = "\
USAGE:
    remote-hand-server setup-code [--instance <instance>]

Shows the one-time setup code the server of an instance with no account
printed when it started: open the instance's page, enter the code and choose
the first account's name and password. The code works once, only until an
account exists and only while that server runs; a new one is made each time it
starts without an account. `remote-hand-server user add <name>` makes the first
account here instead, with no code.";

/// What the operator asked for, once the arguments have been read.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    /// `add <name> [--app]`.
    Add(String, Kind),
    /// `list [--app]`.
    List(Kind),
    /// `passwd <name>`.
    Passwd(String),
    /// `rm <name> [--yes]`.
    Remove(String, bool),
    /// `mark-app <name>`.
    MarkApp(String),
}

/// A user account, or the desktop app's own ([`UsersStore::add_app`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    User,
    App,
}

/// Run a `user` subcommand. `args` is everything after `user`.
///
/// The arguments are read before the configuration is: a mistyped subcommand
/// must answer with the usage on any machine, including one where the
/// configuration will not load or the instance cannot be worked out.
pub async fn run(args: &[String]) -> Result<(), String> {
    let (args, instance) = take_instance(args)?;
    let action = parse(&args)?;
    let paths = home_paths(instance.as_deref())?;
    let store = UsersStore::new(&paths);
    match action {
        Action::Add(name, kind) => add(&store, &paths, &name, kind).await,
        Action::List(kind) => list(&store, &paths, kind).await,
        Action::Passwd(name) => passwd(&store, &paths, &name).await,
        Action::Remove(name, yes) => remove(&store, &paths, &name, yes).await,
        Action::MarkApp(name) => mark_app(&store, &paths, &name).await,
    }
}

/// The subcommand, or the usage.
///
/// The argument count is part of every arm, so a stray word — the shape a
/// half-remembered `user add ada --instance bob` takes when the flag is
/// misspelled — is the usage rather than a silently ignored argument.
fn parse(args: &[String]) -> Result<Action, String> {
    let app = args.iter().any(|arg| arg == "--app");
    let yes = args.iter().any(|arg| arg == "--yes");
    let words: Vec<&str> = args.iter().map(String::as_str).filter(|arg| *arg != "--app" && *arg != "--yes").collect();
    let kind = if app { Kind::App } else { Kind::User };
    match (words.as_slice(), app, yes) {
        (["add", name], _, false) => Ok(Action::Add((*name).to_string(), kind)),
        (["list"], _, false) => Ok(Action::List(kind)),
        (["passwd", name], false, false) => Ok(Action::Passwd((*name).to_string())),
        (["rm", name], false, _) => Ok(Action::Remove((*name).to_string(), yes)),
        (["mark-app", name], false, false) => Ok(Action::MarkApp((*name).to_string())),
        _ => Err(USAGE.to_string()),
    }
}

/// Pull `--instance <name>` (or `--instance=<name>`) out of the arguments,
/// wherever in them it was written.
///
/// Hand-parsed rather than handed to a parser crate, for the reason the rest of
/// this binary's argument handling is: there are four subcommands and one flag.
/// A `--instance` with nothing after it is refused rather than defaulted — the
/// whole point of the flag is to be sure which instance is meant, so falling
/// back to a default would defeat it at exactly the moment somebody was trying
/// to be explicit. The name is validated here, before it can become a path
/// segment under the data directory.
pub(crate) fn take_instance(args: &[String]) -> Result<(Vec<String>, Option<String>), String> {
    let mut rest = Vec::new();
    let mut instance = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let value = if let Some(value) = arg.strip_prefix("--instance=") {
            value.to_string()
        } else if arg == "--instance" {
            iter.next().ok_or_else(|| "--instance needs a name".to_string())?.clone()
        } else {
            rest.push(arg.clone());
            continue;
        };
        instance = Some(named(value)?);
    }
    Ok((rest, instance))
}

/// An instance name, or the same refusal [`crate::instance`] gives for one.
fn named(name: String) -> Result<String, String> {
    if crate::instance::valid_name(&name) {
        Ok(name)
    } else {
        Err(format!("instance name {name:?} must be 1-32 characters of a-z, 0-9 or '-'"))
    }
}

/// The path set the CLI works against — the configured home, not `$HOME`
/// blindly, so `APP_HOME_DIR` points the CLI at the same tree it points a server.
///
/// The instance is the one that was asked for, or the one this machine's
/// configuration and `$APP_INSTANCE` would start a server on. A *defaulted*
/// instance with no directory yet is refused, and the refusal lists what is
/// there: on a machine whose servers all run under names of their own, the
/// default is `default`, `default` is nobody, and an account added to it would
/// be an account added to no running server by a command that said "added". An
/// instance named outright is never second-guessed — preparing the accounts of
/// a server that has not been started yet is a thing somebody may legitimately
/// mean.
pub(crate) fn home_paths(requested: Option<&str>) -> Result<Paths, String> {
    cli_context(requested).map(|(paths, _)| paths)
}

/// [`home_paths`] and the configuration it was derived from, loaded once.
///
/// A command that needs both — the paths to find the instance, the config for
/// its own switches — used to load and merge every source twice.
pub(crate) fn cli_context(requested: Option<&str>) -> Result<(Paths, gotcha::ConfigWrapper<crate::config::AppConfig>), String> {
    let config = crate::config::load(std::env::var("GOTCHA_ACTIVE_PROFILE").ok()).map_err(|error| error.to_string())?;
    let instance = match requested {
        Some(name) => name.to_string(),
        None => named(config.app.instance())?,
    };
    let paths = Paths::for_instance(config.app.home(), &instance);
    if requested.is_none() && !paths.instance_dir().exists() {
        let known = instances_on_this_machine(&paths);
        if !known.is_empty() {
            return Err(format!(
                "there is no instance {instance:?} on this machine, and an account belongs to one instance.\n\
                 Say which one you mean: remote-hand-server user ... --instance <name>\n\
                 Under {}: {}",
                paths.data_dir().join("instances").display(),
                known.join(", ")
            ));
        }
    }
    Ok((paths, config))
}

async fn add(store: &UsersStore, paths: &Paths, name: &str, kind: Kind) -> Result<(), String> {
    let first = !store.has_users().await;
    let password = ask_for_a_new_password(name)?;
    if kind == Kind::App {
        store.add_app(name, &password).await.map_err(refusal)?;
        say!(
            "added {name} to instance {} as an app-internal account ({})",
            paths.instance(),
            store.path().display()
        );
        return Ok(());
    }
    store.add(name, &password).await.map_err(refusal)?;
    // A setup code is only good while the instance has no account. Its
    // server drops its own copy within a few seconds; the file goes now, so
    // `setup-code` never shows a code that has stopped working. The pages
    // opened as the device's owner end now too — every token check already
    // refuses them once an account exists; this takes them off the books.
    super::setup::remove_code_file(&super::setup::code_file(paths));
    sessions_of(paths).revoke_user(super::LOCAL_OWNER, None).await;
    // A Droppen link the device's owner signed in passes to this account, but
    // not from here: `cloud.json` is the running server's to write, under its
    // lock, and it does this within a few seconds — or, if it is not running,
    // when it next starts (`Cloud::hand_over_from_local_owner`). Until then
    // nothing acts on the link as the owner: the owner's sessions are gone
    // (above), and the owner counts only while there is no account.
    say!("added {name} to instance {} ({})", paths.instance(), store.path().display());
    if first {
        say!(
            "that is instance {}'s first account: from now on its page asks for a login everywhere, this device included; its setup code and pages opened with `remote-hand-server open` no longer work, and no other instance is touched",
            paths.instance()
        );
    }
    Ok(())
}

async fn list(store: &UsersStore, paths: &Paths, kind: Kind) -> Result<(), String> {
    let wanted = kind == Kind::App;
    let users: Vec<_> = store.list().await.into_iter().filter(|user| user.app == wanted).collect();
    if users.is_empty() {
        // "no accounts" first, whatever follows: the installer and the desktop
        // app read exactly that prefix as "none".
        if wanted {
            say!(
                "no accounts of the desktop app on instance {} ({})",
                paths.instance(),
                store.path().display()
            );
        } else {
            say!(
                "no accounts on instance {} ({}) — open its page on this device with `remote-hand-server open`, or make the first account: `remote-hand-server user add <name>` here, or on the page with the code `remote-hand-server setup-code` shows",
                paths.instance(),
                store.path().display()
            );
        }
        return Ok(());
    }
    let what = if wanted { "app-internal accounts" } else { "accounts" };
    say!("{what} on instance {} ({}):", paths.instance(), store.path().display());
    for user in users {
        say!("{}\tcreated {}\tpassword changed {}", user.name, user.created_at, user.updated_at);
    }
    Ok(())
}

async fn mark_app(store: &UsersStore, paths: &Paths, name: &str) -> Result<(), String> {
    store.mark_app(name).await.map_err(refusal)?;
    say!("{name} on instance {} is now an app-internal account", paths.instance());
    if !store.has_users().await {
        say!(
            "instance {} has no user account now: it is in its setup state (`remote-hand-server open` opens its page on this device)",
            paths.instance()
        );
    }
    Ok(())
}

async fn passwd(store: &UsersStore, paths: &Paths, name: &str) -> Result<(), String> {
    if !store.exists(name).await {
        return Err(format!("no such user: {name}"));
    }
    let password = ask_for_a_new_password(name)?;
    store.set_password(name, &password).await.map_err(refusal)?;
    // This instance's sessions, and deliberately not every instance's, which is
    // what this used to be. While one account set was shared, a live token on
    // the staging instance was a live token for the password just replaced. Now
    // an account of the same name on another instance is a *different account*,
    // quite possibly a different person's, and revoking their logins would be
    // this command doing something nobody asked it to.
    let revoked = sessions_of(paths).revoke_user(name, None).await;
    say!(
        "changed the password for {name} on instance {}; {revoked} session(s) revoked",
        paths.instance()
    );
    Ok(())
}

/// What removing the last user account does, said before it is done.
fn last_account_warning(paths: &Paths, name: &str) -> String {
    let instance = paths.instance();
    format!(
        "{name} is instance {instance}'s last user account: without it the instance goes back to its setup state, every browser signed in with an account is signed out, and the page is opened again with `remote-hand-server open` on this device (or the desktop app), or with a new first account made with the setup code."
    )
}

async fn remove(store: &UsersStore, paths: &Paths, name: &str, yes: bool) -> Result<(), String> {
    let last = store.has_users().await
        && !store.is_app(name).await
        && store.list().await.iter().filter(|user| !user.app).count() == 1
        && store.exists(name).await;
    if last && !yes {
        return Err(format!(
            "{}\nRun again with --yes to remove it anyway: remote-hand-server user rm {name} --yes --instance {}",
            last_account_warning(paths, name),
            paths.instance()
        ));
    }
    store.remove(name).await.map_err(refusal)?;
    // Belt and braces: every token check already re-confirms the account against
    // *this* instance's store, so the sessions are dead the moment the row is
    // gone. This is what stops them from sitting in the database until they
    // expire.
    let mut remaining: Vec<String> = store.list().await.into_iter().map(|user| user.name).collect();
    if !store.has_users().await {
        // With no user account, a page opened as the device's owner is a
        // login that still stands.
        remaining.push(super::LOCAL_OWNER.to_string());
    }
    sessions_of(paths).sweep(&remaining).await;
    say!("removed {name} from instance {}", paths.instance());
    if last {
        say!("{}", last_account_warning(paths, name));
        say!(
            "A new first account: `remote-hand-server user add <name>`, or the setup code its running server now prints (`remote-hand-server setup-code`)."
        );
    }
    Ok(())
}

/// `remote-hand-server setup-code`: the code a running server in its setup
/// state printed, read back from the file it wrote. `args` is everything after
/// `setup-code`.
pub async fn setup_code(args: &[String]) -> Result<(), String> {
    let (rest, instance) = take_instance(args)?;
    if !rest.is_empty() {
        return Err(SETUP_CODE_USAGE.to_string());
    }
    let (paths, config) = cli_context(instance.as_deref())?;
    let store = UsersStore::new(&paths);
    let name = paths.instance();
    let file = super::setup::code_file(&paths);
    if config.app.auth.enabled() == Some(false) {
        return Err(format!(
            "local authentication is off in this configuration (auth.enabled = false): a server started with it answers anyone and has no setup state, so there is no setup code.\n\
             Add accounts with: remote-hand-server user add <name> --instance {name}"
        ));
    }
    if store.has_users().await {
        return Err(format!(
            "instance {name} already has an account, so there is no setup code: sign in on its page, or add another account with remote-hand-server user add <name> --instance {name}"
        ));
    }
    let Some(code) = super::setup::read_code_file(&file) else {
        return Err(format!(
            "instance {name} has no account and no setup code: its server is not running (it makes a new code each time it starts without an account), or it is not the one serving this instance.\n\
             Start it, or create the account here: remote-hand-server user add <name> --instance {name}"
        ));
    };
    let port = crate::instance::InstanceDirs::new(&paths.data_dir(), name)
        .ok()
        .and_then(|dirs| std::fs::read_to_string(dirs.port_file()).ok())
        .and_then(|text| text.trim().parse::<u16>().ok());
    say!("setup code for instance {name}: {}", super::setup::display(&code));
    match port {
        Some(port) => say!(
            "Open {} and enter it to create the first account.",
            super::setup::local_url(&config.server.host, port)
        ),
        None => say!("Open the instance's page and enter it to create the first account."),
    }
    say!("It works once, only until an account exists, and only while the server that printed it runs.");
    Ok(())
}

/// `remote-hand-server open`: open the instance's page in this device's
/// browser — signed in as its owner while it has no user account. `args` is
/// everything after `open`.
pub async fn open(args: &[String]) -> Result<(), String> {
    let (rest, instance) = take_instance(args)?;
    let print = match rest.as_slice() {
        [] => false,
        [flag] if flag == "--print" => true,
        _ => return Err(OPEN_USAGE.to_string()),
    };
    let paths = home_paths(instance.as_deref())?;
    let answer = super::local_open::ask(&paths).await?;
    if let Some(error) = answer.error {
        return Err(format!("instance {}: {error}", paths.instance()));
    }
    let Some(url) = answer.url else {
        // An account exists: the login page, which has nothing secret in it.
        let opened = if print { Opened::NoBrowser } else { open_in_browser(Opening::Address(&answer.page)) };
        if opened != Opened::Yes {
            say!("{}", answer.page);
        }
        say!("instance {} has an account: sign in on its page ({})", paths.instance(), answer.page);
        return opened.outcome();
    };
    let opened = if print {
        Opened::NoBrowser
    } else {
        match answer.redirect.as_deref() {
            Some(page) => open_in_browser(Opening::RedirectPage(page, &url)),
            None => open_in_browser(Opening::Secret(&url)),
        }
    };
    if opened == Opened::Yes {
        say!(
            "opened {} in your browser, signed in as this device's owner (the link works once, within a minute)",
            answer.page
        );
        // A browser in a sandbox (a snap) may not be let read the redirect
        // page in a hidden directory. The link is not printed here: it is
        // live, and this output may be read by more than the person who asked
        // (a wrapper, a log, an AI CLI that ran the command). The command it
        // suggests reaches this instance, as the one that was run did.
        #[cfg(not(target_os = "macos"))]
        say!(
            "If the browser shows nothing, run `{}remote-hand-server open --print` and open the link it prints instead.",
            crate::program::command_prefix(&paths)
        );
        return Ok(());
    }
    say!("{url}");
    say!("Open that link in a browser on this device: it signs the page in as this device's owner, works once, and only within a minute.");
    opened.outcome()
}

/// How handing something to the browser went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opened {
    /// The browser has it.
    Yes,
    /// There is no browser to hand it to (no desktop session), or it was not
    /// asked for (`--print`): printing it is the answer, not a failure.
    NoBrowser,
    /// A browser was tried and did not take it.
    Failed,
}

impl Opened {
    /// What `open` exits with, once it has printed what there is to print: an
    /// error when a browser was tried and failed — so a caller that ran it for
    /// a browser (the desktop app) does not take a printed link for an opened
    /// page.
    fn outcome(self) -> Result<(), String> {
        match self {
            Self::Yes | Self::NoBrowser => Ok(()),
            Self::Failed => Err("could not open a browser on this device: open the address printed above in one".to_string()),
        }
    }
}

/// What to hand the browser.
///
/// macOS reads every field (the link goes to `osascript`, the page is the
/// fallback); elsewhere only the redirect page is opened and a bare link is
/// never handed to an opener, so those fields go unread there.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
enum Opening<'a> {
    /// An address with nothing secret in it.
    Address(&'a str),
    /// The `0600` redirect page the server wrote, and the link it leads to.
    RedirectPage(&'a std::path::Path, &'a str),
    /// A link with the token in it and no page to carry it.
    Secret(&'a str),
}

/// Open something in this device's default browser, keeping a token off every
/// command line — where another user's `ps` could read it.
/// [`Opened::NoBrowser`] when there is none to open (no desktop session), so
/// the caller prints instead; [`Opened::Failed`] when the opener did not take
/// it.
fn open_in_browser(what: Opening<'_>) -> Opened {
    use std::process::{Command, Stdio};
    let tried = |ok: bool| if ok { Opened::Yes } else { Opened::Failed };
    #[cfg(target_os = "macos")]
    {
        // `open location` through AppleScript read from standard input: the
        // default browser, and the link in nobody's arguments.
        let script = |url: &str| format!("open location {}\n", serde_json::to_string(url).unwrap_or_default());
        let via_osascript = |url: &str| {
            let Ok(mut child) = Command::new("osascript")
                .arg("-")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            else {
                return false;
            };
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                let _ = stdin.write_all(script(url).as_bytes());
            }
            child.wait().is_ok_and(|status| status.success())
        };
        tried(match what {
            Opening::Address(url) => Command::new("open").arg(url).status().is_ok_and(|s| s.success()),
            // AppleScript unavailable (a locked-down account): the redirect page,
            // which is this user's alone to read.
            Opening::RedirectPage(page, url) => via_osascript(url) || Command::new("open").arg(page).status().is_ok_and(|s| s.success()),
            Opening::Secret(url) => via_osascript(url),
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
            return Opened::NoBrowser;
        }
        let target = match what {
            Opening::Address(url) => url.to_string(),
            // The redirect page's path is no secret; what it holds is, and it
            // is readable by this user alone.
            Opening::RedirectPage(page, _) => page.display().to_string(),
            Opening::Secret(_) => return Opened::NoBrowser,
        };
        tried(
            Command::new("xdg-open")
                .arg(target)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success()),
        )
    }
}

/// The session store of the instance being worked on.
///
/// Keyed by that instance's own state database — [`Paths::instance_db`] — which
/// is where its logins live and what identifies the instance.
fn sessions_of(paths: &Paths) -> SessionsStore {
    SessionsStore::new(paths.instance_db(), NOMINAL_TTL)
}

/// Every instance that has a directory on this machine, in name order.
///
/// Only ever printed, and only in the refusal that asks the operator which
/// instance they meant. Filtered on the **directory** rather than on any file
/// in it, and that distinction was a bug here once: an instance stopped writing
/// `auth-sessions.json` when its sessions moved into the database, so a listing
/// keyed on that file leaves out exactly the instances made most recently —
/// which are the ones somebody typing this command is looking for.
fn instances_on_this_machine(paths: &Paths) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(paths.data_dir().join("instances")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .collect();
    names.sort();
    names
}

fn refusal(error: UserError) -> String {
    error.to_string()
}

/// Read a new password: twice from a terminal, once from a pipe.
///
/// The confirmation only exists because a terminal shows nothing while it is
/// being typed. A pipe has no such problem, and asking a script to send the same
/// line twice would be a worse interface, not a safer one.
fn ask_for_a_new_password(name: &str) -> Result<String, String> {
    if !stdin_is_a_terminal() {
        let password = read_line().map_err(|error| format!("could not read a password: {error}"))?;
        super::users::check_password(name, &password).map_err(refusal)?;
        return Ok(password);
    }
    let password = read_secret("Password: ").map_err(|error| format!("could not read a password: {error}"))?;
    // Checked before the confirmation, so a password that was never going to be
    // accepted is refused after one attempt rather than two.
    super::users::check_password(name, &password).map_err(refusal)?;
    let again = read_secret("Confirm password: ").map_err(|error| format!("could not read a password: {error}"))?;
    if password != again {
        return Err("the two passwords do not match".to_string());
    }
    Ok(password)
}

#[cfg(unix)]
pub(crate) fn stdin_is_a_terminal() -> bool {
    // SAFETY: `isatty` only inspects a file descriptor and cannot fail unsafely.
    unsafe { libc::isatty(libc::STDIN_FILENO) == 1 }
}

#[cfg(not(unix))]
pub(crate) fn stdin_is_a_terminal() -> bool {
    false
}

fn read_line() -> std::io::Result<String> {
    use std::io::BufRead;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

/// Prompt and read a line with the terminal's echo switched off.
///
/// The terminal state is restored whatever happens to the read, including a
/// `^D` at the prompt: leaving a shell with `ECHO` off is the kind of damage a
/// password prompt has no business doing.
#[cfg(unix)]
pub(crate) fn read_secret(prompt: &str) -> std::io::Result<String> {
    use std::io::Write;

    // Echo off before the prompt shows: whatever is typed (or pasted) the
    // moment it appears must not be echoed. And back on however this ends —
    // a return, an error, or a signal that kills the process.
    let echo = EchoOff::new();
    print!("{prompt}");
    std::io::stdout().flush()?;
    let line = read_line();
    drop(echo);
    // The newline the user typed was not echoed, so the next output would
    // otherwise start on the prompt's line.
    println!();
    line
}

/// The terminal's echo, off for as long as this lives: put back when it is
/// dropped, and — through [`echo_restore`] — when SIGINT, SIGTERM or SIGHUP
/// ends the process first, which a drop never sees.
#[cfg(unix)]
struct EchoOff {
    original: Option<libc::termios>,
    _armed: Option<echo_restore::Armed>,
}

#[cfg(unix)]
impl EchoOff {
    fn new() -> Self {
        let original = echo_off();
        Self {
            original,
            _armed: original.map(echo_restore::arm),
        }
    }
}

#[cfg(unix)]
impl Drop for EchoOff {
    fn drop(&mut self) {
        if let Some(original) = self.original {
            // SAFETY: `original` came from `tcgetattr` on this same descriptor.
            unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &original) };
        }
        // `_armed` is dropped after this: the handlers stay until echo is back.
    }
}

/// A signal that kills the process while echo is off would leave the shell
/// that ran it with echo off too. While [`Armed`] lives, SIGINT, SIGTERM and
/// SIGHUP first put the terminal back, then end the process as they would
/// have (default action, raised again).
#[cfg(unix)]
mod echo_restore {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::OnceLock;

    const SIGNALS: [libc::c_int; 3] = [libc::SIGINT, libc::SIGTERM, libc::SIGHUP];

    /// The terminal as first found: what a signal puts back.
    static SAVED: OnceLock<libc::termios> = OnceLock::new();
    static ARMED: AtomicBool = AtomicBool::new(false);

    extern "C" fn put_back_and_die(sig: libc::c_int) {
        if ARMED.swap(false, Ordering::SeqCst) {
            if let Some(original) = SAVED.get() {
                // SAFETY: `tcsetattr` is async-signal-safe, and `original`
                // came from `tcgetattr` on this descriptor.
                unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, original) };
            }
        }
        // SAFETY: `signal` and `raise` are async-signal-safe: die of the
        // signal as if this handler had never been installed.
        unsafe {
            libc::signal(sig, libc::SIG_DFL);
            libc::raise(sig);
        }
    }

    /// The handlers installed by [`arm`]; dropping it puts the previous ones back.
    pub(super) struct Armed {
        previous: [libc::sigaction; 3],
    }

    pub(super) fn arm(original: libc::termios) -> Armed {
        let _ = SAVED.set(original);
        ARMED.store(true, Ordering::SeqCst);
        // SAFETY: zeroed `sigaction`s are valid to fill in; each call is given
        // a handler of the plain `fn(c_int)` shape `sa_flags = 0` asks for.
        let mut previous: [libc::sigaction; 3] = unsafe { std::mem::zeroed() };
        for (signal, previous) in SIGNALS.iter().zip(previous.iter_mut()) {
            unsafe {
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = put_back_and_die as extern "C" fn(libc::c_int) as libc::sighandler_t;
                libc::sigemptyset(&mut action.sa_mask);
                libc::sigaction(*signal, &action, previous);
                // A signal this process was started ignoring (`nohup`, a
                // shell's `trap '' INT`) stays ignored.
                if previous.sa_sigaction == libc::SIG_IGN {
                    libc::sigaction(*signal, previous, std::ptr::null_mut());
                }
            }
        }
        Armed { previous }
    }

    impl Drop for Armed {
        fn drop(&mut self) {
            ARMED.store(false, Ordering::SeqCst);
            for (signal, previous) in SIGNALS.iter().zip(self.previous.iter()) {
                // SAFETY: `previous` is what `sigaction` gave back for this signal.
                unsafe { libc::sigaction(*signal, previous, std::ptr::null_mut()) };
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn handler(signal: libc::c_int) -> libc::sighandler_t {
            // SAFETY: reading the current action into a zeroed struct.
            unsafe {
                let mut current: libc::sigaction = std::mem::zeroed();
                libc::sigaction(signal, std::ptr::null(), &mut current);
                current.sa_sigaction
            }
        }

        #[test]
        fn arming_puts_the_handlers_in_and_dropping_puts_the_previous_ones_back() {
            let ours = put_back_and_die as extern "C" fn(libc::c_int) as libc::sighandler_t;
            // What this process started with: a test run in the background
            // starts with SIGINT ignored, and that stays so.
            let before: Vec<_> = SIGNALS.iter().map(|s| handler(*s)).collect();
            // SAFETY: a zeroed termios is only stored, never applied here (the
            // handlers are not triggered).
            let armed = arm(unsafe { std::mem::zeroed() });
            for (signal, was) in SIGNALS.iter().zip(&before) {
                let expected = if *was == libc::SIG_IGN { libc::SIG_IGN } else { ours };
                assert_eq!(handler(*signal), expected, "signal {signal}");
            }
            assert!(ARMED.load(Ordering::SeqCst));
            drop(armed);
            assert!(!ARMED.load(Ordering::SeqCst));
            assert_eq!(SIGNALS.iter().map(|s| handler(*s)).collect::<Vec<_>>(), before);

            // An ignored signal stays ignored, and one that is not is taken,
            // whatever this process started with. One test, not two: signal
            // dispositions are the whole process's.
            // SAFETY: SIGHUP ignored and SIGTERM at its default for the length
            // of this check only.
            let (hup, term) = unsafe { (libc::signal(libc::SIGHUP, libc::SIG_IGN), libc::signal(libc::SIGTERM, libc::SIG_DFL)) };
            // SAFETY: as above.
            let armed = arm(unsafe { std::mem::zeroed() });
            assert_eq!(handler(libc::SIGHUP), libc::SIG_IGN, "an ignored signal stays ignored");
            assert_eq!(handler(libc::SIGTERM), ours);
            drop(armed);
            assert_eq!((handler(libc::SIGHUP), handler(libc::SIGTERM)), (libc::SIG_IGN, libc::SIG_DFL));
            // SAFETY: putting back what `signal` returned above.
            unsafe {
                libc::signal(libc::SIGHUP, hup);
                libc::signal(libc::SIGTERM, term);
            }
        }
    }
}

#[cfg(not(unix))]
pub(crate) fn read_secret(prompt: &str) -> std::io::Result<String> {
    use std::io::Write;
    print!("{prompt}");
    std::io::stdout().flush()?;
    read_line()
}

/// Switch `ECHO` off, returning the settings to put back.
#[cfg(unix)]
fn echo_off() -> Option<libc::termios> {
    // SAFETY: a zeroed `termios` is a valid one to read into, and both calls are
    // given a descriptor this process owns.
    unsafe {
        let mut term: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(libc::STDIN_FILENO, &mut term) != 0 {
            return None;
        }
        let original = term;
        term.c_lflag &= !libc::ECHO;
        if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &term) != 0 {
            return None;
        }
        Some(original)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_unparseable_subcommand_answers_with_the_usage() {
        for args in [vec![], vec!["add".to_string()], vec!["nonsense".to_string()]] {
            let error = run(&args).await.expect_err("should refuse");
            assert!(error.contains("USAGE"), "{error}");
        }
    }

    /// The refusal that asks which instance was meant lists them; a machine
    /// with none must not make that an error.
    #[test]
    fn a_machine_with_no_instances_has_no_instances_to_offer() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(instances_on_this_machine(&Paths::new(dir.path().to_path_buf())).is_empty());
    }

    #[test]
    fn every_instance_directory_is_offered_whatever_is_in_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(dir.path().to_path_buf());
        for name in ["staging", "default"] {
            let instance = paths.data_dir().join("instances").join(name);
            std::fs::create_dir_all(&instance).expect("mkdir");
            std::fs::write(instance.join("auth-sessions.json"), "{}").expect("write");
        }
        // An instance with no `auth-sessions.json` counts, and that is the whole
        // point: since the state moved into the database no instance writes one,
        // so keying on it would leave out every instance made from now on — and
        // those are the ones an operator asking "which instances are there?" has
        // just made and cannot remember the name of.
        std::fs::create_dir_all(paths.data_dir().join("instances").join("bare")).expect("mkdir");

        assert_eq!(instances_on_this_machine(&paths), vec!["bare", "default", "staging"]);
    }

    /// The flag can be written either way and in any position, because it is
    /// not the subject of the command and nobody remembers where it goes.
    #[test]
    fn the_instance_flag_comes_out_of_the_arguments_wherever_it_was_put() {
        let args = |words: &[&str]| words.iter().map(|word| word.to_string()).collect::<Vec<_>>();

        let (rest, instance) = take_instance(&args(&["add", "ada", "--instance", "bob"])).expect("parse");
        assert_eq!(rest, args(&["add", "ada"]));
        assert_eq!(instance.as_deref(), Some("bob"));

        let (rest, instance) = take_instance(&args(&["--instance=bob", "passwd", "ada"])).expect("parse");
        assert_eq!(rest, args(&["passwd", "ada"]));
        assert_eq!(instance.as_deref(), Some("bob"));

        let (rest, instance) = take_instance(&args(&["list"])).expect("parse");
        assert_eq!(rest, args(&["list"]));
        assert_eq!(instance, None);
    }

    /// Both refusals matter. A flag with nothing after it must not quietly
    /// become the default instance — somebody who typed `--instance` was trying
    /// to be sure — and a name that is not a legal instance name must not become
    /// a path segment under the data directory.
    #[test]
    fn a_flag_with_no_name_and_a_name_that_is_not_one_are_both_refused() {
        let args = |words: &[&str]| words.iter().map(|word| word.to_string()).collect::<Vec<_>>();
        assert!(take_instance(&args(&["list", "--instance"])).is_err());
        assert!(take_instance(&args(&["list", "--instance", "../etc"])).is_err());
        assert!(take_instance(&args(&["list", "--instance="])).is_err());
        assert!(
            take_instance(&args(&["list", "--instance", "Staging"])).is_err(),
            "upper case is not a name"
        );
    }

    /// The flag is removed before the subcommand is read, so its presence can
    /// never turn a good command into the usage.
    #[test]
    fn the_subcommand_is_read_out_of_what_the_flag_left_behind() {
        let args = |words: &[&str]| words.iter().map(|word| word.to_string()).collect::<Vec<_>>();
        let (rest, _) = take_instance(&args(&["rm", "ada", "--instance", "bob"])).expect("parse");
        assert!(matches!(parse(&rest), Ok(Action::Remove(name, false)) if name == "ada"));
        assert!(
            parse(&args(&["add", "ada", "extra"])).is_err(),
            "a stray word is the usage, not an ignored word"
        );
        assert!(parse(&args(&["list", "ada"])).is_err());
    }

    #[tokio::test]
    async fn open_takes_only_its_flag() {
        let args = |words: &[&str]| words.iter().map(|word| word.to_string()).collect::<Vec<_>>();
        let refusal = open(&args(&["nonsense"])).await.expect_err("usage");
        assert!(refusal.contains("USAGE"), "{refusal}");
        let refusal = open(&args(&["--print", "--print"])).await.expect_err("usage");
        assert!(refusal.contains("USAGE"), "{refusal}");
    }

    #[tokio::test]
    async fn removing_the_last_user_account_needs_a_yes_and_says_why() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::for_instance(dir.path().to_path_buf(), "rmtest");
        let store = UsersStore::new(&paths);
        store.add("ada", "correct horse battery").await.unwrap();
        store.add("grace", "correct horse battery").await.unwrap();
        store.add_app("app", "correct horse battery").await.unwrap();

        remove(&store, &paths, "grace", false).await.expect("not the last one");
        remove(&store, &paths, "app", false)
            .await
            .expect("the app's own account is not a user account");
        let refusal = remove(&store, &paths, "ada", false).await.expect_err("the last one");
        assert!(refusal.contains("setup state"), "{refusal}");
        assert!(refusal.contains("signed out"), "{refusal}");
        assert!(refusal.contains("--yes"), "{refusal}");
        assert!(store.exists("ada").await, "nothing removed without the yes");

        remove(&store, &paths, "ada", true).await.expect("with --yes");
        assert!(!store.has_users().await);
    }

    #[test]
    fn app_internal_accounts_are_asked_for_with_a_flag() {
        let args = |words: &[&str]| words.iter().map(|word| word.to_string()).collect::<Vec<_>>();
        assert_eq!(parse(&args(&["add", "ada"])), Ok(Action::Add("ada".into(), Kind::User)));
        assert_eq!(parse(&args(&["add", "--app", "ada"])), Ok(Action::Add("ada".into(), Kind::App)));
        assert_eq!(parse(&args(&["add", "ada", "--app"])), Ok(Action::Add("ada".into(), Kind::App)));
        assert_eq!(parse(&args(&["list", "--app"])), Ok(Action::List(Kind::App)));
        assert_eq!(parse(&args(&["list"])), Ok(Action::List(Kind::User)));
        assert_eq!(parse(&args(&["mark-app", "ada"])), Ok(Action::MarkApp("ada".into())));
        assert!(parse(&args(&["rm", "ada", "--app"])).is_err(), "the flag means nothing to rm");
        assert_eq!(parse(&args(&["rm", "--yes", "ada"])), Ok(Action::Remove("ada".into(), true)));
        assert!(parse(&args(&["add", "ada", "--yes"])).is_err(), "--yes is rm's alone");
        assert!(parse(&args(&["mark-app"])).is_err());
    }
}
