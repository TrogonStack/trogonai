use super::super::tests_support::{FakeControl, TestResult};
use super::*;
use crate::ate_client::AteError;

fn actor() -> Result<proto::ObjectRef, Box<dyn Error>> {
    Ok(object_ref(atespace()?.as_str(), "sbx-01"))
}

#[tokio::test]
async fn resume_accepts_an_actor_that_was_already_running() -> TestResult {
    let control = Arc::new(FakeControl::running());
    let provider = provider(control.clone())?;
    provider.resume(&actor()?).await?;
    assert_eq!(control.resume_count(), 1);
    assert_eq!(control.get_count(), 0);
    Ok(())
}

#[tokio::test]
async fn resume_waits_through_the_resuming_status() -> TestResult {
    let control = Arc::new(FakeControl::with_statuses(vec![
        proto::ActorState::Resuming,
        proto::ActorState::Resuming,
        proto::ActorState::Running,
    ]));
    let provider = provider(control.clone())?;
    provider.resume(&actor()?).await?;
    assert!(control.get_count() >= 2);
    Ok(())
}

#[tokio::test]
async fn resume_gives_up_rather_than_hanging_on_a_stuck_actor() -> TestResult {
    let control = Arc::new(FakeControl::with_statuses(vec![proto::ActorState::Resuming]));
    let provider = provider(control)?;
    let error = provider.resume(&actor()?).await.err().ok_or("resumed")?;
    match error {
        SubstrateError::ResumeTimeout { actor, status, .. } => {
            assert_eq!(actor, "sbx-01");
            assert_eq!(status, "Resuming");
            Ok(())
        }
        other => Err(format!("expected ResumeTimeout, got {other:?}").into()),
    }
}

#[tokio::test]
async fn a_crash_during_resume_fails_immediately() -> TestResult {
    let control = Arc::new(FakeControl::with_statuses(vec![proto::ActorState::Crashed]));
    let provider = provider(control.clone())?;
    let error = provider.resume(&actor()?).await.err().ok_or("resumed")?;
    assert!(matches!(error, SubstrateError::Ate(AteError::Other { ref message }) if message.contains("crashed")));
    assert_eq!(control.get_count(), 1);
    Ok(())
}

#[tokio::test]
async fn a_failed_poll_ends_the_resume() -> TestResult {
    let control = Arc::new(FakeControl::with_statuses(vec![proto::ActorState::Resuming]));
    control.fail_get_with(AteError::Unavailable { message: "down".into() });
    let provider = provider(control)?;
    let error = provider.resume(&actor()?).await.err().ok_or("resumed")?;
    assert!(matches!(error, SubstrateError::Ate(AteError::Unavailable { .. })));
    Ok(())
}
