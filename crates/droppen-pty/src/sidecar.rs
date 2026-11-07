//! The four files one surviving terminal leaves on disk.
//!
//! ```text
//! <instance>/pty-shims/<id>.meta.json   the terminal's identity  (server-owned)
//! <instance>/pty-shims/<id>.cursor      how far the server read  (server-owned)
//! <instance>/pty-shims/<id>.exit        the tombstone            (shim-owned)
//! <instance>/pty-shims/<short>.sock     the shim's unix socket   (shim-owned)
//! ```
//!
//! Ownership is split on purpose, and adoption reads them in a fixed order —
//! meta, then tombstone, then socket — because each answers a different
//! question and only the first is *authoritative*. A socket with no meta is
//! debris from a crash between spawn and record; a meta with no socket and no
//! tombstone is a shim that died without saying so. Neither is a terminal.
//!
//! # Why the socket is named differently
//!
//! A unix socket address is not a path, it is 104 bytes of `sun_path` on macOS
//! (108 on Linux) — including the terminating NUL and counted before any symlink
//! is resolved. A full uuid plus `.sock` is 41 of them, which leaves 62 for the
//! directory, and the *real* directory is already
//! `~/.local/share/remote-hand/instances/default/pty-shims/` at 68. It does not
//! fit, and the failure mode is `bind` returning `EINVAL` on a machine where
//! nothing looks wrong.
//!
//! So the socket takes a short stem derived from the id, and — for the
//! directories that still do not fit, of which a macOS temporary directory at 60
//! characters before anything is added is the everyday example — falls back to a
//! per-instance directory under `/tmp`, `tmux(1)`-style. The instance directory
//! stays the authority either way: the three files that say what a terminal *is*
//! never move, and the socket path is derived from the instance directory, so
//! two instances (and two tests) can never land on the same one.

use std::path::{Path, PathBuf};


use super::proto::{Meta, Tombstone};

/// The portable `sun_path` budget: 104 bytes on the BSDs, minus the NUL.
const SUN_PATH_MAX: usize = 103;

/// How many hex characters of the id name the socket.
///
/// 64 bits. The registry holds at most 64 terminals at a time, so a collision
/// needs a birthday coincidence about 27 orders of magnitude out of reach.
const SOCKET_STEM_LEN: usize = 16;

/// The socket's filename stem: the id's hex digits, or a hash for anything else.
fn socket_stem(id: &str) -> String {
    let hex: String = id.chars().filter(char::is_ascii_hexdigit).take(SOCKET_STEM_LEN).collect();
    if hex.len() == SOCKET_STEM_LEN {
        return hex;
    }
    // Ids are uuids, so this is unreachable in practice; a hash rather than a
    // panic because a short socket name is a convenience, not an invariant.
    hex_digest(id.as_bytes(), SOCKET_STEM_LEN)
}

/// The first `chars` hex characters of a sha-256 digest.
fn hex_digest(bytes: &[u8], chars: usize) -> String {
    droppen_base::text::sha256_hex(bytes).chars().take(chars).collect()
}

/// Where a terminal's socket goes: beside its other sidecars if that address
/// fits, and in a per-instance directory under `/tmp` if it does not.
///
/// Shared with the server's `provider::shim`, whose sockets live one directory over
/// and hit the same `sun_path` ceiling for the same reason. The budget is a
/// property of the kernel, not of what is on the other end of the socket, so
/// there is one implementation of it.
pub fn socket_path(dir: &Path, id: &str) -> PathBuf {
    let stem = socket_stem(id);
    let beside = dir.join(format!("{stem}.sock"));
    if beside.as_os_str().len() <= SUN_PATH_MAX {
        return beside;
    }
    runtime_dirs(dir)[0].join(format!("{stem}.sock"))
}

/// Every socket path a shim of `id` may have bound, the current spelling first.
///
/// Adoption asks for this rather than [`socket_path`]: a shim started before
/// the `/tmp` fallback folder was renamed keeps running across the upgrade —
/// a terminal's shell lives as long as its tab — and the old folder's socket
/// is the only way to reach it.
///
/// The second entry stays until the upgrade floor passes the release that
/// introduced `droppen-…`: an install at v0.2.125 must still upgrade without
/// stranding a terminal whose socket took the fallback.
pub fn socket_candidates(dir: &Path, id: &str) -> [PathBuf; 2] {
    let stem = socket_stem(id);
    let beside = dir.join(format!("{stem}.sock"));
    if beside.as_os_str().len() <= SUN_PATH_MAX {
        return [beside.clone(), beside];
    }
    let [current, legacy] = runtime_dirs(dir);
    [current.join(format!("{stem}.sock")), legacy.join(format!("{stem}.sock"))]
}

