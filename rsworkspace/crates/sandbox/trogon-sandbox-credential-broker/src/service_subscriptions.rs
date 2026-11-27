use std::future::Future;
use std::sync::Arc;

use async_nats::{Message, Subscriber};
use bytes::Bytes;
use futures::StreamExt;
use futures::stream::select;
use tokio::task::JoinSet;
use tracing::warn;

use crate::{BrokerService, ServiceError};

#[derive(Clone, Copy)]
enum Verb {
    Issue,
    Revoke,
    Rotate,
}

/// The broker service's live subscriptions, ready to be served.
pub struct ServiceSubscriptions {
    service: Arc<BrokerService>,
    client: async_nats::Client,
    issue: Subscriber,
    revoke: Subscriber,
    rotate: Subscriber,
}

impl ServiceSubscriptions {
    pub(crate) fn new(
        service: Arc<BrokerService>,
        client: async_nats::Client,
        issue: Subscriber,
        revoke: Subscriber,
        rotate: Subscriber,
    ) -> Self {
        Self {
            service,
            client,
            issue,
            revoke,
            rotate,
        }
    }

    /// Answers requests until `shutdown` completes, then waits for the requests already being handled.
    pub async fn run(self, shutdown: impl Future<Output = ()>) {
        let Self {
            service,
            client,
            issue,
            revoke,
            rotate,
        } = self;
        let mut requests = select(
            select(issue.map(|m| (Verb::Issue, m)), revoke.map(|m| (Verb::Revoke, m))),
            rotate.map(|m| (Verb::Rotate, m)),
        );
        let mut handlers = JoinSet::new();
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                () = &mut shutdown => break,
                next = requests.next() => {
                    let Some((verb, message)) = next else { break };
                    handlers.spawn(Self::handle(Arc::clone(&service), client.clone(), verb, message));
                }
            }
        }
        while handlers.join_next().await.is_some() {}
    }

    async fn handle(service: Arc<BrokerService>, client: async_nats::Client, verb: Verb, message: Message) {
        if let Err(error) = Self::reply(&service, &client, verb, message).await {
            warn!(%error, "credential request failed");
        }
    }

    async fn reply(
        service: &BrokerService,
        client: &async_nats::Client,
        verb: Verb,
        message: Message,
    ) -> Result<(), ServiceError> {
        let Some(reply) = message.reply else {
            return Ok(());
        };
        let subject = message.subject.as_str();
        let payload = match verb {
            Verb::Issue => service.issue(subject, &message.payload).await?,
            Verb::Revoke => service.revoke(&message.payload).await?,
            Verb::Rotate => service.rotate(subject, &message.payload).await?,
        };
        client.publish(reply, Bytes::from(payload)).await?;
        Ok(())
    }
}
