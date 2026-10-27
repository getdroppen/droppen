//! The setup state, and the one-time code that ends it.
//!
//! An instance with no user account is in its **setup state**: without a
//! session, every route refuses except the status route, the two ways to a
//! session (`POST /api/auth/setup`, `POST /api/auth/local-open`) and the static
//! shell that draws the page. On the device itself the way in is
//! `remote-hand-server open` (or the desktop app), which proves to the running
//! server over a socket only this OS user can reach that it is that user
//! ([`super::local_open`]). Anywhere else — a headless server set up from a
//! laptop's browser — it is the **setup code**, Jupyter-token style:
//!
//! * the server that owns the instance mints one when it starts in the setup
//!   state (and again if the state comes back while it runs — the last user
//!   account removed with `user rm`) and keeps it in `<instance>/setup-code`
//!   (0600), which `remote-hand-server setup-code` reads back; it is printed
//!   to standard error only when that is a terminal somebody is watching —
//!   never into a log file — and the log says where it is instead;
//! * the code is 80 random bits, written as four groups of four Crockford
//!   base32 symbols (`7KQ2-M9XD-4HTB-W0RC`) — short enough to type on a phone,
//!   too long to guess through the rate limit below;
//! * it is compared in constant time, works **once**, and dies — memory and
//!   file — the moment the instance has a user account, however that account
//!   came to be (this page, or `user add`, which removes the file too), and
//!   with the server that minted it;
//! * a server that does not own its instance (`export-openapi`, a test's
//!   throwaway router) never mints one, so it can never overwrite the real
//!   server's file with a code nobody is checking.
//!
//! The surface it opens says nothing else — the status route answers a few
//! booleans and a name, and the setup route answers a token or a refusal.
//!
//! # The rate limit
//!
//! Every wrong code is counted twice: against the peer address it came from
//! (an IPv6 peer by its /64, which is what one host can rotate through), and
//! against the server as a whole. A few mistakes are free; after that each
//! further wrong code makes that peer — or, past a larger allowance, everyone —
//! wait twice as long as the last one did, up to fifteen minutes. Attempts are
//! also taken one at a time, so concurrency cannot outrun the counters. The
//! peer is the socket's, never a forwarded header: behind a tunnel every peer
//! is loopback and only the global limit means anything, which is the right
//! way round — a header would let a guesser be a new peer on every request.
//!
//! The global limit can be tripped on purpose to keep the owner waiting. The
//! way round it is local: `remote-hand-server user add` on the device itself,
//! or a restart (the counters are in memory, and the restart mints a fresh
//! code).

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rand::RngCore;

use crate::sessions::Paths;

pub use super::routes::SETUP_PATH;

/// The file, in the instance directory, the live code is kept in.
pub const CODE_FILE: &str = "setup-code";

/// Crockford's base32: no I, L, O or U, so nothing reads as anything else.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Symbols in a code: 16 × 5 bits = 80 bits.
pub const CODE_SYMBOLS: usize = 16;
/// Symbols per printed group.
const GROUP: usize = 4;

/// Wrong codes one peer gets before it has to wait.
const PEER_FREE: u32 = 3;
/// The first wait for a peer; each further wrong code doubles it.
const PEER_BASE: Duration = Duration::from_secs(2);
/// Wrong codes the whole server takes before *everyone* has to wait.
const GLOBAL_FREE: u32 = 10;
/// The first wait for everyone; each further wrong code doubles it.
const GLOBAL_BASE: Duration = Duration::from_secs(1);
/// No wait is longer than this.
const MAX_WAIT: Duration = Duration::from_secs(15 * 60);
/// Peers remembered at once. Past this the global limit is what holds.
const MAX_PEERS: usize = 4096;

/// `<instance>/setup-code`.
pub fn code_file(paths: &Paths) -> PathBuf {
    paths.instance_dir().join(CODE_FILE)
}

/// A new code, normalized (16 symbols, no separators).
fn mint() -> String {
    let mut bytes = [0u8; CODE_SYMBOLS];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    // 32 divides 256, so masking keeps every symbol equally likely.
    bytes.iter().map(|b| ALPHABET[usize::from(b & 31)] as char).collect()
}

