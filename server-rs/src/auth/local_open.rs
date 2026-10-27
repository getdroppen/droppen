//! The local capability: opening the page as this device's owner.
//!
//! An instance with no user account has to let *somebody* in without a
//! password, or nobody could ever make the first account from the page. The
//! answer is not "whoever connects from 127.0.0.1": a TCP forwarder (`ssh -R`,
//! frp, a proxy "shared with the LAN") makes a stranger's connection arrive from
//! loopback, with whatever `Host` header the stranger likes. It is "whoever is
//! the operating-system user this server runs as" — the user the AI CLIs run
//! as, who can read every file the page could show anyway — and the operating
//! system is what proves it:
//!
//! * the server that owns its instance listens on a **unix socket** in
//!   `<instance>/control/` (the directory `0700`, the socket `0600`, and every
//!   connection's peer uid checked against the server's own), which nobody but
//!   that user can reach;
//! * `remote-hand-server open` asks it for a **one-time token** — 256 random
//!   bits, good for one use within [`TTL`], kept only as its sha-256 in memory;
//! * the server writes a redirect page for it (`0600`, in the same `0700`
//!   directory) that sends the browser to `http://127.0.0.1:<port>/#local-open=<token>`,
//!   so the token never sits in a process's arguments where another user's `ps`
//!   could read it, and deletes that page the moment the token is spent or
//!   expires;
//! * the page takes the token out of its address before anything else runs and
//!   exchanges it, in a request body, for an ordinary session of
//!   [`super::LOCAL_OWNER`] (`POST /api/auth/local-open`) — so a reload, or a
//!   restart of the server, keeps the page signed in.
//!
//! Only while the instance has no user account: with one, `open` just opens the
//! login page, and every local-owner session is already gone
//! ([`super::Auth::still_valid`]).
//!
//! `open` believes nothing but this user's own server ([`ask`]): the socket's
//! directory has to be this user's `0700` one and the peer this user — the
//! `/tmp` fallback's name is predictable, and somebody else could make it
//! first — and the answer has to be a loopback page on the port this instance
//! recorded, a link to it, and a redirect page that is this user's own file in
//! [`control_dir`]. The page's address is the listener's bound address as a
//! literal, never a configured host name the local network could answer for.
//!
//! What the socket does not stop: **any process of the same operating-system
//! user** can ask it — the AI CLIs this server runs included, unless their
//! sandbox keeps them from arbitrary unix sockets. A sandbox for them should
//! deny `<instance>/control/` and `/tmp/remote-hand-<uid>-*/`.
//!
//! **Not on the desktop app's instance.** That instance never leaves its setup
//! state — its only account, the app's own (`droppen-app`), is app-internal —
//! so a socket that answered there would hand an owner session to any process
//! of that macOS user, for as long as the app runs, where before this the
//! instance asked for the password in the app's keychain item. Its server
//! (`crate::desktop_app`) answers the socket with a refusal and mints nothing
//! there; the app's *Open in Browser* asks for the token over HTTP instead,
//! signed in as its own account (`POST /api/auth/local-open/grant`,
//! [`crate::auth::routes::local_open_grant`]) — the same password as before.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::sessions::Paths;

use super::Auth;

/// How long a token is good for.
pub const TTL: Duration = Duration::from_secs(60);
/// Random bytes in a token.
const TOKEN_BYTES: usize = 32;
/// Live tokens at once. Minting is only reachable over the control socket, so
/// this bounds a loop, not an attacker.
const MAX_LIVE: usize = 16;
/// The longest request line the socket reads.
const MAX_LINE: u64 = 512;
/// How long a connection may take to say what it wants.
const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// The fragment key the page reads the token from (`web/src/lib/auth.ts`).
pub const FRAGMENT_KEY: &str = "local-open";

/// `<instance>/control/`: the socket and the redirect pages, `0700`.
pub fn control_dir(paths: &Paths) -> PathBuf {
    paths.instance_dir().join("control")
}

