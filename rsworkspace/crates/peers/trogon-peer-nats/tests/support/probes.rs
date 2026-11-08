use crate::support::{broker::TestResult, constants::PROBE_TIMEOUT};
use async_nats::{Client, ServerError};
use tokio::sync::mpsc::Receiver;
pub async fn denied_publish(
    client: &Client,
    errors: &mut Receiver<ServerError>,
    subject: &str,
    payload: Vec<u8>,
) -> TestResult {
    client
        .publish_with_reply(
            subject.to_owned(),
            "_INBOX.tenant.alice.one.probe".to_owned(),
            payload.into(),
        )
        .await?;
    client.flush().await?;
    let error = tokio::time::timeout(PROBE_TIMEOUT, errors.recv())
        .await?
        .ok_or("missing broker denial")?;
    assert_eq!(
        error,
        ServerError::Other(format!("Permissions Violation for Publish to {subject:?}"))
    );
    println!("broker denial: {error}");
    Ok(())
}
pub async fn denied_subscription(client: &Client, errors: &mut Receiver<ServerError>, subject: &str) -> TestResult {
    let _subscription = client.subscribe(subject.to_owned()).await?;
    client.flush().await?;
    let error = tokio::time::timeout(PROBE_TIMEOUT, errors.recv())
        .await?
        .ok_or("missing broker denial")?;
    assert_eq!(
        error,
        ServerError::Other(format!("Permissions Violation for Subscription to {subject:?}"))
    );
    println!("broker denial: {error}");
    Ok(())
}
