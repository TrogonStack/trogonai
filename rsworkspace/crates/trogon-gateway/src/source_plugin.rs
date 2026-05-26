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

use crate::config::ResolvedConfig;

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

impl SourcePlugin for GithubPlugin {
    fn id(&self) -> SourceId {
        "github"
    }

    async fn provision<C: JetStreamContext>(&self, client: &C, config: &ResolvedConfig) -> Result<(), C::Error> {
        for integration in &config.github {
            crate::source::github::provision(client, &integration.config).await?;
            info!(
                source = self.id(),
                integration = integration.id.as_str(),
                "stream provisioned"
            );
        }
        Ok(())
    }

    fn mount<P, S>(&self, mut app: Router, publisher: ClaimCheckPublisher<P, S>, config: &ResolvedConfig) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        for integration in &config.github {
            let path = format!("{}/{}", self.path_prefix(), integration.id);
            app = app.nest(
                &path,
                crate::source::github::router(publisher.clone(), &integration.config),
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

impl SourcePlugin for SlackPlugin {
    fn id(&self) -> SourceId {
        "slack"
    }

    async fn provision<C: JetStreamContext>(&self, client: &C, config: &ResolvedConfig) -> Result<(), C::Error> {
        for integration in &config.slack {
            crate::source::slack::provision(client, &integration.config).await?;
            info!(
                source = self.id(),
                integration = integration.id.as_str(),
                "stream provisioned"
            );
        }
        Ok(())
    }

    fn mount<P, S>(&self, mut app: Router, publisher: ClaimCheckPublisher<P, S>, config: &ResolvedConfig) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        // Socket-mode-only integrations are spawned as long-running runners
        // in `main.rs`; they have no HTTP route to mount.
        for integration in &config.slack {
            if integration.config.webhook().is_none() {
                continue;
            }
            let path = format!("{}/{}", self.path_prefix(), integration.id);
            app = app.nest(
                &path,
                crate::source::slack::router(publisher.clone(), &integration.config),
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
        for integration in &config.telegram {
            crate::source::telegram::provision(client, &integration.config).await?;
            info!(
                source = self.id(),
                integration = integration.id.as_str(),
                "stream provisioned"
            );
        }
        Ok(())
    }

    fn mount<P, S>(&self, mut app: Router, publisher: ClaimCheckPublisher<P, S>, config: &ResolvedConfig) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        for integration in &config.telegram {
            let path = format!("{}/{}", self.path_prefix(), integration.id);
            app = app.nest(
                &path,
                crate::source::telegram::router(publisher.clone(), &integration.config),
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

impl SourcePlugin for TwitterPlugin {
    fn id(&self) -> SourceId {
        "twitter"
    }

    async fn provision<C: JetStreamContext>(&self, client: &C, config: &ResolvedConfig) -> Result<(), C::Error> {
        for integration in &config.twitter {
            crate::source::twitter::provision(client, &integration.config).await?;
            info!(
                source = self.id(),
                integration = integration.id.as_str(),
                "stream provisioned"
            );
        }
        Ok(())
    }

    fn mount<P, S>(&self, mut app: Router, publisher: ClaimCheckPublisher<P, S>, config: &ResolvedConfig) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        for integration in &config.twitter {
            let path = format!("{}/{}", self.path_prefix(), integration.id);
            app = app.nest(
                &path,
                crate::source::twitter::router(publisher.clone(), &integration.config),
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

impl SourcePlugin for GitlabPlugin {
    fn id(&self) -> SourceId {
        "gitlab"
    }

    async fn provision<C: JetStreamContext>(&self, client: &C, config: &ResolvedConfig) -> Result<(), C::Error> {
        for integration in &config.gitlab {
            crate::source::gitlab::provision(client, &integration.config).await?;
            info!(
                source = self.id(),
                integration = integration.id.as_str(),
                "stream provisioned"
            );
        }
        Ok(())
    }

    fn mount<P, S>(&self, mut app: Router, publisher: ClaimCheckPublisher<P, S>, config: &ResolvedConfig) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        for integration in &config.gitlab {
            let path = format!("{}/{}", self.path_prefix(), integration.id);
            app = app.nest(
                &path,
                crate::source::gitlab::router(publisher.clone(), &integration.config),
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

impl SourcePlugin for IncidentioPlugin {
    fn id(&self) -> SourceId {
        "incidentio"
    }

    async fn provision<C: JetStreamContext>(&self, client: &C, config: &ResolvedConfig) -> Result<(), C::Error> {
        for integration in &config.incidentio {
            crate::source::incidentio::provision(client, &integration.config).await?;
            info!(
                source = self.id(),
                integration = integration.id.as_str(),
                "stream provisioned"
            );
        }
        Ok(())
    }

    fn mount<P, S>(&self, mut app: Router, publisher: ClaimCheckPublisher<P, S>, config: &ResolvedConfig) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        for integration in &config.incidentio {
            let path = format!("{}/{}", self.path_prefix(), integration.id);
            app = app.nest(
                &path,
                crate::source::incidentio::router(publisher.clone(), &integration.config),
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

impl SourcePlugin for LinearPlugin {
    fn id(&self) -> SourceId {
        "linear"
    }

    async fn provision<C: JetStreamContext>(&self, client: &C, config: &ResolvedConfig) -> Result<(), C::Error> {
        for integration in &config.linear {
            crate::source::linear::provision(client, &integration.config).await?;
            info!(
                source = self.id(),
                integration = integration.id.as_str(),
                "stream provisioned"
            );
        }
        Ok(())
    }

    fn mount<P, S>(&self, mut app: Router, publisher: ClaimCheckPublisher<P, S>, config: &ResolvedConfig) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        for integration in &config.linear {
            let path = format!("{}/{}", self.path_prefix(), integration.id);
            app = app.nest(
                &path,
                crate::source::linear::router(publisher.clone(), &integration.config),
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

impl SourcePlugin for MicrosoftGraphPlugin {
    fn id(&self) -> SourceId {
        "microsoft-graph"
    }

    async fn provision<C: JetStreamContext>(&self, client: &C, config: &ResolvedConfig) -> Result<(), C::Error> {
        for integration in &config.microsoft_graph {
            crate::source::microsoft_graph::provision(client, &integration.config).await?;
            info!(
                source = self.id(),
                integration = integration.id.as_str(),
                "stream provisioned"
            );
        }
        Ok(())
    }

    fn mount<P, S>(&self, mut app: Router, publisher: ClaimCheckPublisher<P, S>, config: &ResolvedConfig) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        for integration in &config.microsoft_graph {
            let path = format!("{}/{}", self.path_prefix(), integration.id);
            app = app.nest(
                &path,
                crate::source::microsoft_graph::router(publisher.clone(), &integration.config),
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

impl SourcePlugin for NotionPlugin {
    fn id(&self) -> SourceId {
        "notion"
    }

    async fn provision<C: JetStreamContext>(&self, client: &C, config: &ResolvedConfig) -> Result<(), C::Error> {
        for integration in &config.notion {
            crate::source::notion::provision(client, &integration.config).await?;
            info!(
                source = self.id(),
                integration = integration.id.as_str(),
                "stream provisioned"
            );
        }
        Ok(())
    }

    fn mount<P, S>(&self, mut app: Router, publisher: ClaimCheckPublisher<P, S>, config: &ResolvedConfig) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        for integration in &config.notion {
            let path = format!("{}/{}", self.path_prefix(), integration.id);
            app = app.nest(
                &path,
                crate::source::notion::router(publisher.clone(), &integration.config),
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

impl SourcePlugin for SentryPlugin {
    fn id(&self) -> SourceId {
        "sentry"
    }

    async fn provision<C: JetStreamContext>(&self, client: &C, config: &ResolvedConfig) -> Result<(), C::Error> {
        for integration in &config.sentry {
            crate::source::sentry::provision(client, &integration.config).await?;
            info!(
                source = self.id(),
                integration = integration.id.as_str(),
                "stream provisioned"
            );
        }
        Ok(())
    }

    fn mount<P, S>(&self, mut app: Router, publisher: ClaimCheckPublisher<P, S>, config: &ResolvedConfig) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        for integration in &config.sentry {
            let path = format!("{}/{}", self.path_prefix(), integration.id);
            app = app.nest(
                &path,
                crate::source::sentry::router(publisher.clone(), &integration.config),
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
