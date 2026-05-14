use axum::Router;
use tracing::info;
use trogon_nats::jetstream::{ClaimCheckPublisher, JetStreamContext, JetStreamPublisher, ObjectStorePut};

use crate::config::ResolvedConfig;

/// Stable identifier for a source. Matches the TOML key and the URL prefix.
pub type SourceId = &'static str;

/// One source connector (one webhook receiver). Discord's gateway runner is
/// NOT a `SourcePlugin` — it is spawned explicitly in `main.rs`.
pub trait SourcePlugin: Send + Sync {
    fn id(&self) -> SourceId;
    fn is_enabled(&self, resolved: &ResolvedConfig) -> bool;

    async fn provision<C: JetStreamContext>(
        &self,
        js: &C,
        resolved: &ResolvedConfig,
    ) -> Result<(), C::Error>;

    fn routes<P, S>(
        &self,
        publisher: ClaimCheckPublisher<P, S>,
        resolved: &ResolvedConfig,
    ) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut;
}

// ── Wrappers ───────────────────────────────────────────────────────

pub struct GithubPlugin;
pub struct SlackPlugin;
pub struct TelegramPlugin;
pub struct TwitterPlugin;
pub struct GitlabPlugin;
pub struct IncidentioPlugin;
pub struct LinearPlugin;
pub struct NotionPlugin;
pub struct SentryPlugin;

// ── Helpers (monomorphised per concrete plugin) ──────────────────

async fn provision_one<C, T>(
    plugin: &T,
    js: &C,
    resolved: &ResolvedConfig,
) -> Result<(), C::Error>
where
    C: JetStreamContext,
    T: SourcePlugin,
{
    if plugin.is_enabled(resolved) {
        plugin.provision(js, resolved).await?;
        info!(source = plugin.id(), "stream provisioned");
    }
    Ok(())
}

fn mount_one<P, S, T>(
    plugin: &T,
    router: Router,
    publisher: ClaimCheckPublisher<P, S>,
    resolved: &ResolvedConfig,
) -> Router
where
    P: JetStreamPublisher,
    S: ObjectStorePut,
    T: SourcePlugin,
{
    if plugin.is_enabled(resolved) {
        let id = plugin.id();
        let sub = plugin.routes(publisher, resolved);
        let r = router.nest(format!("/{id}").as_str(), sub);
        info!(source = id, "mounted at /{id}");
        return r;
    }
    router
}

// ── GitHub ─────────────────────────────────────────────────────────

impl SourcePlugin for GithubPlugin {
    fn id(&self) -> SourceId {
        "github"
    }
    fn is_enabled(&self, r: &ResolvedConfig) -> bool {
        r.github.is_some()
    }
    async fn provision<C: JetStreamContext>(
        &self,
        js: &C,
        r: &ResolvedConfig,
    ) -> Result<(), C::Error> {
        match &r.github {
            Some(cfg) => trogon_source_github::provision(js, cfg).await,
            None => Ok(()),
        }
    }
    fn routes<P, S>(
        &self,
        publisher: ClaimCheckPublisher<P, S>,
        r: &ResolvedConfig,
    ) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        match &r.github {
            Some(cfg) => trogon_source_github::router(publisher, cfg),
            None => Router::new(),
        }
    }
}

// ── Slack ──────────────────────────────────────────────────────────

impl SourcePlugin for SlackPlugin {
    fn id(&self) -> SourceId {
        "slack"
    }
    fn is_enabled(&self, r: &ResolvedConfig) -> bool {
        r.slack.is_some()
    }
    async fn provision<C: JetStreamContext>(
        &self,
        js: &C,
        r: &ResolvedConfig,
    ) -> Result<(), C::Error> {
        match &r.slack {
            Some(cfg) => trogon_source_slack::provision(js, cfg).await,
            None => Ok(()),
        }
    }
    fn routes<P, S>(
        &self,
        publisher: ClaimCheckPublisher<P, S>,
        r: &ResolvedConfig,
    ) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        match &r.slack {
            Some(cfg) => trogon_source_slack::router(publisher, cfg),
            None => Router::new(),
        }
    }
}