/// The candidate adoption should dial: the first spelling whose socket exists
/// inside a folder this process may use.
///
/// `None` covers both "no shim at all" and "a socket in a folder that is not
/// ours" — [`socket_candidate_is_trusted`] refuses the latter. A caller that
/// has to tell those apart asks about the candidates themselves, which is why
/// only the path is returned here.
pub fn adopted_socket(dir: &Path, id: &str) -> Option<PathBuf> {
    socket_candidates(dir, id)
        .into_iter()
        .find(|socket| socket.exists() && socket_candidate_is_trusted(socket, dir))
}

/// Whether a socket adoption found may be dialled.
///
/// A socket beside the sidecars lives in the instance directory, which this
/// server made for itself. A socket under `/tmp` lives under a name any local
/// user can create first, and the connect that follows cannot tell whose shim
/// is listening from the socket alone — so its folder has to pass the same
/// three checks as one a new shim would reuse: a real directory rather than a
/// symlink, this uid's, and `0o700` exactly. A refused socket is skipped, never
/// repaired.
pub fn socket_candidate_is_trusted(socket: &Path, dir: &Path) -> bool {
    let Some(parent) = socket.parent() else { return false };
    if parent == dir {
        return true;
    }
    match claim_existing_socket_dir(parent) {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(socket = %socket.display(), %error, "refusing a shim socket in a folder that is not ours");
            false
        }
    }
}

/// The `/tmp` fallback directories for one instance, current spelling first.
///
/// Named after the *instance directory*, not the instance, so two servers with
/// the same instance name under different data directories — which is every pair
/// of integration tests — never share one.
fn runtime_dirs(dir: &Path) -> [PathBuf; 2] {
    use std::os::unix::ffi::OsStrExt;
    // SAFETY: `getuid` reads a process property and cannot fail.
    let uid = unsafe { libc::getuid() };
    let tag = hex_digest(dir.as_os_str().as_bytes(), 12);
    let tmp = PathBuf::from("/tmp");
    [
        tmp.join(format!("droppen-{uid}-{tag}")),
        // A shim of an earlier release bound here, and its socket is the only
        // way back to a live shell. See [`socket_candidates`].
        tmp.join(format!("remote-hand-{uid}-{tag}")),
    ]
}

/// Create whatever directory `socket` needs, if it is not `beside` — the
/// directory the caller's other sidecars already live in.
///
/// The `0o700`, the check of a directory that is already there, and why both
/// matter are [`Sidecars::ensure_socket_dir`]'s; this is that contract applied
/// to a path pair, plus the retry below.
///
/// # Why it may have to go round twice
///
/// The directory is shared by every shim of one instance and its *last* user
/// takes it with them ([`Sidecars::remove_all`], and the same removal in
/// `child::unlink_if_ours`). So a shim on its way out can unlink the last socket
/// and remove the directory in between this function finding it already there
/// and looking at it — which is not an error, it is two processes doing exactly
/// what they should in an unlucky order. A resident claude restarted for a
/// settings change hits it routinely, because the outgoing shim and the incoming
/// one are the only two there are.
pub fn ensure_socket_dir(socket: &Path, beside: &Path) -> std::io::Result<()> {
    let Some(parent) = socket.parent() else { return Ok(()) };
    if parent == beside {
        return Ok(());
    }
    for _ in 1..SOCKET_DIR_ATTEMPTS {
        match make_socket_dir(parent) {
            // The directory went away under us. Whoever removed it is on their
            // way out and will not do it twice, so making it again is the whole
            // of the recovery.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            settled => return settled,
        }
    }
    make_socket_dir(parent)
}

/// One attempt at [`ensure_socket_dir`]: make it, or accept what is there.
fn make_socket_dir(parent: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    match std::fs::DirBuilder::new().mode(SOCKET_DIR_MODE).create(parent) {
        Ok(()) => Ok(()),
        // Not recursive, so this really means "the last component is there" —
        // the everyday case, since every shim of one instance shares the
        // directory and the second one onwards finds it already made.
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => claim_existing_socket_dir(parent),
        Err(error) => Err(error),
    }
}

