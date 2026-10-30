//! The chat kinds (`crate::chats`) against the providers the server registers
//! (`crate::app`): which CLIs have instant chats.

use std::sync::Arc;

use droppen_instance::paths::Paths;
use droppen_sessions::Source;

use crate::chats::ChatKind;

/// The chat kinds and the CLIs are tied by wire name: of the CLIs the server
/// registers, one has the chat kind of its name exactly when its provider runs
/// instant chats, and each CLI kind names one that does. The kind is read by
/// its wire name, as the chat list files a session.
#[test]
fn every_cli_with_chats_has_the_chat_kind_of_its_name() {
    let home = tempfile::tempdir().expect("tempdir");
    let paths = Arc::new(Paths::new(home.path().to_path_buf()));
    let dirs = droppen_instance::instance::InstanceDirs::new(&paths.data_dir(), "chats-test").expect("valid name");
    dirs.create().expect("instance dirs");
    let accounts = Arc::new(droppen_accounts::store::AccountsStore::new(paths.clone(), crate::app::logins()));
    let quota = Arc::new(droppen_accounts::quota::QuotaCache::new(paths.clone(), crate::app::logins()));
    let decoration = crate::drive::NotifyDecoration::new(paths.clone(), dirs.clone());
    let injections = droppen_provider::InjectionRegistry::new(paths, &dirs, accounts, quota, decoration);
    let providers = droppen_provider::ProviderRegistry::new(injections, crate::app::register_providers);
    let with_chats: Vec<Source> = providers.sources().filter(|source| providers.capabilities(*source).chats).collect();
    assert!(!with_chats.is_empty());
    let kind_of = |source: Source| serde_json::from_value::<ChatKind>(serde_json::to_value(source).expect("a wire name")).ok();
    for source in providers.sources() {
        assert_eq!(kind_of(source).is_some(), with_chats.contains(&source), "{source}");
    }
    for (kind, source) in [(ChatKind::Claude, Source::Claude), (ChatKind::Codex, Source::Codex)] {
        assert_eq!(kind_of(source), Some(kind), "{source}");
        assert!(with_chats.contains(&source), "{source}");
    }
}