/// `ABCD-EFGH-JKMN-PQRS`, for people.
pub fn display(code: &str) -> String {
    code.as_bytes()
        .chunks(GROUP)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

/// What somebody typed, the way the code is stored: upper case, separators
/// and spaces dropped, and the letters Crockford reads as digits read as them.
pub fn normalize(typed: &str) -> String {
    typed
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        })
        .collect()
}

/// Whether `typed` is `code`, in time that does not depend on where they
/// first differ.
fn matches(code: &str, typed: &str) -> bool {
    let typed = normalize(typed);
    bool::from(subtle::ConstantTimeEq::ct_eq(typed.as_bytes(), code.as_bytes()))
}

/// The bucket a peer's failures are counted in.
fn peer_key(ip: Option<IpAddr>) -> IpAddr {
    match ip {
        None => IpAddr::from([0u8, 0, 0, 0]),
        Some(IpAddr::V4(v4)) => IpAddr::V4(v4),
        Some(IpAddr::V6(v6)) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let mut segments = v6.segments();
                for segment in &mut segments[4..] {
                    *segment = 0;
                }
                IpAddr::from(segments)
            }
        },
    }
}

/// Failures and the wait they earned.
#[derive(Debug, Clone, Copy, Default)]
struct Strikes {
    failures: u32,
    until: Option<Instant>,
}

impl Strikes {
    fn wait(&self, now: Instant) -> Option<Duration> {
        self.until.and_then(|until| until.checked_duration_since(now)).filter(|d| !d.is_zero())
    }

    fn fail(&mut self, now: Instant, free: u32, base: Duration) {
        self.failures = self.failures.saturating_add(1);
        if self.failures >= free {
            let doublings = (self.failures - free).min(20);
            let wait = base.saturating_mul(1u32 << doublings).min(MAX_WAIT);
            self.until = Some(now + wait);
        }
    }
}

/// The two counters.
#[derive(Debug, Default)]
struct Limiter {
    peers: HashMap<IpAddr, Strikes>,
    global: Strikes,
}

impl Limiter {
    /// How long `peer` has to wait before its next attempt, if at all.
    fn wait(&self, peer: IpAddr, now: Instant) -> Option<Duration> {
        let own = self.peers.get(&peer).and_then(|s| s.wait(now));
        let everyone = self.global.wait(now);
        own.max(everyone)
    }

    fn fail(&mut self, peer: IpAddr, now: Instant) {
        self.global.fail(now, GLOBAL_FREE, GLOBAL_BASE);
        if !self.peers.contains_key(&peer) && self.peers.len() >= MAX_PEERS {
            // Forget whoever is not being made to wait; if that frees nothing,
            // this peer goes uncounted and the global counter holds the line.
            self.peers.retain(|_, s| s.wait(now).is_some());
            if self.peers.len() >= MAX_PEERS {
                return;
            }
        }
        self.peers.entry(peer).or_default().fail(now, PEER_FREE, PEER_BASE);
    }
}

/// Where the owner keeps the code and what it prints with it.
#[derive(Debug, Clone)]
struct Armed {
    file: PathBuf,
    url: String,
    /// What the commands it suggests need before them to reach this instance
    /// (`APP_INSTANCE=bob `), or nothing ([`crate::program::command_prefix`]).
    env: String,
}

/// The code, as far as this server is concerned.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum Code {
    /// None live: not minted yet, or used up.
    #[default]
    None,
    /// Live, normalized: the next right attempt spends it.
    Live(String),
    /// Spent by an attempt whose account is still being made: no attempt can
    /// use it and none is minted in its place, until [`Setup::spent`] says the
    /// account exists or [`Setup::restore`] gives it back.
    Spending(String),
}

/// Why an attempt was refused before or at the code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// Wait this long.
    TooMany(Duration),
    /// This server has no live code (it does not own its instance).
    NoCode,
    /// The code is wrong.
    Wrong,
}

/// The setup state's moving parts, one per server.
#[derive(Default)]
pub struct Setup {
    /// The code: live, being spent, or none.
    code: Mutex<Code>,
    /// Set once the server owns its instance and knows its address.
    armed: Mutex<Option<Armed>>,
    limiter: Mutex<Limiter>,
    /// One attempt at a time, across the check, the account and the session.
    pub gate: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for Setup {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Setup").finish_non_exhaustive()
    }
}

