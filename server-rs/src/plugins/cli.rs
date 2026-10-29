//! `remote-hand-server plugin list|install|uninstall|enable|disable|export|import|verify|purge`.
//!
//! Safe while a server runs, for the reason `user` is: an install swaps a
//! package directory in place, the switches live in the instance's `state.db`
//! and each store is its own SQLite file, so SQLite arbitrates between this
//! process and the server, and no instance lock is taken. A running server
//! reads the switch on every request and the package list on every status, so
//! `install`, `uninstall`, `enable` and `disable` take effect at once; it is
//! not told about documents this writes until a client next reads them.
//!
//! The instance is chosen exactly as `user` chooses it — `--instance`, else
//! `$APP_INSTANCE`, else `default`, and a defaulted instance that does not
//! exist is a refusal — and named in every line of output.

use std::path::Path;

use clap::{Args, Subcommand};
use droppen_plugin_sdk::{valid_id, PluginId, StoreExport, EXPORT_FORMAT};

use super::{DecisionReason, PluginError, Plugins, StoreError};
use droppen_instance::paths::Paths;

/// The rest of `plugin --help`, after the options clap lists.
pub const USAGE: &str = "\
USAGE:
    remote-hand-server plugin list                              [--instance <name>]
    remote-hand-server plugin install <file.tgz>                [--instance <name>]
    remote-hand-server plugin uninstall <id>                    [--instance <name>]
    remote-hand-server plugin enable  <id>                      [--instance <name>]
    remote-hand-server plugin disable <id>                      [--instance <name>]
    remote-hand-server plugin export  <id> [--out <file>]       [--instance <name>]
    remote-hand-server plugin import  <id> <file> [--replace]   [--instance <name>]
    remote-hand-server plugin verify  <id>                      [--instance <name>]
    remote-hand-server plugin purge   <id> --yes                [--instance <name>]

`list` shows the plugins installed in the instance, the build each was
installed at, and whether each is on. `install` unpacks and checks a package
(the tarball `pnpm pack` writes) and switches it on, replacing an installed
plugin of the same id atomically; `uninstall` removes the code and keeps the
plugin's documents and its switch. `enable`/`disable` switch one for the
instance (the configuration's [plugins] enable/disable, or
APP_PLUGINS__ENABLE/DISABLE, win over both). `export` writes the plugin's
documents as JSON (to stdout without --out); `import` restores such a file,
refusing over existing documents unless --replace, which keeps the replaced
values in their history. `verify` checks a plugin's store and its documents.
`purge` moves a plugin's data aside to plugins/<id>.purged-<time>/; the plugin
must be off.

Safe while the server runs.";

/// A plugin id, checked by the same rule every shape used.
fn plugin_id(raw: &str) -> Result<String, String> {
    if valid_id(raw) {
        Ok(raw.to_string())
    } else {
        Err(format!(
            "{raw:?} is not a plugin id: 2-32 characters of a-z, 0-9 or '-', starting with a letter"
        ))
    }
}

/// The `plugin` command line.
#[derive(Debug, PartialEq, Eq, Args)]
#[command(disable_help_subcommand = true, args_override_self = true)]
pub struct PluginArgs {
    /// The instance, wherever among the arguments it is written.
    #[arg(long, global = true, action = clap::ArgAction::Append, value_parser = crate::auth::cli::instance_name, allow_hyphen_values = true)]
    pub instance: Vec<String>,
    /// What to do.
    #[command(subcommand)]
    pub command: PluginCommand,
}

