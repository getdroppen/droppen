//! The one spelling of "which session, on which server".
//!
//! A session is identified by a `(source, id)` pair, and that pair used to be
//! written out with `format!` at eight different call sites — three of them
//! deliberately different from the other five. Nothing said which was which, so
//! nothing stopped a sixth site from picking the wrong one, and a wrong key is
//! not a compile error: it is a row nobody reads any more.
//!
//! So the pair lives here, in [`SessionRef`], with **exactly four** spellings
//! and a doc comment on each saying what makes it that shape. The interesting
//! part is which of them carry the [`ServerId`] and which refuse to:
//!
//! | spelling | carries the server | because |
//! |---|---|---|
//! | [`SessionRef::scoped_key`] | yes | in-memory only; it never has to be read back by anyone else |
//! | [`SessionRef::local_key`] | no | it is on the wire, in a token, and in per-machine rows |
//! | [`SessionRef::title_key`] | no | a name a person gave a session is not one server's |
//! | [`SessionRef::lock_name`] | **must not** | the lock exists so two servers on one machine contend |
//!
//! The last row is the one worth stating twice. `inject-locks/` lives in the
//! *shared* data directory precisely so a second instance is excluded from a
//! session the first is driving; putting the server in the filename would give
//! each of them their own lock, they would both acquire it, and two CLIs would
//! append to one transcript. See [`crate::inject::lock`].
//!
//! Three of the four are also free functions, because three of their call sites
//! have no server to name and — per the table above — must not name one:
//! `crate::attention::seen::key_of` is called from a token mint that holds only
//! a `(source, id)`, and `crate::inject::lock::acquire` takes the source as a
//! `&str` on purpose so the separator property can be tested against sources
//! that do not exist. The method bodies *are* the free functions, so there is
//! still one `format!` per spelling.

use sha2::{Digest, Sha256};

use crate::sessions::{Paths, Source};

/// How long a session id may be before [`stem`] hashes it instead.
pub(crate) const MAX_STEM: usize = 64;

/// The name of one remote-hand server.
///
/// A newtype rather than a `String` because the name is not free-form: it is
/// the instance name, it becomes a path segment under the data directory, and
/// [`crate::instance::valid_name`] is the one definition of what may be one.
/// Validating on construction means every later use — a map key, a scoped
/// session key, a field on `Settings` — is holding something that already
/// passed.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ServerId(String);

impl ServerId {
    /// A server id, or `None` if [`crate::instance::valid_name`] refuses it.
    pub fn new(name: &str) -> Option<Self> {
        crate::instance::valid_name(name).then(|| Self(name.to_string()))
    }

    /// This process' own server, named by the instance its paths are rooted at.
    ///
    /// A running server's instance name has already been through
    /// [`crate::instance::InstanceDirs`], so the validation here cannot fail in
    /// production. The fallback exists for the synthetic `Paths` a unit test
    /// builds, and names the default server rather than panicking — the keys it
    /// would appear in are in-memory ones that die with the test.
    pub fn local(paths: &Paths) -> Self {
        Self::new(paths.instance()).unwrap_or_default()
    }

    /// The name, as it is spelled everywhere.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ServerId {
    /// The server a process with nothing to say about instances is.
    fn default() -> Self {
        Self(crate::instance::DEFAULT_INSTANCE.to_string())
    }
}

impl std::fmt::Display for ServerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One session, on one server.
///
/// Construct it once and ask it for the spelling the call site needs, rather
/// than writing a `format!` that looks right.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionRef {
    /// Which remote-hand server this reference is being made from.
    pub server: ServerId,
    /// Which CLI the session belongs to.
    pub source: Source,
    /// The CLI's own id for the session.
    pub id: String,
}

impl SessionRef {
    /// A reference to `id` under `source`, as seen from `server`.
    pub fn new(server: ServerId, source: Source, id: impl Into<String>) -> Self {
        Self {
            server,
            source,
            id: id.into(),
        }
    }

    /// `{server}/{source}/{id}` — **in-memory maps only, nothing durable.**
    ///
    /// The server belongs in this one because the maps it keys — the injection
    /// registry's `runs` and `residents`, the feed registry, the `DriverStates`
    /// snapshot taken from them — are private to a process and die with it. No
    /// other server reads them and no restart reads them back, so the prefix
    /// costs nothing today and is what lets a later slice hold another server's
    /// sessions in the same map without the two colliding.
    ///
    /// Write it to disk or put it on the wire and that stops being true: the
    /// key would name the server that happened to write it, and a rename or a
    /// second instance would orphan every row. Use [`Self::local_key`] there.
    pub fn scoped_key(&self) -> String {
        format!("{}/{}/{}", self.server, self.source, self.id)
    }