/// Where the control socket is: in [`control_dir`] when the address fits a
/// `sun_path`, else in the instance's `0700` directory under `/tmp`
/// ([`crate::pty_shim::sidecar::socket_path`]).
pub fn socket_path(paths: &Paths) -> PathBuf {
    crate::pty_shim::sidecar::socket_path(&control_dir(paths), "local-open")
}

struct Live {
    expires: Instant,
    /// The redirect page written for it, deleted with it.
    page: Option<PathBuf>,
}

/// The live tokens, by digest.
#[derive(Default)]
pub struct LocalOpen {
    tokens: Mutex<HashMap<String, Live>>,
}

impl std::fmt::Debug for LocalOpen {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("LocalOpen").finish_non_exhaustive()
    }
}

fn digest(token: &str) -> String {
    crate::text::sha256_hex(token)
}

impl LocalOpen {
    /// How many tokens are live, for tests.
    #[cfg(test)]
    pub(crate) fn live(&self) -> usize {
        self.tokens.lock().expect("local-open lock").len()
    }

    /// A new token, or `None` with [`MAX_LIVE`] already live.
    pub fn mint(&self) -> Option<String> {
        self.mint_at(Instant::now())
    }

    fn mint_at(&self, now: Instant) -> Option<String> {
        let mut tokens = self.tokens.lock().expect("local-open lock");
        sweep(&mut tokens, now);
        if tokens.len() >= MAX_LIVE {
            return None;
        }
        let mut bytes = [0u8; TOKEN_BYTES];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        tokens.insert(
            digest(&token),
            Live {
                expires: now + TTL,
                page: None,
            },
        );
        Some(token)
    }

    /// Remember the redirect page written for `token`, to delete with it.
    fn attach_page(&self, token: &str, page: PathBuf) {
        if let Some(live) = self.tokens.lock().expect("local-open lock").get_mut(&digest(token)) {
            live.page = Some(page);
        }
    }

    /// Spend `token`: `true` once, within [`TTL`]; never again.
    pub fn redeem(&self, token: &str) -> bool {
        self.redeem_at(token, Instant::now())
    }

    fn redeem_at(&self, token: &str, now: Instant) -> bool {
        let mut tokens = self.tokens.lock().expect("local-open lock");
        sweep(&mut tokens, now);
        match tokens.remove(&digest(token)) {
            Some(live) => {
                if let Some(page) = live.page {
                    let _ = std::fs::remove_file(page);
                }
                live.expires > now
            }
            None => false,
        }
    }

    /// Forget what has expired, and its redirect pages.
    fn sweep_now(&self) {
        sweep(&mut self.tokens.lock().expect("local-open lock"), Instant::now());
    }
}

fn sweep(tokens: &mut HashMap<String, Live>, now: Instant) {
    tokens.retain(|_, live| {
        let keep = live.expires > now;
        if !keep {
            if let Some(page) = live.page.take() {
                let _ = std::fs::remove_file(page);
            }
        }
        keep
    });
}

/// What the control socket answers `open` with.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OpenAnswer {
    /// The page's address, with nothing secret in it.
    pub page: String,
    /// The page's address with the token in its fragment; `None` when the
    /// instance has an account (log in instead).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// A `0600` page that redirects there, for a browser to open without the
    /// token on anybody's command line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect: Option<PathBuf>,
    /// The instance has a user account: there is nothing to open but the
    /// login page.
    #[serde(default)]
    pub accounts: bool,
    /// Why there is no token, when there is none for a reason other than
    /// accounts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The page's address, for a browser on this device: the address the
/// listener actually bound, as a literal — loopback for an unspecified one —
/// and never a host *name* from the configuration, which the local network's
/// DNS (`mymac.local`) could answer for somebody else.
pub fn page_url(bound: std::net::SocketAddr) -> String {
    let port = bound.port();
    match bound.ip() {
        std::net::IpAddr::V4(v4) if v4.is_unspecified() => format!("http://127.0.0.1:{port}/"),
        std::net::IpAddr::V6(v6) if v6.is_unspecified() => format!("http://[::1]:{port}/"),
        std::net::IpAddr::V4(v4) => format!("http://{v4}:{port}/"),
        std::net::IpAddr::V6(v6) => format!("http://[{v6}]:{port}/"),
    }
}

