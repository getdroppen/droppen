//! `remote-hand-server replay`: a recording, played back as a filesystem.
//!
//! A recording is only worth making if something can consume it, and the thing
//! that should consume it is *the real server*. So replay deliberately does not
//! render anything, parse anything or serve anything: it writes the transcript
//! files back out, in their original order and at their original pace, under a
//! throwaway `$HOME`. A second server started on that home then discovers the
//! session the way it discovers any other, its feed watches the files the way
//! it watches any other, and the Session page renders real data through the
//! real pipeline — the whole point being that a rendering bug you can only
//! reproduce with one particular subagent-heavy session becomes something you
//! can reproduce on demand, as often as you like, without spending an agent's
//! afternoon to get there.
//!
//! ```text
//! remote-hand-server replay --log <recording> --home /tmp/replay --max-gap 3
//! APP_HOME_DIR=/tmp/replay APP_SERVER__PORT=8701 remote-hand-server serve
//! ```
//!
//! # The clock
//!
//! Played back at the pace it was recorded at, because that pace is data: how a
//! session *feels* — the pauses, the burst when four subagents report at once,
//! the twenty seconds of nothing while a build runs — is exactly what a
//! rendering iteration is trying to get right, and a file written all at once
//! shows none of it.
//!
//! What is not data is idle time. A session that ran for six hours spent most
//! of them waiting for a human, so `--max-gap` (three seconds by default)
//! compresses any silence longer than itself, `--speed` divides what is left,
//! and `--instant` skips the clock entirely for the case where only the final
//! state matters.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Args;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

use super::{Event, Line};
use droppen_instance::instance::DEFAULT_INSTANCE;
use droppen_instance::paths::{ClaudeRoot, Paths};
use droppen_base::time::parse_iso_ms;

/// How much silence is replayed before it is compressed, by default.
const DEFAULT_MAX_GAP: Duration = Duration::from_secs(3);

/// How often the progress line is printed, at most.
const PROGRESS_EVERY: Duration = Duration::from_millis(500);

/// The rest of `replay --help`, after the options clap lists.
pub const USAGE: &str = "\
USAGE:
    remote-hand-server replay --log <recording.ndjson> --home <scratch-home>
                              [--instance <name>] [--speed <n>]
                              [--max-gap <seconds>] [--instant]

Rebuilds the transcript files a recorded session wrote, under a scratch $HOME,
at the pace they were originally written. Then:

    APP_HOME_DIR=<scratch-home> APP_SERVER__PORT=8701 remote-hand-server serve

…renders them through the real pipeline. Recordings live in
<data>/instances/<name>/session-logs/<session-id>.ndjson.";

/// The `replay` command line.
#[derive(Debug, PartialEq, Args)]
#[command(disable_help_subcommand = true, args_override_self = true)]
pub struct ReplayArgs {
    /// The recording to read.
    #[arg(long, allow_hyphen_values = true)]
    pub log: PathBuf,
    /// The scratch `$HOME` to rebuild under (`APP_HOME_DIR` for the server).
    #[arg(long, allow_hyphen_values = true)]
    pub home: PathBuf,
    /// Which instance's managed root a `managed` file is written to.
    #[arg(long, default_value = DEFAULT_INSTANCE, allow_hyphen_values = true)]
    pub instance: String,
    /// How much faster than life.
    #[arg(long, default_value_t = 1.0, value_parser = speed_value, allow_hyphen_values = true)]
    pub speed: f64,
    /// The longest silence replayed in full, in seconds.
    #[arg(long, default_value_t = 3.0, value_parser = max_gap_value, allow_hyphen_values = true)]
    pub max_gap: f64,
    /// Write everything at once, with no clock.
    #[arg(long, action = clap::ArgAction::Count)]
    pub instant: u8,
}

/// `--speed`: a finite number greater than zero.
fn speed_value(raw: &str) -> Result<f64, String> {
    let value: f64 = raw.parse().map_err(|_| format!("--speed wants a number, not {raw:?}"))?;
    if !(value.is_finite() && value > 0.0) {
        return Err("--speed must be greater than zero".to_string());
    }
    Ok(value)
}

