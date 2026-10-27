//! The app's local account, made with the server's own `user` subcommand.
//! (*Open in Browser* asks the running server with that account's login:
//! [`crate::server::local_open_grant`].)
//!
//! No new server code and no new trust: `remote-hand-server user add|passwd`
//! is how every install creates its first login, and it is trusted because
//! whoever can run the binary already owns the operating-system account the
//! server runs as. The app runs it against its own instance (`--instance
//! droppen-app`, and the same scrubbed environment the server gets), with the
//! password on standard input — never on the command line, where `ps` would
//! show it.
//!
//! The account is [`APP_ACCOUNT`], `droppen-app`, and **app-internal** (`user
//! add --app`): it signs the app's window in, but it is not a user account —
//! the server leaves it out of `user list`, and the login name stays free for
//! the person's own account. Installs made before this named the app's account
//! after the macOS login name ([`legacy_username`]); [`ready_login`] gives them
//! a `droppen-app` login, and the old account is removed once that has signed
//! in ([`Cli::retire`]). Until then it is app-internal too, so a failure on the
//! way never leaves the instance behind a login nobody can pass.
//!
//! The account is made **before** the server first starts, so the window can
//! sign in the moment the server answers.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use crate::credentials::{generate_password, Credential};

/// The app's own account on its instance: app-internal, never a person's.
pub const APP_ACCOUNT: &str = "droppen-app";

/// The name earlier builds gave the app's account: the login name (`$USER`)
/// as the server allows it — lower-case `a-z`, `0-9`, `.`, `_`, `-`, at most
/// 32 — or `me`. Only for finding that account again ([`ready_login`]).
pub fn legacy_username(user: Option<&str>) -> String {
    let name: String = user
        .unwrap_or_default()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
        .take(32)
        .collect();
    if name.is_empty() {
        "me".to_string()
    } else {
        name
    }
}

/// How to run the server binary for a one-off command.
#[derive(Debug, Clone)]
pub struct Cli {
    /// The server binary.
    pub program: PathBuf,
    /// Its complete environment (see [`crate::sidecar::server_env`]).
    pub env: Vec<(String, String)>,
    /// Its working directory.
    pub cwd: PathBuf,
    /// The instance to act on.
    pub instance: String,
    /// Where its output is appended (the server log), if anywhere.
    pub log: Option<PathBuf>,
}

/// What one command printed and how it ended.
#[derive(Debug, Clone)]
pub struct Ran {
    /// Whether it exited 0.
    pub ok: bool,
    /// Its standard output.
    pub stdout: String,
    /// Its standard error.
    pub stderr: String,
}

impl Cli {
    /// Run `user <args…> --instance <instance>`, writing `stdin` (if any) to it.
    pub fn user(&self, args: &[&str], stdin: Option<&str>) -> std::io::Result<Ran> {
        self.command("user", args, stdin, None)
    }

    /// Run `<subcommand> <args…> --instance <instance>`, writing `stdin` (if
    /// any) to it; with `timeout`, one still running after it is killed (by
    /// its own pid) and is an error.
    fn command(&self, subcommand: &str, args: &[&str], stdin: Option<&str>, timeout: Option<Duration>) -> std::io::Result<Ran> {
        let mut command = Command::new(&self.program);
        command
            .arg(subcommand)
            .args(args)
            .args(["--instance", &self.instance])
            .env_clear()
            .envs(self.env.iter().map(|(name, value)| (name, value)))
            .current_dir(&self.cwd)
            .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        if let Some(text) = stdin {
            if let Some(mut pipe) = child.stdin.take() {
                // A command that exits without reading (a refusal) closes the
                // pipe; its exit status, not a broken pipe, is the answer.
                let _ = pipe.write_all(format!("{text}\n").as_bytes());
            }
        }
        let output = match timeout {
            None => child.wait_with_output()?,
            Some(limit) => wait_bounded(child, limit)?,
        };
        let ran = Ran {
            ok: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        };
        if let Some(path) = &self.log {
            // The command's own words only: a password goes in on stdin and is
            // in neither stream, and a one-time sign-in token `open` prints is
            // cut out.
            if let Ok(mut file) = crate::logging::open_append(path) {
                let _ = writeln!(
                    file,
                    "[droppen-desktop] {subcommand} {} --instance {}: {}\n{}{}",
                    args.join(" "),
                    self.instance,
                    if ran.ok { "ok" } else { "failed" },
                    redact(&ran.stdout),
                    redact(&ran.stderr)
                );
            }
        }
        Ok(ran)
    }

    /// Run `user <args…>` and insist on success: the command's error message
    /// if it failed.
    fn run(&self, args: &[&str], stdin: Option<&str>) -> Result<Ran, String> {
        let ran = self.user(args, stdin).map_err(|error| format!("cannot run the server binary: {error}"))?;
        if !ran.ok {
            return Err(format!("user {} failed: {}", args.join(" "), ran.stderr.trim()));
        }
        Ok(ran)
    }

