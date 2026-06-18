//! Source plugin trait.
//!
//! Each gateway-managed webhook source implements `SourcePlugin` so the
//! gateway can iterate sources without duplicating the per-source plumbing
//! in `http.rs` and `streams.rs`. The same plugin owns both provisioning
//! (JetStream streams) and HTTP route mounting for its integrations.
//!
//! Discord is intentionally NOT a `SourcePlugin`: its primary path is a
//! WebSocket gateway runner spawned in `main.rs`, not a webhook receiver.
//! Slack's socket-mode runners are spawned the same way — `SlackPlugin`
//! only mounts integrations that expose a webhook config.

use axum::Router;
use tracing::info;
use trogon_nats::jetstream::{ClaimCheckPublisher, JetStreamContext, JetStreamPublisher, ObjectStorePut};

use crate::config::{ResolvedConfig, SourceIntegration};

pub type SourceId = &'static str;

pub trait SourcePlugin: Send + Sync {
    fn id(&self) -> SourceId;

    fn path_prefix(&self) -> String {
        format!("/sources/{}", self.id())
    }

    async fn provision<C: JetStreamContext>(&self, client: &C, config: &ResolvedConfig) -> Result<(), C::Error>;

    fn mount<P, S>(&self, app: Router, publisher: ClaimCheckPublisher<P, S>, config: &ResolvedConfig) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut;
}

pub struct GithubPlugin;
pub struct SlackPlugin;
pub struct TelegramPlugin;
pub struct TwitterPlugin;
pub struct GitlabPlugin;
pub struct IncidentioPlugin;
pub struct LinearPlugin;
pub struct MicrosoftGraphPlugin;
pub struct NotionPlugin;
pub struct SentryPlugin;

async fn provision_integrations<C, T, F>(
    integrations: &[SourceIntegration<T>],
    source: SourceId,
    client: &C,
    provision_fn: F,
) -> Result<(), C::Error>
where
    C: JetStreamContext,
    F: AsyncFn(&C, &T) -> Result<(), C::Error>,
{
    for integration in integrations {
        provision_fn(client, &integration.config).await?;
        info!(source, integration = integration.id.as_str(), "stream provisioned");
    }
    Ok(())
}

fn mount_integrations<P, S, T, F>(
    integrations: &[SourceIntegration<T>],
    mut app: Router,
    publisher: ClaimCheckPublisher<P, S>,
    source: SourceId,
    path_prefix: &str,
    router_fn: F,
) -> Router
where
    P: JetStreamPublisher,
    S: ObjectStorePut,
    F: Fn(ClaimCheckPublisher<P, S>, &T) -> Router,
{
    for integration in integrations {
        let path = format!("{}/{}", path_prefix, integration.id);
        app = app.nest(&path, router_fn(publisher.clone(), &integration.config));
        info!(
            source,
            integration = integration.id.as_str(),
            path,
            "mounted source integration"
        );
    }
    app
}

impl SourcePlugin for GithubPlugin {
    fn id(&self) -> SourceId {
        "github"
    }

    async fn provision<C: JetStreamContext>(&self, client: &C, config: &ResolvedConfig) -> Result<(), C::Error> {
        provision_integrations(&config.github, self.id(), client, crate::source::github::provision).await
    }

    fn mount<P, S>(&self, app: Router, publisher: ClaimCheckPublisher<P, S>, config: &ResolvedConfig) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        mount_integrations(
            &config.github,
            app,
            publisher,
            self.id(),
            &self.path_prefix(),
            |p, cfg| crate::source::github::router(p, cfg),
        )
    }
}

impl SourcePlugin for SlackPlugin {
    fn id(&self) -> SourceId {
        "slack"
    }

    async fn provision<C: JetStreamContext>(&self, client: &C, config: &ResolvedConfig) -> Result<(), C::Error> {
        provision_integrations(&config.slack, self.id(), client, crate::source::slack::provision).await
    }