impl Setup {
    /// Let this server mint codes: it owns the instance, and `url` is where
    /// its page is. Nothing is minted here; [`Setup::refresh`] does that. A
    /// file left by an earlier run (one that crashed) is removed: whatever
    /// code it holds died with that run.
    pub fn arm(&self, file: PathBuf, url: String) {
        self.arm_with(file, url, String::new());
    }

    /// [`Setup::arm`] for the instance `paths` names: its code file, and the
    /// commands it suggests made to reach it.
    pub fn arm_for(&self, paths: &Paths, url: String) {
        self.arm_with(code_file(paths), url, crate::program::command_prefix(paths));
    }

    fn arm_with(&self, file: PathBuf, url: String, env: String) {
        remove_code_file(&file);
        *self.armed.lock().expect("setup lock") = Some(Armed { file, url, env });
    }

    /// Bring the code in line with whether the instance is in its setup
    /// state: mint one (print it, write the file) if it is and there is none —
    /// not while one is being spent, whose account may yet fail and give it
    /// back; drop it (and the file) if it is not.
    pub fn refresh(&self, setup: bool) {
        let armed = self.armed.lock().expect("setup lock").clone();
        let mut code = self.code.lock().expect("setup lock");
        match (setup, armed) {
            (true, Some(armed)) if *code == Code::None => {
                let fresh = mint();
                if let Err(error) = write_code_file(&armed.file, &fresh) {
                    tracing::error!(path = %armed.file.display(), %error, "cannot write the setup code file; the code is only on standard error");
                }
                announce(&armed, &fresh);
                *code = Code::Live(fresh);
            }
            (false, armed) if *code != Code::None => {
                *code = Code::None;
                if let Some(armed) = armed {
                    remove_code_file(&armed.file);
                }
            }
            _ => {}
        }
    }

    /// Check an attempt from `peer`, counting it if the code is wrong. On
    /// success the code is being spent — its file gone, no attempt able to use
    /// it, and none minted in its place — before this returns, so a second
    /// attempt with it fails however quickly it follows. The caller then makes
    /// the account and says how that went: [`Setup::spent`], or
    /// [`Setup::restore`].
    ///
    /// The caller holds [`Setup::gate`].
    pub fn attempt(&self, peer: Option<IpAddr>, typed: &str) -> Result<(), Refused> {
        self.attempt_at(peer, typed, Instant::now())
    }

    fn attempt_at(&self, peer: Option<IpAddr>, typed: &str, now: Instant) -> Result<(), Refused> {
        let peer = peer_key(peer);
        let mut limiter = self.limiter.lock().expect("setup lock");
        if let Some(wait) = limiter.wait(peer, now) {
            return Err(Refused::TooMany(wait));
        }
        let mut code = self.code.lock().expect("setup lock");
        let Code::Live(live) = &*code else {
            return Err(Refused::NoCode);
        };
        if !matches(live, typed) {
            limiter.fail(peer, now);
            return Err(Refused::Wrong);
        }
        *code = Code::Spending(live.clone());
        drop(code);
        if let Some(armed) = self.armed.lock().expect("setup lock").as_ref() {
            remove_code_file(&armed.file);
        }
        Ok(())
    }

    /// The account [`Setup::attempt`] let through exists: the code is used up.
    pub fn spent(&self) {
        let mut code = self.code.lock().expect("setup lock");
        if matches!(*code, Code::Spending(_)) {
            *code = Code::None;
        }
    }

    /// Put back the code [`Setup::attempt`] let through, because the account
    /// could not be made after all (a name taken, a disk error): it is live
    /// again, file and all — the same code, since none was minted meanwhile.
    pub fn restore(&self) {
        let mut code = self.code.lock().expect("setup lock");
        if let Code::Spending(spent) = &*code {
            let spent = spent.clone();
            if let Some(armed) = self.armed.lock().expect("setup lock").as_ref() {
                if let Err(error) = write_code_file(&armed.file, &spent) {
                    tracing::error!(path = %armed.file.display(), %error, "cannot write the setup code file back");
                }
            }
            *code = Code::Live(spent);
        }
    }

