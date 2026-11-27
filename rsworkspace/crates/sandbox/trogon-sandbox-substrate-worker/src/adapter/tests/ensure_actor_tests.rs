use super::super::tests_support::{FakeControl, TestResult, provider_with};
use super::*;
use crate::ate_client::AteError;

fn actor_ref(name: &str) -> Result<proto::ObjectRef, Box<dyn Error>> {
    Ok(object_ref(atespace()?.as_str(), name))
}

fn template_ref(name: &str) -> Result<proto::ObjectRef, Box<dyn Error>> {
    Ok(object_ref(atespace()?.as_str(), name))
}

fn ownership(error: SubstrateError, needle: &str) -> TestResult {
    match error {
        SubstrateError::Ownership(message) => {
            assert!(message.contains(needle), "{message:?} does not mention {needle:?}");
            Ok(())
        }
        other => Err(format!("expected Ownership, got {other:?}").into()),
    }
}

#[tokio::test]
async fn a_new_actor_is_created_with_its_uid_and_did_not_pre_exist() -> TestResult {
    let provider = provider(Arc::new(FakeControl::running()))?;
    let (actor, existed) = provider
        .ensure_actor(
            &actor_ref("sbx-01")?,
            &template_ref("v-abc")?,
            proto::Selector::default(),
            None,
        )
        .await?;
    assert!(!existed);
    assert_eq!(actor_uid(&actor)?.as_str(), TEST_UID);
    Ok(())
}

#[tokio::test]
async fn an_actor_that_already_exists_without_a_receipt_is_refused() -> TestResult {
    let control = Arc::new(FakeControl::running());
    control.fail_create_with(AteError::AlreadyExists {
        message: "exists".into(),
    });
    let provider = provider(control)?;
    let error = provider
        .ensure_actor(
            &actor_ref("sbx-01")?,
            &template_ref("v-abc")?,
            proto::Selector::default(),
            None,
        )
        .await
        .err()
        .ok_or("accepted")?;
    ownership(error, "no confirmed allocation receipt")
}

#[tokio::test]
async fn any_other_create_failure_is_reported_as_a_control_plane_error() -> TestResult {
    let control = Arc::new(FakeControl::running());
    control.fail_create_with(AteError::Unavailable { message: "down".into() });
    let provider = provider(control)?;
    let error = provider
        .ensure_actor(
            &actor_ref("sbx-01")?,
            &template_ref("v-abc")?,
            proto::Selector::default(),
            None,
        )
        .await
        .err()
        .ok_or("accepted")?;
    assert!(matches!(error, SubstrateError::Ate(AteError::Unavailable { .. })));
    Ok(())
}

#[tokio::test]
async fn an_actor_with_the_expected_uid_and_template_is_adopted() -> TestResult {
    let control = Arc::new(FakeControl::running());
    control.set_existing_template("v-abc");
    let provider = provider(control)?;
    let uid = test_uid()?;
    let (_, existed) = provider
        .ensure_actor(
            &actor_ref("sbxn-01")?,
            &template_ref("v-abc")?,
            proto::Selector::default(),
            Some(&uid),
        )
        .await?;
    assert!(existed);
    Ok(())
}

#[tokio::test]
async fn an_actor_whose_uid_changed_is_not_adopted() -> TestResult {
    let control = Arc::new(FakeControl::running());
    control.set_existing_template("v-abc");
    let provider = provider(control.clone())?;
    control.set_uid("00000000-0000-4000-8000-000000000002");
    let uid = test_uid()?;
    let error = provider
        .ensure_actor(
            &actor_ref("sbxn-01")?,
            &template_ref("v-abc")?,
            proto::Selector::default(),
            Some(&uid),
        )
        .await
        .err()
        .ok_or("accepted")?;
    ownership(error, "UID changed")
}

#[tokio::test]
async fn a_named_actor_running_a_different_image_is_refused_rather_than_reused() -> TestResult {
    let control = Arc::new(FakeControl::running());
    control.set_existing_template("v-old");
    let provider = provider(control)?;
    let uid = test_uid()?;
    let error = provider
        .ensure_actor(
            &actor_ref("sbxn-01")?,
            &template_ref("v-new")?,
            proto::Selector::default(),
            Some(&uid),
        )
        .await
        .err()
        .ok_or("accepted")?;
    match error {
        SubstrateError::TemplateMismatch { actor, wanted, found } => {
            assert_eq!(
                (actor.as_str(), wanted.as_str(), found.as_str()),
                ("sbxn-01", "v-new", "v-old")
            );
            Ok(())
        }
        other => Err(format!("expected TemplateMismatch, got {other:?}").into()),
    }
}

#[tokio::test]
async fn a_template_of_the_same_name_in_another_atespace_is_a_mismatch() -> TestResult {
    let control = Arc::new(FakeControl::running());
    control.set_existing_template("v-abc");
    control.set_existing_template_space("tenant-b");
    let provider = provider(control)?;
    let uid = test_uid()?;
    let error = provider
        .ensure_actor(
            &actor_ref("sbxn-01")?,
            &template_ref("v-abc")?,
            proto::Selector::default(),
            Some(&uid),
        )
        .await
        .err()
        .ok_or("accepted")?;
    assert!(matches!(error, SubstrateError::TemplateMismatch { .. }));
    Ok(())
}

#[tokio::test]
async fn an_actor_without_a_template_is_a_mismatch() -> TestResult {
    let provider = provider(Arc::new(FakeControl::running()))?;
    let uid = test_uid()?;
    let error = provider
        .ensure_actor(
            &actor_ref("sbxn-01")?,
            &template_ref("v-abc")?,
            proto::Selector::default(),
            Some(&uid),
        )
        .await
        .err()
        .ok_or("accepted")?;
    assert!(matches!(error, SubstrateError::TemplateMismatch { ref found, .. } if found.is_empty()));
    Ok(())
}

#[tokio::test]
async fn an_allocation_reply_naming_another_actor_is_refused() -> TestResult {
    let control = Arc::new(FakeControl::running());
    control.rename_created_actor("someone-else");
    let provider = provider(control)?;
    let error = provider
        .ensure_actor(
            &actor_ref("sbx-01")?,
            &template_ref("v-abc")?,
            proto::Selector::default(),
            None,
        )
        .await
        .err()
        .ok_or("accepted")?;
    ownership(error, "names a different actor")
}

#[tokio::test]
async fn an_allocation_reply_without_a_uid_is_refused() -> TestResult {
    let provider = provider_with(Arc::new(FakeControl::running()), Arc::new(MemoryActorIndex::default()))?;
    let error = provider
        .ensure_actor(
            &actor_ref("sbx-01")?,
            &template_ref("v-abc")?,
            proto::Selector::default(),
            None,
        )
        .await
        .err()
        .ok_or("accepted")?;
    ownership(error, "UID is unconfirmed")
}
