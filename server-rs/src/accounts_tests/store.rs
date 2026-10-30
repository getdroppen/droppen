//! The store's checks that ask a credential's own CLI's login.

use std::sync::Arc;

use droppen_accounts::store::*;
use droppen_base::env_block::parse_env_block;
use droppen_instance::paths::Paths;

fn oauth(secret: &str) -> Credential {
    Credential::Oauth(secret.to_string())
}

fn env(block: &str) -> Credential {
    Credential::Env(parse_env_block(block).expect("a block the tests wrote"))
}

/// A store on the logins the entry builds.
fn store() -> (tempfile::TempDir, AccountsStore) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = AccountsStore::new(Arc::new(Paths::new(dir.path().to_path_buf())), crate::app::logins());
    (dir, store)
}

/// An env account for the wrong CLI, and a credential of the wrong shape.
#[tokio::test]
async fn a_row_nothing_could_spawn_is_refused_before_it_is_stored() {
    let (_dir, store) = store();
    assert!(matches!(
        store.create("Nope", AccountProvider::Codex, oauth("tok")).await,
        Err(AccountError::Invalid(_))
    ));
    // A codex block with no key variable never reaches a spawn, so it never
    // reaches the table either.
    assert!(matches!(
        store.create("Nope", AccountProvider::Codex, env("SOMETHING_ELSE=x")).await,
        Err(AccountError::Invalid(_))
    ));
    assert!(store.list().await.is_empty(), "neither refusal left a row behind");

    let account = store
        .create("Gateway", AccountProvider::Claude, env("ANTHROPIC_AUTH_TOKEN=bearer"))
        .await
        .expect("create");
    assert_eq!(account.kind, AccountKind::Env);
    assert_eq!(
        account.env_keys.as_ref().map(Vec::as_slice),
        Some(["ANTHROPIC_AUTH_TOKEN".to_string()].as_slice())
    );
    let json = serde_json::to_string(&account).expect("serialize");
    assert!(!json.contains("bearer"), "a value is shown back to nobody: {json}");

    // A patch has to match the account's own kind in both directions.
    assert!(matches!(
        store
            .update(
                &account.id,
                AccountPatch {
                    credential: Some(oauth("tok")),
                    ..Default::default()
                }
            )
            .await,
        Err(AccountError::Invalid(_))
    ));
    let replaced = store
        .update(
            &account.id,
            AccountPatch {
                credential: Some(env("ANTHROPIC_AUTH_TOKEN=bearer2\nANTHROPIC_BASE_URL=https://gw/v1")),
                ..Default::default()
            },
        )
        .await
        .expect("update")
        .expect("found");
    assert_eq!(
        replaced.env_keys.as_ref().map(Vec::as_slice),
        Some(["ANTHROPIC_AUTH_TOKEN".to_string(), "ANTHROPIC_BASE_URL".to_string()].as_slice()),
        "the block is replaced whole: what is not in the text is not set"
    );
}

/// A replacement is held to the rule a new row is: a codex block with no key
/// variable never reaches the table, and the block it would have replaced
/// is still the one stored.
#[tokio::test]
async fn a_replacement_nothing_could_spawn_is_refused_before_it_is_stored() {
    let (_dir, store) = store();
    let codex = store
        .create("Codex", AccountProvider::Codex, env("OPENAI_API_KEY=not-a-real-openai-key"))
        .await
        .expect("create");
    let refused = store
        .update(
            &codex.id,
            AccountPatch {
                credential: Some(env("SOMETHING_ELSE=x")),
                ..Default::default()
            },
        )
        .await;
    assert!(matches!(refused, Err(AccountError::Invalid(_))), "{refused:?}");
    assert_eq!(
        parse_env_block(&store.env_block(&codex.id).await.expect("the block")).expect("a block"),
        parse_env_block("OPENAI_API_KEY=not-a-real-openai-key").expect("a block"),
        "the stored block is untouched"
    );
}