    /// The names of the instance's accounts of one kind: `user list`, or
    /// `user list --app`.
    pub fn list(&self, kind: Kind) -> Result<Vec<String>, String> {
        let args: &[&str] = match kind {
            Kind::User => &["list"],
            Kind::App => &["list", "--app"],
        };
        Ok(parse_list(&self.run(args, None)?.stdout))
    }

    /// Make sure `credential.username` is an app-internal account on the
    /// instance. With `reset_password`, its password is set to
    /// `credential.password` even if it already existed; without, an existing
    /// account keeps its password.
    ///
    /// An existing **user** account of that name is turned into an
    /// app-internal one (`user mark-app`) rather than added again, which would
    /// fail and leave the app unable to start: an account made before the app's
    /// login was app-internal can carry the app's name (the first launch then
    /// gives it a new password).
    pub fn ensure(&self, credential: &Credential, reset_password: bool) -> Result<Ensured, String> {
        let name = credential.username.as_str();
        let password = Some(credential.password.as_str());
        if self.list(Kind::App)?.iter().any(|app| app == name) {
            if !reset_password {
                return Ok(Ensured::Existed);
            }
            self.run(&["passwd", name], password)?;
            return Ok(Ensured::PasswordReset);
        }
        if self.list(Kind::User)?.iter().any(|user| user == name) {
            self.run(&["mark-app", name], None)?;
            if reset_password {
                self.run(&["passwd", name], password)?;
            }
            return Ok(Ensured::Migrated {
                password_reset: reset_password,
            });
        }
        self.run(&["add", name, "--app"], password)?;
        Ok(Ensured::Created)
    }

    /// Remove `old`, the account an earlier version of the app signed in as
    /// (named after the macOS login name) — that one account and nothing
    /// else, and only while it is app-internal ([`ready_login`] made it so if
    /// it was not). Only once the app has signed in as [`APP_ACCOUNT`], which
    /// must be app-internal by then, and never [`APP_ACCOUNT`] itself.
    ///
    /// A **user** account of that name is left alone ([`Retired::Kept`]),
    /// never removed: by the time this runs it may be the person's own, made
    /// under their login name once the old one was gone (a removal recorded
    /// but not yet forgotten, a login read back from a stale keychain item).
    pub fn retire(&self, old: &str) -> Result<Retired, String> {
        if old == APP_ACCOUNT {
            return Err(format!("refusing to remove {APP_ACCOUNT}: it is the app's own account"));
        }
        let apps = self.list(Kind::App)?;
        if !apps.iter().any(|name| name == APP_ACCOUNT) {
            return Err(format!(
                "refusing to remove {old}: {APP_ACCOUNT} is not an app-internal account on instance {}",
                self.instance
            ));
        }
        if !apps.iter().any(|name| name == old) {
            if self.list(Kind::User)?.iter().any(|name| name == old) {
                return Ok(Retired::Kept);
            }
            return Ok(Retired::Gone);
        }
        // `--yes`, which the server asks for before it removes what could be
        // an instance's last account.
        self.run(&["rm", old, "--yes"], None)?;
        Ok(Retired::Removed)
    }

}

impl Cli {
    /// Remove every account on the instance but `keep` — every user account
    /// and every other app-internal one — returning the names removed.
    ///
    /// This finishes the cleanup an older build's first start may have left:
    /// such a build inherited the machine-wide accounts into a new instance
    /// and removed them right after creating its own account, so an instance
    /// whose first start was cut short in between still has them, with the
    /// marker saying so. Every start retries until they are gone; an instance
    /// created by this build has nothing but `keep` to begin with. `keep` must
    /// be an app-internal account already, so the app's window can always sign
    /// in. Every removal passes `--yes`, which the server asks for before it
    /// removes an instance's last user account.
    pub fn prune_others(&self, keep: &str) -> Result<Vec<String>, String> {
        let apps = self.list(Kind::App)?;
        if !apps.iter().any(|name| name == keep) {
            return Err(format!(
                "refusing to prune: {keep} is not an app-internal account on instance {}",
                self.instance
            ));
        }
        let users = self.list(Kind::User)?;
        let mut removed = Vec::new();
        for name in users.into_iter().chain(apps.into_iter().filter(|name| name != keep)) {
            self.run(&["rm", &name, "--yes"], None)?;
            removed.push(name);
        }
        Ok(removed)
    }
}

/// [`std::process::Child::wait_with_output`], but a command still running
/// after `limit` is killed — by its own pid, through its handle — and is an
/// error.
fn wait_bounded(mut child: Child, limit: Duration) -> std::io::Result<Output> {
    fn drain(pipe: Option<impl Read + Send + 'static>) -> Option<std::thread::JoinHandle<Vec<u8>>> {
        pipe.map(|mut pipe| {
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                let _ = pipe.read_to_end(&mut bytes);
                bytes
            })
        })
    }
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let deadline = Instant::now() + limit;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("it did not finish within {} s and was stopped", limit.as_secs()),
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let collect = |handle: Option<std::thread::JoinHandle<Vec<u8>>>| handle.and_then(|handle| handle.join().ok()).unwrap_or_default();
    Ok(Output {
        status,
        stdout: collect(stdout),
        stderr: collect(stderr),
    })
}

