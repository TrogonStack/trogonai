use std::sync::Arc;

use async_nats::{Message, Subscriber};
use futures::StreamExt;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::info;

use super::lease_route::LeaseRoute;
use super::route_context::RouteContext;
use super::serve_error::ServeError;
use super::tenant_route::TenantRoute;

/// Subscribes routes and hands each message to its handler on a task of its own.
///
/// Every subscription stops taking messages once `shutdown` fires; the tasks it started are
/// tracked so the caller can wait for in-flight replies.
pub(crate) struct Router {
    context: Arc<RouteContext>,
    tasks: TaskTracker,
    shutdown: CancellationToken,
}

impl Router {
    pub(crate) fn new(context: Arc<RouteContext>, tasks: TaskTracker, shutdown: CancellationToken) -> Self {
        Self {
            context,
            tasks,
            shutdown,
        }
    }

    pub(crate) async fn tenant<Req, Resp, H, Fut>(
        &self,
        subject: &'static str,
        route: TenantRoute<Req, Resp>,
        handle: H,
    ) -> Result<(), ServeError>
    where
        Req: DeserializeOwned + Send + 'static,
        Resp: Serialize + Send + Sync + 'static,
        H: Fn(Arc<RouteContext>, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Resp> + Send + 'static,
    {
        let route = Arc::new((route, handle));
        self.raw(subject, move |context, message| {
            let route = Arc::clone(&route);
            async move { route.0.serve(context, message, &route.1).await }
        })
        .await
    }

    pub(crate) async fn lease<Req, Resp, H, Fut>(
        &self,
        subject: &'static str,
        route: LeaseRoute<Req, Resp>,
        handle: H,
    ) -> Result<(), ServeError>
    where
        Req: DeserializeOwned + Send + 'static,
        Resp: Serialize + Send + Sync + 'static,
        H: Fn(Arc<RouteContext>, Req) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Resp> + Send + 'static,
    {
        let route = Arc::new((route, handle));
        self.raw(subject, move |context, message| {
            let route = Arc::clone(&route);
            async move { route.0.serve(context, message, &route.1).await }
        })
        .await
    }

    /// Hands every message on `subject` to `handle` as is.
    pub(crate) async fn raw<H, Fut>(&self, subject: &'static str, handle: H) -> Result<(), ServeError>
    where
        H: Fn(Arc<RouteContext>, Message) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let subscriber = self
            .context
            .nats
            .subscribe(subject)
            .await
            .map_err(|source| ServeError::Subscribe { subject, source })?;
        self.drive(subject, None, subscriber, handle);
        Ok(())
    }

    /// Like [`Router::raw`], sharing the messages with the other members of `queue`.
    pub(crate) async fn queue<H, Fut>(
        &self,
        subject: &'static str,
        queue: &'static str,
        handle: H,
    ) -> Result<(), ServeError>
    where
        H: Fn(Arc<RouteContext>, Message) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let subscriber = self
            .context
            .nats
            .queue_subscribe(subject, queue.to_owned())
            .await
            .map_err(|source| ServeError::Subscribe { subject, source })?;
        self.drive(subject, Some(queue), subscriber, handle);
        Ok(())
    }

    fn drive<H, Fut>(&self, subject: &'static str, queue: Option<&'static str>, mut subscriber: Subscriber, handle: H)
    where
        H: Fn(Arc<RouteContext>, Message) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        info!(subject, queue, "subscribed");
        let context = Arc::clone(&self.context);
        let tasks = self.tasks.clone();
        let shutdown = self.shutdown.clone();
        self.tasks.spawn(async move {
            while let Some(message) = shutdown.run_until_cancelled(subscriber.next()).await.flatten() {
                tasks.spawn(handle(Arc::clone(&context), message));
            }
        });
    }
}