/// Make `dir` a `0700` directory, whatever it was.
fn private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Listen on the control socket until `cancel`, answering for `auth`; `page`
/// is this server's address. Only the server that owns its instance calls
/// this ([`crate::app::RemoteHandApp::prepare`]).
///
/// On a server the desktop app runs (`desktop_app`), the socket refuses every
/// request: see the module notes.
pub fn serve(paths: &Paths, auth: Arc<Auth>, page: String, desktop_app: bool, cancel: CancellationToken) -> std::io::Result<PathBuf> {
    let dir = control_dir(paths);
    private_dir(&dir)?;
    let socket = socket_path(paths);
    crate::pty_shim::sidecar::ensure_socket_dir(&socket, &dir)?;
    // A socket left by a server that did not get to remove its own: this one
    // owns the instance now, so nobody is listening on it.
    let _ = std::fs::remove_file(&socket);
    let listener = tokio::net::UnixListener::bind(&socket)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    }
    let path = socket.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = cancel.cancelled() => break,
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else { continue };
                    let (auth, page, dir) = (auth.clone(), page.clone(), dir.clone());
                    tokio::spawn(async move { answer(stream, &auth, &page, &dir, desktop_app).await });
                }
            }
        }
        let _ = std::fs::remove_file(&path);
        auth.local_open.sweep_now();
    });
    Ok(socket)
}

/// Whether the peer of `stream` is the user this server runs as.
fn same_user(stream: &tokio::net::UnixStream) -> bool {
    // SAFETY: `geteuid` reads a process property and cannot fail.
    let me = unsafe { libc::geteuid() };
    stream.peer_cred().is_ok_and(|cred| cred.uid() == me)
}

async fn answer(stream: tokio::net::UnixStream, auth: &Auth, page: &str, dir: &Path, desktop_app: bool) {
    if !same_user(&stream) {
        tracing::warn!("a control-socket connection from another user was refused");
        return;
    }
    let (read, mut write) = stream.into_split();
    let mut line = String::new();
    let mut reader = tokio::io::BufReader::new(tokio::io::AsyncReadExt::take(read, MAX_LINE));
    if tokio::time::timeout(READ_TIMEOUT, reader.read_line(&mut line)).await.is_err() {
        return;
    }
    let reply = match line.trim() {
        "open" if desktop_app => OpenAnswer {
            page: page.to_string(),
            error: Some(DESKTOP_APP_REFUSAL.into()),
            ..OpenAnswer::default()
        },
        "open" => open(auth, page, dir).await,
        _ => OpenAnswer {
            page: page.to_string(),
            error: Some("unknown request".into()),
            ..OpenAnswer::default()
        },
    };
    let mut body = serde_json::to_vec(&reply).unwrap_or_default();
    body.push(b'\n');
    let _ = write.write_all(&body).await;
    let _ = write.shutdown().await;
}

/// Why the desktop app's server opens nothing over its socket.
pub const DESKTOP_APP_REFUSAL: &str =
    "this instance is the Droppen app's: open its page with the app's Open in Browser (its own login asks for it), not over the control socket";

/// The answer to `open`: a token and its redirect page, or why not.
async fn open(auth: &Auth, page: &str, dir: &Path) -> OpenAnswer {
    let mut answer = OpenAnswer {
        page: page.to_string(),
        ..OpenAnswer::default()
    };
    if auth.is_off() {
        answer.error = Some("authentication is off on this instance (auth.enabled = false): the page needs no opening".into());
        return answer;
    }
    if auth.users.has_users().await {
        answer.accounts = true;
        return answer;
    }
    let Some(token) = auth.local_open.mint() else {
        answer.error = Some("too many unused open links: wait a minute and try again".into());
        return answer;
    };
    let url = format!("{page}#{FRAGMENT_KEY}={token}");
    let mut name = [0u8; 8];
    rand::rngs::OsRng.fill_bytes(&mut name);
    let file = dir.join(format!("open-{}.html", crate::text::hex(&name)));
    match write_redirect(&file, &url) {
        Ok(()) => {
            auth.local_open.attach_page(&token, file.clone());
            answer.redirect = Some(file);
        }
        Err(error) => tracing::warn!(%error, "could not write the local-open redirect page"),
    }
    answer.url = Some(url);
    answer
}