/// `text` with every one-time sign-in token (`local-open=<token>`, which
/// `open --print` prints) cut out, for a log.
pub fn redact(text: &str) -> String {
    const KEY: &str = "local-open=";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(KEY) {
        let (kept, after) = rest.split_at(at + KEY.len());
        out.push_str(kept);
        out.push('…');
        let end = after
            .find(|c: char| c.is_whitespace() || matches!(c, '&' | '"' | '\'' | '<' | '>'))
            .unwrap_or(after.len());
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

/// The app's login, made ready before the server starts ([`ready_login`]).
#[derive(Debug)]
pub struct Ready {
    /// The login to sign in with: always [`APP_ACCOUNT`]'s.
    pub credential: Credential,
    /// What happened to the account.
    pub ensured: Ensured,
    /// Where a new password was kept; `None` when the stored one was used.
    pub kept_in: Option<&'static str>,
    /// The stored login's account this one replaces, if it was another
    /// name's: remove it with [`Cli::retire`] once the new login has signed
    /// in, and not before.
    pub replaces: Option<String>,
}

/// Make the app's login ready from the stored one, `stored`, keeping a new
/// one with `keep`:
///
/// * [`APP_ACCOUNT`]'s — made sure of, its password left as it is;
/// * none — a first launch (or a secret that is gone): [`APP_ACCOUNT`] with a
///   new password;
/// * another name's — an install from before the app's account had a name of
///   its own (it took the macOS login name, the one a person wants for their
///   own account): [`APP_ACCOUNT`] with a new password, kept in place of the
///   old login.
///
/// The old account — the stored login's, or, with nothing stored and no
/// [`APP_ACCOUNT`] yet, an account named `login_name` ([`legacy_username`]:
/// an earlier build's login whose secret could not be read) — is handed to
/// `record` before anything changes, so the caller can finish with it even if
/// this start is cut short. Then, if it is a user account (the earliest
/// builds made it one), it is made app-internal: nobody knows its password,
/// and as a user account it would hold the instance behind a login page that
/// nobody can pass, where an app-internal one leaves it in its setup state
/// (which *Open in Browser* opens). It is not removed here: the caller removes
/// it ([`Cli::retire`]) once the new login has signed in.
///
/// Once [`APP_ACCOUNT`] exists the migration is done, and a **user** account
/// of the old name is never touched — not the login name with nothing
/// stored, nor a stored login of another name (a stale keychain item the
/// last save could not replace): it may be the person's own by then. An
/// app-internal one of that name is the app's leftover, still to be removed.
pub fn ready_login(
    cli: &Cli,
    stored: Option<Credential>,
    login_name: &str,
    record: impl FnOnce(&str),
    keep: impl FnOnce(&Credential) -> Result<&'static str, String>,
) -> Result<Ready, String> {
    if let Some(credential) = stored.as_ref().filter(|credential| credential.username == APP_ACCOUNT) {
        let ensured = cli.ensure(credential, false)?;
        return Ok(Ready {
            credential: credential.clone(),
            ensured,
            kept_in: None,
            replaces: None,
        });
    }
    let replaces = match stored {
        Some(old) => old_login(cli, old.username)?,
        None => legacy_account(cli, login_name)?,
    };
    if let Some(old) = replaces.as_deref() {
        record(old);
        if cli.list(Kind::User)?.iter().any(|name| name == old) {
            cli.run(&["mark-app", old], None)?;
        }
    }
    let credential = Credential {
        username: APP_ACCOUNT.to_string(),
        password: generate_password().map_err(|error| format!("Droppen could not make a password: {error}"))?,
    };
    let ensured = cli.ensure(&credential, true)?;
    let kept_in = keep(&credential)?;
    Ok(Ready {
        credential,
        ensured,
        kept_in: Some(kept_in),
        replaces,
    })
}

/// The account a stored login of another name than [`APP_ACCOUNT`] stands
/// for, to replace: that account — unless [`APP_ACCOUNT`] exists already and
/// it is a user account, which is then left alone (the stored login is stale,
/// and the name may be a person's own account since).
fn old_login(cli: &Cli, old: String) -> Result<Option<String>, String> {
    let apps = cli.list(Kind::App)?;
    if apps.iter().any(|name| name == APP_ACCOUNT) && !apps.iter().any(|name| *name == old) {
        if cli.list(Kind::User)?.iter().any(|name| *name == old) {
            log::warn!("the stored login is {old}, but {APP_ACCOUNT} exists and {old} is a user account: leaving it alone");
        }
        return Ok(None);
    }
    Ok(Some(old))
}

/// With no stored login: `login_name`, if it is an account on an instance
/// that has no [`APP_ACCOUNT`] yet — an earlier build's login, whose secret
/// could not be read — else nobody.
fn legacy_account(cli: &Cli, login_name: &str) -> Result<Option<String>, String> {
    if login_name == APP_ACCOUNT {
        return Ok(None);
    }
    let apps = cli.list(Kind::App)?;
    if apps.iter().any(|name| name == APP_ACCOUNT) {
        return Ok(None);
    }
    let found = apps.iter().any(|name| name == login_name) || cli.list(Kind::User)?.iter().any(|name| name == login_name);
    Ok(found.then(|| login_name.to_string()))
}

/// What [`Cli::retire`] did with the old login's account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retired {
    /// It was app-internal, and is gone now.
    Removed,
    /// There was no account of that name.
    Gone,
    /// It is a user account — maybe a person's own — and was left alone.
    Kept,
}

/// Which accounts a `user list` asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Accounts people sign in with (`user list`).
    User,
    /// App-internal accounts, like the app's own (`user list --app`).
    App,
}

