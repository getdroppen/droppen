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

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use droppen_plugin_sdk::{valid_id, PluginId, StoreExport, EXPORT_FORMAT};

use super::{DecisionReason, PluginError, Plugins, StoreError};
use crate::paths::Paths;

/// What to print when the subcommand does not parse.
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

#[derive(Debug, PartialEq, Eq)]
enum Action {
    List,
    Install(String),
    Uninstall(String),
    Enable(String),
    Disable(String),
    Export { id: String, out: Option<String> },
    Import { id: String, file: String, replace: bool },
    Verify(String),
    Purge(String),
}

/// What `plugin` was asked, or clap's help text.
enum Parsed {
    Help(String),
    Run { action: Action, instance: Option<String> },
}

fn parse(args: &[String]) -> Result<Parsed, String> {
    let usage = || USAGE.to_string();
    let matches = match plugin_command().try_get_matches_from(std::iter::once("plugin".to_string()).chain(args.iter().cloned())) {
        Ok(matches) => matches,
        Err(error) if error.kind() == clap::error::ErrorKind::DisplayHelp => return Ok(Parsed::Help(error.to_string())),
        Err(error) => return Err(format!("{error}\n\n{}", usage())),
    };
    let cli = PluginCli::from_arg_matches(&matches).map_err(|error| format!("{error}\n\n{}", usage()))?;
    let instance = cli.instance.last().cloned();
    let action = match cli.command {
        Some(PluginCommand::List) => Action::List,
        Some(PluginCommand::Install { file }) => Action::Install(file),
        Some(PluginCommand::Uninstall { id }) => Action::Uninstall(id),
        Some(PluginCommand::Enable { id }) => Action::Enable(id),
        Some(PluginCommand::Disable { id }) => Action::Disable(id),
        Some(PluginCommand::Export { id, out }) => Action::Export { id, out },
        Some(PluginCommand::Import { id, file, replace }) => Action::Import { id, file, replace: replace > 0 },
        Some(PluginCommand::Verify { id }) => Action::Verify(id),
        Some(PluginCommand::Purge { yes: 0, .. }) => {
            return Err("purge moves the plugin's data aside; say --yes to mean it".to_string());
        }
        Some(PluginCommand::Purge { id, .. }) => Action::Purge(id),
        None => return Err(usage()),
    };
    Ok(Parsed::Run { action, instance })
}

/// `plugin`'s command tree, shown under the name this binary was run as.
fn plugin_command() -> clap::Command {
    let name = crate::program::Name::invoked();
    PluginCli::command()
        .bin_name(format!("{} plugin", name.command()))
        .after_long_help(name.rewrite(USAGE).into_owned())
}

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

/// `plugin`'s subcommands. `--out`, `--replace` and `--yes` are clap options, so
/// they may be written before or after the id and in the `--name=value` form;
/// repeated `--replace`/`--yes` count. Help is clap's.
#[derive(Debug, Parser)]
#[command(name = "plugin", disable_help_subcommand = true, args_override_self = true)]
struct PluginCli {
    #[arg(long, global = true, action = clap::ArgAction::Append, value_parser = crate::auth::cli::instance_name, allow_hyphen_values = true)]
    instance: Vec<String>,
    #[command(subcommand)]
    command: Option<PluginCommand>,
}

#[derive(Debug, Subcommand)]
enum PluginCommand {
    List,
    Install {
        #[arg(allow_hyphen_values = true)]
        file: String,
    },
    Uninstall {
        #[arg(value_parser = plugin_id)]
        id: String,
    },
    Enable {
        #[arg(value_parser = plugin_id)]
        id: String,
    },
    Disable {
        #[arg(value_parser = plugin_id)]
        id: String,
    },
    Export {
        #[arg(value_parser = plugin_id)]
        id: String,
        #[arg(long, allow_hyphen_values = true)]
        out: Option<String>,
    },
    Import {
        #[arg(value_parser = plugin_id)]
        id: String,
        #[arg(allow_hyphen_values = true)]
        file: String,
        #[arg(long, action = clap::ArgAction::Count)]
        replace: u8,
    },
    Verify {
        #[arg(value_parser = plugin_id)]
        id: String,
    },
    Purge {
        #[arg(value_parser = plugin_id)]
        id: String,
        #[arg(long, action = clap::ArgAction::Count)]
        yes: u8,
    },
}

