//! The spawn gate and the claims it hands out: what keeps a second CLI off a
//! session.
//!
//! Two locks do it. The gate ([`SpawnGate`]) is this process's: every send,
//! start and archive takes it, which closes the gap between "this server runs
//! nothing for the session" and "it runs one". The claim is the machine's: a
//! `flock` on a file in the shared data directory ([`lock`]), which keeps
//! another instance, or this server after a restart, off a session one of
//! them is driving.
//!
//! A session's CLI is started only with a [`SpawnPermit`]: the shim's
//! [`Spawn`](super::shim::Spawn) takes one, and so does an ACP agent's start.
//! Outside this module a permit comes only from the gate, from a lease (a
//! respawn, which the session's entry in the map already guards) or from the
//! one boot pass, and a claim only through a permit, a lease or that pass.

mod lock;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use droppen_sessions::Source;

/// A held claim on one session; dropping it lets the session go.
pub type SessionClaim = lock::InjectLock;

/// Why a session could not be claimed.
#[derive(Debug)]
pub enum ClaimError {
    /// Another server, or another run of this one, holds it.
    Busy {
        /// The pid the holder recorded, when one could be read.
        holder: Option<u32>,
    },
    /// The claim's file could not be made or locked.
    Io(std::io::Error),
}

/// Where the claims live, shared by the gate and every lease it hands out.
#[derive(Debug)]
struct Claims {
    dir: PathBuf,
}

impl Claims {
    fn take(&self, source: Source, id: &str) -> Result<SessionClaim, ClaimError> {
        match lock::acquire(&self.dir, source, id) {
            Ok(lock::Outcome::Acquired(claim)) => Ok(claim),
            Ok(lock::Outcome::Busy { holder }) => Err(ClaimError::Busy { holder }),
            Err(error) => Err(ClaimError::Io(error)),
        }
    }
}

/// The one spawn gate, serialized across sessions: what it guards is the gap
/// between "no CLI for this session" and "one in the map", and spawns are rare
/// enough that one lock costs less than a lock per session.
#[derive(Debug)]
pub struct SpawnGate {
    lock: tokio::sync::Mutex<()>,
    claims: Arc<Claims>,
    /// Set once [`SpawnGate::boot`] has handed out the boot pass.
    booted: AtomicBool,
    /// How many are waiting to take the gate, for a test that waits until a
    /// call is blocked on it.
    #[cfg(any(test, feature = "test-support"))]
    waiting: std::sync::atomic::AtomicUsize,
}

impl SpawnGate {
    /// The gate, with its claims in `dir`: the **shared** data directory's,
    /// so a second instance contends with this one rather than ignoring it.
    /// Only [`super::ProviderRegistry::new`] makes one.
    pub(super) fn new(dir: PathBuf) -> Self {
        Self {
            lock: tokio::sync::Mutex::new(()),
            claims: Arc::new(Claims { dir }),
            booted: AtomicBool::new(false),
            #[cfg(any(test, feature = "test-support"))]
            waiting: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Take the gate, for as long as the permit keeps it.
    pub async fn permit(&self) -> SpawnPermit<'_> {
        #[cfg(any(test, feature = "test-support"))]
        let _waiting = Waiting::on(&self.waiting);
        SpawnPermit {
            claim: None,
            guard: Some(self.lock.lock().await),
            claims: self.claims.clone(),
        }
    }

    /// The boot pass, once: the claims adoption takes before the server
    /// listens, which no send can race and so need no gate. `None` after the
    /// first call.
    pub(super) fn boot(&self) -> Option<Boot<'_>> {
        (!self.booted.swap(true, Ordering::SeqCst)).then_some(Boot { gate: self })
    }
}

/// Proof that a session's start was reserved under the gate: the gate while
/// the permit keeps it, and the session's claim once it has one.
///
/// Field order is drop order: the claim goes before the gate.
#[must_use]
#[derive(Debug)]
pub struct SpawnPermit<'g> {
    claim: Option<SessionClaim>,
    guard: Option<tokio::sync::MutexGuard<'g, ()>>,
    claims: Arc<Claims>,
}

impl SpawnPermit<'_> {
    /// A permit for a shim that runs no session, with neither the gate nor a
    /// claim: the shim's own end-to-end test starts the real binary with it.
    /// Nothing in the server may, and a test reads every file under `src/` to
    /// keep it so.
    #[doc(hidden)]
    pub fn without_a_session() -> SpawnPermit<'static> {
        SpawnPermit {
            claim: None,
            guard: None,
            claims: Arc::new(Claims { dir: PathBuf::new() }),
        }
    }

    /// Claim session `id` of `source` for this start.
    pub fn claim(&mut self, source: Source, id: &str) -> Result<(), ClaimError> {
        self.claim = Some(self.claims.take(source, id)?);
        Ok(())
    }

    /// Let the claim go before the permit does: a start that failed after it
    /// claimed lets its session go where the claim's own scope used to end.
    pub fn unclaim(&mut self) {
        self.claim = None;
    }

    /// Give up the gate and keep the rest: an ACP start, once its placeholder
    /// is in the map, starts its agent outside the gate.
    pub fn ungate(&mut self) {
        self.guard = None;
    }

    /// The claim, for the driver that runs the session from here; the gate
    /// stays with the permit.
    pub fn lease(&mut self) -> SessionLease {
        SessionLease {
            claim: self.claim.take(),
            claims: self.claims.clone(),
        }
    }
}

/// What a driver holds for its process's life: the session's claim, or none
/// (a renamed session whose new id could not be claimed).
#[derive(Debug)]
pub struct SessionLease {
    claim: Option<SessionClaim>,
    claims: Arc<Claims>,
}

