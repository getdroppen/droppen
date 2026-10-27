//! Carrying a local-login conversation onto an account, as a new session.
//!
//! A claude session this server started on the *local login* lives in the
//! user's own `~/.claude`, and it stays on that login: an account spawn runs
//! against this instance's own Claude root, and `claude --resume` only sees
//! what is under the root it was given. When the local login stops working —
//! an OAuth session that expired and could not be refreshed — such a session is
//! stuck: it cannot send, and it cannot be moved onto an account
//! (`local_session_cannot_use_account`).
//!
//! This is the way out, and it is deliberately **not** a move:
//!
//! * the original transcript is read, never written. It is the user's own file
//!   in the user's own root, and it stays exactly where and what it was — the
//!   old session keeps listing, opening and (once the login works again)
//!   sending as before;
//! * the copy goes into this instance's managed root under a **new** session
//!   id, so the two are two sessions and never one id in two roots (which the
//!   list would resolve by hiding the original);
//! * every line's `sessionId` is rewritten to the new id, which is what claude
//!   itself does on `--fork-session`. Nothing else in a line is touched — not
//!   the text, not the paths into the original's sidecar directory a large tool
//!   result was saved under, which still exist where they point.
//!
//! The new session is then an ordinary transcript in the managed root: owned by
//! the directory it sits in, resumed (`--resume <new id>`) by the first message
//! sent to it, under whichever account it is bound to.
//!
//! # Copying a file that is being written to
//!
//! Only whole lines are copied. A transcript whose CLI is mid-turn can end in a
//! half-written line, and a JSONL append is not atomic; the fragment after the
//! last newline is dropped, the same rule every reader in the crate applies
//! ([`super::util::read_head_lines`]). The copy is a fork from that instant, so
//! there is no later append to lose.
//!
//! # Where it may read and write
//!
//! The id is checked against the strict directory rule before anything touches
//! the disk, because it names a sidecar directory below. The original must be a
//! regular file directly inside a real (not symlinked) project directory of the
//! local root; the copy is written into the managed root's project directory of
//! the same name — Claude Code's own encoding of the working directory, which is
//! how `--resume` finds it — first under a suffix nothing lists, renamed into
//! place last. A symlink anywhere in the sidecar tree is skipped rather than
//! followed.

use std::path::{Path, PathBuf};

use super::{claude, reference, Paths};
use crate::claude_roots::{self, ClaudeRoot};

/// Suffix the copy wears until it is complete; swept at boot by
/// [`super::adoption_leftovers::sweep_partial_adoptions`].
pub const PARTIAL_SUFFIX: &str = "jsonl.continuing";

/// Why a session could not be copied.
#[derive(Debug, thiserror::Error)]
pub enum ContinueError {
    /// No transcript of that id in the user's own root.
    #[error("session not found")]
    NotFound,
    /// The new id is already taken; nothing was written.
    #[error("the new session id is already in use")]
    Taken,
    /// The copy failed part-way; nothing usable was left behind.
    #[error("cannot copy the transcript: {0}")]
    Io(String),
}

