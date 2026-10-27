//! Putting `droppen-notify` and its skill inside a session.
//!
//! Two things have to be true before an agent can nudge, and they are separate
//! on purpose.
//!
//! * **The program has to exist and be reachable.** A directory of one file,
//!   prepended to the spawn's `PATH`. That part is unconditional in the sense
//!   that matters: whenever a channel is configured, every CLI this server
//!   starts can run `droppen-notify`, whichever CLI it is and whichever Claude root
//!   it uses.
//! * **The agent has to know it is there, and when to reach for it.** That is
//!   the skill, and it is written into the Claude root this server *owns*. It is
//!   never written into the user's own `~/.claude`: that directory is theirs, it
//!   is shared with every other thing they run `claude` for, and a server that
//!   scattered files through it would be a server you could not uninstall. A
//!   session on the local login therefore gets the command and not the skill —
//!   which is the honest trade, and is what [`skill_root`] exists to make
//!   explicit rather than accidental.
//!
//! Both are refreshed on every spawn rather than installed once. The program
//! names the server binary by absolute path, and a self-upgrade moves that
//! binary; the skill's text changes when this file does. Rewriting two small
//! files immediately before starting a process that is about to cost seconds is
//! not a cost worth optimising, and "the file on disk is what this build meant"
//! is worth a great deal when the alternative is an install that is one upgrade
//! stale for the rest of its life.
//!
//! Nothing in here can fail a spawn. A session that cannot notify is a session;
//! a session that would not start because a notification could not be set up is
//! a bug with a very unhappy user attached.

use std::path::{Path, PathBuf};

use crate::instance::InstanceDirs;
use crate::process_env::Env;
use crate::sessions::{Paths, Source};

use super::{cli, store, token};

/// The skill's name, which is also its directory and how the CLI lists it.
const SKILL_NAME: &str = cli::NAME;

/// `<instance>/session-bin` — the one directory this server adds to a spawn's
/// `PATH`.
///
/// Instance-scoped, like every other directory a spawned process is pointed at:
/// the script inside it names *this* server's binary and carries *this*
/// instance's tokens, so a sibling instance finding it on a shared path would
/// be a session notifying through a server that does not own it.
pub fn bin_dir(instance: &InstanceDirs) -> PathBuf {
    instance.root().join("session-bin")
}

/// Where a Claude spawn's skills are discovered, given the environment it will
/// run with — or `None` when that is somewhere this server will not write.
///
/// Claude Code reads personal skills out of `$CLAUDE_CONFIG_DIR/skills`, and
/// [`crate::inject`]'s `owned_claude_env` sets that variable to the root this
/// instance owns. So the presence of the variable in the spawn environment is
/// exactly the question "is this a root remote-hand owns" — which is the
/// question that decides whether writing a file into it is housekeeping or
/// trespass. A spawn with no `CLAUDE_CONFIG_DIR` is running against the user's
/// own `~/.claude` and gets `None`.
pub fn skill_root(env: &Env) -> Option<PathBuf> {
    let dir = env.get("CLAUDE_CONFIG_DIR").filter(|value| !value.is_empty())?;
    Some(PathBuf::from(dir).join("skills").join(SKILL_NAME))
}

/// The one line of shell between an agent and [`cli::run`].
///
/// `"$@"` is the whole trick: it is the only construct in POSIX shell that
/// passes arguments through without re-splitting or re-expanding them, so a
/// title full of quotes, backticks and `$` reaches `serde_json` as the string
/// the model wrote. See [`cli`] for why that matters enough to spend a process
/// on.
///
/// Pure, so the quoting can be asserted against a path with a quote in it —
/// which is not hypothetical: `$HOME` is whatever the account is called.
pub fn program_text(binary: &Path) -> String {
    // POSIX single quotes take everything literally except a single quote,
    // which is escaped by ending the string, adding an escaped quote, and
    // starting a new one.
    let quoted = binary.to_string_lossy().replace('\'', r"'\''");
    let name = cli::NAME;
    format!(
        "#!/bin/sh\n\
         # {name} — tell the person running Droppen something worth being\n\
         # interrupted for. Written by Droppen when this session started;\n\
         # edits are overwritten on the next spawn.\n\
         #\n\
         # usage: {name} <title> [body] [--level passive|active|time-sensitive]\n\
         exec '{quoted}' notify \"$@\"\n"
    )
}

/// Write the program, and hand back the directory it is in.
async fn ensure_program(instance: &InstanceDirs) -> std::io::Result<PathBuf> {
    let dir = bin_dir(instance);
    tokio::fs::create_dir_all(&dir).await?;
    let binary = crate::pty_shim::shim_program();
    write_executable(&dir.join(cli::NAME), program_text(&binary).as_bytes()).await?;
    Ok(dir)
}

