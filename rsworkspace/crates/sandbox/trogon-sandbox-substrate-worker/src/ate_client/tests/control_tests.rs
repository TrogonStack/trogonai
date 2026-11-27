use std::error::Error;

use tonic::Code;

use super::fake_ate::{FakeAte, Served, actor};
use crate::ate_client::ResourceRevision;
use crate::ate_client::{AteError, Control};
use crate::proto;

type TestResult = Result<(), Box<dyn Error>>;

fn reference() -> proto::ObjectRef {
    proto::ObjectRef {
        atespace: "tenant".into(),
        name: "actor".into(),
    }
}

fn revision() -> Result<ResourceRevision, AteError> {
    ResourceRevision::from_metadata(actor().metadata.as_ref())
}

#[tokio::test]
async fn atespace_and_actor_reads_and_creates_reach_the_official_route() -> TestResult {
    let served = Served::plaintext(FakeAte::default()).await?;
    let client = served.client().await?;

    let atespace = client.create_atespace("tenant").await?;
    assert_eq!(atespace.metadata.ok_or("metadata")?.name, "tenant");
    assert_eq!(
        client.get_actor(reference()).await?.metadata.ok_or("metadata")?.version,
        17
    );
    assert_eq!(
        client.create_actor(actor()).await?.metadata.ok_or("metadata")?.name,
        "actor"
    );

    let methods: Vec<_> = served.fake.calls().into_iter().map(|call| call.method).collect();
    assert_eq!(methods, ["CreateAtespace", "GetActor", "CreateActor"]);
    assert!(served.fake.calls()[0].request.contains("tenant"));
    Ok(())
}

#[tokio::test]
async fn resume_returns_the_actor_even_when_it_was_already_running() -> TestResult {
    let served = Served::plaintext(FakeAte::default()).await?;
    let resumed = served.client().await?.resume_actor(reference()).await?;
    assert_eq!(resumed.metadata.ok_or("metadata")?.name, "actor");
    Ok(())
}

#[tokio::test]
async fn resume_without_an_actor_is_an_error() -> TestResult {
    let served = Served::plaintext(FakeAte::without_actor()).await?;
    let err = served.client().await?.resume_actor(reference()).await.err();
    assert_eq!(
        err,
        Some(AteError::Other {
            message: "ResumeActor returned no actor".into()
        })
    );
    Ok(())
}

#[tokio::test]
async fn suspend_asks_for_a_synchronous_snapshot() -> TestResult {
    let served = Served::plaintext(FakeAte::default()).await?;
    let suspended = served.client().await?.suspend_actor(reference()).await?;
    assert_eq!(suspended.metadata.ok_or("metadata")?.name, "actor");
    assert!(served.fake.calls()[0].request.contains("Synchronous"));
    Ok(())
}

#[tokio::test]
async fn suspend_without_an_actor_is_an_error() -> TestResult {
    let served = Served::plaintext(FakeAte::without_actor()).await?;
    let err = served.client().await?.suspend_actor(reference()).await.err();
    assert_eq!(
        err,
        Some(AteError::Other {
            message: "SuspendActor returned no actor".into()
        })
    );
    Ok(())
}

#[tokio::test]
async fn deletes_carry_the_observed_uid_and_version() -> TestResult {
    let served = Served::plaintext(FakeAte::default()).await?;
    let client = served.client().await?;
    client.delete_actor(revision()?).await?;
    client.delete_actor_template(revision()?).await?;

    let calls = served.fake.calls();
    assert_eq!(calls[0].method, "DeleteActor");
    assert_eq!(calls[1].method, "DeleteActorTemplate");
    for call in calls {
        assert!(call.request.contains("version: 17"), "{}", call.request);
        assert!(
            call.request.contains("3f234d36-857d-474f-8009-64fdfe4aa84f"),
            "{}",
            call.request
        );
    }
    Ok(())
}

#[tokio::test]
async fn listing_follows_every_page() -> TestResult {
    let served = Served::plaintext(FakeAte::default()).await?;
    let actors = served.client().await?.list_actors("tenant").await?;
    let names: Vec<_> = actors
        .into_iter()
        .map(|actor| actor.metadata.map(|metadata| metadata.name))
        .collect();
    assert_eq!(names, [Some("first".to_string()), Some("second".to_string())]);

    let calls = served.fake.calls();
    assert_eq!(calls.len(), 2);
    assert!(calls[1].request.contains("page-2"));
    Ok(())
}

#[tokio::test]
async fn templates_are_created_read_and_deleted() -> TestResult {
    let served = Served::plaintext(FakeAte::default()).await?;
    let client = served.client().await?;
    client.create_actor_template(proto::ActorTemplate::default()).await?;
    client.get_actor_template(reference()).await?;

    let methods: Vec<_> = served.fake.calls().into_iter().map(|call| call.method).collect();
    assert_eq!(methods, ["CreateActorTemplate", "GetActorTemplate"]);
    Ok(())
}

#[tokio::test]
async fn a_server_status_is_classified_for_the_caller() -> TestResult {
    let served = Served::plaintext(FakeAte::failing(Code::NotFound)).await?;
    let err = served.client().await?.get_actor(reference()).await.err();
    assert_eq!(
        err,
        Some(AteError::NotFound {
            message: "scripted failure".into()
        })
    );
    Ok(())
}

#[tokio::test]
async fn an_rpc_outside_the_worker_surface_is_unimplemented_on_the_fake() -> TestResult {
    let served = Served::plaintext(FakeAte::default()).await?;
    let channel = tonic::transport::Channel::from_shared(served.http_endpoint())?
        .connect()
        .await?;
    let mut raw = proto::control_client::ControlClient::new(channel);
    let status = raw
        .list_workers(proto::ListWorkersRequest::default())
        .await
        .err()
        .ok_or("expected an error")?;
    assert_eq!(status.code(), Code::Unimplemented);
    Ok(())
}