/// `--max-gap`: a finite number of seconds, not negative.
fn max_gap_value(raw: &str) -> Result<f64, String> {
    let secs: f64 = raw.parse().map_err(|_| format!("--max-gap wants seconds, not {raw:?}"))?;
    if !(secs.is_finite() && secs >= 0.0) {
        return Err("--max-gap must not be negative".to_string());
    }
    Ok(secs)
}

impl ReplayArgs {
    /// Play the recording back into the scratch home.
    pub async fn run(self) -> Result<(), String> {
        let max_gap = Duration::from_secs_f64(self.max_gap);
        let instant = self.instant > 0;

        let paths = Paths::for_instance(self.home.clone(), &self.instance);
        let plan = survey(&self.log).await?;
        if plan.acts == 0 {
            println!("replay: nothing to rebuild — this recording has no transcript lines");
            return Ok(());
        }
        println!(
            "replay: {} transcript events over {} of session time, into {}",
            plan.acts,
            span(plan.span_ms),
            self.home.display()
        );
        if instant {
            println!("replay: --instant, so the clock is skipped entirely");
        } else {
            println!(
                "replay: at {}x, with silences longer than {:?} compressed to it",
                self.speed, max_gap
            );
        }

        let mut sinks: HashMap<PathBuf, Sink> = HashMap::new();
        let mut clock: Option<(i64, tokio::time::Instant)> = None;
        let mut done = 0u64;
        let mut first: Option<i64> = None;
        let mut printed = tokio::time::Instant::now();

        let mut lines = open(&self.log).await?;
        while let Some(line) = next_line(&mut lines).await {
            let (root, rel, payload) = match &line.event {
                Event::Transcript { root, path, line, .. } => (root, path, Payload::Line(line)),
                Event::Sidecar { root, path, content, .. } => (root, path, Payload::Whole(content)),
                // Everything else is the session's own life — stdio, lifecycle, the
                // recorder's notes — and rebuilding a filesystem has no use for it.
                // It stays in the recording because reading a recording by hand is
                // the other half of what one is for.
                _ => continue,
            };
            let Some(target) = target_of(&paths, root, rel) else {
                eprintln!("replay: skipping a line whose path is not one this tool will write: {rel}");
                continue;
            };

            // The clock, before the write: a line is due at the moment it was
            // recorded, not the moment after it lands.
            let stamp = parse_iso_ms(&line.at);
            if let Some(stamp) = stamp {
                first.get_or_insert(stamp);
                if !instant {
                    let due = match clock {
                        Some((previous, at)) => at + scaled(stamp - previous, self.speed, max_gap),
                        None => tokio::time::Instant::now(),
                    };
                    tokio::time::sleep_until(due).await;
                    clock = Some((stamp, due));
                } else {
                    clock = Some((stamp, tokio::time::Instant::now()));
                }
            }

            write(&mut sinks, &target, payload).await?;
            done += 1;

            let now = tokio::time::Instant::now();
            if now.duration_since(printed) >= PROGRESS_EVERY {
                printed = now;
                let at = stamp.zip(first).map(|(now, first)| now - first).unwrap_or(0);
                println!("replay: t+{} · {done}/{} events · {} files", span(at), plan.acts, sinks.len());
            }
        }

        for sink in sinks.values_mut() {
            let _ = sink.file.flush().await;
        }
        println!("replay: done — {done} events into {} files under {}", sinks.len(), self.home.display());
        let next = format!(
            "replay: now run  APP_HOME_DIR={} APP_SERVER__PORT=8701 remote-hand-server serve",
            self.home.display()
        );
        println!("{}", droppen_base::program::Name::invoked().rewrite(&next));
        Ok(())
    }
}

/// One file being rebuilt, held open for the whole replay.
struct Sink {
    file: tokio::fs::File,
}