// ── Telegram ───────────────────────────────────────────────────────

impl SourcePlugin for TelegramPlugin {
    fn id(&self) -> SourceId {
        "telegram"
    }
    fn is_enabled(&self, r: &ResolvedConfig) -> bool {
        r.telegram.is_some()
    }
    async fn provision<C: JetStreamContext>(
        &self,
        js: &C,
        r: &ResolvedConfig,
    ) -> Result<(), C::Error> {
        match &r.telegram {
            Some(cfg) => trogon_source_telegram::provision(js, cfg).await,
            None => Ok(()),
        }
    }
    fn routes<P, S>(
        &self,
        publisher: ClaimCheckPublisher<P, S>,
        r: &ResolvedConfig,
    ) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        match &r.telegram {
            Some(cfg) => trogon_source_telegram::router(publisher, cfg),
            None => Router::new(),
        }
    }
}

// ── Twitter ────────────────────────────────────────────────────────

impl SourcePlugin for TwitterPlugin {
    fn id(&self) -> SourceId {
        "twitter"
    }
    fn is_enabled(&self, r: &ResolvedConfig) -> bool {
        r.twitter.is_some()
    }
    async fn provision<C: JetStreamContext>(
        &self,
        js: &C,
        r: &ResolvedConfig,
    ) -> Result<(), C::Error> {
        match &r.twitter {
            Some(cfg) => trogon_source_twitter::provision(js, cfg).await,
            None => Ok(()),
        }
    }
    fn routes<P, S>(
        &self,
        publisher: ClaimCheckPublisher<P, S>,
        r: &ResolvedConfig,
    ) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        match &r.twitter {
            Some(cfg) => trogon_source_twitter::router(publisher, cfg),
            None => Router::new(),
        }
    }
}

// ── GitLab ─────────────────────────────────────────────────────────

impl SourcePlugin for GitlabPlugin {
    fn id(&self) -> SourceId {
        "gitlab"
    }
    fn is_enabled(&self, r: &ResolvedConfig) -> bool {
        r.gitlab.is_some()
    }
    async fn provision<C: JetStreamContext>(
        &self,
        js: &C,
        r: &ResolvedConfig,
    ) -> Result<(), C::Error> {
        match &r.gitlab {
            Some(cfg) => trogon_source_gitlab::provision(js, cfg).await,
            None => Ok(()),
        }
    }
    fn routes<P, S>(
        &self,
        publisher: ClaimCheckPublisher<P, S>,
        r: &ResolvedConfig,
    ) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        match &r.gitlab {
            Some(cfg) => trogon_source_gitlab::router(publisher, cfg),
            None => Router::new(),
        }
    }
}

// ── Incident.io ────────────────────────────────────────────────────

impl SourcePlugin for IncidentioPlugin {
    fn id(&self) -> SourceId {
        "incidentio"
    }
    fn is_enabled(&self, r: &ResolvedConfig) -> bool {
        r.incidentio.is_some()
    }
    async fn provision<C: JetStreamContext>(
        &self,
        js: &C,
        r: &ResolvedConfig,
    ) -> Result<(), C::Error> {
        match &r.incidentio {
            Some(cfg) => trogon_source_incidentio::provision(js, cfg).await,
            None => Ok(()),
        }
    }
    fn routes<P, S>(
        &self,
        publisher: ClaimCheckPublisher<P, S>,
        r: &ResolvedConfig,
    ) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        match &r.incidentio {
            Some(cfg) => trogon_source_incidentio::router(publisher, cfg),
            None => Router::new(),
        }
    }
}

// ── Linear ─────────────────────────────────────────────────────────

impl SourcePlugin for LinearPlugin {
    fn id(&self) -> SourceId {
        "linear"
    }
    fn is_enabled(&self, r: &ResolvedConfig) -> bool {
        r.linear.is_some()
    }
    async fn provision<C: JetStreamContext>(
        &self,
        js: &C,
        r: &ResolvedConfig,
    ) -> Result<(), C::Error> {
        match &r.linear {
            Some(cfg) => trogon_source_linear::provision(js, cfg).await,
            None => Ok(()),
        }
    }
    fn routes<P, S>(
        &self,
        publisher: ClaimCheckPublisher<P, S>,
        r: &ResolvedConfig,
    ) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        match &r.linear {
            Some(cfg) => trogon_source_linear::router(publisher, cfg),
            None => Router::new(),
        }
    }
}