/// Accept a socket directory that already exists, or refuse to use it.
///
/// Three questions, and a "no" to any of them means somebody else got here
/// first: it must be a directory rather than a symlink pointing at one (which is
/// why the metadata is not followed), it must belong to this uid, and it must be
/// `0o700` exactly. Widening or re-owning it is not on the table — that would
/// race the very process being defended against — so the only answer left is to
/// refuse, loudly and with the path in the message, because the person who has
/// to clear it needs to know which path to look at.
fn claim_existing_socket_dir(parent: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let refuse = |why: &str| {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("refusing the shim socket directory {}: {why}", parent.display()),
        ))
    };
    let found = std::fs::symlink_metadata(parent)?;
    if !found.is_dir() {
        return refuse("it is not a directory");
    }
    // SAFETY: `getuid` reads a process property and cannot fail.
    let uid = unsafe { libc::getuid() };
    if found.uid() != uid {
        return refuse(&format!("it belongs to uid {} rather than {uid}", found.uid()));
    }
    let mode = found.mode() & 0o7777;
    if mode != SOCKET_DIR_MODE {
        return refuse(&format!("its mode is {mode:04o} rather than {SOCKET_DIR_MODE:04o}"));
    }
    Ok(())
}

/// The only mode the `/tmp` fallback directory is ever allowed to have.
///
/// Owner-only, and asserted in both directions: it is what the directory is
/// created with, and the exact value one that already exists has to match.
const SOCKET_DIR_MODE: u32 = 0o700;

/// How many times [`ensure_socket_dir`] will make the directory again after
/// finding it removed under it.
///
/// Two would do: the process that removes it is on its way out and does it once.
/// Three costs nothing and covers the case of two of them leaving at once.
const SOCKET_DIR_ATTEMPTS: usize = 3;

/// The extension the socket, tombstone and cursor files use.
const SOCK: &str = "sock";
const EXIT: &str = "exit";
const CURSOR: &str = "cursor";
/// The shim's last words. See [`Sidecars::stderr`].
const STDERR: &str = "stderr";
/// Two-part on purpose: `<id>.meta.json` is a JSON file a human may open.
const META: &str = "meta.json";

/// One terminal's four sidecar paths.
#[derive(Debug, Clone)]
pub struct Sidecars {
    dir: PathBuf,
    id: String,
    /// The socket a shim this server starts binds.
    socket: PathBuf,
    /// The socket adoption found, when the shim bound under the pre-rename
    /// fallback folder. Every later connect and cleanup must use the same
    /// file, so the choice is remembered rather than recomputed.
    adopted: Option<PathBuf>,
}

impl Sidecars {
    /// The sidecars for `id` in a `pty-shims` directory.
    pub fn new(dir: impl Into<PathBuf>, id: impl Into<String>) -> Self {
        let dir = dir.into();
        let id = id.into();
        let socket = socket_path(&dir, &id);
        Self {
            dir,
            id,
            socket,
            adopted: None,
        }
    }

    /// The terminal id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The unix socket the shim binds. See the module docs for why its name and
    /// sometimes its directory differ from the other three.
    pub fn socket(&self) -> PathBuf {
        self.adopted.clone().unwrap_or_else(|| self.socket.clone())
    }

    /// Every socket this terminal's shim may have bound, current spelling
    /// first.
    pub fn socket_candidates(&self) -> [PathBuf; 2] {
        socket_candidates(&self.dir, &self.id)
    }

    /// The socket of a shim that survived a restart, looked for under the
    /// current folder name and then the pre-rename one.
    ///
    /// `None` means no shim at all — or a socket in a folder that is not this
    /// user's, which [`socket_candidate_is_trusted`] refuses — and the caller
    /// reports it as "went away without leaving an exit file". The found path
    /// is remembered, so the reconnecting link and the final cleanup keep
    /// addressing the file the shim actually bound.
    pub fn adopt_socket(&mut self) -> Option<PathBuf> {
        let found = adopted_socket(&self.dir, &self.id)?;
        if found != self.socket {
            tracing::info!(
                id = %self.id,
                socket = %found.display(),
                "adopting a shim bound before the socket fallback folder was renamed"
            );
        }
        self.adopted = Some(found.clone());
        Some(found)
    }