/// Create-or-truncate at `0755`.
///
/// Nothing secret is in it — the token is in the environment, not the file — so
/// this is the ordinary mode an executable has. It is set on every write rather
/// than only on creation, for the reason [`crate::push::store`] gives about the
/// other direction: `open`'s mode applies to a fresh inode only, and a file left
/// behind by an earlier build keeps whatever it had.
#[cfg(unix)]
async fn write_executable(path: &Path, body: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::AsyncWriteExt;

    let mut handle = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o755)
        .open(path)
        .await?;
    handle.write_all(body).await?;
    handle.flush().await?;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).await
}

#[cfg(not(unix))]
async fn write_executable(path: &Path, body: &[u8]) -> std::io::Result<()> {
    tokio::fs::write(path, body).await
}

/// The skill, verbatim.
///
/// Kept in the binary rather than on disk so an installed server and the build
/// that made it can never disagree about what the agent was told, and so a
/// half-written file cannot survive a crash as a skill that says something else.
pub const SKILL: &str = include_str!("SKILL.md");

/// Put the skill in a root, or take it out again.
///
/// Removal matters as much as installation and is the reason this is one
/// function: a user who turns notifications off has to stop being offered a
/// tool that can only refuse, and the next spawn is the first moment anything
/// runs that could tidy up after them.
async fn set_skill(root: &Path, wanted: bool) -> std::io::Result<()> {
    if !wanted {
        return remove_dir(root).await;
    }
    tokio::fs::create_dir_all(root).await?;
    tokio::fs::write(root.join("SKILL.md"), SKILL).await
}