/// A page that sends the browser to `url` at once, `0600`.
fn write_redirect(file: &Path, url: &str) -> std::io::Result<()> {
    let quoted = serde_json::to_string(url).unwrap_or_default();
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><meta name=\"referrer\" content=\"no-referrer\"><title>Droppen</title>\n<script>location.replace({quoted})</script>\n"
    );
    super::write_private(file, body.as_bytes())
}

/// `remote-hand-server open`'s half: ask the running server of `paths`'
/// instance for a token — and believe nothing it was not this user's server
/// that said.
///
/// The socket can sit in a `/tmp` directory whose name anybody can predict
/// ([`socket_path`]); another user who makes it first keeps the real server
/// from listening there and can listen there instead. So: the socket's
/// directory must be this user's own `0700` one, the peer at the other end
/// this user, and the answer one this instance's server could give — a
/// loopback page on the port it recorded, a link to that page, and a redirect
/// page directly in [`control_dir`], ours and no symlink. Anything else is a
/// refusal, never something handed to a browser.
pub async fn ask(paths: &Paths) -> Result<OpenAnswer, String> {
    let socket = socket_path(paths);
    if let Some(dir) = socket.parent() {
        private_to_me(dir).map_err(|why| {
            format!(
                "refusing the control socket in {}: {why} — another user may have made it; remove it and restart the server",
                dir.display()
            )
        })?;
    }
    let stream = tokio::net::UnixStream::connect(&socket).await.map_err(|error| {
        format!(
            "no control socket for instance {} at {} ({error}): its server is not running, or could not open the socket (its log says why); start it first",
            paths.instance(),
            socket.display()
        )
    })?;
    if !same_user(&stream) {
        return Err(format!(
            "refusing the control socket at {}: whatever answers there is not this user's server",
            socket.display()
        ));
    }
    let (read, mut write) = stream.into_split();
    write.write_all(b"open\n").await.map_err(|e| e.to_string())?;
    let mut line = String::new();
    let mut reader = tokio::io::BufReader::new(tokio::io::AsyncReadExt::take(read, 64 * 1024));
    tokio::time::timeout(READ_TIMEOUT, reader.read_line(&mut line))
        .await
        .map_err(|_| "the server did not answer".to_string())?
        .map_err(|e| e.to_string())?;
    if line.trim().is_empty() {
        return Err("the server refused the request".into());
    }
    let answer: OpenAnswer = serde_json::from_str(line.trim()).map_err(|e| format!("the server's answer does not read: {e}"))?;
    let port = recorded_port(paths).ok_or_else(|| format!("instance {} has no recorded port: is its server running?", paths.instance()))?;
    check_answer(&answer, port, &control_dir(paths))?;
    Ok(answer)
}

/// The port this instance's server recorded when it bound.
fn recorded_port(paths: &Paths) -> Option<u16> {
    let dirs = crate::instance::InstanceDirs::new(&paths.data_dir(), paths.instance()).ok()?;
    std::fs::read_to_string(dirs.port_file()).ok()?.trim().parse().ok()
}

/// Whether `dir` is a directory of this user's, reachable by nobody else, and
/// not a symlink.
fn private_to_me(dir: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let found = std::fs::symlink_metadata(dir).map_err(|e| e.to_string())?;
    if !found.is_dir() {
        return Err("it is not a directory".into());
    }
    // SAFETY: `geteuid` reads a process property and cannot fail.
    let me = unsafe { libc::geteuid() };
    if found.uid() != me {
        return Err(format!("it belongs to uid {}, not {me}", found.uid()));
    }
    if found.mode() & 0o077 != 0 {
        return Err(format!("its mode is {:04o}: others can reach it", found.mode() & 0o7777));
    }
    Ok(())
}