    /// Create whatever directory the socket needs, if it is not the one the
    /// other sidecars already live in.
    ///
    /// `0o700`, and only ever under `/tmp`, so the `tmux`-style shared directory
    /// is readable by nobody but its owner. What is inside it is a socket that
    /// hands whoever connects a shell, or a claude with the user's credentials,
    /// so this is the only thing standing between another user of a shared
    /// machine and the user's whole account.
    ///
    /// The mode goes on at `mkdir` time rather than in a `chmod` afterwards:
    /// between the two there is a window, however short, in which the directory
    /// exists at the umask's mode and a socket can be dropped into it. And a
    /// directory that is already there is *checked* rather than reused — see
    /// [`claim_existing_socket_dir`] — because pre-creating a path under a
    /// predictable name is the easy half of the same attack. [`ensure_socket_dir`]
    /// adds one thing: the retry for a directory its last user removes mid-call.
    pub fn ensure_socket_dir(&self) -> std::io::Result<()> {
        ensure_socket_dir(&self.socket, &self.dir)
    }

    /// The tombstone the shim writes when the shell exits.
    pub fn exit(&self) -> PathBuf {
        self.dir.join(format!("{}.{EXIT}", self.id))
    }

    /// The server's record of how much output it has consumed.
    pub fn cursor(&self) -> PathBuf {
        self.dir.join(format!("{}.{CURSOR}", self.id))
    }

    /// Where the shim's own stderr goes.
    ///
    /// A panic in a shim's reader thread prints its message and location to
    /// stderr like any other Rust panic; sending that to a file costs nothing and
    /// is the whole difference between diagnosing the next one in a minute and
    /// reconstructing it from thread names. It is the shim's only log.
    ///
    /// Swept with the rest when it is empty, so a terminal that ends cleanly
    /// leaves nothing.
    pub fn stderr(&self) -> PathBuf {
        self.dir.join(format!("{}.{STDERR}", self.id))
    }

    /// The server's record of what this terminal *is*.
    pub fn meta(&self) -> PathBuf {
        self.dir.join(format!("{}.{META}", self.id))
    }

    /// Write the identity record. Must land before the shim is spawned: a shim
    /// whose meta is missing is indistinguishable from debris.
    pub async fn write_meta(&self, meta: &Meta) -> std::io::Result<()> {
        let encoded = serde_json::to_vec_pretty(meta).map_err(std::io::Error::other)?;
        tokio::fs::write(self.meta(), encoded).await
    }

    /// Read the identity record, or `None` if it is absent or unreadable.
    pub async fn read_meta(&self) -> Option<Meta> {
        let raw = tokio::fs::read(self.meta()).await.ok()?;
        serde_json::from_slice(&raw).ok()
    }

    /// The tombstone, if the shell has exited.
    pub async fn read_tombstone(&self) -> Option<Tombstone> {
        let raw = tokio::fs::read(self.exit()).await.ok()?;
        serde_json::from_slice(&raw).ok()
    }

    /// Whether a tombstone exists at all, however malformed.
    ///
    /// The corpse guard uses this rather than [`Sidecars::read_tombstone`]: a
    /// half-written exit file still means the shell is gone, and attaching to a
    /// corpse is worse than reporting an unknown status.
    pub fn tombstoned(&self) -> bool {
        self.exit().exists()
    }

    /// How far the server had consumed, or `0` when there is no record.
    pub async fn read_cursor(&self) -> u64 {
        tokio::fs::read_to_string(self.cursor())
            .await
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or(0)
    }

    /// Record how far the server has consumed.
    ///
    /// Best effort, and deliberately allowed to lag (see
    /// [`super::CURSOR_FLUSH_EVERY`]): re-delivering output a client has already
    /// seen is invisible after the repaint that follows it, whereas skipping
    /// output is not.
    pub fn write_cursor(&self, cursor: u64) {
        if let Err(error) = std::fs::write(self.cursor(), format!("{cursor}\n")) {
            tracing::debug!(id = %self.id, %error, "cannot record the shim cursor");
        }
    }

