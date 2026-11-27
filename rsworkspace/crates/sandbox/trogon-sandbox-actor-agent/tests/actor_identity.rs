#![cfg(feature = "server")]

use std::error::Error;

use trogon_sandbox_actor_agent::{ACTOR_ID_PATH, ActorIdentityError, ActorIdentitySource};

#[tokio::test]
async fn the_identity_file_is_read_fresh_each_time() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("actor-id");
    let source = ActorIdentitySource::new(&path);
    assert_eq!(source.path(), path.as_path());
    std::fs::write(&path, "first\n")?;
    assert_eq!(source.read().await?.as_str(), "first");
    std::fs::write(&path, "second\n")?;
    assert_eq!(source.read().await?.as_str(), "second");
    Ok(())
}

#[tokio::test]
async fn a_missing_or_blank_identity_file_is_a_typed_error() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("actor-id");
    let source = ActorIdentitySource::new(&path);
    assert!(matches!(
        source.read().await,
        Err(ActorIdentityError::Unreadable { .. })
    ));
    std::fs::write(&path, "  \n")?;
    let error = source.read().await.err().ok_or("a blank identity must not parse")?;
    assert!(matches!(error, ActorIdentityError::Invalid { .. }));
    Ok(())
}

#[test]
fn the_default_source_reads_the_published_path() {
    assert_eq!(
        ActorIdentitySource::default().path(),
        std::path::Path::new(ACTOR_ID_PATH)
    );
}