    /// `{source}/{id}` — the durable, machine-wide spelling.
    ///
    /// Deliberately server-less. It is what the `removed:` array of the global
    /// feed's `sessions:delta` frame carries, so it is a contract with the web
    /// client; it is the `tag` a push notification is collapsed on and the
    /// subject a notify token is minted against; and it keys rows — `seen`,
    /// `composer`, the sidebar pins — that either belong to the machine rather
    /// than to one server, or already sit in a per-instance database and would
    /// only be saying the instance twice.
    pub fn local_key(&self) -> String {
        local_key(self.source, &self.id)
    }

    /// `{source}:{id}` — the same pair, with a colon, for titles.
    ///
    /// A colon and not a slash purely because that is what the `titles` rows on
    /// disk are already keyed by, on two production servers. The separator
    /// carries no meaning the slash does not; changing it would rename every
    /// session anyone has ever renamed back to its default.
    ///
    /// Server-less for the same reason [`Self::local_key`] is, and then some: a
    /// title is a name a person gave a session, and it is the same name from
    /// whichever server they open it on.
    pub fn title_key(&self) -> String {
        title_key(self.source, &self.id)
    }

    /// `{source}--{stem}.lock` — the claim on a session, **ignoring the server.**
    ///
    /// This one does not read [`Self::server`] at all, and that omission is the
    /// whole point of the file. `<data>/inject-locks/` is shared by every
    /// instance on the machine so that a second server is *refused* a session
    /// the first is driving. A server-scoped filename would give each of them
    /// their own file, both `flock`s would succeed, and two CLIs would append
    /// to one transcript — the exact interleaving the lock exists to prevent.
    /// `two_servers_contend_for_one_session_lock` pins it.
    ///
    /// Two dashes because one is already part of a session id's own alphabet;
    /// see [`lock_name`].
    pub fn lock_name(&self) -> String {
        lock_name(self.source.as_str(), &self.id)
    }
}

/// `{source}/{id}`, for a caller that has no server and must not invent one.
///
/// The body of [`SessionRef::local_key`]; see there for why the server is out.
pub fn local_key(source: Source, id: &str) -> String {
    format!("{source}/{id}")
}

/// `{source}:{id}`, for a caller that has no server and must not invent one.
///
/// The body of [`SessionRef::title_key`].
pub fn title_key(source: Source, id: &str) -> String {
    format!("{source}:{id}")
}

/// `{source}--{stem}.lock`, the body of [`SessionRef::lock_name`].
///
/// Two dashes, because a single one is already part of a session id's own
/// alphabet: `<source>-<session>` cannot say which side of a dash it sits on,
/// so `("claude-code", "x")` and `("claude", "code-x")` would be one file and
/// one session's lock would exclude the other's. Nothing collides *today* —
/// there are two sources, `claude` and `codex`, and neither contains a dash —
/// which is precisely the accident worth not depending on: the separator is
/// what should keep the pairs apart, not the current spelling of the enum. The
/// source is taken as a string for the same reason, so the property can be
/// tested rather than argued from the variants that happen to exist.
///
/// Renaming these files needs no migration. They are per-boot state, not a
/// persisted format: an `flock` means something only while the process holding
/// it is alive, so a leftover file from an older server is debris rather than
/// data, and the first server to want the session creates the name it expects.
/// The one thing the rename could not survive is two servers of *different*
/// versions running against one machine at the same time — they would not
/// contend — and that is not a state remote-hand has: a self-upgrade replaces
/// the binary and restarts, so the old server is gone before the new one runs.
pub fn lock_name(source: &str, session_id: &str) -> String {
    format!("{source}--{}.lock", stem(session_id))
}