    /// Remove every file this terminal owns. Idempotent.
    pub fn remove_all(&self) {
        // The log goes only if it has nothing to say. A healthy shim writes
        // nothing to stderr, so the ordinary case leaves nothing behind; a shim
        // that panicked wrote the message and the line that did it, and that is
        // exactly the terminal most likely to be swept — by adoption deciding it
        // "would not answer". Deleting the explanation along with the evidence
        // would leave nothing to diagnose.
        if std::fs::metadata(self.stderr()).is_ok_and(|meta| meta.len() == 0) {
            let _ = std::fs::remove_file(self.stderr());
        }
        for path in [self.meta(), self.cursor(), self.exit(), self.socket()] {
            let _ = std::fs::remove_file(path);
        }
        // If the socket lived in the `/tmp` fallback, take the directory with
        // the last socket in it. The adopted spelling is what was removed
        // above, so it is also the one whose folder may now be empty.
        // `remove_dir` refuses a directory that still has contents, which is
        // exactly the check wanted here.
        let socket = self.socket();
        if let Some(parent) = socket.parent().filter(|parent| *parent != self.dir) {
            let _ = std::fs::remove_dir(parent);
        }
    }

    /// Whether `name` is this terminal's socket file, for the orphan sweep.
    pub fn owns_socket_named(&self, name: &str) -> bool {
        self.socket.file_name().and_then(|n| n.to_str()) == Some(name)
    }