/// Remove a directory and what is in it; one that is not there is fine.
async fn remove_dir(dir: &Path) -> std::io::Result<()> {
    match tokio::fs::remove_dir_all(dir).await {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// `PATH` with `dir` in front of it.
///
/// In front rather than behind, so a `droppen-notify` earlier on the path — there is
/// no reason for one to exist, but "no reason" is not "cannot" — does not
/// silently become the one an agent reaches. Split out so the join is asserted
/// on the case that actually bites: an environment with no `PATH` at all, where
/// a naive `format!` leaves a trailing colon, and a trailing colon on `PATH`
/// means the current directory.
pub fn path_with(dir: &Path, existing: Option<&str>) -> String {
    let dir = dir.to_string_lossy().into_owned();
    match existing.filter(|value| !value.is_empty()) {
        Some(existing) => format!("{dir}:{existing}"),
        None => dir,
    }
}

/// Give a spawn everything it needs to nudge, or leave it exactly as it was.
///
/// Called immediately before a CLI is started, with the environment that CLI
/// will run with. Every failure inside is logged and dropped: see the module
/// note on why none of them may reach the caller.
pub async fn decorate(paths: &Paths, instance: &InstanceDirs, source: Source, id: &str, env: &mut Env) {
    let live = store::read(paths).await.is_live();

    // The skill is settled either way, because "either way" is the point: a
    // channel that has been turned off leaves a directory behind that would go
    // on advertising a tool to every future session.
    if let Some(root) = skill_root(env) {
        if let Err(error) = set_skill(&root, live).await {
            tracing::warn!(%error, root = %root.display(), "notify: cannot settle the droppen-notify skill");
        }
    }
    if !live {
        return;
    }

    let secret = match store::secret(paths).await {
        Ok(secret) => secret,
        Err(error) => {
            tracing::warn!(%error, "notify: no usable token secret; this session cannot notify");
            return;
        }
    };
    let dir = match ensure_program(instance).await {
        Ok(dir) => dir,
        Err(error) => {
            tracing::warn!(%error, "notify: cannot install droppen-notify; this session cannot notify");
            return;
        }
    };
    let Some(url) = notify_url(instance) else {
        tracing::warn!("notify: this server has not written its port; this session cannot notify");
        return;
    };

    let key = crate::attention::seen::key_of(source, id);
    env.insert("PATH".to_string(), path_with(&dir, env.get("PATH").map(String::as_str)));
    let token = token::mint(&secret, instance.name(), &key);
    // Under both names: whatever binary runs `notify` later may be from before
    // the rename to Droppen (a rollback), and an older one reads only the old.
    for name in crate::env::with_old_name(cli::URL_ENV) {
        env.insert(name.to_string(), url.clone());
    }
    for name in crate::env::with_old_name(cli::TOKEN_ENV) {
        env.insert(name.to_string(), token.clone());
    }
}

/// Where this server is listening, as an address a child on this host can reach.
///
/// Read from the port file rather than remembered, because the port file is
/// written by the owner after the bind and is therefore the only account that is
/// true for a server started on port 0. Loopback rather than the configured
/// host: a server binds `127.0.0.1` or `0.0.0.0`, and a child on the same
/// machine reaches both there — while `0.0.0.0` is not an address anything can
/// connect *to*.
fn notify_url(instance: &InstanceDirs) -> Option<String> {
    let raw = std::fs::read_to_string(instance.port_file()).ok()?;
    let port: u16 = raw.trim().parse().ok()?;
    Some(format!("http://127.0.0.1:{port}{}", super::NOTIFY_PATH))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_program_is_one_line_of_shell_and_quotes_its_binary() {
        let text = program_text(Path::new("/usr/local/bin/remote-hand-server"));
        assert!(text.starts_with("#!/bin/sh\n"));
        assert!(text.contains("# usage: droppen-notify <title>"), "{text}");
        assert!(text.ends_with("exec '/usr/local/bin/remote-hand-server' notify \"$@\"\n"), "{text}");

        // A home directory with a quote in it is a home directory.
        let text = program_text(Path::new("/Users/o'brien/bin/remote-hand-server"));
        assert!(text.contains(r"exec '/Users/o'\''brien/bin/remote-hand-server' notify"), "{text}");
        // Whatever the path, the arguments are passed through untouched.
        assert!(text.contains("\"$@\""), "re-splitting the arguments is the bug this exists to avoid");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_script_runs_and_passes_its_arguments_through() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A stand-in for the server binary: prints the arguments it was given.
        let binary = dir.path().join("fake-server");
        std::fs::write(&binary, "#!/bin/sh\nprintf '%s|%s\\n' \"$1\" \"$2\"\n").expect("fake");
        std::fs::set_permissions(&binary, std::os::unix::fs::PermissionsExt::from_mode(0o755)).expect("chmod");
        let script = dir.path().join(cli::NAME);
        write_executable(&script, program_text(&binary).as_bytes()).await.expect("write");
        let out = std::process::Command::new("/bin/sh")
            .arg(&script)
            .arg("a title with $HOME and `ticks`")
            .output()
            .expect("run");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "notify|a title with $HOME and `ticks`\n"
        );
    }

    #[test]
    fn the_skill_only_goes_into_a_root_this_server_owns() {
        let mut env = Env::new();
        assert_eq!(skill_root(&env), None, "a session on the user's own ~/.claude is left alone");

        env.insert("CLAUDE_CONFIG_DIR".into(), String::new());
        assert_eq!(skill_root(&env), None, "and so is an empty override");

        env.insert("CLAUDE_CONFIG_DIR".into(), "/data/instances/default/claude".into());
        assert_eq!(
            skill_root(&env),
            Some(PathBuf::from("/data/instances/default/claude/skills/droppen-notify"))
        );
    }

    #[test]
    fn the_bin_directory_goes_in_front_and_never_leaves_an_empty_entry() {
        let dir = Path::new("/data/instances/default/session-bin");
        assert_eq!(path_with(dir, Some("/usr/bin:/bin")), "/data/instances/default/session-bin:/usr/bin:/bin");
        assert_eq!(
            path_with(dir, None),
            "/data/instances/default/session-bin",
            "a trailing colon on PATH means the current directory"
        );
        assert_eq!(path_with(dir, Some("")), "/data/instances/default/session-bin");
    }

    #[tokio::test]
    async fn a_skill_is_installed_and_then_taken_away_again() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("claude/skills/droppen-notify");

        set_skill(&root, false).await.expect("removing what was never there is not a failure");
        set_skill(&root, true).await.expect("install");
        let body = std::fs::read_to_string(root.join("SKILL.md")).expect("skill");
        assert!(body.starts_with("---\nname: droppen-notify\n"), "the CLI reads the frontmatter first");

        set_skill(&root, false).await.expect("remove");
        assert!(!root.exists(), "a channel that is off must stop advertising a tool that can only refuse");
    }

    /// The frontmatter is a contract with the CLI, not prose: a skill whose
    /// `name` does not match its directory, or whose `description` is missing,
    /// is a skill the model never hears about.
    #[test]
    fn the_skill_carries_the_frontmatter_the_cli_requires() {
        let mut lines = SKILL.lines();
        assert_eq!(lines.next(), Some("---"));
        let front: Vec<&str> = lines.by_ref().take_while(|line| *line != "---").collect();
        let front = front.join("\n");
        assert!(front.contains(&format!("name: {SKILL_NAME}")), "{front}");
        assert!(front.contains("description:"), "{front}");
        assert!(SKILL.contains("droppen-notify"), "the skill has to name the command it is about");
    }
}