/// What to do with the instance's plugins.
#[derive(Debug, PartialEq, Eq, Subcommand)]
pub enum PluginCommand {
    /// List the installed plugins.
    List,
    /// Install a package and switch it on.
    Install {
        /// The package tarball.
        #[arg(allow_hyphen_values = true)]
        file: String,
    },
    /// Remove a plugin's code, keeping its documents and switch.
    Uninstall {
        /// The plugin id.
        #[arg(value_parser = plugin_id)]
        id: String,
    },
    /// Switch a plugin on.
    Enable {
        /// The plugin id.
        #[arg(value_parser = plugin_id)]
        id: String,
    },
    /// Switch a plugin off.
    Disable {
        /// The plugin id.
        #[arg(value_parser = plugin_id)]
        id: String,
    },
    /// Write a plugin's documents as JSON (stdout without `--out`).
    Export {
        /// The plugin id.
        #[arg(value_parser = plugin_id)]
        id: String,
        /// The file to write instead of stdout.
        #[arg(long, allow_hyphen_values = true)]
        out: Option<String>,
    },
    /// Restore an exported document store.
    Import {
        /// The plugin id.
        #[arg(value_parser = plugin_id)]
        id: String,
        /// The exported JSON file.
        #[arg(allow_hyphen_values = true)]
        file: String,
        /// Replace existing documents (their old values stay in history).
        #[arg(long, action = clap::ArgAction::Count)]
        replace: u8,
    },
    /// Check a plugin's store and its documents.
    Verify {
        /// The plugin id.
        #[arg(value_parser = plugin_id)]
        id: String,
    },
    /// Move a plugin's data aside; the plugin must be off.
    Purge {
        /// The plugin id.
        #[arg(value_parser = plugin_id)]
        id: String,
        /// Mean it.
        #[arg(long, action = clap::ArgAction::Count)]
        yes: u8,
    },
}

impl PluginArgs {
    /// Run the chosen action. `purge` without `--yes` is refused before the
    /// configuration is read.
    pub async fn run(self) -> Result<(), String> {
        if let PluginCommand::Purge { yes: 0, .. } = self.command {
            return Err("purge moves the plugin's data aside; say --yes to mean it".to_string());
        }
        let (paths, config) = crate::auth::cli::cli_context(self.instance.last().map(String::as_str))?;
        let plugins = Plugins::new(paths.instance_dir(), paths.instance_db(), &config.app.plugins, None);
        let command = self.command;
        tokio::task::spawn_blocking(move || command.execute(&plugins, &paths))
            .await
            .map_err(|error| error.to_string())?
    }
}

fn refusal(error: PluginError) -> String {
    match error {
        PluginError::Store(StoreError::NotEmpty(count)) => {
            format!("the store already holds {count} documents; add --replace to replace them (the replaced values are kept in their history)")
        }
        other => other.to_string(),
    }
}

/// One line of what an install left the plugin as, for `plugin install`.
fn installed_line(status: &droppen_plugin_sdk::PluginStatus) -> String {
    let state = if status.enabled { "on" } else { "off" };
    match status.reason.as_deref() {
        Some(reason) => format!("{state}: {reason}"),
        None => state.to_string(),
    }
}