    /// Remove the two files the server owns, leaving the shim's alone.
    ///
    /// Used the moment a terminal is killed: the meta is what a later boot would
    /// adopt from, so it must go before the shim has finished dying, or a crash
    /// in between would resurrect a terminal the user closed.
    pub fn remove_server_owned(&self) {
        for path in [self.meta(), self.cursor()] {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Every terminal id with at least one sidecar in `dir`.
///
/// Returns ids, not files, because adoption is per terminal and has to see a
/// terminal's files together — including the case where the only one present is
/// debris.
pub async fn scan(dir: &Path) -> Vec<String> {
    let mut ids = std::collections::BTreeSet::new();
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return Vec::new();
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        // Only the three id-named files that say a terminal *exists*; the
        // socket's stem is a short digest and cannot be turned back into an id.
        // Any socket without one of these beside it is swept by
        // [`sweep_orphan_sockets`].
        //
        // `.stderr` is deliberately not among them. It is a log, and a log with
        // no identity file beside it is the remains of a terminal that has
        // already been swept — resurrecting an id from one would list a terminal
        // that has nothing left to attach to.
        for suffix in [META, EXIT, CURSOR] {
            if let Some(id) = name.strip_suffix(&format!(".{suffix}")) {
                if !id.is_empty() {
                    ids.insert(id.to_string());
                }
                break;
            }
        }
    }
    ids.into_iter().collect()
}

/// Delete every `.sock` in `dir` that no live terminal claims.
///
/// A socket whose terminal has no identity file is debris from a crash between
/// spawning a shim and recording what it is. It cannot be adopted — there is
/// nothing to say what it was — and leaving it would make the next boot's scan
/// slower every time.
pub async fn sweep_orphan_sockets(dir: &Path, claimed: &[Sidecars]) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else { return };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.ends_with(&format!(".{SOCK}")) {
            continue;
        }
        if claimed.iter().any(|sidecars| sidecars.owns_socket_named(name)) {
            continue;
        }
        tracing::info!(socket = name, "sweeping a pty-shim socket with no identity");
        let _ = tokio::fs::remove_file(entry.path()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_terminal_is_found_by_any_of_its_files_and_only_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        let alive = Sidecars::new(dir.path(), "alive");
        let debris = Sidecars::new(dir.path(), "debris");

        alive
            .write_meta(&Meta {
                id: "alive".into(),
                title: "terminal 1".into(),
                cwd: "/tmp".into(),
                created_seq: 3,
                shell: "/bin/sh".into(),
                created_at: "2026-01-01T00:00:00.000Z".into(),
            })
            .await
            .expect("write meta");
        alive.write_cursor(4096);
        std::fs::write(debris.exit(), b"{}").expect("stale tombstone");

        assert_eq!(scan(dir.path()).await, vec!["alive".to_string(), "debris".to_string()]);
        assert_eq!(alive.read_cursor().await, 4096);
        assert_eq!(alive.read_meta().await.expect("meta").created_seq, 3);
        assert!(!alive.tombstoned());
        assert!(debris.read_meta().await.is_none(), "debris has no identity");
        assert!(alive.socket().to_string_lossy().ends_with(".sock"));

        // A missing cursor file reads as "start from the beginning", which is the
        // safe direction: duplicate output rather than lost output.
        assert_eq!(debris.read_cursor().await, 0);

        alive.remove_server_owned();
        assert_eq!(scan(dir.path()).await, vec!["debris".to_string()]);
        debris.remove_all();
        assert!(scan(dir.path()).await.is_empty());
        // Removing twice is not an error: cleanup runs on paths that may already
        // have been swept.
        debris.remove_all();
    }

    /// A directory path long enough that the socket cannot live beside the other
    /// sidecars, which is the only way into the `/tmp` fallback — paired with the
    /// `/tmp` directory it falls back to, swept clean.
    ///
    /// The sweep is not tidiness. The fallback path is a digest of this path, so
    /// it is the same on every run, and a run that failed halfway through would
    /// otherwise leave a directory behind that makes every later run fail in the
    /// same place for a reason that has nothing to do with the code.
    fn a_fallback_case(tag: &str) -> (PathBuf, PathBuf) {
        let instance = PathBuf::from(format!("/tmp/{}/{tag}", "d".repeat(90)));
        let [fallback, legacy] = runtime_dirs(&instance);
        // Both spellings: a case that leaves a socket or a symlink under the
        // old one would poison every later run of the same case, and some of
        // these cases deliberately put one there.
        for path in [&fallback, &legacy] {
            let _ = std::fs::remove_dir_all(path);
            let _ = std::fs::remove_file(path);
        }
        (instance, fallback)
    }

    /// A real socket at `path`, in a `0700` folder of its own, kept bound for
    /// as long as the listener lives — which is the shape adoption is allowed
    /// to trust.
    fn bind_in_private_folder(path: &Path) -> std::os::unix::net::UnixListener {
        use std::os::unix::fs::DirBuilderExt;
        let parent = path.parent().expect("a fallback folder");
        let _ = std::fs::remove_dir_all(parent);
        let _ = std::fs::remove_file(parent);
        std::fs::DirBuilder::new().mode(0o700).create(parent).expect("the fallback folder");
        std::os::unix::net::UnixListener::bind(path).expect("a real socket")
    }

    /// The compatibility surface: a shim spawned by an earlier release bound
    /// under `/tmp/remote-hand-…`, and adoption has to look there.
    #[test]
    fn a_fallback_socket_is_probed_under_the_new_folder_then_the_old_one() {
        let id = "3f2504e04f8911d39a0c0305e82c3301";
        let (instance, _) = a_fallback_case("candidates");
        let [current, legacy] = socket_candidates(&instance, id);
        assert!(current.to_string_lossy().starts_with("/tmp/droppen-"), "{}", current.display());
        assert!(legacy.to_string_lossy().starts_with("/tmp/remote-hand-"), "{}", legacy.display());
        assert_eq!(current.file_name(), legacy.file_name(), "the socket's name did not move, only its folder");

        // A directory with room beside the sidecars has one spelling, not two.
        let dir = tempfile::tempdir().expect("tempdir");
        let [current, legacy] = socket_candidates(dir.path(), id);
        assert_eq!(current, legacy);
        assert_eq!(current, socket_path(dir.path(), id));
    }

    /// A shim adopted from the old folder keeps being addressed there: every
    /// later connect and the final cleanup must not switch to the new one.
    #[test]
    fn adoption_finds_a_socket_left_under_the_old_folder_and_remembers_it() {
        let id = "3f2504e04f8911d39a0c0305e82c3301";
        let (instance, _) = a_fallback_case("adopted");
        let [_, legacy] = socket_candidates(&instance, id);
        let listener = bind_in_private_folder(&legacy);

        let mut sidecars = Sidecars::new(&instance, id);
        assert_eq!(sidecars.adopt_socket(), Some(legacy.clone()));
        assert_eq!(sidecars.socket(), legacy, "the adopted path is the one every caller gets");

        let folder = legacy.parent().expect("old fallback folder").to_path_buf();
        sidecars.remove_all();
        assert!(!legacy.exists(), "cleanup takes the socket it adopted");
        assert!(!folder.exists(), "and the folder with its last socket");
        drop(listener);
    }

    /// A folder another local user could have made first is not a place to
    /// send a shell's keystrokes. Widened or symlinked, it is refused — and
    /// the socket *in* it, however real, is never adopted.
    #[test]
    fn a_socket_in_a_folder_that_is_not_ours_is_not_adopted() {
        use std::os::unix::fs::PermissionsExt;

        let id = "3f2504e04f8911d39a0c0305e82c3301";

        let (squatted, _) = a_fallback_case("squatted-adoption");
        let [_, legacy] = socket_candidates(&squatted, id);
        let folder = legacy.parent().expect("old fallback folder").to_path_buf();
        std::fs::create_dir(&folder).expect("pre-create the old folder");
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o777)).expect("widen it");
        let listener = std::os::unix::net::UnixListener::bind(&legacy).expect("a real socket in it");
        let mut sidecars = Sidecars::new(&squatted, id);
        assert_eq!(sidecars.adopt_socket(), None, "a world-writable folder is not trusted");
        drop(listener);
        std::fs::remove_dir_all(&folder).expect("clean up");