/// Copy a local-root claude session into the managed root as `new_id`.
///
/// Returns the new transcript's path. The original is only read.
pub async fn copy_local_session(paths: &Paths, id: &str, new_id: &str) -> Result<PathBuf, ContinueError> {
    // The strict rule for both: each becomes a directory name below.
    if !reference::is_path_segment_id(id) || !reference::is_path_segment_id(new_id) || id == new_id {
        return Err(ContinueError::NotFound);
    }
    let original = claude::find_in_root(paths, ClaudeRoot::Local, id).await.ok_or(ContinueError::NotFound)?;
    let local_projects = claude_roots::projects_dir(paths, ClaudeRoot::Local);
    let project_dir = original.parent().ok_or(ContinueError::NotFound)?;
    let project = project_dir.file_name().ok_or(ContinueError::NotFound)?.to_owned();
    // Exactly `<local projects>/<project>/<id>.jsonl`, with neither the project
    // directory nor the transcript a symlink: a link planted where a session
    // goes points wherever whoever made it chose.
    if project_dir != local_projects.join(&project)
        || !is_real_dir(project_dir).await
        || !tokio::fs::symlink_metadata(&original).await.is_ok_and(|meta| meta.is_file())
    {
        return Err(ContinueError::NotFound);
    }

    let destination_dir = claude_roots::projects_dir(paths, ClaudeRoot::Managed).join(&project);
    let destination = destination_dir.join(format!("{new_id}.jsonl"));
    let sidecar_to = destination_dir.join(new_id);
    if tokio::fs::symlink_metadata(&destination).await.is_ok() || tokio::fs::symlink_metadata(&sidecar_to).await.is_ok() {
        return Err(ContinueError::Taken);
    }
    let io = |error: std::io::Error| ContinueError::Io(error.to_string());
    tokio::fs::create_dir_all(&destination_dir).await.map_err(io)?;

    let raw = tokio::fs::read(&original).await.map_err(io)?;
    let copied = rewrite_session_id(&raw, id, new_id);

    // The sidecar tree first (subagent transcripts, saved tool results), so a
    // session whose Task output arrived a moment after it would not render
    // half-empty on its first open.
    let sidecar_from = project_dir.join(id);
    if is_real_dir(&sidecar_from).await {
        if let Err(error) = copy_tree(&sidecar_from, &sidecar_to, id, new_id).await {
            // A directory under a uuid minted for this copy alone: nothing else
            // can be in it.
            let _ = tokio::fs::remove_dir_all(&sidecar_to).await;
            return Err(io(error));
        }
    }

    let partial = destination_dir.join(format!("{new_id}.{PARTIAL_SUFFIX}"));
    let written = async {
        tokio::fs::write(&partial, &copied).await?;
        tokio::fs::rename(&partial, &destination).await
    }
    .await;
    if let Err(error) = written {
        let _ = tokio::fs::remove_file(&partial).await;
        let _ = tokio::fs::remove_dir_all(&sidecar_to).await;
        return Err(io(error));
    }
    Ok(destination)
}

/// Whether `path` is a directory and not a symlink to one.
async fn is_real_dir(path: &Path) -> bool {
    tokio::fs::symlink_metadata(path).await.is_ok_and(|meta| meta.is_dir())
}

/// The transcript's whole lines with each line's own `sessionId` set to `new_id`.
///
/// Textual, not a parse and re-serialise: `serde_json` without
/// `preserve_order` would sort every line's keys, and a transcript that reads
/// back differently from the one claude wrote is a risk for nothing. A line is
/// rewritten only where it holds the field with exactly the old id as its value
/// — `"sessionId":"<old>"`, the compact form claude writes — so the id quoted
/// anywhere else (a path, a message, a tool's output) stays as it was.
pub fn rewrite_session_id(raw: &[u8], old_id: &str, new_id: &str) -> Vec<u8> {
    let needle = format!("\"sessionId\":\"{old_id}\"");
    let replacement = format!("\"sessionId\":\"{new_id}\"");
    let mut out = Vec::with_capacity(raw.len());
    let mut rest = raw;
    // Only whole lines: whatever follows the last newline may be half written.
    while let Some(end) = rest.iter().position(|&b| b == b'\n') {
        let (line, tail) = rest.split_at(end + 1);
        match std::str::from_utf8(line) {
            Ok(text) if text.contains(&needle) => out.extend_from_slice(text.replacen(&needle, &replacement, 1).as_bytes()),
            _ => out.extend_from_slice(line),
        }
        rest = tail;
    }
    out
}