/// What an answer may say, to be handed to a browser at all: the page on this
/// instance's recorded `port` at a loopback address, a link that is that page
/// with a token in its fragment, and a redirect page that is a regular file of
/// this user's directly in `control` (no symlink), named as the server names
/// them.
fn check_answer(answer: &OpenAnswer, port: u16, control: &Path) -> Result<(), String> {
    let allowed = [format!("http://127.0.0.1:{port}/"), format!("http://[::1]:{port}/")];
    if !allowed.contains(&answer.page) {
        // A server bound to one LAN or Tailscale address (not to all of them)
        // answers with that address: `open` hands a browser loopback pages
        // only, so the way in there is the code, or an account made here.
        return Err(format!(
            "the server's page is not a loopback address on this instance's port {port}, and `open` hands a browser nothing else. \
             If that server is yours and listens on one address only, open its page yourself and make the first account \
             with the setup code (`remote-hand-server setup-code`), or here with `remote-hand-server user add <name>`"
        ));
    }
    if let Some(url) = &answer.url {
        let token = url
            .strip_prefix(answer.page.as_str())
            .and_then(|rest| rest.strip_prefix(&format!("#{FRAGMENT_KEY}=")))
            .ok_or("the server's link is not its page")?;
        let shaped = token.len() == 43 && token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if !shaped {
            return Err("the server's link does not carry a token".into());
        }
    }
    if let Some(redirect) = &answer.redirect {
        use std::os::unix::fs::MetadataExt;
        private_to_me(control).map_err(|why| format!("the control directory {}: {why}", control.display()))?;
        if redirect.parent() != Some(control) {
            return Err("the redirect page is not in the control directory".into());
        }
        let name = redirect.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        let named = name
            .strip_prefix("open-")
            .and_then(|rest| rest.strip_suffix(".html"))
            .is_some_and(|hex| hex.len() == 16 && hex.bytes().all(|b| b.is_ascii_hexdigit()));
        if !named {
            return Err("the redirect page is not one the server writes".into());
        }
        let found = std::fs::symlink_metadata(redirect).map_err(|e| e.to_string())?;
        // SAFETY: `geteuid` reads a process property and cannot fail.
        let me = unsafe { libc::geteuid() };
        if !found.file_type().is_file() || found.uid() != me || found.mode() & 0o077 != 0 {
            return Err("the redirect page is not a private file of this user's".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_works_once_and_only_while_it_lives() {
        let open = LocalOpen::default();
        let token = open.mint().expect("token");
        assert_eq!(token.len(), 43, "256 bits, base64url");
        assert!(!open.redeem("not-a-token"));
        assert!(open.redeem(&token));
        assert!(!open.redeem(&token), "single use");

        let start = Instant::now();
        let late = open.mint_at(start).expect("token");
        assert!(!open.redeem_at(&late, start + TTL + Duration::from_secs(1)), "expired");
        assert!(!open.redeem_at(&late, start), "and gone");
    }

    #[test]
    fn live_tokens_are_bounded_and_their_pages_go_with_them() {
        let dir = tempfile::tempdir().unwrap();
        let open = LocalOpen::default();
        let start = Instant::now();
        let first = open.mint_at(start).unwrap();
        let page = dir.path().join("open-x.html");
        std::fs::write(&page, "x").unwrap();
        open.attach_page(&first, page.clone());
        for _ in 1..MAX_LIVE {
            open.mint_at(start).unwrap();
        }
        assert!(open.mint_at(start).is_none(), "a loop is bounded");
        assert!(open.redeem_at(&first, start));
        assert!(!page.exists(), "the redirect page goes when the token is spent");

        let other = open.mint_at(start).unwrap();
        let page = dir.path().join("open-y.html");
        std::fs::write(&page, "y").unwrap();
        open.attach_page(&other, page.clone());
        assert!(open.mint_at(start + TTL + Duration::from_secs(1)).is_some(), "the expired make room");
        assert!(!page.exists(), "and an expired token's page goes too");
    }

    #[test]
    fn the_page_url_is_the_bound_address_as_a_literal() {
        let at = |a: &str| page_url(a.parse().unwrap());
        assert_eq!(at("0.0.0.0:8700"), "http://127.0.0.1:8700/");
        assert_eq!(at("127.0.0.1:8700"), "http://127.0.0.1:8700/");
        assert_eq!(at("[::]:8700"), "http://[::1]:8700/");
        assert_eq!(at("[::1]:8700"), "http://[::1]:8700/");
        assert_eq!(at("192.168.1.5:8700"), "http://192.168.1.5:8700/");
    }

    /// A fake server's answers: only this instance's own loopback page, a link
    /// to it, and a private redirect page in the control directory pass.
    #[test]
    fn only_an_answer_this_server_could_give_is_believed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let control = dir.path().join("control");
        private_dir(&control).unwrap();
        let token = "a".repeat(43);
        let page = "http://127.0.0.1:8700/".to_string();
        let good = |redirect: Option<PathBuf>| OpenAnswer {
            page: page.clone(),
            url: Some(format!("{page}#local-open={token}")),
            redirect,
            ..OpenAnswer::default()
        };
        let file = control.join("open-0123456789abcdef.html");
        super::super::write_private(&file, b"x").unwrap();
        assert_eq!(check_answer(&good(Some(file.clone())), 8700, &control), Ok(()));
        assert_eq!(check_answer(&good(None), 8700, &control), Ok(()));
        assert!(check_answer(&good(None), 8701, &control).is_err(), "another port");

        let mut evil = good(None);
        evil.page = "http://attacker.invalid/".into();
        assert!(check_answer(&evil, 8700, &control).is_err(), "not loopback");
        let mut evil = good(None);
        evil.url = Some("http://attacker.invalid/#local-open=x".into());
        assert!(check_answer(&evil, 8700, &control).is_err(), "a link elsewhere");
        let mut evil = good(None);
        evil.url = Some(format!("{page}#local-open=short"));
        assert!(check_answer(&evil, 8700, &control).is_err(), "no token shape");
        assert!(
            check_answer(&good(Some(PathBuf::from("/tmp/x/evil.command"))), 8700, &control).is_err(),
            "a file outside the control directory"
        );
        let other = control.join("evil.command");
        super::super::write_private(&other, b"x").unwrap();
        assert!(check_answer(&good(Some(other)), 8700, &control).is_err(), "not a name the server writes");
        let link = control.join("open-fedcba9876543210.html");
        std::os::unix::fs::symlink("/etc/hosts", &link).unwrap();
        assert!(check_answer(&good(Some(link)), 8700, &control).is_err(), "a symlink");
        let open = control.join("open-1111111111111111.html");
        std::fs::write(&open, "x").unwrap();
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(check_answer(&good(Some(open)), 8700, &control).is_err(), "readable by others");
        std::fs::set_permissions(&control, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            check_answer(&good(Some(file)), 8700, &control).is_err(),
            "a control directory others can reach"
        );
    }

    /// A socket somebody else put where the server would: its directory is not
    /// this user's private one, and nothing is asked of it.
    #[tokio::test]
    async fn a_control_socket_in_somebody_elses_directory_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(home.path().to_path_buf(), "fake");
        let socket = socket_path(&paths);
        let dir = socket.parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_file(&socket);
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let _ = stream
                    .write_all(br#"{"page":"http://attacker.invalid/","url":"http://attacker.invalid/#local-open=x"}"#)
                    .await;
            }
        });
        let refusal = ask(&paths).await.expect_err("refused");
        assert!(refusal.contains("others can reach it"), "{refusal}");
        let _ = std::fs::remove_file(&socket);
        if dir != control_dir(&paths) {
            let _ = std::fs::remove_dir(&dir);
        }
    }

    /// A fake listener in the right place, answering wrong: nothing it says
    /// reaches a browser.
    #[tokio::test]
    async fn a_fake_server_answering_wrong_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(home.path().to_path_buf(), "fakeanswer");
        let dirs = crate::instance::InstanceDirs::new(&paths.data_dir(), paths.instance()).unwrap();
        dirs.create().unwrap();
        dirs.write_port(8700);
        let dir = control_dir(&paths);
        private_dir(&dir).unwrap();
        let socket = socket_path(&paths);
        crate::pty_shim::sidecar::ensure_socket_dir(&socket, &dir).unwrap();
        let _ = std::fs::remove_file(&socket);
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let _ = stream
                    .write_all(b"{\"page\":\"http://attacker.invalid/\",\"redirect\":\"/tmp/x/evil.command\"}\n")
                    .await;
            }
        });
        let refusal = ask(&paths).await.expect_err("refused");
        assert!(refusal.contains("not a loopback address"), "{refusal}");
        let _ = std::fs::remove_file(&socket);
    }

    /// The socket only this user can reach: a `0700` directory, a `0600`
    /// socket, a token for `open` while there is no user account, the login
    /// page once there is one.
    #[tokio::test]
    async fn the_control_socket_hands_this_user_a_token_while_there_is_no_account() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let state = crate::api::test_support::state(home.path()).await;
        let auth = state.drive.auth.clone();
        let cancel = CancellationToken::new();
        let socket = serve(&state.paths, auth.clone(), "http://127.0.0.1:9/".into(), false, cancel.clone()).expect("serve");
        let dir = control_dir(&state.paths);
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777, 0o600);
        if let Some(parent) = socket.parent().filter(|p| *p != dir) {
            assert_eq!(std::fs::metadata(parent).unwrap().permissions().mode() & 0o777, 0o700);
        }

        crate::instance::InstanceDirs::new(&state.paths.data_dir(), state.paths.instance())
            .unwrap()
            .write_port(9);
        let answer = ask(&state.paths).await.expect("an answer");
        assert!(!answer.accounts);
        let url = answer.url.expect("a url");
        let token = url.split_once("#local-open=").expect("a token in the fragment").1.to_string();
        assert!(url.starts_with("http://127.0.0.1:9/#local-open="));
        let redirect = answer.redirect.expect("a redirect page");
        assert_eq!(std::fs::metadata(&redirect).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(std::fs::read_to_string(&redirect).unwrap().contains(&token));
        assert!(auth.local_open.redeem(&token));
        assert!(!redirect.exists(), "gone with the token");

        auth.users.add("ada", "correct horse battery").await.unwrap();
        let answer = ask(&state.paths).await.expect("an answer");
        assert!(answer.accounts, "with an account there is only the login page");
        assert_eq!(answer.url, None);

        cancel.cancel();
        for _ in 0..50 {
            if !socket.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!socket.exists(), "the socket goes with the server");
        assert!(ask(&state.paths).await.is_err());
    }

    /// The desktop app's instance never leaves its setup state, so its
    /// socket would hand any process of that user an owner session: it
    /// refuses, and mints nothing.
    #[tokio::test]
    async fn the_desktop_apps_control_socket_opens_nothing() {
        let home = tempfile::tempdir().unwrap();
        let paths = Paths::for_instance(home.path().to_path_buf(), "droppen-app");
        let state = crate::api::test_support::state(home.path()).await;
        let auth = state.drive.auth.clone();
        let cancel = CancellationToken::new();
        serve(&paths, auth.clone(), "http://127.0.0.1:9/".into(), true, cancel.clone()).expect("serve");
        crate::instance::InstanceDirs::new(&paths.data_dir(), paths.instance()).unwrap().write_port(9);
        let answer = ask(&paths).await.expect("an answer");
        assert_eq!(answer.error.as_deref(), Some(DESKTOP_APP_REFUSAL));
        assert_eq!((answer.url, answer.redirect), (None, None));
        assert!(!auth.users.has_users().await, "in its setup state all the same");
        assert_eq!(auth.local_open.live(), 0, "no token was minted");
        cancel.cancel();
    }
}
