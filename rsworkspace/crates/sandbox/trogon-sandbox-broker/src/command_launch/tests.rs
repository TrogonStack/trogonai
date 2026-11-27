use std::time::{Duration, SystemTime};

use trogon_sandbox_contract::{CommandOutput, LaunchKey, LeaseId, RunCommandRequest, RunCommandResponse};

use super::{Claim, CommandLaunch, CommandLaunchStore, InMemoryCommandLaunchStore, LaunchState, fingerprint};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn key() -> Result<LaunchKey, Box<dyn std::error::Error>> {
    Ok(LaunchKey::new("k")?)
}

fn lease(id: &str) -> LeaseId {
    LeaseId::new(id)
}

fn done(fingerprint: &str) -> CommandLaunch {
    CommandLaunch {
        fingerprint: fingerprint.to_string(),
        recorded_at: None,
        state: LaunchState::Done {
            response: RunCommandResponse::Completed(CommandOutput::default()),
        },
    }
}

#[tokio::test]
async fn in_memory_replace_only_applies_to_the_expected_entry() -> TestResult {
    let store = InMemoryCommandLaunchStore::new();
    let l = lease("l1");
    let first = done("f");
    store.finish(&l, &key()?, &first).await?;
    let retired = first.retired();
    assert!(store.replace(&l, &key()?, &first, &retired).await?);
    assert!(!store.replace(&l, &key()?, &first, &retired).await?);
    assert_eq!(store.claim(&l, &key()?, "f").await?, Claim::Existing(retired));
    assert!(!store.replace(&lease("l2"), &key()?, &first, &first).await?);
    Ok(())
}

#[test]
fn age_is_unknown_for_entries_without_a_timestamp() {
    let now = SystemTime::now();
    assert!(!done("f").older_than(Duration::ZERO, now));
    let mut stamped = done("f");
    stamped.recorded_at = Some(now - Duration::from_secs(10));
    assert!(stamped.older_than(Duration::from_secs(5), now));
    assert!(!stamped.older_than(Duration::from_secs(20), now));
}

#[tokio::test]
async fn in_memory_claim_is_exclusive_until_forgotten() -> TestResult {
    let store = InMemoryCommandLaunchStore::new();
    let l = lease("l1");
    assert_eq!(store.claim(&l, &key()?, "f").await?, Claim::Claimed);
    assert!(matches!(
        store.claim(&l, &key()?, "f").await?,
        Claim::Existing(CommandLaunch {
            state: LaunchState::Pending,
            ..
        })
    ));
    store.finish(&l, &key()?, &done("f")).await?;
    assert_eq!(store.claim(&l, &key()?, "f").await?, Claim::Existing(done("f")));
    store.forget(&l, &key()?).await?;
    assert_eq!(store.claim(&l, &key()?, "f").await?, Claim::Claimed);
    Ok(())
}

#[tokio::test]
async fn in_memory_keys_are_scoped_to_their_lease() -> TestResult {
    let store = InMemoryCommandLaunchStore::new();
    assert_eq!(store.claim(&lease("a"), &key()?, "f").await?, Claim::Claimed);
    assert_eq!(store.claim(&lease("b"), &key()?, "f").await?, Claim::Claimed);
    store.forget_lease(&lease("a")).await?;
    assert_eq!(store.claim(&lease("a"), &key()?, "f").await?, Claim::Claimed);
    assert!(matches!(
        store.claim(&lease("b"), &key()?, "f").await?,
        Claim::Existing(_)
    ));
    Ok(())
}

#[tokio::test]
async fn in_memory_keys_expire_after_their_max_age_like_the_kv_bucket() -> TestResult {
    let store = InMemoryCommandLaunchStore::with_max_age(Duration::from_millis(40));
    let l = lease("l1");
    assert_eq!(store.claim(&l, &key()?, "f").await?, Claim::Claimed);
    store.finish(&l, &key()?, &done("f")).await?;
    assert!(matches!(store.claim(&l, &key()?, "f").await?, Claim::Existing(_)));
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(!store.replace(&l, &key()?, &done("f"), &done("g")).await?);
    assert_eq!(store.claim(&l, &key()?, "f").await?, Claim::Claimed);
    Ok(())
}

#[test]
fn fingerprint_ignores_the_key_and_redaction_values() -> TestResult {
    let req: RunCommandRequest = serde_json::from_value(serde_json::json!({
        "lease_id": "l1",
        "command": ["echo", "hi"],
    }))?;
    let mut keyed = req.clone();
    keyed.launch_key = Some(LaunchKey::new("k")?);
    assert_eq!(fingerprint(&req)?, fingerprint(&keyed)?);
    let mut other = req.clone();
    other.command = vec!["echo".into(), "bye".into()];
    assert_ne!(fingerprint(&req)?, fingerprint(&other)?);
    Ok(())
}