/// What is being written to a file.
enum Payload<'a> {
    /// One line of an append-only transcript.
    Line(&'a str),
    /// The whole of a file the CLI rewrites in place.
    Whole(&'a str),
}

/// Append a line, or replace a file.
async fn write(sinks: &mut HashMap<PathBuf, Sink>, target: &Path, payload: Payload<'_>) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    match payload {
        Payload::Line(line) => {
            // The handle is kept open across lines: a transcript is thousands of
            // them, and re-opening per line would make the replay's own I/O the
            // thing being measured.
            if !sinks.contains_key(target) {
                // Truncated on the first line of *this* run, so replaying a
                // recording twice into one home rebuilds the same session
                // rather than a doubled one. Only the files this recording
                // names are touched; the rest of the home is left alone.
                let file = tokio::fs::OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(target)
                    .await
                    .map_err(|error| format!("cannot open {}: {error}", target.display()))?;
                sinks.insert(target.to_path_buf(), Sink { file });
            }
            let sink = sinks.get_mut(target).expect("just inserted");
            sink.file
                .write_all(format!("{line}\n").as_bytes())
                .await
                .map_err(|error| format!("cannot write {}: {error}", target.display()))?;
            // Flushed per line rather than per batch, because the whole point is
            // that a server watching this file sees it grow the way it grew.
            sink.file
                .flush()
                .await
                .map_err(|error| format!("cannot write {}: {error}", target.display()))?;
        }
        Payload::Whole(content) => {
            // A snapshot replaces whatever is there, which is what makes
            // re-recording one harmless — and it must not go through the append
            // handle above, so any open one is dropped first.
            sinks.remove(target);
            tokio::fs::write(target, content)
                .await
                .map_err(|error| format!("cannot write {}: {error}", target.display()))?;
        }
    }
    Ok(())
}

/// Where a recorded path lands under the scratch home.
///
/// The recording is a file that may have come from another machine or another
/// person's bug report, so its paths are treated as untrusted input: anything
/// absolute, anything with a `..` in it and anything empty is refused rather
/// than joined. A replay must not be a way to write outside the home it was
/// given.
fn target_of(paths: &Paths, root: &str, rel: &str) -> Option<PathBuf> {
    let root = ClaudeRoot::parse(root)?;
    let mut safe = PathBuf::new();
    for part in Path::new(rel).components() {
        match part {
            std::path::Component::Normal(part) => safe.push(part),
            _ => return None,
        }
    }
    (!safe.as_os_str().is_empty()).then(|| paths.claude_root(root).join(safe))
}

/// What a first pass over the recording found.
struct Plan {
    /// How many events rebuild a file.
    acts: u64,
    /// How long the session ran, in milliseconds of wall time.
    span_ms: i64,
}

/// Read the recording once for its shape, so the progress line has a total.
///
/// Two passes over a file that is being read sequentially anyway is cheaper
/// than holding a whole recording in memory, and a progress line with no
/// denominator is a progress line that tells you nothing.
async fn survey(log: &Path) -> Result<Plan, String> {
    let mut lines = open(log).await?;
    let mut acts = 0;
    let mut first = None;
    let mut last = None;
    while let Some(line) = next_line(&mut lines).await {
        if matches!(line.event, Event::Transcript { .. } | Event::Sidecar { .. }) {
            acts += 1;
            if let Some(stamp) = parse_iso_ms(&line.at) {
                first.get_or_insert(stamp);
                last = Some(stamp);
            }
        }
    }
    Ok(Plan {
        acts,
        span_ms: first.zip(last).map(|(first, last)| last - first).unwrap_or(0),
    })
}

async fn open(log: &Path) -> Result<tokio::io::Lines<tokio::io::BufReader<tokio::fs::File>>, String> {
    let file = tokio::fs::File::open(log)
        .await
        .map_err(|error| format!("cannot read {}: {error}", log.display()))?;
    Ok(tokio::io::BufReader::new(file).lines())
}