/// The filename stem for a session id.
///
/// Session ids are uuids and would pass through untouched, but the id reaches
/// this function from a URL path, so "would" is not good enough: anything that
/// is not plainly a filename becomes a digest rather than being sanitised in
/// place. Two ids must never collapse onto one stem — that would hand one
/// session's lock to another — so the fallback is a hash of the whole id, not a
/// truncation of it.
pub(crate) fn stem(session_id: &str) -> String {
    let plain =
        !session_id.is_empty() && session_id.len() <= MAX_STEM && session_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if plain {
        return session_id.to_string();
    }
    let digest = Sha256::digest(session_id.as_bytes());
    let mut out = String::with_capacity(32);
    for byte in digest.iter().take(16) {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Whether `id` may be one path segment of its own.
///
/// The rule for an id that is joined as a directory or file *name*, not merely
/// part of a `<id>.jsonl`: ASCII alphanumerics, `_` or `-`, 1 to 128 bytes, and
/// no leading `-`. Dots are refused outright, so `..` cannot walk out of the
/// directory it is joined to; a leading `-` is refused so a file name can never
/// read as an option to a command.
///
/// This is the strictest of the rules the path-segment checks carried before
/// they were shared. Every id a supported CLI writes is a uuid (`claude` and
/// `codex` both mint them, and `codex`'s rollout file names are matched against
/// a hex-uuid pattern), so nothing legitimate is refused: a uuid is 36 ASCII
/// bytes, contains neither a dot nor a non-ASCII character, and does not start
/// with `-`.
pub(crate) fn is_path_segment_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') && !id.starts_with('-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(server: &str, source: Source, id: &str) -> SessionRef {
        SessionRef::new(ServerId::new(server).expect("a valid server name"), source, id)
    }

    /// The four spellings, pinned to their literal output.
    ///
    /// Every one of these strings is either in a live database on a production
    /// server, on the wire to the web client, or a filename two processes
    /// contend for. A change here is not a refactor; it orphans rows.
    #[test]
    fn each_spelling_is_exactly_the_string_it_has_always_been() {
        let claude = reference("alice", Source::Claude, "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa");
        assert_eq!(claude.scoped_key(), "alice/claude/aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa");
        assert_eq!(claude.local_key(), "claude/aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa");
        assert_eq!(claude.title_key(), "claude:aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa");
        assert_eq!(claude.lock_name(), "claude--aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa.lock");

        let codex = reference("bob", Source::Codex, "new-1234");
        assert_eq!(codex.scoped_key(), "bob/codex/new-1234");
        assert_eq!(codex.local_key(), "codex/new-1234");
        assert_eq!(codex.title_key(), "codex:new-1234");
        assert_eq!(codex.lock_name(), "codex--new-1234.lock");

        // The free functions are the method bodies, so they cannot drift.
        assert_eq!(local_key(Source::Codex, "new-1234"), codex.local_key());
        assert_eq!(title_key(Source::Codex, "new-1234"), codex.title_key());
        assert_eq!(lock_name(Source::Codex.as_str(), "new-1234"), codex.lock_name());
    }

    /// The regression this type exists to make impossible.
    ///
    /// Two servers on one machine share `<data>/inject-locks/`, and they share
    /// it so that the second is refused a session the first is driving. If the
    /// server ever reached the filename they would claim two different files,
    /// both `flock`s would succeed, and two CLIs would write one transcript.
    #[test]
    fn two_servers_contend_for_one_session_lock() {
        let here = reference("alice", Source::Claude, "shared-session");
        let there = reference("bob", Source::Claude, "shared-session");

        assert_ne!(here.server, there.server, "two different servers");
        assert_ne!(here.scoped_key(), there.scoped_key(), "and their in-memory maps stay apart");
        assert_eq!(
            here.lock_name(),
            there.lock_name(),
            "but they must contend for one lock file, or both will drive the session"
        );

        // The other two durable spellings are server-free for their own
        // reasons, and a second server must read back what the first wrote.
        assert_eq!(here.local_key(), there.local_key());
        assert_eq!(here.title_key(), there.title_key());
    }

    /// A server id is an instance name, and nothing else.
    #[test]
    fn a_server_id_admits_exactly_what_an_instance_name_does() {
        for good in ["default", "alice", "a", "ci-2", "0", &"x".repeat(32)] {
            assert_eq!(ServerId::new(good).as_ref().map(ServerId::as_str), Some(good), "{good:?}");
            assert!(crate::instance::valid_name(good));
        }
        for bad in ["", "Default", "with space", ".", "..", "a/b", "under_score", &"x".repeat(33)] {
            assert!(ServerId::new(bad).is_none(), "{bad:?} must be refused");
            assert!(!crate::instance::valid_name(bad), "{bad:?}");
        }
    }

    /// The local server is the instance its path set is rooted at.
    #[test]
    fn the_local_server_is_the_instance_the_paths_name() {
        let home = std::path::PathBuf::from("/home/someone");
        assert_eq!(ServerId::local(&Paths::for_instance(home.clone(), "ci-1")).as_str(), "ci-1");
        assert_eq!(ServerId::local(&Paths::new(home)).as_str(), crate::instance::DEFAULT_INSTANCE);
        assert_eq!(ServerId::default().as_str(), crate::instance::DEFAULT_INSTANCE);
    }
}