/// What [`Cli::ensure`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ensured {
    /// The app-internal account was already there; nothing changed.
    Existed,
    /// The account was created, app-internal.
    Created,
    /// The app-internal account was there and its password was set.
    PasswordReset,
    /// The account was there as a user account and is now app-internal
    /// (`user mark-app`); its password was set only if `password_reset`.
    Migrated {
        /// Whether its password was set too.
        password_reset: bool,
    },
}

/// The names in `user list` (or `user list --app`) output: nothing for the
/// answer that starts "no accounts", otherwise the first tab-separated field
/// of every line after the heading.
pub fn parse_list(stdout: &str) -> Vec<String> {
    if stdout.trim_start().starts_with("no accounts") {
        return Vec::new();
    }
    stdout
        .lines()
        .skip(1)
        .filter_map(|line| line.split('\t').next())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    /// A stand-in server binary implementing `user list [--app]|add [--app]|
    /// passwd|mark-app|rm [--yes]` against a text file of `name<TAB>kind`
    /// lines (kind `user` or `app`), recording each password it is given on
    /// stdin in `passwords.txt` and each command line in `calls.txt`. Like
    /// the server, `rm` of the last user account needs `--yes`, and `--app`
    /// means nothing to `passwd`, `rm` or `mark-app`. It hangs while
    /// `server-hangs` exists.
    fn fake_server(dir: &Path) -> PathBuf {
        let path = dir.join("remote-hand-server");
        let body = r#"#!/bin/sh
db="$PWD/users.txt"
cmd="$1"
shift
case "$cmd" in user) ;; *) exit 64 ;; esac
[ "$APP_INSTANCE" = droppen-app ] || { echo "wrong instance env: $APP_INSTANCE" >&2; exit 65; }
case "$*" in *"--instance droppen-app"*) ;; *) echo "no --instance" >&2; exit 66 ;; esac
echo "$cmd $*" >> "$PWD/calls.txt"
[ -e "$PWD/server-hangs" ] && exec sleep 5
app=no; yes=no
for arg in "$@"; do
  case "$arg" in --app) app=yes ;; --yes) yes=yes ;; esac
done
touch "$db"
usage() { echo "USAGE: remote-hand-server user …" >&2; exit 2; }
has() { awk -F '\t' -v n="$1" '$1 == n { found = 1 } END { exit !found }' "$db"; }
kind_of() { awk -F '\t' -v n="$1" '$1 == n { print $2 }' "$db"; }
user_count() { awk -F '\t' '$2 == "user"' "$db" | wc -l | tr -d ' '; }
if [ $app = yes ]; then kind=app; else kind=user; fi
case "$1" in
  list)
    names=$(awk -F '\t' -v k="$kind" '$2 == k { print $1 }' "$db")
    if [ -z "$names" ]; then
      if [ $app = yes ]; then echo "no accounts of the desktop app on instance droppen-app (x)"
      else echo "no accounts on instance droppen-app (x) — open its page with the setup code"; fi
      exit 0
    fi
    if [ $app = yes ]; then echo "app-internal accounts on instance droppen-app (x):"; else echo "accounts on instance droppen-app (x):"; fi
    for name in $names; do printf '%s\tcreated 1\tpassword changed 1\n' "$name"; done
    ;;
  add)
    has "$2" && { echo "user $2 already exists" >&2; exit 1; }
    read -r password; echo "$password" >> "$PWD/passwords.txt"
    printf '%s\t%s\n' "$2" "$kind" >> "$db"
    echo "added $2 to instance droppen-app"
    ;;
  passwd)
    [ $app = no ] || usage
    has "$2" || { echo "no such user: $2" >&2; exit 1; }
    read -r password; echo "$password" >> "$PWD/passwords.txt"
    echo "changed the password for $2"
    ;;
  mark-app)
    [ $app = no ] || usage
    has "$2" || { echo "no such user: $2" >&2; exit 1; }
    awk -F '\t' -v OFS='\t' -v n="$2" '$1 == n { $2 = "app" } { print }' "$db" > "$db.next"; mv "$db.next" "$db"
    echo "$2 on instance droppen-app is now an app-internal account"
    ;;
  rm)
    [ $app = no ] || usage
    has "$2" || { echo "no such user: $2" >&2; exit 1; }
    if [ "$(kind_of "$2")" = user ] && [ "$(user_count)" = 1 ] && [ $yes = no ]; then
      echo "$2 is the last user account on instance droppen-app: pass --yes" >&2; exit 1
    fi
    awk -F '\t' -v n="$2" '$1 != n' "$db" > "$db.next"; mv "$db.next" "$db"
    echo "removed $2 from instance droppen-app"
    ;;
  *) usage ;;