    fn mount<P, S>(&self, mut app: Router, publisher: ClaimCheckPublisher<P, S>, config: &ResolvedConfig) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        for integration in &config.slack {
            let Some(webhook) = integration.config.webhook() else {
                continue;
            };
            let path = format!("{}/{}", self.path_prefix(), integration.id);
            app = app.nest(
                &path,
                crate::source::slack::router(publisher.clone(), &integration.config, webhook),
            );
            info!(
                source = self.id(),
                integration = integration.id.as_str(),
                path,
                "mounted source integration"
            );
        }
        app
    }
}

impl SourcePlugin for TelegramPlugin {
    fn id(&self) -> SourceId {
        "telegram"
    }

    async fn provision<C: JetStreamContext>(&self, client: &C, config: &ResolvedConfig) -> Result<(), C::Error> {
        provision_integrations(&config.telegram, self.id(), client, crate::source::telegram::provision).await
    }

    fn mount<P, S>(&self, app: Router, publisher: ClaimCheckPublisher<P, S>, config: &ResolvedConfig) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        mount_integrations(
            &config.telegram,
            app,
            publisher,
            self.id(),
            &self.path_prefix(),
            |p, cfg| crate::source::telegram::router(p, cfg),
        )
    }
}

impl SourcePlugin for TwitterPlugin {
    fn id(&self) -> SourceId {
        "twitter"
    }

    async fn provision<C: JetStreamContext>(&self, client: &C, config: &ResolvedConfig) -> Result<(), C::Error> {
        provision_integrations(&config.twitter, self.id(), client, crate::source::twitter::provision).await
    }

    fn mount<P, S>(&self, app: Router, publisher: ClaimCheckPublisher<P, S>, config: &ResolvedConfig) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        mount_integrations(
            &config.twitter,
            app,
            publisher,
            self.id(),
            &self.path_prefix(),
            |p, cfg| crate::source::twitter::router(p, cfg),
        )
    }
}

impl SourcePlugin for GitlabPlugin {
    fn id(&self) -> SourceId {
        "gitlab"
    }

    async fn provision<C: JetStreamContext>(&self, client: &C, config: &ResolvedConfig) -> Result<(), C::Error> {
        provision_integrations(&config.gitlab, self.id(), client, crate::source::gitlab::provision).await
    }

    fn mount<P, S>(&self, app: Router, publisher: ClaimCheckPublisher<P, S>, config: &ResolvedConfig) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        mount_integrations(
            &config.gitlab,
            app,
            publisher,
            self.id(),
            &self.path_prefix(),
            |p, cfg| crate::source::gitlab::router(p, cfg),
        )
    }
}

impl SourcePlugin for IncidentioPlugin {
    fn id(&self) -> SourceId {
        "incidentio"
    }

    async fn provision<C: JetStreamContext>(&self, client: &C, config: &ResolvedConfig) -> Result<(), C::Error> {
        provision_integrations(
            &config.incidentio,
            self.id(),
            client,
            crate::source::incidentio::provision,
        )
        .await
    }

    fn mount<P, S>(&self, app: Router, publisher: ClaimCheckPublisher<P, S>, config: &ResolvedConfig) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        mount_integrations(
            &config.incidentio,
            app,
            publisher,
            self.id(),
            &self.path_prefix(),
            |p, cfg| crate::source::incidentio::router(p, cfg),
        )
    }
}

impl SourcePlugin for LinearPlugin {
    fn id(&self) -> SourceId {
        "linear"
    }

    async fn provision<C: JetStreamContext>(&self, client: &C, config: &ResolvedConfig) -> Result<(), C::Error> {
        provision_integrations(&config.linear, self.id(), client, crate::source::linear::provision).await
    }

    fn mount<P, S>(&self, app: Router, publisher: ClaimCheckPublisher<P, S>, config: &ResolvedConfig) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        mount_integrations(
            &config.linear,
            app,
            publisher,
            self.id(),
            &self.path_prefix(),
            |p, cfg| crate::source::linear::router(p, cfg),
        )
    }
}