impl SessionLease {
    /// A permit for a respawn: no gate, because the driver's entry in the map
    /// already keeps a second start away, and the claim stays with the lease.
    pub fn respawn_permit(&self) -> SpawnPermit<'static> {
        SpawnPermit {
            claim: None,
            guard: None,
            claims: self.claims.clone(),
        }
    }

    /// A claim on session `id` of `source`, for a driver that follows its
    /// session to a new id.
    pub fn claim_other(&self, source: Source, id: &str) -> Result<SessionClaim, ClaimError> {
        self.claims.take(source, id)
    }

    /// Hold `claim` in place of the one held now, which goes.
    pub fn replace(&mut self, claim: Option<SessionClaim>) {
        drop(std::mem::replace(&mut self.claim, claim));
    }

    /// Let the session go.
    pub fn release(&mut self) {
        self.claim = None;
    }
}

/// The boot pass: the claims adoption takes, once, before the server listens.
pub(crate) struct Boot<'g> {
    gate: &'g SpawnGate,
}

impl Boot<'_> {
    /// Claim session `id` of `source` for the run adoption takes back.
    pub(crate) fn claim(&self, source: Source, id: &str) -> Result<SessionLease, ClaimError> {
        Ok(SessionLease {
            claim: Some(self.gate.claims.take(source, id)?),
            claims: self.gate.claims.clone(),
        })
    }

    /// Take session `id`'s claim and let it go at once: a claim file whose run
    /// is over is debris, and leaving it would keep the session claimed by
    /// nobody.
    pub(crate) fn sweep(&self, source: Source, id: &str) {
        let _ = self.gate.claims.take(source, id);
    }
}

/// A caller counted as waiting for the gate until it has it, or gives up.
#[cfg(any(test, feature = "test-support"))]
struct Waiting<'a>(&'a std::sync::atomic::AtomicUsize);

#[cfg(any(test, feature = "test-support"))]
impl<'a> Waiting<'a> {
    fn on(count: &'a std::sync::atomic::AtomicUsize) -> Self {
        count.fetch_add(1, Ordering::SeqCst);
        Self(count)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(any(test, feature = "test-support"))]
impl SpawnGate {
    /// How many are waiting to take the gate now.
    pub fn waiting(&self) -> usize {
        self.waiting.load(Ordering::SeqCst)
    }

    /// Whether anyone holds the gate.
    pub fn held(&self) -> bool {
        self.lock.try_lock().is_err()
    }

    /// Whether anyone holds session `id`'s claim: its file is locked. The probe
    /// neither creates the file nor removes it, so a test still sees what the
    /// server left behind.
    pub fn claim_held(&self, source: Source, id: &str) -> bool {
        use std::os::fd::AsRawFd;
        let path = self.claims.dir.join(droppen_sessions::reference::lock_name(source.as_str(), id));
        let Ok(file) = std::fs::File::open(path) else { return false };
        // SAFETY: `file` is an open descriptor owned for the whole call, and
        // `LOCK_EX | LOCK_NB` takes no further arguments. A lock that is
        // granted goes when `file` closes.
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) != 0 }
    }

    /// A lease with no claim, for a driver a test builds by hand.
    pub fn empty_lease(&self) -> SessionLease {
        SessionLease {
            claim: None,
            claims: self.claims.clone(),
        }
    }

    /// A lease on session `id` of `source`, for a run a test hands a driver.
    pub fn lease(&self, source: Source, id: &str) -> Result<SessionLease, ClaimError> {
        Ok(SessionLease {
            claim: Some(self.claims.take(source, id)?),
            claims: self.claims.clone(),
        })
    }
}

#[cfg(any(test, feature = "test-support"))]
impl SessionLease {
    /// Whether the lease still holds its claim.
    pub fn holds(&self) -> bool {
        self.claim.is_some()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// Every source file of the server but this one, and what it says.
    fn server_sources() -> Vec<(String, String)> {
        fn walk(dir: &Path, found: &mut Vec<(String, String)>) {
            for entry in std::fs::read_dir(dir).expect("read_dir").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, found);
                } else if path.extension().is_some_and(|ext| ext == "rs") && !path.ends_with("provider/gate.rs") {
                    found.push((path.display().to_string(), std::fs::read_to_string(&path).expect("source")));
                }
            }
        }
        let mut found = Vec::new();
        walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut found);
        found
    }

    /// Whether `c` can be part of an identifier.
    fn ident(c: char) -> bool {
        c.is_alphanumeric() || c == '_'
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
            .filter(|(_, text)| text.match_indices(NAME).any(|(at, _)| !text[..at].ends_with(ident) && !text[at + NAME.len()..].starts_with(ident)))
            .map(|(path, _)| path)
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
        let makes = |text: &str| {
            let code = text.split("\n#[cfg(test)]\nmod tests").next().unwrap_or_default();
            let code: Vec<&str> = code.lines().filter(|line| !line.trim_start().starts_with("//")).collect();
            let code = code.join("\n");
            code.match_indices("SpawnGate").any(|(at, name)| {
                let rest = code[at + name.len()..].trim_start();
                !code[..at].ends_with(ident)
                    && rest.strip_prefix("::").is_some_and(|rest| {
                        let rest = rest.trim_start();
                        rest.starts_with("new") && !rest["new".len()..].starts_with(ident)
                    })
            })
        };
        let sources = server_sources();
        assert!(
            sources.iter().any(|(path, text)| path.ends_with("provider/registry.rs") && makes(text)),
            "the provider registry makes the gate"
        );
        let found: Vec<&str> = sources
            .iter()
            .filter(|(path, text)| !path.ends_with("provider/registry.rs") && makes(text))
            .map(|(path, _)| path.as_str())
            .collect();
        assert!(found.is_empty(), "a spawn gate made outside the provider registry: {found:?}");
    }
}
