use std::time::Duration;

use async_trait::async_trait;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::Channel;

use super::ResourceRevision;
use super::bearer_auth::BearerAuth;
use super::{AteError, AteResult, ConnectError, ConnectSecurity, Control};
use crate::proto;

type AuthenticatedChannel = InterceptedService<Channel, BearerAuth>;

/// The tonic-backed [`Control`].
pub struct TonicControl {
    client: proto::control_client::ControlClient<AuthenticatedChannel>,
}

impl TonicControl {
    /// Connects to `ate-api-server`.
    ///
    /// TLS and the bearer token are both driven by `security`: `ate-api-server` verifies
    /// callers by Kubernetes service-account JWT over TLS, not by pod identity alone.
    pub async fn connect(endpoint: &str, timeout: Duration, security: ConnectSecurity) -> Result<Self, ConnectError> {
        let mut builder = tonic::transport::Endpoint::from_shared(endpoint.to_string())
            .map_err(ConnectError::Endpoint)?
            .connect_timeout(timeout)
            .timeout(timeout);
        if let Some(tls) = security.tls {
            builder = builder
                .tls_config(tls.into_client_tls_config())
                .map_err(ConnectError::Tls)?;
        }
        let channel = builder.connect().await.map_err(ConnectError::Connect)?;
        let auth = BearerAuth {
            token_file: security.bearer_token_file,
        };
        Ok(Self {
            client: proto::control_client::ControlClient::with_interceptor(channel, auth),
        })
    }
}

#[async_trait]
impl Control for TonicControl {
    async fn create_atespace(&self, atespace: &str) -> AteResult<proto::Atespace> {
        let request = proto::CreateAtespaceRequest {
            atespace: Some(proto::Atespace {
                metadata: Some(proto::ResourceMetadata {
                    name: atespace.to_string(),
                    ..Default::default()
                }),
            }),
        };
        Ok(self.client.clone().create_atespace(request).await?.into_inner())
    }

    async fn get_actor(&self, actor: proto::ObjectRef) -> AteResult<proto::Actor> {
        let request = proto::GetActorRequest { actor: Some(actor) };
        Ok(self.client.clone().get_actor(request).await?.into_inner())
    }

    async fn create_actor(&self, actor: proto::Actor) -> AteResult<proto::Actor> {
        let request = proto::CreateActorRequest { actor: Some(actor) };
        Ok(self.client.clone().create_actor(request).await?.into_inner())
    }

    async fn resume_actor(&self, actor: proto::ObjectRef) -> AteResult<proto::Actor> {
        let request = proto::ResumeActorRequest { actor: Some(actor) };
        let response = self.client.clone().resume_actor(request).await?.into_inner();
        // `resumed: false` means the actor was already RUNNING, which is a successful
        // outcome for an idempotent lease retry rather than a no-op error.
        response.actor.ok_or_else(|| AteError::Other {
            message: "ResumeActor returned no actor".to_string(),
        })
    }

    async fn suspend_actor(&self, actor: proto::ObjectRef) -> AteResult<proto::Actor> {
        let request = proto::SuspendActorRequest {
            actor: Some(actor),
            options: Some(proto::SuspendOptions {
                mode: proto::DurabilityMode::Synchronous as i32,
            }),
        };
        let response = self.client.clone().suspend_actor(request).await?.into_inner();
        response.actor.ok_or_else(|| AteError::Other {
            message: "SuspendActor returned no actor".to_string(),
        })
    }

    async fn delete_actor(&self, actor: ResourceRevision) -> AteResult<proto::Actor> {
        let request = proto::DeleteActorRequest {
            actor: Some(actor.resource().clone()),
            options: Some(actor.options()),
        };
        Ok(self.client.clone().delete_actor(request).await?.into_inner())
    }

    async fn list_actors(&self, atespace: &str) -> AteResult<Vec<proto::Actor>> {
        let mut actors = Vec::new();
        let mut page_token = String::new();
        loop {
            let request = proto::ListActorsRequest {
                atespace: atespace.to_string(),
                options: Some(proto::ListOptions {
                    page_size: 1000,
                    page_token: page_token.clone(),
                }),
            };
            let response = self.client.clone().list_actors(request).await?.into_inner();
            actors.extend(response.actors);
            if response.next_page_token.is_empty() {
                break;
            }
            page_token = response.next_page_token;
        }
        Ok(actors)
    }

    async fn create_actor_template(&self, template: proto::ActorTemplate) -> AteResult<proto::ActorTemplate> {
        let request = proto::CreateActorTemplateRequest {
            actor_template: Some(template),
        };
        Ok(self.client.clone().create_actor_template(request).await?.into_inner())
    }

    async fn get_actor_template(&self, template: proto::ObjectRef) -> AteResult<proto::ActorTemplate> {
        let request = proto::GetActorTemplateRequest {
            actor_template: Some(template),
        };
        Ok(self.client.clone().get_actor_template(request).await?.into_inner())
    }

    async fn delete_actor_template(&self, template: ResourceRevision) -> AteResult<()> {
        let request = proto::DeleteActorTemplateRequest {
            actor_template: Some(template.resource().clone()),
            options: Some(template.options()),
        };
        self.client.clone().delete_actor_template(request).await?;
        Ok(())
    }
}