/// Copy a directory and everything under it, rewriting the `sessionId` of
/// every `.jsonl` on the way. Symlinks and other oddities are skipped.
async fn copy_tree(from: &Path, to: &Path, old_id: &str, new_id: &str) -> std::io::Result<()> {
    let mut pending = vec![(from.to_path_buf(), to.to_path_buf())];
    while let Some((source, destination)) = pending.pop() {
        tokio::fs::create_dir_all(&destination).await?;
        let mut entries = tokio::fs::read_dir(&source).await?;
        while let Some(entry) = entries.next_entry().await? {
            let kind = entry.file_type().await?;
            let target = destination.join(entry.file_name());
            if kind.is_dir() {
                pending.push((entry.path(), target));
            } else if kind.is_file() {
                if entry.path().extension().is_some_and(|ext| ext == "jsonl") {
                    let raw = tokio::fs::read(entry.path()).await?;
                    tokio::fs::write(&target, rewrite_session_id(&raw, old_id, new_id)).await?;
                } else {
                    tokio::fs::copy(entry.path(), &target).await?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: &str = "11111111-1111-4111-8111-111111111111";
    const NEW: &str = "22222222-2222-4222-8222-222222222222";

    fn line(session: &str, text: &str) -> String {
        format!("{{\"type\":\"user\",\"cwd\":\"/w\",\"sessionId\":\"{session}\",\"message\":{{\"content\":\"{text}\"}}}}\n")
    }

    /// Seed a local-root session with a subagent transcript and a saved tool
    /// result beside it.
    fn seed(paths: &Paths, id: &str) -> PathBuf {
        let project = claude_roots::projects_dir(paths, ClaudeRoot::Local).join("-w");
        std::fs::create_dir_all(project.join(id).join("subagents")).expect("sidecar");
        std::fs::create_dir_all(project.join(id).join("tool-results")).expect("tool results");
        let path = project.join(format!("{id}.jsonl"));
        std::fs::write(&path, format!("{}{}", line(id, "hello"), line(id, "again"))).expect("transcript");
        std::fs::write(project.join(id).join("subagents").join("agent-1.jsonl"), line(id, "sub")).expect("subagent");
        std::fs::write(project.join(id).join("tool-results").join("t1.txt"), "big output").expect("tool result");
        path
    }

    #[tokio::test]
    async fn the_copy_is_a_new_session_in_the_managed_root_and_the_original_is_untouched() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        let original = seed(&paths, OLD);
        let before = std::fs::read(&original).expect("read");

        let copy = copy_local_session(&paths, OLD, NEW).await.expect("copy");
        assert_eq!(
            copy,
            claude_roots::projects_dir(&paths, ClaudeRoot::Managed)
                .join("-w")
                .join(format!("{NEW}.jsonl"))
        );
        assert_eq!(
            std::fs::read(&original).expect("the original"),
            before,
            "not one byte of the original changed"
        );

        let text = std::fs::read_to_string(&copy).expect("the copy");
        assert_eq!(
            text,
            format!("{}{}", line(NEW, "hello"), line(NEW, "again")),
            "every line now names the new session"
        );
        let sidecar = copy.parent().expect("project").join(NEW);
        assert_eq!(
            std::fs::read_to_string(sidecar.join("subagents").join("agent-1.jsonl")).expect("subagent copied"),
            line(NEW, "sub")
        );
        assert_eq!(
            std::fs::read_to_string(sidecar.join("tool-results").join("t1.txt")).expect("tool result copied"),
            "big output"
        );
        assert!(original
            .parent()
            .expect("project")
            .join(OLD)
            .join("subagents")
            .join("agent-1.jsonl")
            .is_file());

        // Both are listed, each in its own root.
        assert_eq!(claude::find_in_root(&paths, ClaudeRoot::Local, OLD).await, Some(original));
        assert_eq!(claude::find_in_root(&paths, ClaudeRoot::Managed, NEW).await, Some(copy));
        assert!(
            claude::find_in_root(&paths, ClaudeRoot::Managed, OLD).await.is_none(),
            "the original id never enters the managed root"
        );
    }

    #[test]
    fn only_the_session_id_field_of_whole_lines_is_rewritten() {
        let raw = format!(
            "{}{{\"type\":\"assistant\",\"sessionId\":\"{OLD}\",\"text\":\"see /x/{OLD}/tool-results/a.txt and sessionId {OLD}\"}}\n{{\"type\":\"summary\",\"leafUuid\":\"u\"}}\n{{\"type\":\"user\",\"sessionId\":\"{OLD}\",\"torn",
            line(OLD, "hi")
        );
        let out = String::from_utf8(rewrite_session_id(raw.as_bytes(), OLD, NEW)).expect("utf8");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3, "the torn last line is dropped: {out}");
        assert!(lines[0].contains(&format!("\"sessionId\":\"{NEW}\"")));
        assert!(lines[1].contains(&format!("\"sessionId\":\"{NEW}\"")));
        assert!(
            lines[1].contains(&format!("/x/{OLD}/tool-results/a.txt and sessionId {OLD}")),
            "the id quoted anywhere else is left alone: {}",
            lines[1]
        );
        assert_eq!(lines[2], "{\"type\":\"summary\",\"leafUuid\":\"u\"}");
        assert!(out.ends_with('\n'));
    }

    #[tokio::test]
    async fn an_unknown_traversing_or_managed_id_copies_nothing() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        seed(&paths, OLD);
        // A decoy where `find_in_root(id = "..")` would look.
        let project = claude_roots::projects_dir(&paths, ClaudeRoot::Local).join("-w");
        std::fs::write(project.join("...jsonl"), "{}\n").expect("decoy");
        for id in ["nope", "..", ".", "...", "../../etc/passwd", "-w/../x", ""] {
            assert!(
                matches!(copy_local_session(&paths, id, NEW).await, Err(ContinueError::NotFound)),
                "{id:?}"
            );
        }
        for new_id in ["..", "a/b", "", OLD] {
            assert!(
                matches!(copy_local_session(&paths, OLD, new_id).await, Err(ContinueError::NotFound)),
                "{new_id:?}"
            );
        }
        assert!(
            !claude_roots::projects_dir(&paths, ClaudeRoot::Managed).exists(),
            "nothing was written anywhere"
        );

        // A session already in the managed root is not this operation's to copy.
        let managed = claude_roots::projects_dir(&paths, ClaudeRoot::Managed).join("-w");
        std::fs::create_dir_all(&managed).expect("managed project");
        let other = "33333333-3333-4333-8333-333333333333";
        std::fs::write(managed.join(format!("{other}.jsonl")), line(other, "mine")).expect("managed transcript");
        assert!(matches!(copy_local_session(&paths, other, NEW).await, Err(ContinueError::NotFound)));
    }

    #[tokio::test]
    async fn a_taken_id_is_refused_without_overwriting() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        seed(&paths, OLD);
        let managed = claude_roots::projects_dir(&paths, ClaudeRoot::Managed).join("-w");
        std::fs::create_dir_all(&managed).expect("managed project");
        std::fs::write(managed.join(format!("{NEW}.jsonl")), "theirs\n").expect("existing");
        assert!(matches!(copy_local_session(&paths, OLD, NEW).await, Err(ContinueError::Taken)));
        assert_eq!(std::fs::read_to_string(managed.join(format!("{NEW}.jsonl"))).expect("kept"), "theirs\n");
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn symlinks_are_never_followed() {
        let home = tempfile::tempdir().expect("tempdir");
        let paths = Paths::new(home.path().to_path_buf());
        let original = seed(&paths, OLD);
        let project = original.parent().expect("project").to_path_buf();
        let elsewhere = home.path().join("secrets");
        std::fs::create_dir_all(&elsewhere).expect("elsewhere");
        std::fs::write(elsewhere.join("id_rsa"), "private").expect("a file that is not ours");

        // A symlink inside the sidecar tree is skipped.
        std::os::unix::fs::symlink(&elsewhere, project.join(OLD).join("linked")).expect("symlink in the sidecar");
        let copy = copy_local_session(&paths, OLD, NEW).await.expect("the transcript still copies");
        assert!(
            !copy.parent().expect("project").join(NEW).join("linked").exists(),
            "the link's target was not copied"
        );

        // A symlinked transcript is not a session to copy.
        let linked_id = "44444444-4444-4444-8444-444444444444";
        std::fs::write(elsewhere.join("t.jsonl"), line(linked_id, "x")).expect("target");
        std::os::unix::fs::symlink(elsewhere.join("t.jsonl"), project.join(format!("{linked_id}.jsonl"))).expect("symlink transcript");
        let fresh = "55555555-5555-4555-8555-555555555555";
        assert!(matches!(copy_local_session(&paths, linked_id, fresh).await, Err(ContinueError::NotFound)));

        // Nor is a transcript in a symlinked project directory.
        let outside = home.path().join("outside-project");
        std::fs::create_dir_all(&outside).expect("outside");
        let outside_id = "66666666-6666-4666-8666-666666666666";
        std::fs::write(outside.join(format!("{outside_id}.jsonl")), line(outside_id, "x")).expect("outside transcript");
        std::os::unix::fs::symlink(&outside, claude_roots::projects_dir(&paths, ClaudeRoot::Local).join("-linked")).expect("symlink project");
        assert!(matches!(
            copy_local_session(&paths, outside_id, fresh).await,
            Err(ContinueError::NotFound)
        ));
    }
}