impl PluginCommand {
    /// Do it, against the instance these paths are for.
    fn execute(self, plugins: &Plugins, paths: &Paths) -> Result<(), String> {
        let instance = paths.instance();
        match self {
            PluginCommand::List => {
                let statuses = plugins.status();
                if statuses.is_empty() {
                    println!("no plugins are installed on instance {instance}");
                    return Ok(());
                }
                println!("instance {instance}:");
                for status in statuses {
                    let state = if status.enabled { "on" } else { "off" };
                    let pinned = if status.forced { ", pinned by the configuration" } else { "" };
                    let stored = match plugins.existing_store(status.id.as_str()) {
                        Some(store) => format!(", {} documents", store.count().map_err(|e| e.to_string())?),
                        None => String::new(),
                    };
                    println!("  {} {} (build {}) — {state}{pinned}{stored}", status.id, status.version, status.build);
                    if let Some(reason) = status.reason {
                        println!("      {reason}");
                    }
                    if let Some(notice) = status.notice {
                        println!("      ! {notice}");
                    }
                }
                Ok(())
            }
            PluginCommand::Install { file } => {
                let bytes = std::fs::read(&file).map_err(|error| format!("cannot read {file}: {error}"))?;
                let status = plugins.install(&bytes, None).map_err(refusal)?;
                println!(
                    "installed {} {} (build {}) on instance {instance}",
                    status.id, status.version, status.build
                );
                println!("  {}", installed_line(&status));
                Ok(())
            }
            PluginCommand::Uninstall { id } => {
                plugins.uninstall(&id).map_err(refusal)?;
                println!("uninstalled {id} on instance {instance}; its documents and switch are kept");
                Ok(())
            }
            PluginCommand::Enable { id } | PluginCommand::Disable { id } if plugins.manifest(&id).is_none() => {
                Err(format!("this instance has no plugin {id:?} installed"))
            }
            PluginCommand::Enable { id } => {
                let status = plugins.set_enabled(&id, true, None, DecisionReason::Cli).map_err(refusal)?;
                println!("{id} is {} on instance {instance}", if status.enabled { "on" } else { "still off" });
                if let Some(reason) = status.reason {
                    println!("  {reason}");
                }
                if let Some(notice) = plugins.status_of(&id).and_then(|status| status.notice) {
                    println!("  ! {notice}");
                }
                Ok(())
            }
            PluginCommand::Disable { id } => {
                plugins.set_enabled(&id, false, None, DecisionReason::Cli).map_err(refusal)?;
                println!("{id} is off on instance {instance}; its data is kept ({})", plugins.dir(&id).display());
                Ok(())
            }
            PluginCommand::Export { id, out } => {
                let store = plugins
                    .existing_store(&id)
                    .ok_or_else(|| format!("plugin {id} has no store on instance {instance} ({})", plugins.store_path(&id).display()))?;
                let export = store
                    .export(&PluginId::new(id.clone()).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
                let json = serde_json::to_string_pretty(&export).map_err(|e| e.to_string())? + "\n";
                match out {
                    Some(out) => {
                        droppen_base::fsutil::write_private(Path::new(&out), json.as_bytes()).map_err(|error| format!("cannot write {out}: {error}"))?;
                        println!("exported {} documents of {id} on instance {instance} to {out}", export.docs.len());
                    }
                    None => {
                        print!("{json}");
                        eprintln!("exported {} documents of {id} on instance {instance}", export.docs.len());
                    }
                }
                Ok(())
            }
            PluginCommand::Import { id, .. } if plugins.manifest(&id).is_none() => Err(format!("this instance has no plugin {id:?} installed")),
            PluginCommand::Import { id, file, replace } => {
                let text = std::fs::read_to_string(&file).map_err(|error| format!("cannot read {file}: {error}"))?;
                let export: StoreExport = serde_json::from_str(&text).map_err(|error| format!("{file} is not a plugin export: {error}"))?;
                if export.format != EXPORT_FORMAT {
                    return Err(format!("{file} is in format {:?}; this binary reads {EXPORT_FORMAT}", export.format));
                }
                if export.plugin.as_str() != id {
                    return Err(format!("{file} holds the documents of {}, not {id}", export.plugin));
                }
                let expected: Vec<(String, serde_json::Value)> = export.docs.iter().map(|doc| (doc.name.clone(), doc.value.clone())).collect();
                let stored = plugins
                    .open_store(&id)
                    .replace_all(export.docs, replace > 0, None, |stored| {
                        let mut got: Vec<(String, serde_json::Value)> = stored.iter().map(|doc| (doc.name.clone(), doc.value.clone())).collect();
                        let mut want = expected.clone();
                        got.sort_by(|a, b| a.0.cmp(&b.0));
                        want.sort_by(|a, b| a.0.cmp(&b.0));
                        if got == want {
                            Ok(())
                        } else {
                            Err("the store does not read back as the export".to_string())
                        }
                    })
                    .map_err(|error| refusal(error.into()))?;
                println!("imported {} documents into {id} on instance {instance} from {file}", stored.len());
                if !plugins.is_enabled(&id) {
                    let hint = format!("  {id} is off on this instance; turn it on with: remote-hand-server plugin enable {id} --instance {instance}");
                    println!("{}", droppen_base::program::Name::invoked().rewrite(&hint));
                }
                Ok(())
            }
            PluginCommand::Verify { id } => {
                let store = plugins
                    .existing_store(&id)
                    .ok_or_else(|| format!("plugin {id} has no store on instance {instance}"))?;
                let integrity = store.integrity().map_err(|e| e.to_string())?;
                let count = store.count().map_err(|e| e.to_string())?;
                let bytes = store.bytes().map_err(|e| e.to_string())?;
                store.all().map_err(|e| e.to_string())?;
                println!("instance {instance}, plugin {id}: {count} documents, {bytes} bytes of JSON, integrity {integrity}");
                if integrity == "ok" {
                    Ok(())
                } else {
                    Err(format!("the store's integrity check failed: {integrity}"))
                }
            }
            PluginCommand::Purge { id, .. } => {
                if plugins.is_enabled(&id) {
                    return Err(format!(
                        "{id} is on for instance {instance}; turn it off first (remote-hand-server plugin disable {id}), or take it out of [plugins] enable"
                    ));
                }
                let dir = plugins.dir(&id);
                if !dir.exists() {
                    println!("{id} has no data on instance {instance}; nothing to purge");
                    return Ok(());
                }
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|since| since.as_secs())
                    .unwrap_or_default();
                let aside = plugins.instance_dir().join("plugins").join(format!("{id}.purged-{stamp}"));
                std::fs::rename(&dir, &aside).map_err(|error| format!("cannot move {} aside: {error}", dir.display()))?;
                droppen_instance::db::forget(&plugins.store_path(&id));
                println!("moved {id}'s data on instance {instance} to {}", aside.display());
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::error::ErrorKind;

    use super::*;
    use droppen_http::config::PluginSettings;
    use crate::plugins::tests::{host, installed_host};
    use serde_json::json;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    fn parse(words: &[String]) -> Result<PluginArgs, clap::Error> {
        match crate::cli::tests::parse_words("plugin", words)? {
            crate::cli::Command::Plugin(parsed) => Ok(parsed),
            other => panic!("{words:?} parsed as {other:?}"),
        }
    }

    /// The refusal `run` gives `purge` without `--yes`.
    const PURGE_WITHOUT_YES: &str = "purge moves the plugin's data aside; say --yes to mean it";

    /// Parsing, and the check the command applies before it runs.
    fn checked(words: &[String]) -> Result<PluginCommand, String> {
        let parsed = parse(words).map_err(|error| error.to_string())?;
        match parsed.command {
            PluginCommand::Purge { yes: 0, .. } => Err(PURGE_WITHOUT_YES.to_string()),
            command => Ok(command),
        }
    }

    fn action(line: &str) -> Result<PluginCommand, String> {
        checked(&args(line))
    }

    /// The clap refusal of a command line.
    fn refusal(line: &[&str]) -> ErrorKind {
        let words: Vec<String> = line.iter().map(|word| (*word).to_string()).collect();
        parse(&words).unwrap_err().kind()
    }

    #[test]
    fn the_subcommands_parse_and_anything_else_is_the_usage() {
        assert_eq!(action("list"), Ok(PluginCommand::List));
        assert_eq!(action("install /tmp/tasks.tgz"), Ok(PluginCommand::Install { file: "/tmp/tasks.tgz".into() }));
        assert_eq!(action("uninstall tasks"), Ok(PluginCommand::Uninstall { id: "tasks".into() }));
        assert_eq!(action("enable tasks"), Ok(PluginCommand::Enable { id: "tasks".into() }));
        assert_eq!(
            action("export tasks --out /tmp/x.json"),
            Ok(PluginCommand::Export {
                id: "tasks".into(),
                out: Some("/tmp/x.json".into())
            })
        );
        assert_eq!(
            action("import tasks backup.json"),
            Ok(PluginCommand::Import {
                id: "tasks".into(),
                file: "backup.json".into(),
                replace: 0
            })
        );
        assert_eq!(
            action("import tasks backup.json --replace"),
            Ok(PluginCommand::Import {
                id: "tasks".into(),
                file: "backup.json".into(),
                replace: 1
            })
        );
        assert_eq!(action("purge tasks --yes"), Ok(PluginCommand::Purge { id: "tasks".into(), yes: 1 }));
        assert_eq!(action("purge tasks").unwrap_err(), PURGE_WITHOUT_YES);
        assert_eq!(
            parse(&args("enable ../x")).unwrap_err().kind(),
            ErrorKind::ValueValidation,
            "{}",
            parse(&args("enable ../x")).unwrap_err()
        );
        assert!(parse(&args("enable ../x")).unwrap_err().to_string().contains("is not a plugin id"));
        for (bad, expected) in [
            ("", ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand),
            ("list extra", ErrorKind::UnknownArgument),
            ("install", ErrorKind::MissingRequiredArgument),
            ("uninstall", ErrorKind::MissingRequiredArgument),
            ("enable", ErrorKind::MissingRequiredArgument),
            ("import tasks", ErrorKind::MissingRequiredArgument),
            ("import tasks a b", ErrorKind::UnknownArgument),
            ("export tasks --out", ErrorKind::InvalidValue),
        ] {
            assert_eq!(parse(&args(bad)).unwrap_err().kind(), expected, "{bad:?}");
        }
    }

    #[test]
    fn install_and_uninstall_work_through_the_cli() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = host(dir.path(), PluginSettings::default(), None);
        let paths = Paths::new(dir.path().to_path_buf());
        let file = dir.path().join("tasks.tgz");
        std::fs::write(&file, crate::plugins::tests::package_bytes("tasks", "0.1.0")).unwrap();
        PluginCommand::Install { file: file.to_string_lossy().into_owned() }.execute(&plugins, &paths).unwrap();
        assert_eq!(plugins.status_of("tasks").unwrap().version.to_string(), "0.1.0");
        assert_eq!(plugins.status_of("tasks").unwrap().entry.is_some(), true, "an install switches it on");
        PluginCommand::Uninstall { id: "tasks".into() }.execute(&plugins, &paths).unwrap();
        assert!(!plugins.package_dir("tasks").exists());
        assert!(PluginCommand::Uninstall { id: "tasks".into() }.execute(&plugins, &paths).unwrap_err().contains("no plugin"));
    }

    #[test]
    fn purge_wants_the_plugin_off_and_moves_its_data_aside() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = installed_host(dir.path(), &["notes"], PluginSettings::default(), None);
        let paths = Paths::new(dir.path().to_path_buf());
        plugins
            .put(
                "notes",
                "a",
                droppen_plugin_sdk::PutDoc {
                    expected_revision: None,
                    value: json!(1),
                },
                None,
            )
            .unwrap();
        assert!(PluginCommand::Purge { id: "notes".into(), yes: 1 }
            .execute(&plugins, &paths)
            .unwrap_err()
            .contains("turn it off"));
        PluginCommand::Disable { id: "notes".into() }.execute(&plugins, &paths).unwrap();
        PluginCommand::Purge { id: "notes".into(), yes: 1 }.execute(&plugins, &paths).unwrap();
        assert!(!plugins.dir("notes").exists());
        let moved: Vec<String> = std::fs::read_dir(plugins.instance_dir().join("plugins"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(moved.len() == 1 && moved[0].starts_with("notes.purged-"), "{moved:?}");
        PluginCommand::Purge { id: "notes".into(), yes: 1 }.execute(&plugins, &paths).expect("nothing left to purge is not an error");
    }

    #[test]
    fn an_export_to_a_missing_directory_is_refused_with_enoent() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = installed_host(dir.path(), &["notes"], PluginSettings::default(), None);
        let paths = Paths::new(dir.path().to_path_buf());
        plugins
            .put(
                "notes",
                "state",
                droppen_plugin_sdk::PutDoc {
                    expected_revision: None,
                    value: json!({ "name": "state" }),
                },
                None,
            )
            .unwrap();
        let out = dir.path().join("missing").join("notes.json");
        let error = PluginCommand::Export {
            id: "notes".into(),
            out: Some(out.to_string_lossy().into_owned()),
        }
        .execute(&plugins, &paths)
        .expect_err("writing into a directory that is not there is the old ENOENT");
        assert!(error.contains("cannot write"), "{error}");
        assert!(error.contains("os error 2"), "{error}");
        assert!(!out.exists());
        assert!(!out.parent().expect("parent").exists(), "the missing directory is not created");
    }

    #[test]
    fn an_export_imports_back_and_refuses_to_overwrite_without_replace() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = installed_host(dir.path(), &["notes"], PluginSettings::default(), None);
        let paths = Paths::new(dir.path().to_path_buf());
        for name in ["state", "events/2026-10"] {
            plugins
                .put(
                    "notes",
                    name,
                    droppen_plugin_sdk::PutDoc {
                        expected_revision: None,
                        value: json!({ "name": name }),
                    },
                    None,
                )
                .unwrap();
        }
        let file = dir.path().join("notes.json");
        PluginCommand::Export {
            id: "notes".into(),
            out: Some(file.to_string_lossy().into_owned()),
        }
        .execute(&plugins, &paths)
        .unwrap();
        let import = |replace: u8| PluginCommand::Import {
            id: "notes".into(),
            file: file.to_string_lossy().into_owned(),
            replace,
        };
        assert!(import(0).execute(&plugins, &paths).unwrap_err().contains("--replace"));
        import(1).execute(&plugins, &paths).unwrap();
        let state = plugins.get("notes", "state").unwrap();
        assert_eq!(state.revision, 2, "a replaced document moves forward");
        assert_eq!(plugins.history("notes", "state", 0, None).unwrap().versions.len(), 1);

        let other_dir = tempfile::tempdir().unwrap();
        let other = installed_host(other_dir.path(), &["notes"], PluginSettings::default(), None);
        import(0).execute(&other, &paths).unwrap();
        assert_eq!(
            other.get("notes", "events/2026-10").unwrap().revision,
            1,
            "an empty store takes the export as it is"
        );
    }

    /// clap parses the subcommands and options; `--out`, `--replace` and
    /// `--yes` may be spelled `--name=value` and written in any order, and
    /// repeated `--replace`/`--yes` count.
    #[test]
    fn the_plugin_argument_table() {
        let words = |line: &[&str]| -> Vec<String> { line.iter().map(|word| (*word).to_string()).collect() };
        let accepted: Vec<(&[&str], PluginCommand)> = vec![
            (&["list"][..], PluginCommand::List),
            (&["list", "--instance", "--hub"], PluginCommand::List),
            (&["install", "/tmp/tasks.tgz"], PluginCommand::Install { file: "/tmp/tasks.tgz".into() }),
            (&["install", "-x"], PluginCommand::Install { file: "-x".into() }),
            (&["install", "-"], PluginCommand::Install { file: "-".into() }),
            // A file that looks like a flag is a file now.
            (&["install", "--force"], PluginCommand::Install { file: "--force".into() }),
            (&["install", "--", "file"], PluginCommand::Install { file: "file".into() }),
            (&["uninstall", "tasks"], PluginCommand::Uninstall { id: "tasks".into() }),
            (&["enable", "tasks"], PluginCommand::Enable { id: "tasks".into() }),
            (&["disable", "tasks"], PluginCommand::Disable { id: "tasks".into() }),
            (&["verify", "tasks"], PluginCommand::Verify { id: "tasks".into() }),
            (&["export", "tasks"], PluginCommand::Export { id: "tasks".into(), out: None }),
            (
                &["export", "tasks", "--out", "/tmp/x.json"],
                PluginCommand::Export { id: "tasks".into(), out: Some("/tmp/x.json".into()) },
            ),
            (
                &["export", "--out", "/tmp/x.json", "tasks"],
                PluginCommand::Export { id: "tasks".into(), out: Some("/tmp/x.json".into()) },
            ),
            (
                &["export", "tasks", "--out=/tmp/x.json"],
                PluginCommand::Export { id: "tasks".into(), out: Some("/tmp/x.json".into()) },
            ),
            (&["export", "tasks", "--out", "--"], PluginCommand::Export { id: "tasks".into(), out: Some("--".into()) }),
            (&["export", "tasks", "--out", "--out"], PluginCommand::Export { id: "tasks".into(), out: Some("--out".into()) }),
            (
                &["import", "tasks", "backup.json"],
                PluginCommand::Import { id: "tasks".into(), file: "backup.json".into(), replace: 0 },
            ),
            (
                &["import", "tasks", "backup.json", "--replace"],
                PluginCommand::Import { id: "tasks".into(), file: "backup.json".into(), replace: 1 },
            ),
            (
                &["import", "tasks", "--replace", "backup.json"],
                PluginCommand::Import { id: "tasks".into(), file: "backup.json".into(), replace: 1 },
            ),
            (
                &["import", "--replace", "tasks", "backup.json"],
                PluginCommand::Import { id: "tasks".into(), file: "backup.json".into(), replace: 1 },
            ),
            (
                &["import", "tasks", "-x"],
                PluginCommand::Import { id: "tasks".into(), file: "-x".into(), replace: 0 },
            ),
            (
                &["import", "tasks", "--force"],
                PluginCommand::Import { id: "tasks".into(), file: "--force".into(), replace: 0 },
            ),
            (&["import", "--", "tasks", "f"], PluginCommand::Import { id: "tasks".into(), file: "f".into(), replace: 0 }),
            (&["purge", "tasks", "--yes"], PluginCommand::Purge { id: "tasks".into(), yes: 1 }),
            (&["purge", "--yes", "tasks"], PluginCommand::Purge { id: "tasks".into(), yes: 1 }),
            (&["purge", "tasks", "--yes", "--yes"], PluginCommand::Purge { id: "tasks".into(), yes: 2 }),
            (&["uninstall", "--", "tasks"], PluginCommand::Uninstall { id: "tasks".into() }),
            (&["export", "--", "tasks"], PluginCommand::Export { id: "tasks".into(), out: None }),
        ];
        for (line, expected) in accepted {
            assert_eq!(checked(&words(line)), Ok(expected), "{line:?}");
        }

        for (line, expected) in [
            (&[][..], ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand),
            (&["list", "extra"], ErrorKind::UnknownArgument),
            (&["install"], ErrorKind::MissingRequiredArgument),
            (&["uninstall"], ErrorKind::MissingRequiredArgument),
            (&["uninstall", "../x"], ErrorKind::ValueValidation),
            (&["enable"], ErrorKind::MissingRequiredArgument),
            (&["disable"], ErrorKind::MissingRequiredArgument),
            (&["verify"], ErrorKind::MissingRequiredArgument),
            (&["import", "tasks"], ErrorKind::MissingRequiredArgument),
            (&["import", "tasks", "a", "b"], ErrorKind::UnknownArgument),
            (&["import", "tasks", "--replace"], ErrorKind::MissingRequiredArgument),
            (&["export", "tasks", "--out"], ErrorKind::InvalidValue),
            (&["export", "tasks", "f"], ErrorKind::UnknownArgument),
            (&["purge", "tasks", "--yes=x"], ErrorKind::TooManyValues),
            (&["--", "list"], ErrorKind::UnknownArgument),
            (&["export", "tasks", "--", "--out", "f"], ErrorKind::UnknownArgument),
        ] {
            assert_eq!(refusal(line), expected, "{line:?}");
        }
        assert_eq!(checked(&words(&["purge", "tasks"])).unwrap_err(), PURGE_WITHOUT_YES);
    }

    #[test]
    fn help_is_claps() {
        for line in ["--help", "-h", "list --help", "export --help", "install --help", "export tasks --help"] {
            assert_eq!(parse(&args(line)).unwrap_err().kind(), ErrorKind::DisplayHelp, "{line:?}");
        }
    }

    #[test]
    fn every_instance_is_validated_and_the_last_is_used() {
        let parsed = parse(&args("list --instance bob --instance carol")).expect("parsed");
        assert_eq!(parsed.instance.last().map(String::as_str), Some("carol"));
        for (line, expected) in [
            ("list --instance Bad --instance probe", ErrorKind::ValueValidation),
            ("list --instance=Bad --instance=probe", ErrorKind::ValueValidation),
            ("list --instance=", ErrorKind::ValueValidation),
            ("list --instance", ErrorKind::InvalidValue),
            ("list --instance Staging", ErrorKind::ValueValidation),
        ] {
            assert_eq!(parse(&args(line)).unwrap_err().kind(), expected, "{line:?}");
        }
    }
}