esac
"#;
        std::fs::write(&path, body).expect("write");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }

    fn cli(dir: &Path) -> Cli {
        Cli {
            program: fake_server(dir),
            env: vec![("PATH".into(), "/usr/bin:/bin".into()), ("APP_INSTANCE".into(), "droppen-app".into())],
            cwd: dir.to_path_buf(),
            instance: "droppen-app".into(),
            log: Some(dir.join("server.log")),
        }
    }

    fn credential(username: &str, password: &str) -> Credential {
        Credential {
            username: username.into(),
            password: password.into(),
        }
    }

    /// Accounts already on the instance, as `name<TAB>kind` lines.
    fn seed(dir: &Path, accounts: &[(&str, &str)]) {
        let lines: String = accounts.iter().map(|(name, kind)| format!("{name}\t{kind}\n")).collect();
        std::fs::write(dir.join("users.txt"), lines).expect("seed");
    }

    fn read(dir: &Path, file: &str) -> String {
        std::fs::read_to_string(dir.join(file)).unwrap_or_default()
    }

    /// A store standing in for the keychain: what was kept, in order.
    #[derive(Default)]
    struct Kept(RefCell<Vec<Credential>>);

    impl Kept {
        fn keep(&self) -> impl FnOnce(&Credential) -> Result<&'static str, String> + '_ {
            |credential| {
                self.0.borrow_mut().push(credential.clone());
                Ok("test store")
            }
        }

        fn all(&self) -> Vec<Credential> {
            self.0.borrow().clone()
        }
    }

    fn never_kept(_: &Credential) -> Result<&'static str, String> {
        panic!("nothing new to keep")
    }

    fn never_recorded(old: &str) {
        panic!("nothing to replace, but {old} was recorded")
    }

    /// The login name of the person in these tests.
    const LOGIN: &str = "ada";

    /// What `record` was handed, with the commands run by then.
    #[derive(Default)]
    struct Recorded(RefCell<Vec<(String, String)>>);

    impl Recorded {
        fn record<'a>(&'a self, dir: &'a Path) -> impl FnOnce(&str) + 'a {
            move |old| self.0.borrow_mut().push((old.to_string(), read(dir, "calls.txt")))
        }

        fn names(&self) -> Vec<String> {
            self.0.borrow().iter().map(|(name, _)| name.clone()).collect()
        }

        /// The commands that had run when the first name was recorded.
        fn calls_by_then(&self) -> String {
            self.0.borrow().first().map(|(_, calls)| calls.clone()).unwrap_or_default()
        }
    }

    #[test]
    fn the_old_name_is_the_login_name_as_the_server_allows_it() {
        assert_eq!(legacy_username(Some("Ada")), "ada");
        assert_eq!(legacy_username(Some("ada.lovelace")), "ada.lovelace");
        assert_eq!(legacy_username(Some("José Ñ")), "jos");
        assert_eq!(legacy_username(Some(&"x".repeat(40))).len(), 32);
        assert_eq!(legacy_username(Some("")), "me");
        assert_eq!(legacy_username(None), "me");
    }

    #[test]
    fn the_list_parser_reads_names_and_the_empty_answer() {
        assert!(parse_list("no accounts on instance desktop (/x/users.json) — its API is open\n").is_empty());
        assert!(parse_list("no accounts of the desktop app on instance droppen-app (/x/users.json)\n").is_empty());
        let listing = "accounts on instance desktop (/x):\nada\tcreated 2026\tpassword changed 2026\nbob\tcreated 2026\tpassword changed 2026\n";
        assert_eq!(parse_list(listing), vec!["ada", "bob"]);
        let apps = "app-internal accounts on instance droppen-app (/x):\ndroppen-app\tcreated 2026\tpassword changed 2026\n";
        assert_eq!(parse_list(apps), vec!["droppen-app"]);
    }

    #[test]
    fn a_first_launch_makes_the_app_internal_droppen_app_account_with_the_password_on_stdin() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cli = cli(dir.path());
        let kept = Kept::default();
        let ready = ready_login(&cli, None, LOGIN, never_recorded, kept.keep()).expect("ready");
        assert_eq!(ready.credential.username, APP_ACCOUNT);
        assert_eq!(ready.ensured, Ensured::Created);
        assert_eq!(ready.kept_in, Some("test store"));
        assert_eq!(ready.replaces, None);
        assert_eq!(kept.all(), vec![ready.credential.clone()]);
        assert_eq!(cli.list(Kind::App).expect("list"), vec![APP_ACCOUNT]);
        assert_eq!(cli.list(Kind::User).expect("list"), Vec::<String>::new(), "not a user account");
        let calls = read(dir.path(), "calls.txt");
        assert!(calls.contains("user add droppen-app --app --instance droppen-app"), "{calls}");
        assert_eq!(read(dir.path(), "passwords.txt"), format!("{}\n", ready.credential.password));
        assert!(
            !read(dir.path(), "server.log").contains(&ready.credential.password),
            "the password is never logged"
        );
        assert!(!calls.contains(&ready.credential.password), "nor put on the command line");
    }

    #[test]
    fn a_later_launch_uses_the_stored_login_as_it_is() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed(dir.path(), &[(APP_ACCOUNT, "app")]);
        let cli = cli(dir.path());
        let stored = credential(APP_ACCOUNT, "stored-pass-123456");
        let ready = ready_login(&cli, Some(stored.clone()), LOGIN, never_recorded, never_kept).expect("ready");
        assert_eq!(ready.credential, stored);
        assert_eq!(ready.ensured, Ensured::Existed);
        assert_eq!((ready.kept_in, ready.replaces), (None, None));
        assert_eq!(read(dir.path(), "passwords.txt"), "", "the password is left as it was");
    }

    #[test]
    fn a_stored_login_whose_account_is_gone_gets_it_back() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cli = cli(dir.path());
        let stored = credential(APP_ACCOUNT, "stored-pass-123456");
        let ready = ready_login(&cli, Some(stored), LOGIN, never_recorded, never_kept).expect("ready");
        assert_eq!(ready.ensured, Ensured::Created);
        assert_eq!(read(dir.path(), "passwords.txt"), "stored-pass-123456\n");
    }

    #[test]
    fn a_new_password_is_set_on_the_app_account_when_asked() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed(dir.path(), &[(APP_ACCOUNT, "app")]);
        let cli = cli(dir.path());
        assert_eq!(
            cli.ensure(&credential(APP_ACCOUNT, "first-pass-123456"), false).expect("again"),
            Ensured::Existed
        );
        assert_eq!(
            cli.ensure(&credential(APP_ACCOUNT, "second-pass-654321"), true).expect("reset"),
            Ensured::PasswordReset
        );
        assert_eq!(read(dir.path(), "passwords.txt"), "second-pass-654321\n");
        assert_eq!(cli.list(Kind::App).expect("list"), vec![APP_ACCOUNT], "still app-internal");
        assert!(!read(dir.path(), "calls.txt").contains("mark-app"));
    }

    #[test]
    fn a_login_named_after_the_login_name_moves_to_droppen_app_and_the_old_account_goes_after() {
        // The app's account was app-internal but took the macOS login name;
        // `bob` is a person's own account and is never touched.
        let dir = tempfile::tempdir().expect("tempdir");
        seed(dir.path(), &[("ada", "app"), ("bob", "user")]);
        let cli = cli(dir.path());
        let kept = Kept::default();
        let recorded = Recorded::default();
        let stored = Some(credential("ada", "old-pass-123456"));
        let ready = ready_login(&cli, stored, LOGIN, recorded.record(dir.path()), kept.keep()).expect("ready");
        assert_eq!(recorded.names(), vec!["ada"], "the old account is recorded for removal");
        assert_eq!(ready.credential.username, APP_ACCOUNT);
        assert_ne!(ready.credential.password, "old-pass-123456", "a new password");
        assert_eq!(ready.ensured, Ensured::Created);
        assert_eq!(ready.replaces.as_deref(), Some("ada"));
        assert_eq!(kept.all(), vec![ready.credential.clone()], "the new login replaces the stored one");
        assert_eq!(
            cli.list(Kind::App).expect("list"),
            vec!["ada", APP_ACCOUNT],
            "the old account stays until the new login has signed in"
        );

        // Signed in: now the old one goes, and only it.
        assert_eq!(cli.retire("ada").expect("retire"), Retired::Removed);
        assert_eq!(cli.list(Kind::App).expect("list"), vec![APP_ACCOUNT]);
        assert_eq!(cli.list(Kind::User).expect("list"), vec!["bob"]);
        let calls = read(dir.path(), "calls.txt");
        assert!(calls.contains("user rm ada --yes --instance droppen-app"), "{calls}");
        assert!(!calls.contains("rm bob") && !calls.contains("mark-app"), "{calls}");
        assert_eq!(cli.retire("ada").expect("again"), Retired::Gone, "already gone");
    }

    #[test]
    fn a_review_build_user_account_login_is_made_app_internal_at_once_and_removed_after() {
        // The earliest builds made the app's login an ordinary user account,
        // the instance's only one. Nobody knows its password: left a user
        // account, it would keep every browser on a login page nobody can
        // pass, so it is made app-internal before anything else changes —
        // after it is recorded for removal, which comes once the new login
        // has signed in.
        let dir = tempfile::tempdir().expect("tempdir");
        seed(dir.path(), &[("ada", "user")]);
        let cli = cli(dir.path());
        let kept = Kept::default();
        let recorded = Recorded::default();
        let stored = Some(credential("ada", "old-pass-123456"));
        let ready = ready_login(&cli, stored, LOGIN, recorded.record(dir.path()), kept.keep()).expect("ready");
        assert_eq!(ready.ensured, Ensured::Created);
        assert_eq!(ready.replaces.as_deref(), Some("ada"));
        assert_eq!(recorded.names(), vec!["ada"]);
        assert!(!recorded.calls_by_then().contains("mark-app"), "recorded first");
        assert_eq!(cli.list(Kind::User).expect("list"), Vec::<String>::new(), "no user account nobody can sign in to");
        assert_eq!(cli.list(Kind::App).expect("list"), vec!["ada", APP_ACCOUNT]);
        let calls = read(dir.path(), "calls.txt");
        let marked = calls.find("user mark-app ada --instance droppen-app").expect("marked app-internal");
        assert!(marked < calls.find("user add droppen-app").expect("added"), "before the new account: {calls}");
        assert_eq!(cli.retire("ada").expect("retire"), Retired::Removed);
        assert_eq!(cli.list(Kind::App).expect("list"), vec![APP_ACCOUNT]);
    }

    #[test]
    fn an_old_login_whose_secret_could_not_be_read_is_found_by_its_name() {
        // Nothing stored (the keychain would not give the old login up) and no
        // droppen-app yet: an account of the login name is the old login.
        let dir = tempfile::tempdir().expect("tempdir");
        seed(dir.path(), &[("ada", "user"), ("bob", "user")]);
        let cli = cli(dir.path());
        let recorded = Recorded::default();
        let ready = ready_login(&cli, None, LOGIN, recorded.record(dir.path()), Kept::default().keep()).expect("ready");
        assert_eq!(ready.credential.username, APP_ACCOUNT);
        assert_eq!(ready.replaces.as_deref(), Some("ada"));
        assert_eq!(recorded.names(), vec!["ada"]);
        assert!(!recorded.calls_by_then().contains("mark-app"), "recorded first");
        assert_eq!(cli.list(Kind::User).expect("list"), vec!["bob"], "a person's account is left alone");
        assert_eq!(cli.list(Kind::App).expect("list"), vec!["ada", APP_ACCOUNT]);
        assert_eq!(cli.retire("ada").expect("retire"), Retired::Removed);
        assert_eq!(cli.list(Kind::User).expect("list"), vec!["bob"]);
    }

    #[test]
    fn once_droppen_app_exists_an_account_of_the_login_name_is_the_persons() {
        // The migration is done; the person has since made their own account
        // under their login name. A start with nothing stored resets the app's
        // password and leaves that account alone.
        let dir = tempfile::tempdir().expect("tempdir");
        seed(dir.path(), &[(APP_ACCOUNT, "app"), ("ada", "user")]);
        let cli = cli(dir.path());
        let ready = ready_login(&cli, None, LOGIN, never_recorded, Kept::default().keep()).expect("ready");
        assert_eq!(ready.ensured, Ensured::PasswordReset);
        assert_eq!(ready.replaces, None);
        assert_eq!(cli.list(Kind::User).expect("list"), vec!["ada"]);
        assert!(!read(dir.path(), "calls.txt").contains("mark-app"));
    }

    #[test]
    fn a_stale_stored_login_never_touches_a_persons_account_of_that_name() {
        // Migrated already: the keychain would not take the new login, so it
        // went to the file and the old item stayed — and is read first. The
        // old account was removed, and the person has since made their own
        // `ada`. Nothing may happen to it.
        let dir = tempfile::tempdir().expect("tempdir");
        seed(dir.path(), &[(APP_ACCOUNT, "app"), ("ada", "user")]);
        let cli = cli(dir.path());
        let kept = Kept::default();
        let stored = Some(credential("ada", "stale-pass-123456"));
        let ready = ready_login(&cli, stored, LOGIN, never_recorded, kept.keep()).expect("ready");
        assert_eq!(ready.credential.username, APP_ACCOUNT, "the app signs in as its own account");
        assert_eq!(ready.ensured, Ensured::PasswordReset);
        assert_eq!(ready.replaces, None, "nothing to replace");
        assert_eq!(cli.list(Kind::User).expect("list"), vec!["ada"], "the person's account is untouched");
        let calls = read(dir.path(), "calls.txt");
        assert!(!calls.contains("mark-app") && !calls.contains(" rm "), "{calls}");

        // An app-internal leftover of that name is still the app's to remove.
        seed(dir.path(), &[(APP_ACCOUNT, "app"), ("ada", "app")]);
        let recorded = Recorded::default();
        let ready = ready_login(&cli, Some(credential("ada", "stale")), LOGIN, recorded.record(dir.path()), kept.keep()).expect("ready");
        assert_eq!(ready.replaces.as_deref(), Some("ada"));
        assert_eq!(recorded.names(), vec!["ada"]);
    }

    #[test]
    fn retiring_leaves_a_user_account_alone() {
        // A removal recorded for `ada` and never forgotten (state.json could
        // not be saved) meets the person's own `ada`: left alone.
        let dir = tempfile::tempdir().expect("tempdir");
        seed(dir.path(), &[(APP_ACCOUNT, "app"), ("ada", "user")]);
        let cli = cli(dir.path());
        assert_eq!(cli.retire("ada").expect("retire"), Retired::Kept);
        assert_eq!(cli.list(Kind::User).expect("list"), vec!["ada"]);
        assert!(!read(dir.path(), "calls.txt").contains(" rm "));
    }

    #[test]
    fn retiring_never_touches_the_apps_own_account_or_runs_without_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed(dir.path(), &[("ada", "app")]);
        let cli = cli(dir.path());
        let error = cli.retire("ada").expect_err("no droppen-app yet");
        assert!(error.contains("not an app-internal account"), "{error}");
        assert_eq!(cli.list(Kind::App).expect("list"), vec!["ada"], "nothing removed");
        seed(dir.path(), &[("ada", "app"), (APP_ACCOUNT, "app")]);
        assert!(cli.retire(APP_ACCOUNT).is_err());
        assert_eq!(cli.list(Kind::App).expect("list"), vec!["ada", APP_ACCOUNT]);
    }

    #[test]
    fn a_user_account_with_the_apps_name_becomes_the_apps_with_a_new_password() {
        // An account made before the app's login was app-internal can carry the
        // app's name; it is made the app's on this instance only, rather than
        // failing to add.
        let dir = tempfile::tempdir().expect("tempdir");
        seed(dir.path(), &[(APP_ACCOUNT, "user")]);
        let cli = cli(dir.path());
        let ready = ready_login(&cli, None, LOGIN, never_recorded, Kept::default().keep()).expect("ready");
        assert_eq!(ready.ensured, Ensured::Migrated { password_reset: true });
        assert_eq!(cli.list(Kind::App).expect("list"), vec![APP_ACCOUNT]);
        assert_eq!(read(dir.path(), "passwords.txt"), format!("{}\n", ready.credential.password));
    }

    #[test]
    fn a_new_instance_keeps_only_the_apps_account() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Left by an older build's first start: people's logins, and an
        // app-internal account that is not this app's.
        seed(
            dir.path(),
            &[("someone-else", "user"), ("ada", "user"), ("third", "user"), ("stray-app", "app")],
        );
        let cli = cli(dir.path());
        ready_login(&cli, None, "carol", never_recorded, Kept::default().keep()).expect("ready");
        let removed = cli.prune_others(APP_ACCOUNT).expect("prune");
        assert_eq!(removed, vec!["someone-else", "ada", "third", "stray-app"]);
        assert_eq!(cli.list(Kind::App).expect("list"), vec![APP_ACCOUNT]);
        assert_eq!(cli.list(Kind::User).expect("list"), Vec::<String>::new());
        // The last user account needs --yes, and got it.
        assert!(read(dir.path(), "calls.txt").contains("user rm third --yes --instance droppen-app"));
        assert!(cli.prune_others("nobody").is_err(), "never prunes when the keeper is missing");
        assert_eq!(cli.list(Kind::App).expect("list"), vec![APP_ACCOUNT]);
    }

    #[test]
    fn pruning_refuses_while_the_keeper_is_a_user_account() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed(dir.path(), &[("someone-else", "user"), (APP_ACCOUNT, "user")]);
        let cli = cli(dir.path());
        let error = cli.prune_others(APP_ACCOUNT).expect_err("refuses");
        assert!(error.contains("not an app-internal account"), "{error}");
        assert_eq!(cli.list(Kind::User).expect("list"), vec!["someone-else", APP_ACCOUNT], "nothing removed");
    }

    #[test]
    fn a_failing_command_is_an_error_with_its_message() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut cli = cli(dir.path());
        cli.instance = "other".into();
        let error = cli.ensure(&credential(APP_ACCOUNT, "whatever-123456"), false).expect_err("fails");
        assert!(error.contains("no --instance"), "{error}");
    }

    #[test]
    fn a_command_that_hangs_is_stopped_at_its_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("server-hangs"), "").expect("hangs");
        let cli = cli(dir.path());
        let started = Instant::now();
        let error = cli.command("user", &["list"], None, Some(Duration::from_millis(300))).expect_err("times out");
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    }

    #[test]
    fn redaction_cuts_out_every_token_and_nothing_else() {
        assert_eq!(redact("no token here\n"), "no token here\n");
        assert_eq!(
            redact("open http://127.0.0.1:1/#local-open=abc-123 now\nand #local-open=xyz"),
            "open http://127.0.0.1:1/#local-open=… now\nand #local-open=…"
        );
        assert_eq!(redact("\"#local-open=abc\" &local-open=d&e"), "\"#local-open=…\" &local-open=…&e");
    }
}