impl SourcePlugin for MicrosoftGraphPlugin {
    fn id(&self) -> SourceId {
        "microsoft-graph"
    }

    async fn provision<C: JetStreamContext>(&self, client: &C, config: &ResolvedConfig) -> Result<(), C::Error> {
        provision_integrations(
            &config.microsoft_graph,
            self.id(),
            client,
            crate::source::microsoft_graph::provision,
        )
        .await
    }

    fn mount<P, S>(&self, app: Router, publisher: ClaimCheckPublisher<P, S>, config: &ResolvedConfig) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        mount_integrations(
            &config.microsoft_graph,
            app,
            publisher,
            self.id(),
            &self.path_prefix(),
            |p, cfg| crate::source::microsoft_graph::router(p, cfg),
        )
    }
}

impl SourcePlugin for NotionPlugin {
    fn id(&self) -> SourceId {
        "notion"
    }

    async fn provision<C: JetStreamContext>(&self, client: &C, config: &ResolvedConfig) -> Result<(), C::Error> {
        provision_integrations(&config.notion, self.id(), client, crate::source::notion::provision).await
    }

    fn mount<P, S>(&self, app: Router, publisher: ClaimCheckPublisher<P, S>, config: &ResolvedConfig) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        mount_integrations(
            &config.notion,
            app,
            publisher,
            self.id(),
            &self.path_prefix(),
            |p, cfg| crate::source::notion::router(p, cfg),
        )
    }
}

impl SourcePlugin for SentryPlugin {
    fn id(&self) -> SourceId {
        "sentry"
    }

    async fn provision<C: JetStreamContext>(&self, client: &C, config: &ResolvedConfig) -> Result<(), C::Error> {
        provision_integrations(&config.sentry, self.id(), client, crate::source::sentry::provision).await
    }

    fn mount<P, S>(&self, app: Router, publisher: ClaimCheckPublisher<P, S>, config: &ResolvedConfig) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        mount_integrations(
            &config.sentry,
            app,
            publisher,
            self.id(),
            &self.path_prefix(),
            |p, cfg| crate::source::sentry::router(p, cfg),
        )
    }
}

/// Provision JetStream streams for every webhook source. Discord is provisioned separately by the caller.
pub async fn provision_webhook_sources<C: JetStreamContext>(
    client: &C,
    config: &ResolvedConfig,
) -> Result<(), C::Error> {
    GithubPlugin.provision(client, config).await?;
    SlackPlugin.provision(client, config).await?;
    TelegramPlugin.provision(client, config).await?;
    TwitterPlugin.provision(client, config).await?;
    GitlabPlugin.provision(client, config).await?;
    IncidentioPlugin.provision(client, config).await?;
    LinearPlugin.provision(client, config).await?;
    MicrosoftGraphPlugin.provision(client, config).await?;
    NotionPlugin.provision(client, config).await?;
    SentryPlugin.provision(client, config).await?;
    Ok(())
}

/// Mount HTTP routes for every webhook source.
pub fn mount_webhook_sources<P, S>(
    mut app: Router,
    publisher: ClaimCheckPublisher<P, S>,
    config: &ResolvedConfig,
) -> Router
where
    P: JetStreamPublisher,
    S: ObjectStorePut,
{
    app = GithubPlugin.mount(app, publisher.clone(), config);
    app = SlackPlugin.mount(app, publisher.clone(), config);
    app = TelegramPlugin.mount(app, publisher.clone(), config);
    app = TwitterPlugin.mount(app, publisher.clone(), config);
    app = GitlabPlugin.mount(app, publisher.clone(), config);
    app = IncidentioPlugin.mount(app, publisher.clone(), config);
    app = LinearPlugin.mount(app, publisher.clone(), config);
    app = MicrosoftGraphPlugin.mount(app, publisher.clone(), config);
    app = NotionPlugin.mount(app, publisher.clone(), config);
    SentryPlugin.mount(app, publisher, config)
}