        let elsewhere = tempfile::tempdir().expect("tempdir");
        let (symlinked, _) = a_fallback_case("symlinked-adoption");
        let [_, legacy] = socket_candidates(&symlinked, id);
        let folder = legacy.parent().expect("old fallback folder").to_path_buf();
        let target = elsewhere.path().join(legacy.file_name().expect("socket name"));
        let listener = std::os::unix::net::UnixListener::bind(&target).expect("a real socket behind it");
        std::os::unix::fs::symlink(elsewhere.path(), &folder).expect("symlink the folder");
        let mut sidecars = Sidecars::new(&symlinked, id);
        assert_eq!(sidecars.adopt_socket(), None, "a symlinked folder is not trusted");
        drop(listener);
        std::fs::remove_file(&folder).expect("clean up the symlink");
    }

    /// The current spelling wins when both exist, so an upgrade does not keep
    /// dialling an old shim while a new one is already there for the same id.
    #[test]
    fn the_current_folder_wins_when_both_spellings_have_a_socket() {
        let id = "3f2504e04f8911d39a0c0305e82c3301";
        let (instance, _) = a_fallback_case("both");
        let [current, legacy] = socket_candidates(&instance, id);
        let current_listener = bind_in_private_folder(&current);
        let legacy_listener = bind_in_private_folder(&legacy);

        let mut sidecars = Sidecars::new(&instance, id);
        assert_eq!(sidecars.adopt_socket(), Some(current.clone()), "the current spelling is tried first");
        assert_eq!(sidecars.socket(), current);

        let legacy_folder = legacy.parent().expect("old folder").to_path_buf();
        sidecars.remove_all();
        assert!(!current.exists(), "cleanup takes the socket it adopted");
        assert!(legacy.exists(), "the other spelling belongs to the shim that bound it");
        drop(current_listener);
        drop(legacy_listener);
        std::fs::remove_dir_all(&legacy_folder).expect("clean up the old folder");
    }

    /// A squatted *current* folder must not shadow a trustworthy socket left
    /// under the old name: the checks apply to both candidates, not only the
    /// legacy one.
    #[test]
    fn a_squatted_current_folder_does_not_shadow_a_trustworthy_old_socket() {
        use std::os::unix::fs::PermissionsExt;

        let id = "3f2504e04f8911d39a0c0305e82c3301";
        let (instance, current_dir) = a_fallback_case("mixed");
        let [current, legacy] = socket_candidates(&instance, id);
        std::fs::create_dir(&current_dir).expect("pre-create the current folder");
        std::fs::set_permissions(&current_dir, std::fs::Permissions::from_mode(0o777)).expect("widen it");
        let squatted = std::os::unix::net::UnixListener::bind(&current).expect("a real socket in it");
        let legacy_listener = bind_in_private_folder(&legacy);

        let mut sidecars = Sidecars::new(&instance, id);
        assert_eq!(sidecars.adopt_socket(), Some(legacy.clone()), "the trustworthy spelling is used");

        sidecars.remove_all();
        assert!(!legacy.exists(), "cleanup takes the socket it adopted");
        assert!(current.exists(), "the squatted socket is not touched");
        drop(squatted);
        drop(legacy_listener);
        std::fs::remove_dir_all(&current_dir).expect("clean up the squatted folder");
    }

    /// The `/tmp` fallback is the one sidecar that leaves the instance
    /// directory, and what sits inside it is a socket that hands out a shell. So
    /// it is created owner-only, and one that is already there is inspected
    /// rather than trusted.
    ///
    /// The owner check cannot be exercised here: proving that a directory
    /// belonging to *another* uid is refused needs a second uid, which a test
    /// running as one unprivileged user has no way to conjure. What is asserted
    /// instead is everything that can be: the mode on creation, the refusal of a
    /// wide mode, and the refusal of a symlink — which is how a directory that
    /// looks like ours comes to have somebody else's contents.
    #[test]
    fn the_tmp_socket_directory_is_owner_only_and_never_adopted_from_a_stranger() {
        use std::os::unix::fs::PermissionsExt;

        let id = "3f2504e04f8911d39a0c0305e82c3301";
        let mode_of = |path: &Path| std::fs::symlink_metadata(path).expect("stat").permissions().mode() & 0o7777;

        let (fresh, dir) = a_fallback_case("fresh");
        let sidecars = Sidecars::new(&fresh, id);
        assert_eq!(sidecars.socket().parent(), Some(dir.as_path()), "this case is about the fallback");

        sidecars.ensure_socket_dir().expect("create the fallback directory");
        assert_eq!(mode_of(&dir), 0o700, "created owner-only, not at whatever the umask allows");
        // Every terminal of one instance shares this directory, so finding it
        // already made is the common case rather than the suspicious one.
        sidecars.ensure_socket_dir().expect("a directory we made ourselves is ours to reuse");
        std::fs::remove_dir_all(&dir).expect("clean up");

        let (squatted, dir) = a_fallback_case("squatted");
        let sidecars = Sidecars::new(&squatted, id);
        std::fs::create_dir(&dir).expect("pre-create");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).expect("widen");
        let error = sidecars.ensure_socket_dir().expect_err("a world-writable directory must be refused");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(
            error.to_string().contains(&dir.display().to_string()),
            "the refusal names the path: {error}"
        );
        assert_eq!(mode_of(&dir), 0o777, "and it is refused rather than quietly repaired");
        std::fs::remove_dir_all(&dir).expect("clean up");

        let elsewhere = tempfile::tempdir().expect("tempdir");
        let (symlinked, dir) = a_fallback_case("symlinked");
        let sidecars = Sidecars::new(&symlinked, id);
        std::os::unix::fs::symlink(elsewhere.path(), &dir).expect("symlink");
        let error = sidecars.ensure_socket_dir().expect_err("a symlink is not a directory we made");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        std::fs::remove_file(&dir).expect("clean up");
    }

    /// The directory's last user takes it with them, so a shim starting while
    /// one leaves can find it gone between `mkdir` saying it is already there
    /// and the check that it is ours. That is two processes behaving correctly
    /// in an unlucky order, and it would cost a whole session: a resident
    /// claude restarted for a settings change is exactly the case where the
    /// outgoing shim and the incoming one are the only two there are.
    ///
    /// The removal is simulated rather than raced, because a test that raced it
    /// would fail only sometimes.
    #[test]
    fn a_socket_directory_removed_by_the_shim_leaving_is_simply_made_again() {
        let id = "3f2504e04f8911d39a0c0305e82c3301";
        let (instance, dir) = a_fallback_case("vanishing");
        let sidecars = Sidecars::new(&instance, id);
        assert_eq!(sidecars.socket().parent(), Some(dir.as_path()), "this case is about the fallback");

        // Exactly what an outgoing shim leaves behind at the moment it removes
        // the directory: `mkdir` will say it is there, and it will not be.
        std::fs::create_dir(&dir).expect("pre-create");
        std::fs::remove_dir(&dir).expect("and take it away again");

        sidecars.ensure_socket_dir().expect("a directory that has just gone is made again");
        assert!(dir.is_dir(), "and it really is there afterwards");
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[tokio::test]
    async fn a_malformed_tombstone_still_says_the_shell_is_gone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sidecars = Sidecars::new(dir.path(), "half-written");
        std::fs::write(sidecars.exit(), b"{\"exit_c").expect("write");
        assert!(sidecars.tombstoned(), "the file's existence is the fact");
        assert!(sidecars.read_tombstone().await.is_none(), "but its contents are not readable");
    }
}
