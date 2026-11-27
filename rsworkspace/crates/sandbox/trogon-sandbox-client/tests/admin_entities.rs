mod admin_support;
mod common;

use admin_support::{tenant, unauthorized};
use common::{TestResult, client_replying};
use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::{
    DeleteEntityResponse, EntityRef, GetEntityResponse, LeaseError, ListEntitiesRequest, ListEntitiesResponse,
};
use trogon_sandbox_protocol::subjects;

#[tokio::test]
async fn get_entity_sends_its_subject_and_returns_the_answer() -> TestResult {
    let (client, log) = client_replying(&GetEntityResponse::NotFound)?;

    let got = client.get_entity(tenant()?, EntityRef::Name("web".into())).await?;

    assert_eq!(got, GetEntityResponse::NotFound);
    let sent = log.only()?;
    assert_eq!(sent.subject, subjects::ENTITY_GET_REQUEST);
    assert_eq!(sent.json()?["entity"]["name"], "web");
    Ok(())
}

#[tokio::test]
async fn list_entities_sends_the_request_as_given() -> TestResult {
    let (client, log) = client_replying(&ListEntitiesResponse::Listed {
        rows: vec![],
        next_cursor: None,
    })?;
    let request = ListEntitiesRequest {
        tenant: tenant()?,
        limit: Some(5),
        cursor: None,
    };

    client.list_entities(request.clone()).await?;

    let sent = log.only()?;
    assert_eq!(sent.subject, subjects::ENTITY_LIST_REQUEST);
    assert_eq!(serde_json::from_slice::<ListEntitiesRequest>(&sent.payload)?, request);
    Ok(())
}

#[tokio::test]
async fn delete_entity_sends_its_subject_and_returns_the_answer() -> TestResult {
    let (client, log) = client_replying(&DeleteEntityResponse::Deleted)?;

    let deleted = client.delete_entity(tenant()?, EntityRef::Name("web".into())).await?;

    assert_eq!(deleted, DeleteEntityResponse::Deleted);
    assert_eq!(log.only()?.subject, subjects::ENTITY_DELETE_REQUEST);
    Ok(())
}

#[tokio::test]
async fn entity_verbs_surface_denials() -> TestResult {
    let (client, _) = client_replying(&GetEntityResponse::Denied(LeaseError::EntityNotEnabled))?;
    let got = client.get_entity(tenant()?, EntityRef::Name("web".into())).await;
    assert!(matches!(got, Err(ClientError::Denied(LeaseError::EntityNotEnabled))));

    let (client, _) = client_replying(&ListEntitiesResponse::Denied(unauthorized()))?;
    let listed = client
        .list_entities(ListEntitiesRequest {
            tenant: tenant()?,
            limit: None,
            cursor: None,
        })
        .await;
    assert!(matches!(listed, Err(ClientError::Denied(_))), "{listed:?}");

    let (client, _) = client_replying(&DeleteEntityResponse::Denied(unauthorized()))?;
    let deleted = client.delete_entity(tenant()?, EntityRef::Name("web".into())).await;
    assert!(matches!(deleted, Err(ClientError::Denied(_))), "{deleted:?}");
    Ok(())
}