/// Run a `plugin` subcommand. `args` is everything after `plugin`.
pub async fn run(args: &[String]) -> Result<(), String> {
    let (action, instance) = match parse(args)? {
        Parsed::Help(text) => {
            print!("{text}");
            return Ok(());
        }
        Parsed::Run { action, instance } => (action, instance),
    };
    let (paths, config) = crate::auth::cli::cli_context(instance.as_deref())?;
    let plugins = Plugins::new(paths.instance_dir(), paths.instance_db(), &config.app.plugins, None);
    tokio::task::spawn_blocking(move || execute(&plugins, &paths, action))
        .await
        .map_err(|error| error.to_string())?
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

fn execute(plugins: &Plugins, paths: &Paths, action: Action) -> Result<(), String> {
    let instance = paths.instance();
    match action {
        Action::List => {
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
        Action::Install(file) => {
            let bytes = std::fs::read(&file).map_err(|error| format!("cannot read {file}: {error}"))?;
            let status = plugins.install(&bytes, None).map_err(refusal)?;
            println!(
                "installed {} {} (build {}) on instance {instance}",
                status.id, status.version, status.build
            );
            println!("  {}", installed_line(&status));
            Ok(())
        }
        Action::Uninstall(id) => {
            plugins.uninstall(&id).map_err(refusal)?;
            println!("uninstalled {id} on instance {instance}; its documents and switch are kept");
            Ok(())
        }
        Action::Enable(id) | Action::Disable(id) if plugins.manifest(&id).is_none() => {
            Err(format!("this instance has no plugin {id:?} installed"))
        }
        Action::Enable(id) => {
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
        Action::Disable(id) => {
            plugins.set_enabled(&id, false, None, DecisionReason::Cli).map_err(refusal)?;
            println!("{id} is off on instance {instance}; its data is kept ({})", plugins.dir(&id).display());
            Ok(())
        }
        Action::Export { id, out } => {
            let store = plugins
                .existing_store(&id)
                .ok_or_else(|| format!("plugin {id} has no store on instance {instance} ({})", plugins.store_path(&id).display()))?;
            let export = store
                .export(&PluginId::new(id.clone()).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            let json = serde_json::to_string_pretty(&export).map_err(|e| e.to_string())? + "\n";
            match out {
                Some(out) => {
                    crate::fsutil::write_private(Path::new(&out), json.as_bytes()).map_err(|error| format!("cannot write {out}: {error}"))?;
                    println!("exported {} documents of {id} on instance {instance} to {out}", export.docs.len());
                }
                None => {
                    print!("{json}");
                    eprintln!("exported {} documents of {id} on instance {instance}", export.docs.len());
                }
            }
            Ok(())
        }
        Action::Import { id, .. } if plugins.manifest(&id).is_none() => Err(format!("this instance has no plugin {id:?} installed")),
        Action::Import { id, file, replace } => {
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
                .replace_all(export.docs, replace, None, |stored| {
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
                println!("{}", crate::program::Name::invoked().rewrite(&hint));
            }
            Ok(())
        }
        Action::Verify(id) => {
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
        Action::Purge(id) => {
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
            crate::db::forget(&plugins.store_path(&id));
            println!("moved {id}'s data on instance {instance} to {}", aside.display());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PluginSettings;
    use crate::plugins::tests::{host, installed_host};
    use serde_json::json;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    fn action(line: &str) -> Result<Action, String> {
        match parse(&args(line)) {
            Ok(Parsed::Run { action, .. }) => Ok(action),
            Ok(Parsed::Help(text)) => panic!("unexpected help: {text}"),
            Err(error) => Err(error),
        }
    }

    #[test]
    fn the_subcommands_parse_and_anything_else_is_the_usage() {
        assert_eq!(action("list"), Ok(Action::List));
        assert_eq!(action("install /tmp/tasks.tgz"), Ok(Action::Install("/tmp/tasks.tgz".into())));
        assert_eq!(action("uninstall tasks"), Ok(Action::Uninstall("tasks".into())));
        assert_eq!(action("enable tasks"), Ok(Action::Enable("tasks".into())));
        assert_eq!(
            action("export tasks --out /tmp/x.json"),
            Ok(Action::Export {
                id: "tasks".into(),
                out: Some("/tmp/x.json".into())
            })
        );
        assert_eq!(
            action("import tasks backup.json"),
            Ok(Action::Import {
                id: "tasks".into(),
                file: "backup.json".into(),
                replace: false
            })
        );
        assert_eq!(
            action("import tasks backup.json --replace"),
            Ok(Action::Import {
                id: "tasks".into(),
                file: "backup.json".into(),
                replace: true
            })
        );
        assert_eq!(action("purge tasks --yes"), Ok(Action::Purge("tasks".into())));
        assert!(action("purge tasks").unwrap_err().contains("--yes"));
        assert!(action("enable ../x").unwrap_err().contains("not a plugin id"));
        for bad in ["", "list extra", "install", "uninstall", "enable", "import tasks", "import tasks a b", "export tasks --out"] {
            assert!(action(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn install_and_uninstall_work_through_the_cli() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = host(dir.path(), PluginSettings::default(), None);
        let paths = Paths::new(dir.path().to_path_buf());
        let file = dir.path().join("tasks.tgz");
        std::fs::write(&file, crate::plugins::tests::package_bytes("tasks", "0.1.0")).unwrap();
        execute(&plugins, &paths, Action::Install(file.to_string_lossy().into_owned())).unwrap();
        assert_eq!(plugins.status_of("tasks").unwrap().version.to_string(), "0.1.0");
        assert_eq!(plugins.status_of("tasks").unwrap().entry.is_some(), true, "an install switches it on");
        execute(&plugins, &paths, Action::Uninstall("tasks".into())).unwrap();
        assert!(!plugins.package_dir("tasks").exists());
        assert!(execute(&plugins, &paths, Action::Uninstall("tasks".into())).unwrap_err().contains("no plugin"));
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
        assert!(execute(&plugins, &paths, Action::Purge("notes".into()))
            .unwrap_err()
            .contains("turn it off"));
        execute(&plugins, &paths, Action::Disable("notes".into())).unwrap();
        execute(&plugins, &paths, Action::Purge("notes".into())).unwrap();
        assert!(!plugins.dir("notes").exists());
        let moved: Vec<String> = std::fs::read_dir(plugins.instance_dir().join("plugins"))
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(moved.len() == 1 && moved[0].starts_with("notes.purged-"), "{moved:?}");
        execute(&plugins, &paths, Action::Purge("notes".into())).expect("nothing left to purge is not an error");
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
        let error = execute(
            &plugins,
            &paths,
            Action::Export {
                id: "notes".into(),
                out: Some(out.to_string_lossy().into_owned()),
            },
        )
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
        execute(
            &plugins,
            &paths,
            Action::Export {
                id: "notes".into(),
                out: Some(file.to_string_lossy().into_owned()),
            },
        )
        .unwrap();
        let import = |replace| Action::Import {
            id: "notes".into(),
            file: file.to_string_lossy().into_owned(),
            replace,
        };
        assert!(execute(&plugins, &paths, import(false)).unwrap_err().contains("--replace"));
        execute(&plugins, &paths, import(true)).unwrap();
        let state = plugins.get("notes", "state").unwrap();
        assert_eq!(state.revision, 2, "a replaced document moves forward");
        assert_eq!(plugins.history("notes", "state", 0, None).unwrap().versions.len(), 1);

        let other_dir = tempfile::tempdir().unwrap();
        let other = installed_host(other_dir.path(), &["notes"], PluginSettings::default(), None);
        execute(&other, &paths, import(false)).unwrap();
        assert_eq!(
            other.get("notes", "events/2026-10").unwrap().revision,
            1,
            "an empty store takes the export as it is"
        );
    }
    /// The plugin command lines, including the two literal flag words and the
    /// `--` this grammar treats as a word.
    /// clap parses the subcommands and options; `--out`, `--replace` and
    /// `--yes` may be spelled `--name=value` and written in any order, and
    /// repeated `--replace`/`--yes` count. Help is clap's.
    #[test]
    fn the_plugin_argument_table() {
        let words = |line: &[&str]| -> Vec<String> { line.iter().map(|word| (*word).to_string()).collect() };
        let run = |line: &[&str]| match parse(&words(line)) {
            Ok(Parsed::Run { action, .. }) => Ok(action),
            Ok(Parsed::Help(text)) => panic!("unexpected help: {text}"),
            Err(error) => Err(error),
        };
        let accepted: Vec<(&[&str], Action)> = vec![
            (&["list"][..], Action::List),
            (&["list", "--instance", "--hub"], Action::List),
            (&["install", "/tmp/tasks.tgz"], Action::Install("/tmp/tasks.tgz".into())),
            (&["install", "-x"], Action::Install("-x".into())),
            (&["install", "-"], Action::Install("-".into())),
            // A file that looks like a flag is a file now.
            (&["install", "--force"], Action::Install("--force".into())),
            (&["install", "--", "file"], Action::Install("file".into())),
            (&["uninstall", "tasks"], Action::Uninstall("tasks".into())),
            (&["enable", "tasks"], Action::Enable("tasks".into())),
            (&["disable", "tasks"], Action::Disable("tasks".into())),
            (&["verify", "tasks"], Action::Verify("tasks".into())),
            (&["export", "tasks"], Action::Export { id: "tasks".into(), out: None }),
            (
                &["export", "tasks", "--out", "/tmp/x.json"],
                Action::Export { id: "tasks".into(), out: Some("/tmp/x.json".into()) },
            ),
            (
                &["export", "--out", "/tmp/x.json", "tasks"],
                Action::Export { id: "tasks".into(), out: Some("/tmp/x.json".into()) },
            ),
            (
                &["export", "tasks", "--out=/tmp/x.json"],
                Action::Export { id: "tasks".into(), out: Some("/tmp/x.json".into()) },
            ),
            (&["export", "tasks", "--out", "--"], Action::Export { id: "tasks".into(), out: Some("--".into()) }),
            (&["export", "tasks", "--out", "--out"], Action::Export { id: "tasks".into(), out: Some("--out".into()) }),
            (
                &["import", "tasks", "backup.json"],
                Action::Import { id: "tasks".into(), file: "backup.json".into(), replace: false },
            ),
            (
                &["import", "tasks", "backup.json", "--replace"],
                Action::Import { id: "tasks".into(), file: "backup.json".into(), replace: true },
            ),
            (
                &["import", "tasks", "--replace", "backup.json"],
                Action::Import { id: "tasks".into(), file: "backup.json".into(), replace: true },
            ),
            (
                &["import", "--replace", "tasks", "backup.json"],
                Action::Import { id: "tasks".into(), file: "backup.json".into(), replace: true },
            ),
            (
                &["import", "tasks", "-x"],
                Action::Import { id: "tasks".into(), file: "-x".into(), replace: false },
            ),
            (
                &["import", "tasks", "--force"],
                Action::Import { id: "tasks".into(), file: "--force".into(), replace: false },
            ),
            (&["import", "--", "tasks", "f"], Action::Import { id: "tasks".into(), file: "f".into(), replace: false }),
            (&["purge", "tasks", "--yes"], Action::Purge("tasks".into())),
            (&["purge", "--yes", "tasks"], Action::Purge("tasks".into())),
            (&["purge", "tasks", "--yes", "--yes"], Action::Purge("tasks".into())),
            (&["uninstall", "--", "tasks"], Action::Uninstall("tasks".into())),
            (&["export", "--", "tasks"], Action::Export { id: "tasks".into(), out: None }),
        ];
        for (line, expected) in accepted {
            assert_eq!(run(line), Ok(expected), "{line:?}");
        }

        let refused: Vec<&[&str]> = vec![
            &[][..],
            &["list", "extra"],
            &["install"],
            &["uninstall"],
            &["uninstall", "../x"],
            &["enable"],
            &["disable"],
            &["verify"],
            &["import", "tasks"],
            &["import", "tasks", "a", "b"],
            &["import", "tasks", "--replace"],
            &["export", "tasks", "--out"],
            &["export", "tasks", "f"],
            &["purge", "tasks", "--yes=x"],
            &["--", "list"],
            &["purge", "--", "tasks", "--yes"],
            &["export", "tasks", "--", "--out", "f"],
        ];
        for line in refused {
            assert!(run(line).is_err(), "{line:?} must be refused");
        }
    }

    #[test]
    fn help_is_claps() {
        for line in ["--help", "-h", "list --help", "export --help", "export tasks --help"] {
            assert!(matches!(parse(&args(line)), Ok(Parsed::Help(_))), "{line:?} must answer help");
        }
    }

}