/// The next line that parses, skipping the ones that do not.
///
/// Tolerant on purpose: a recording is appended to by a live server, so its
/// last line may be half-written, and an older build's recording may hold event
/// kinds this one has never heard of.
async fn next_line(lines: &mut tokio::io::Lines<tokio::io::BufReader<tokio::fs::File>>) -> Option<Line> {
    while let Ok(Some(raw)) = lines.next_line().await {
        if let Ok(line) = serde_json::from_str::<Line>(&raw) {
            return Some(line);
        }
    }
    None
}

/// How long to wait for a gap of `gap_ms`, once compressed and divided.
///
/// A negative gap is a recording whose clock went backwards — an NTP step, two
/// servers appending to one file — and is worth no wait at all rather than a
/// panic.
fn scaled(gap_ms: i64, speed: f64, max_gap: Duration) -> Duration {
    let gap = Duration::from_millis(gap_ms.max(0) as u64).min(max_gap);
    gap.div_f64(speed)
}

/// `1h02m03s`, for a duration in milliseconds.
fn span(ms: i64) -> String {
    let secs = ms.max(0) / 1_000;
    let (hours, minutes, seconds) = (secs / 3_600, (secs % 3_600) / 60, secs % 60);
    if hours > 0 {
        format!("{hours}h{minutes:02}m{seconds:02}s")
    } else if minutes > 0 {
        format!("{minutes}m{seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use clap::error::ErrorKind;

    use super::*;
    use droppen_base::time::iso8601;
    use std::time::SystemTime;

    fn parse_args(argv: &[String]) -> Result<ReplayArgs, clap::Error> {
        match crate::cli::tests::parse_words("replay", argv)? {
            crate::cli::Command::Replay(parsed) => Ok(parsed),
            other => panic!("{argv:?} parsed as {other:?}"),
        }
    }

    #[test]
    fn the_command_line_is_read_or_refused_with_a_reason() {
        let argv: Vec<String> = [
            "--log",
            "/tmp/a.ndjson",
            "--home",
            "/tmp/home",
            "--speed",
            "4",
            "--max-gap",
            "0.5",
            "--instant",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let options = parse_args(&argv).expect("a valid command line");
        assert_eq!(options.log, PathBuf::from("/tmp/a.ndjson"));
        assert_eq!(options.home, PathBuf::from("/tmp/home"));
        assert_eq!(options.speed, 4.0);
        assert_eq!(options.max_gap, 0.5);
        assert_eq!(options.instant, 1);
        assert_eq!(options.instance, DEFAULT_INSTANCE, "the default matches the server's own");

        let bare = |args: &[&str]| parse_args(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>()).expect_err("must be refused").to_string();
        assert!(bare(&["--home", "/tmp"]).contains("--log"));
        assert!(bare(&["--log", "/tmp/a"]).contains("--home"));
        assert!(bare(&["--log", "/tmp/a", "--home", "/tmp", "--speed", "0"]).contains("greater than zero"));
        assert!(bare(&["--log", "/tmp/a", "--home", "/tmp", "--speed", "fast"]).contains("wants a number"));
        assert!(
            bare(&["--log", "/tmp/a", "--home", "/tmp", "--nope"]).contains("unexpected argument '--nope'"),
            "unknown option"
        );
        assert!(bare(&["--log"]).contains("a value is required for '--log <LOG>'"), "needs a value");
    }

    /// The two knobs that make a six-hour session watchable in five minutes.
    #[test]
    fn a_long_silence_is_compressed_and_the_rest_is_divided() {
        let gap = Duration::from_secs(3);
        assert_eq!(scaled(500, 1.0, gap), Duration::from_millis(500), "a short gap is played in full");
        assert_eq!(scaled(90_000, 1.0, gap), gap, "a minute and a half of nothing becomes the ceiling");
        assert_eq!(scaled(2_000, 4.0, gap), Duration::from_millis(500), "and speed divides what is left");
        assert_eq!(
            scaled(90_000, 3.0, gap),
            Duration::from_secs(1),
            "the ceiling applies before the speed, so --speed never lengthens a wait"
        );
        assert_eq!(scaled(-5_000, 1.0, gap), Duration::ZERO, "a clock that went backwards waits for nothing");
        assert_eq!(scaled(10_000, 1.0, Duration::ZERO), Duration::ZERO, "--max-gap 0 is instant pacing");
    }

    #[test]
    fn a_stamp_survives_the_round_trip_it_was_written_for() {
        for millis in [0i64, 1, 1_000, 1_700_000_000_123, 253_370_764_800_000] {
            let stamp = iso8601(SystemTime::UNIX_EPOCH + Duration::from_millis(millis as u64));
            assert_eq!(parse_iso_ms(&stamp), Some(millis), "{stamp}");
        }
        assert_eq!(parse_iso_ms("2026-08-29T12:00:00Z"), parse_iso_ms("2026-08-29T12:00:00.000Z"));
        assert_eq!(parse_iso_ms("2026-08-29T12:00:00.5Z"), parse_iso_ms("2026-08-29T12:00:00.500Z"));
        for bad in ["", "nonsense", "2026-08-29", "2026-13-01T00:00:00.000Z", "2026-08-32T00:00:00.000Z"] {
            assert_eq!(parse_iso_ms(bad), None, "{bad}");
        }
        assert_eq!(span(0), "0s");
        assert_eq!(span(90_000), "1m30s");
        assert_eq!(span(3_723_000), "1h02m03s");
    }

    /// A recording is a file that may arrive from anywhere; it must not be able
    /// to write outside the home it was pointed at.
    #[test]
    fn a_path_that_escapes_the_home_is_refused() {
        let paths = Paths::new(PathBuf::from("/tmp/scratch"));
        assert!(target_of(&paths, "managed", "../../etc/passwd").is_none());
        assert!(target_of(&paths, "managed", "/etc/passwd").is_none());
        assert!(target_of(&paths, "managed", "projects/../../x").is_none());
        assert!(target_of(&paths, "managed", "").is_none());
        assert!(target_of(&paths, "sideways", "projects/a/b.jsonl").is_none(), "and an unknown root too");
        let ok = target_of(&paths, "local", "projects/-w-repo/s.jsonl").expect("an ordinary path");
        assert_eq!(ok, paths.claude_root(ClaudeRoot::Local).join("projects/-w-repo/s.jsonl"));
    }

    /// End to end: a recording of a session with a subagent rebuilds the exact
    /// files the CLI wrote — byte for byte, because a transcript that differs
    /// from the original renders differently, which would defeat the point.
    #[tokio::test]
    async fn an_instant_replay_rebuilds_the_files_byte_for_byte() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("session.ndjson");
        let home = dir.path().join("home");

        let main = "projects/-w-repo/abc.jsonl";
        let agent = "projects/-w-repo/abc/subagents/agent-a1.jsonl";
        let meta = "projects/-w-repo/abc/subagents/agent-a1.meta.json";
        let mut recording = String::new();
        let at = |ms: u64| iso8601(SystemTime::UNIX_EPOCH + Duration::from_millis(1_700_000_000_000 + ms));
        for (index, (path, file, line)) in [
            (
                main,
                "main",
                r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"go"}]}}"#,
            ),
            (
                main,
                "main",
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"done"}]}}"#,
            ),
            (
                agent,
                "agent:a1",
                r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"child"}]}}"#,
            ),
            (main, "main", r#"{"type":"result","subtype":"success"}"#),
        ]
        .into_iter()
        .enumerate()
        {
            recording.push_str(&format!(
                "{}\n",
                serde_json::json!({
                    "v": 1, "at": at(index as u64 * 250), "kind": "transcript",
                    "file": file, "root": "managed", "path": path, "line": line,
                })
            ));
        }
        // A snapshot, and a line that no build has ever written — both have to
        // be survivable.
        recording.push_str(&format!(
            "{}\n",
            serde_json::json!({
                "v": 1, "at": at(1_000), "kind": "sidecar",
                "file": "agent-meta:a1", "root": "managed", "path": meta,
                "content": r#"{"agentType":"Explore","toolUseId":"toolu_1"}"#,
            })
        ));
        recording.push_str("{this line is not json\n");
        recording.push_str(&format!(
            "{}\n",
            serde_json::json!({"v": 1, "at": at(1_100), "kind": "stdout", "line": "ignored", "replayed": false})
        ));
        std::fs::write(&log, &recording).expect("write the recording");

        ReplayArgs {
            log: log.clone(),
            home: home.clone(),
            instance: DEFAULT_INSTANCE.to_string(),
            speed: 1.0,
            max_gap: 3.0,
            instant: 1,
        }
        .run()
        .await
        .expect("replay");

        let paths = Paths::new(home.clone());
        let root = paths.claude_root(ClaudeRoot::Managed);
        assert_eq!(
            std::fs::read_to_string(root.join(main)).expect("the main transcript"),
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"go\"}]}}\n\
             {\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"done\"}]}}\n\
             {\"type\":\"result\",\"subtype\":\"success\"}\n",
            "the transcript is the lines it was recorded from, in order, one per line"
        );
        assert_eq!(
            std::fs::read_to_string(root.join(agent)).expect("the subagent transcript"),
            "{\"type\":\"assistant\",\"isSidechain\":true,\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"child\"}]}}\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join(meta)).expect("the meta"),
            r#"{"agentType":"Explore","toolUseId":"toolu_1"}"#,
            "a snapshot is written whole, with no newline appended"
        );

        // And it really is the file the CLI would have written: the parser that
        // feeds the Session page reads it back without complaint.
        let items = crate::provider::claude_code::sessions::transcript::parse_claude_transcript(&paths, &root.join(main), false)
            .await
            .expect("the rebuilt transcript parses");
        let rendered: Vec<&str> = items.iter().map(|item| item.text.as_str()).collect();
        assert_eq!(rendered, ["go", "done"], "the session renders exactly what it did live");

        // The subagent's own transcript is a sidechain, and drills down the way
        // the Session page expects it to.
        let child = crate::provider::claude_code::sessions::transcript::parse_claude_transcript(&paths, &root.join(agent), true)
            .await
            .expect("the rebuilt subagent transcript parses");
        assert_eq!(child.len(), 1);
        assert_eq!(child[0].text, "child");
    }

    /// Replaying twice into one home rebuilds the same session rather than a
    /// doubled one — the loop this tool exists for is "tweak the renderer,
    /// replay, look", and a home that had to be deleted between rounds would
    /// make that loop a chore.
    #[tokio::test]
    async fn a_second_replay_into_one_home_replaces_rather_than_doubles() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("session.ndjson");
        let home = dir.path().join("home");
        let stamp = iso8601(SystemTime::UNIX_EPOCH + Duration::from_millis(1_700_000_000_000));
        std::fs::write(
            &log,
            format!(
                "{}\n",
                serde_json::json!({
                    "v": 1, "at": stamp, "kind": "transcript", "file": "main",
                    "root": "managed", "path": "projects/-w/abc.jsonl", "line": "{\"n\":1}",
                })
            ),
        )
        .expect("write");

        let args = || ReplayArgs {
            log: log.clone(),
            home: home.clone(),
            instance: DEFAULT_INSTANCE.to_string(),
            speed: 1.0,
            max_gap: 3.0,
            instant: 1,
        };
        args().run().await.expect("first replay");
        args().run().await.expect("second replay");

        let file = Paths::new(home).claude_root(ClaudeRoot::Managed).join("projects/-w/abc.jsonl");
        let content = std::fs::read_to_string(&file).expect("the transcript");
        assert_eq!(content, "{\"n\":1}\n", "the second replay replaced the first: {content:?}");
    }
    /// The replay command lines. A value-taking option consumes the next token
    /// whatever it looks like, `--flag=value` is now a clap spelling, a bare
    /// `--` is clap's terminator, and repeats take the last. Help is clap's.
    #[test]
    fn the_replay_argument_table() {
        let words = |line: &[&str]| -> Vec<String> { line.iter().map(|word| (*word).to_string()).collect() };
        let accepted: Vec<Vec<String>> = [
            &["--log", "a", "--home", "b"][..],
            &["--home", "b", "--log", "a"],
            &["--log", "a", "--home", "b", "--instance", "i", "--speed", "4", "--max-gap", "0.5", "--instant"],
            &["--log", "a", "--log", "c", "--home", "b"],
            &["--log", "--", "--home", "b"],
            &["--log", "a", "--home", "--"],
            &["--log", "a", "--home", "b", "--instant", "--instant"],
            &["--log=a", "--home", "b"],
            &["--log", "a", "--home", "b", "--"],
        ]
        .into_iter()
        .map(words)
        .collect();
        for argv in &accepted {
            assert!(parse_args(argv).is_ok(), "{argv:?} must parse: {:?}", parse_args(argv).err());
        }
        let parsed = parse_args(&words(&["--log", "a", "--log", "c", "--home", "b"])).expect("parsed");
        assert_eq!(parsed.log, PathBuf::from("c"), "the last --log wins");
        assert_eq!(parsed.home, PathBuf::from("b"));
        assert_eq!(parse_args(&words(&["--log", "--", "--home", "b"])).expect("parsed").log, PathBuf::from("--"));
        assert_eq!(parse_args(&words(&["--log=a", "--home", "b"])).expect("parsed").log, PathBuf::from("a"));

        for (argv, expected) in [
            (&[][..], ErrorKind::MissingRequiredArgument),
            (&["--log", "a"], ErrorKind::MissingRequiredArgument),
            (&["--home", "b"], ErrorKind::MissingRequiredArgument),
            (&["--log"], ErrorKind::InvalidValue),
            (&["--log", "a", "--home", "b", "--nope"], ErrorKind::UnknownArgument),
            (&["--instant"], ErrorKind::MissingRequiredArgument),
            (&["--log", "a", "--home", "b", "--speed"], ErrorKind::InvalidValue),
            (&["--log", "a", "--home", "b", "--speed", "fast"], ErrorKind::ValueValidation),
            (&["--log", "a", "--home", "b", "--speed", "0"], ErrorKind::ValueValidation),
            (&["--log", "a", "--home", "b", "--speed", "-1"], ErrorKind::ValueValidation),
            (&["--log", "a", "--home", "b", "--speed", "inf"], ErrorKind::ValueValidation),
            (&["--log", "a", "--home", "b", "--speed", "infinity"], ErrorKind::ValueValidation),
            (&["--log", "a", "--home", "b", "--max-gap", "x"], ErrorKind::ValueValidation),
            (&["--log", "a", "--home", "b", "--max-gap", "-2"], ErrorKind::ValueValidation),
            (&["--log", "a", "--home", "b", "--max-gap", "NaN"], ErrorKind::ValueValidation),
            (&["--log", "a", "--home", "b", "extra"], ErrorKind::UnknownArgument),
            (&["--log", "a", "--home", "b", "--", "x"], ErrorKind::UnknownArgument),
            // A value-taking option before --instance takes it as its value;
            // the word after it is then an unknown argument.
            (&["--log", "--instance", "x", "--home", "y"], ErrorKind::UnknownArgument),
        ] {
            assert_eq!(parse_args(&words(argv)).unwrap_err().kind(), expected, "{argv:?}");
        }
    }

    #[test]
    fn help_is_claps() {
        for argv in [vec!["--help"], vec!["-h"], vec!["--log", "a", "--help"], vec!["--help", "junk"]] {
            let argv: Vec<String> = argv.iter().map(|word| (*word).to_string()).collect();
            assert_eq!(parse_args(&argv).unwrap_err().kind(), clap::error::ErrorKind::DisplayHelp, "{argv:?}");
        }
    }

}