    /// At shutdown: a code dies with the server that minted it, so its file
    /// does not outlive it to be read back as if it still worked.
    pub fn disarm(&self) {
        let armed = self.armed.lock().expect("setup lock").take();
        if std::mem::take(&mut *self.code.lock().expect("setup lock")) != Code::None {
            if let Some(armed) = armed {
                remove_code_file(&armed.file);
            }
        }
    }

    /// The live code, for tests that play the person reading standard error.
    #[cfg(test)]
    pub(crate) fn live(&self) -> Option<String> {
        match &*self.code.lock().expect("setup lock") {
            Code::Live(code) => Some(code.clone()),
            Code::None | Code::Spending(_) => None,
        }
    }
}

/// Write the code, `0600`, atomically.
fn write_code_file(path: &Path, code: &str) -> std::io::Result<()> {
    crate::fsutil::write_private(path, format!("{}\n", display(code)).as_bytes())
}

/// Remove the code file; it not being there is fine.
pub fn remove_code_file(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(path = %path.display(), %error, "cannot remove the setup code file"),
    }
}

/// The code, read back from the file a running server wrote.
pub fn read_code_file(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let code = normalize(text.trim());
    (code.len() == CODE_SYMBOLS && code.bytes().all(|b| ALPHABET.contains(&b))).then_some(code)
}

/// Say where the code is: the code itself only to a terminal, where the
/// person who started the server is looking; a log file (launchd, systemd, a
/// `nohup` redirect) gets where to find it, never the code. The commands the
/// banner suggests read in the name the server was started as
/// ([`crate::program::Name::rewrite`]), and reach this instance.
fn announce(armed: &Armed, code: &str) {
    let env = &armed.env;
    if stderr_is_a_terminal() {
        let rule = "=".repeat(72);
        let banner = format!(
            "\n{rule}\n  Droppen: this device has no account yet.\n\n  On this device, open the page signed in with:\n      {env}remote-hand-server open\n\n  From another device, open this device's page ({url})\n  and enter this one-time setup code to create the first account:\n\n      {code}\n\n  It works once, only until an account exists, and only while this\n  server runs. `{env}remote-hand-server setup-code` shows it again.\n  Or create the account on this device: {env}remote-hand-server user add <name>\n{rule}\n",
            url = armed.url,
            code = display(code),
        );
        eprintln!("{}", crate::program::Name::invoked().rewrite(&banner));
    }
    tracing::warn!(
        url = %armed.url,
        file = %armed.file.display(),
        "this device has no account yet: open the page with `{env}remote-hand-server open` on it, or create the first account with the one-time setup code (`{env}remote-hand-server setup-code` shows it; it is in the file, readable only by this user)"
    );
}

#[cfg(unix)]
fn stderr_is_a_terminal() -> bool {
    // SAFETY: `isatty` only inspects a file descriptor and cannot fail unsafely.
    unsafe { libc::isatty(libc::STDERR_FILENO) == 1 }
}

#[cfg(not(unix))]
fn stderr_is_a_terminal() -> bool {
    false
}