// ── Notion ─────────────────────────────────────────────────────────

impl SourcePlugin for NotionPlugin {
    fn id(&self) -> SourceId {
        "notion"
    }
    fn is_enabled(&self, r: &ResolvedConfig) -> bool {
        r.notion.is_some()
    }
    async fn provision<C: JetStreamContext>(
        &self,
        js: &C,
        r: &ResolvedConfig,
    ) -> Result<(), C::Error> {
        match &r.notion {
            Some(cfg) => trogon_source_notion::provision(js, cfg).await,
            None => Ok(()),
        }
    }
    fn routes<P, S>(
        &self,
        publisher: ClaimCheckPublisher<P, S>,
        r: &ResolvedConfig,
    ) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        match &r.notion {
            Some(cfg) => trogon_source_notion::router(publisher, cfg),
            None => Router::new(),
        }
    }
}

// ── Sentry ─────────────────────────────────────────────────────────

impl SourcePlugin for SentryPlugin {
    fn id(&self) -> SourceId {
        "sentry"
    }
    fn is_enabled(&self, r: &ResolvedConfig) -> bool {
        r.sentry.is_some()
    }
    async fn provision<C: JetStreamContext>(
        &self,
        js: &C,
        r: &ResolvedConfig,
    ) -> Result<(), C::Error> {
        match &r.sentry {
            Some(cfg) => trogon_source_sentry::provision(js, cfg).await,
            None => Ok(()),
        }
    }
    fn routes<P, S>(
        &self,
        publisher: ClaimCheckPublisher<P, S>,
        r: &ResolvedConfig,
    ) -> Router
    where
        P: JetStreamPublisher,
        S: ObjectStorePut,
    {
        match &r.sentry {
            Some(cfg) => trogon_source_sentry::router(publisher, cfg),
            None => Router::new(),
        }
    }
}

// ── Dispatch entry points ──────────────────────────────────────────

/// Provision JetStream streams for every enabled source.
pub async fn provision_all<C: JetStreamContext>(
    js: &C,
    resolved: &ResolvedConfig,
) -> Result<(), C::Error> {
    provision_one(&GithubPlugin, js, resolved).await?;
    provision_one(&SlackPlugin, js, resolved).await?;
    provision_one(&TelegramPlugin, js, resolved).await?;
    provision_one(&TwitterPlugin, js, resolved).await?;
    provision_one(&GitlabPlugin, js, resolved).await?;
    provision_one(&IncidentioPlugin, js, resolved).await?;
    provision_one(&LinearPlugin, js, resolved).await?;
    provision_one(&NotionPlugin, js, resolved).await?;
    provision_one(&SentryPlugin, js, resolved).await?;
    Ok(())
}

/// Mount webhook routes for every enabled source.
pub fn mount_all<P, S>(
    router: Router,
    publisher: ClaimCheckPublisher<P, S>,
    resolved: &ResolvedConfig,
) -> Router
where
    P: JetStreamPublisher,
    S: ObjectStorePut,
{
    let router = mount_one(&GithubPlugin, router, publisher.clone(), resolved);
    let router = mount_one(&SlackPlugin, router, publisher.clone(), resolved);
    let router = mount_one(&TelegramPlugin, router, publisher.clone(), resolved);
    let router = mount_one(&TwitterPlugin, router, publisher.clone(), resolved);
    let router = mount_one(&GitlabPlugin, router, publisher.clone(), resolved);
    let router = mount_one(&IncidentioPlugin, router, publisher.clone(), resolved);
    let router = mount_one(&LinearPlugin, router, publisher.clone(), resolved);
    let router = mount_one(&NotionPlugin, router, publisher.clone(), resolved);
    mount_one(&SentryPlugin, router, publisher.clone(), resolved)
}
