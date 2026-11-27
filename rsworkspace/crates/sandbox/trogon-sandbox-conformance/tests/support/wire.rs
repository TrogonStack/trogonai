//! Helpers for tests that answer the provider subjects by hand.

use std::error::Error;
use std::time::Duration;

use async_nats::{Client, Message, Subscriber};
use bytes::Bytes;
use futures::StreamExt;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::task::JoinHandle;

use super::Fallible;

/// What a task spawned to answer requests returns.
pub type Sendable<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// How long a hand-written responder waits for the next request.
pub const WAIT: Duration = Duration::from_secs(3);

/// The next request on `subscription`, within [`WAIT`].
pub async fn next(subscription: &mut Subscriber) -> Sendable<Message> {
    tokio::time::timeout(WAIT, subscription.next())
        .await?
        .ok_or_else(|| "the subscription ended".into())
}

/// The JSON body of `message`.
pub fn body<T: DeserializeOwned>(message: &Message) -> Sendable<T> {
    Ok(serde_json::from_slice(&message.payload)?)
}

/// Answers `message` with `body` as JSON.
pub async fn reply(client: &Client, message: &Message, body: &impl Serialize) -> Sendable<()> {
    reply_raw(client, message, serde_json::to_vec(body)?).await
}

/// Answers `message` with `payload` as is.
pub async fn reply_raw(client: &Client, message: &Message, payload: impl Into<Bytes>) -> Sendable<()> {
    let reply = message.reply.clone().ok_or("the request has no reply subject")?;
    client.publish(reply, payload.into()).await?;
    client.flush().await?;
    Ok(())
}

/// The result of a responder task.
pub async fn join<T>(task: JoinHandle<Sendable<T>>) -> Fallible<T> {
    local(task.await?)
}

/// `result` with its error no longer required to cross threads.
pub fn local<T>(result: Sendable<T>) -> Fallible<T> {
    result.map_err(|error| -> Box<dyn Error> { error })
}