/// The page's address, for the banner: the bound host, or loopback for an
/// unspecified one (with a word about the network, which is where else it is).
pub fn local_url(host: &str, port: u16) -> String {
    let unspecified = host.parse::<IpAddr>().is_ok_and(|ip| ip.is_unspecified());
    if unspecified {
        format!("http://127.0.0.1:{port}/ (or this device's address on your network)")
    } else if host.contains(':') && !host.starts_with('[') {
        format!("http://[{host}]:{port}/")
    } else {
        format!("http://{host}:{port}/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_is_sixteen_crockford_symbols_and_reads_back_in_groups() {
        let code = mint();
        assert_eq!(code.len(), CODE_SYMBOLS);
        assert!(code.bytes().all(|b| ALPHABET.contains(&b)), "{code}");
        let shown = display(&code);
        assert_eq!(shown.len(), CODE_SYMBOLS + 3);
        assert_eq!(shown.matches('-').count(), 3);
        assert_eq!(normalize(&shown), code);
        assert_ne!(mint(), mint(), "80 random bits");
    }

    #[test]
    fn what_a_person_types_is_read_the_way_crockford_meant() {
        assert_eq!(normalize(" ab0d-efgh jkmn-pqrs "), "AB0DEFGHJKMNPQRS");
        assert_eq!(normalize("o0iIlL"), "001111");
        assert!(matches("AB0DEFGHJKMNPQ11", "abOd-efgh-jkmn-pqil"));
        assert!(!matches("AB0DEFGHJKMNPQ11", "AB0DEFGHJKMNPQ1"), "a symbol short");
        assert!(!matches("AB0DEFGHJKMNPQ11", "AB0DEFGHJKMNPQ12"));
        assert!(!matches("AB0DEFGHJKMNPQ11", ""));
    }

    #[test]
    fn an_ipv6_peer_is_counted_by_its_64() {
        let a: IpAddr = "2001:db8:1:2:aaaa::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:bbbb::9".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(peer_key(Some(a)), peer_key(Some(b)));
        assert_ne!(peer_key(Some(a)), peer_key(Some(c)));
        let mapped: IpAddr = "::ffff:192.168.1.9".parse().unwrap();
        assert_eq!(peer_key(Some(mapped)), "192.168.1.9".parse::<IpAddr>().unwrap());
    }

    fn armed(dir: &Path) -> Setup {
        let setup = Setup::default();
        setup.arm(dir.join(CODE_FILE), "http://127.0.0.1:1/".into());
        setup
    }

    #[test]
    fn a_code_is_minted_in_the_setup_state_only_by_an_armed_server() {
        let dir = tempfile::tempdir().unwrap();

        let unarmed = Setup::default();
        unarmed.refresh(true);
        assert_eq!(unarmed.live(), None, "a server that does not own its instance mints nothing");
        assert_eq!(unarmed.attempt(None, "anything"), Err(Refused::NoCode));

        let setup = armed(dir.path());
        setup.refresh(true);
        let code = setup.live().expect("minted");
        let file = dir.path().join(CODE_FILE);
        assert_eq!(read_code_file(&file).as_deref(), Some(code.as_str()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        }
        setup.refresh(true);
        assert_eq!(setup.live().as_deref(), Some(code.as_str()), "one code until it is used");

        setup.refresh(false);
        assert_eq!(setup.live(), None, "an account ends the setup state");
        assert!(!file.exists(), "and the file goes with the code");
        assert_eq!(setup.attempt(None, &code), Err(Refused::NoCode));
    }

    #[test]
    fn the_right_code_works_once_and_the_file_goes_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let setup = armed(dir.path());
        setup.refresh(true);
        let code = setup.live().unwrap();
        assert_eq!(setup.attempt(None, &display(&code).to_lowercase()), Ok(()));
        assert!(!dir.path().join(CODE_FILE).exists());
        assert_eq!(setup.attempt(None, &code), Err(Refused::NoCode), "single use");

        setup.restore();
        assert_eq!(
            setup.live().as_deref(),
            Some(code.as_str()),
            "put back when the account could not be made"
        );
        assert_eq!(read_code_file(&dir.path().join(CODE_FILE)).as_deref(), Some(code.as_str()));
        setup.disarm();
        assert!(!dir.path().join(CODE_FILE).exists(), "a code dies with its server");
    }

    #[test]
    fn no_code_is_minted_while_one_is_being_spent() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(CODE_FILE);
        let setup = armed(dir.path());
        setup.refresh(true);
        let code = setup.live().unwrap();
        assert_eq!(setup.attempt(None, &code), Ok(()));

        // The account is still being made: the instance is in its setup state,
        // and whatever asks (the status route, the 3 s check) mints nothing.
        setup.refresh(true);
        assert_eq!(setup.live(), None);
        assert!(!file.exists(), "no new code on file either");
        assert_eq!(setup.attempt(None, &code), Err(Refused::NoCode), "and the spent one opens nothing meanwhile");

        // The account could not be made: the same code is good again.
        setup.restore();
        assert_eq!(setup.live().as_deref(), Some(code.as_str()));
        assert_eq!(read_code_file(&file).as_deref(), Some(code.as_str()));

        // Spent for good: the account exists, and the setup state is over.
        assert_eq!(setup.attempt(None, &code), Ok(()));
        setup.spent();
        setup.restore();
        assert_eq!(setup.live(), None, "nothing to give back once it is used up");
        setup.refresh(false);
        assert!(!file.exists());

        // A setup state that comes back (the last account removed) gets a new one.
        setup.refresh(true);
        let next = setup.live().expect("a new code");
        assert_ne!(next, code);
    }

    #[test]
    fn wrong_codes_make_a_peer_and_then_everyone_wait() {
        let dir = tempfile::tempdir().unwrap();
        let setup = armed(dir.path());
        setup.refresh(true);
        let code = setup.live().unwrap();
        let wrong = "0000-0000-0000-0000";
        let one: IpAddr = "192.168.1.9".parse().unwrap();
        let two: IpAddr = "192.168.1.10".parse().unwrap();
        let t0 = Instant::now();

        for _ in 0..PEER_FREE - 1 {
            assert_eq!(setup.attempt_at(Some(one), wrong, t0), Err(Refused::Wrong));
        }
        assert_eq!(setup.attempt_at(Some(one), wrong, t0), Err(Refused::Wrong), "the last free one");
        match setup.attempt_at(Some(one), &code, t0) {
            Err(Refused::TooMany(wait)) => assert_eq!(wait, PEER_BASE),
            other => panic!("the peer has to wait: {other:?}"),
        }
        assert_eq!(setup.attempt_at(Some(two), wrong, t0), Err(Refused::Wrong), "another peer does not");

        // Each further wrong code doubles the peer's wait.
        let t1 = t0 + PEER_BASE;
        assert_eq!(setup.attempt_at(Some(one), wrong, t1), Err(Refused::Wrong));
        assert_eq!(setup.attempt_at(Some(one), wrong, t1), Err(Refused::TooMany(PEER_BASE * 2)));

        // Enough wrong codes from anywhere and everyone waits — the right code
        // included, from a peer that never got one wrong.
        let three: IpAddr = "10.0.0.3".parse().unwrap();
        let mut now = t1;
        let mut peer = 0u8;
        while setup.limiter.lock().unwrap().global.wait(now).is_none() {
            peer = peer.wrapping_add(1);
            let _ = setup.attempt_at(Some(IpAddr::from([10, 1, 0, peer])), wrong, now);
            now += Duration::from_millis(1);
        }
        assert!(matches!(setup.attempt_at(Some(three), &code, now), Err(Refused::TooMany(_))));

        // Waited out, the right code still works: the limit never spends it.
        let later = now + MAX_WAIT;
        assert_eq!(setup.attempt_at(Some(three), &code, later), Ok(()));
    }

    #[test]
    fn no_wait_is_longer_than_the_cap() {
        let mut strikes = Strikes::default();
        let now = Instant::now();
        for _ in 0..200 {
            strikes.fail(now, PEER_FREE, PEER_BASE);
        }
        assert_eq!(strikes.wait(now), Some(MAX_WAIT));
    }

    #[test]
    fn a_full_peer_table_still_counts_everyone() {
        let mut limiter = Limiter::default();
        let now = Instant::now();
        for i in 0..MAX_PEERS as u32 {
            let ip = IpAddr::from(i.to_be_bytes());
            for _ in 0..PEER_FREE {
                limiter.fail(ip, now);
            }
        }
        assert_eq!(limiter.peers.len(), MAX_PEERS);
        limiter.fail("203.0.113.1".parse().unwrap(), now);
        assert!(limiter.peers.len() <= MAX_PEERS);
        assert!(limiter.global.wait(now).is_some(), "the global counter saw every one");
    }

    #[test]
    fn the_banner_url_names_loopback_for_an_unspecified_host() {
        assert_eq!(local_url("127.0.0.1", 8700), "http://127.0.0.1:8700/");
        assert!(local_url("0.0.0.0", 8700).starts_with("http://127.0.0.1:8700/ (or this device's address"));
        assert!(local_url("::", 8700).starts_with("http://127.0.0.1:8700/"));
        assert_eq!(local_url("::1", 8700), "http://[::1]:8700/");
        assert_eq!(local_url("box.lan", 8700), "http://box.lan:8700/");
    }
}
