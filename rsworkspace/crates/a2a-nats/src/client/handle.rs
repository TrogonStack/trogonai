//! `A2aClient` — JSON-RPC client facade over NATS request/reply + JetStream.
//!
//! The struct holds the request/reply path (prefix + agent id + NATS + JetStream)
//! plus the ingress overlay (talk to the agent directly, or route via a gateway
//! with a minted caller JWT). Per-operation methods (`message_send`, `tasks_get`,
//! `message_stream`, …) land in their dedicated PRs so each operation's wire
//! contract is reviewed on its own.

use std::time::Duration;

use a2a::agent_card::AgentCard;
use a2a::types::{
    CancelTaskRequest, DeleteTaskPushNotificationConfigRequest, GetExtendedAgentCardRequest,
    GetTaskPushNotificationConfigRequest, GetTaskRequest, ListTaskPushNotificationConfigsRequest,
    ListTaskPushNotificationConfigsResponse, ListTasksRequest, ListTasksResponse, SendMessageRequest,
    SendMessageResponse, SubscribeToTaskRequest, Task, TaskPushNotificationConfig,
};
use a2a_identity_types::MintedUserJwt;
use trogon_nats::RequestClient;
use trogon_nats::jetstream::{JetStreamCreateConsumer, JetStreamGetStream, JsAck, JsMessageOf, JsMessageRef};

use crate::a2a_prefix::A2aPrefix;
use crate::agent_id::A2aAgentId;
use crate::constants::{DEFAULT_OPERATION_TIMEOUT, MIN_TIMEOUT_SECS};
use crate::gateway_ingress::gateway_ingress_subject_from_agent_subject;
use crate::nats::subjects::agents::{
    AgentCardSubject, MessageSendSubject, MessageStreamSubject, PushDeleteSubject, PushGetSubject, PushListSubject,
    PushSetSubject, TasksCancelSubject, TasksGetSubject, TasksListSubject, TasksResubscribeSubject,
};
use crate::req_id::ReqId;
use crate::task_id::A2aTaskId;

use super::error::ClientError;
use super::event_stream::TypedEventStream;
use super::resubscribe::open_resubscribe_stream;
use super::streaming::{StreamingRequest, send_streaming};
use super::unary::send_unary;

/// Whether `A2aClient` publishes to `{prefix}.agents.{agent_id}.…` (talking to the
/// agent directly on a trusted NATS connection) or to `{prefix}.gateway.…` with a
/// minted caller JWT (going through the policy edge).
#[derive(Clone, Debug)]
#[allow(dead_code)] // GatewayIngress JWT is unwrapped by per-operation methods that land afterward
enum ClientIngressTarget {
    AgentSubjects,
    GatewayIngress(MintedUserJwt),
}

#[derive(Clone)]
#[allow(dead_code)] // js is read by per-operation methods that land afterward
pub struct A2aClient<N, J> {
    prefix: A2aPrefix,
    agent_id: A2aAgentId,
    operation_timeout: Duration,
    nats: N,
    js: J,
    ingress: ClientIngressTarget,
}

impl<N, J> A2aClient<N, J> {
    pub fn new(prefix: A2aPrefix, agent_id: A2aAgentId, nats: N, js: J) -> Self {
        Self {
            prefix,
            agent_id,
            operation_timeout: DEFAULT_OPERATION_TIMEOUT,
            nats,
            js,
            ingress: ClientIngressTarget::AgentSubjects,
        }
    }

    #[must_use]
    pub fn with_operation_timeout(mut self, timeout: Duration) -> Self {
        self.operation_timeout = timeout.max(Duration::from_secs(MIN_TIMEOUT_SECS));
        self
    }

    /// Routes unary / bootstrap RPCs through `a2a-gateway`: subjects become
    /// `{prefix}.gateway.{agent_id}.{method…}` (see
    /// [`gateway_ingress_subject_from_agent_subject`]), with `caller_jwt`
    /// attached as the caller-JWT header on every gateway publish.
    ///
    /// Refresh and replace the JWT on the client when a per-operation call
    /// returns [`ClientError::GatewayCallerJwtExpired`].
    #[must_use]
    pub fn routing_via_gateway_ingress(mut self, caller_jwt: MintedUserJwt) -> Self {
        self.ingress = ClientIngressTarget::GatewayIngress(caller_jwt);
        self
    }

    /// Default (direct) routing to `{prefix}.agents.{agent_id}.{method…}`.
    #[must_use]
    pub fn routing_to_agent(mut self) -> Self {
        self.ingress = ClientIngressTarget::AgentSubjects;
        self
    }

    pub fn agent_id(&self) -> &A2aAgentId {
        &self.agent_id
    }

    pub(super) fn prefix(&self) -> &A2aPrefix {
        &self.prefix
    }

    pub(super) fn operation_timeout(&self) -> Duration {
        self.operation_timeout
    }

    pub(super) fn gateway_caller_jwt(&self) -> Option<&MintedUserJwt> {
        match &self.ingress {
            ClientIngressTarget::AgentSubjects => None,
            ClientIngressTarget::GatewayIngress(jwt) => Some(jwt),
        }
    }

    pub(super) fn outbound_rpc_subject(&self, agent_subject: String) -> Result<String, ClientError> {
        match &self.ingress {
            ClientIngressTarget::AgentSubjects => Ok(agent_subject),
            ClientIngressTarget::GatewayIngress(_) => {
                gateway_ingress_subject_from_agent_subject(&agent_subject, &self.prefix)
                    .ok_or(ClientError::InvalidRpcSubjectOverlay)
            }
        }
    }
}

impl<N, J> A2aClient<N, J>
where
    N: RequestClient,
{
    pub async fn tasks_cancel(&self, req: &CancelTaskRequest) -> Result<Task, ClientError> {
        let subject = self.outbound_rpc_subject(TasksCancelSubject::new(self.prefix(), &self.agent_id).to_string())?;
        let req_id = ReqId::new();
        send_unary(
            &self.nats,
            &subject,
            "tasks/cancel",
            req,
            &req_id,
            self.operation_timeout(),
            self.gateway_caller_jwt(),
        )
        .await
    }

    pub async fn tasks_list(&self, req: &ListTasksRequest) -> Result<ListTasksResponse, ClientError> {
        let subject = self.outbound_rpc_subject(TasksListSubject::new(self.prefix(), &self.agent_id).to_string())?;
        let req_id = ReqId::new();
        send_unary(
            &self.nats,
            &subject,
            "tasks/list",
            req,
            &req_id,
            self.operation_timeout(),
            self.gateway_caller_jwt(),
        )
        .await
    }

    pub async fn tasks_get(&self, req: &GetTaskRequest) -> Result<Task, ClientError> {
        let subject = self.outbound_rpc_subject(TasksGetSubject::new(self.prefix(), &self.agent_id).to_string())?;
        let req_id = ReqId::new();
        send_unary(
            &self.nats,
            &subject,
            "tasks/get",
            req,
            &req_id,
            self.operation_timeout(),
            self.gateway_caller_jwt(),
        )
        .await
    }

    pub async fn message_send(&self, req: &SendMessageRequest) -> Result<SendMessageResponse, ClientError> {
        let subject = self.outbound_rpc_subject(MessageSendSubject::new(self.prefix(), &self.agent_id).to_string())?;
        let req_id = ReqId::new();
        send_unary(
            &self.nats,
            &subject,
            "message/send",
            req,
            &req_id,
            self.operation_timeout(),
            self.gateway_caller_jwt(),
        )
        .await
    }

    pub async fn push_delete(&self, req: &DeleteTaskPushNotificationConfigRequest) -> Result<(), ClientError> {
        let subject = self.outbound_rpc_subject(PushDeleteSubject::new(self.prefix(), &self.agent_id).to_string())?;
        let req_id = ReqId::new();
        send_unary::<N, _, ()>(
            &self.nats,
            &subject,
            "tasks/pushNotificationConfig/delete",
            req,
            &req_id,
            self.operation_timeout(),
            self.gateway_caller_jwt(),
        )
        .await
    }

    pub async fn push_list(
        &self,
        req: &ListTaskPushNotificationConfigsRequest,
    ) -> Result<ListTaskPushNotificationConfigsResponse, ClientError> {
        let subject = self.outbound_rpc_subject(PushListSubject::new(self.prefix(), &self.agent_id).to_string())?;
        let req_id = ReqId::new();
        send_unary(
            &self.nats,
            &subject,
            "tasks/pushNotificationConfig/list",
            req,
            &req_id,
            self.operation_timeout(),
            self.gateway_caller_jwt(),
        )
        .await
    }

    pub async fn push_get(
        &self,
        req: &GetTaskPushNotificationConfigRequest,
    ) -> Result<TaskPushNotificationConfig, ClientError> {
        let subject = self.outbound_rpc_subject(PushGetSubject::new(self.prefix(), &self.agent_id).to_string())?;
        let req_id = ReqId::new();
        send_unary(
            &self.nats,
            &subject,
            "tasks/pushNotificationConfig/get",
            req,
            &req_id,
            self.operation_timeout(),
            self.gateway_caller_jwt(),
        )
        .await
    }

    pub async fn push_set(&self, req: &TaskPushNotificationConfig) -> Result<TaskPushNotificationConfig, ClientError> {
        let subject = self.outbound_rpc_subject(PushSetSubject::new(self.prefix(), &self.agent_id).to_string())?;
        let req_id = ReqId::new();
        send_unary(
            &self.nats,
            &subject,
            "tasks/pushNotificationConfig/set",
            req,
            &req_id,
            self.operation_timeout(),
            self.gateway_caller_jwt(),
        )
        .await
    }

    pub async fn agent_card(&self) -> Result<AgentCard, ClientError> {
        let subject = self.outbound_rpc_subject(AgentCardSubject::new(self.prefix(), &self.agent_id).to_string())?;
        let req_id = ReqId::new();
        let req = GetExtendedAgentCardRequest { tenant: None };
        send_unary(
            &self.nats,
            &subject,
            "agent/getAuthenticatedExtendedCard",
            &req,
            &req_id,
            self.operation_timeout(),
            self.gateway_caller_jwt(),
        )
        .await
    }
}

impl<N, J> A2aClient<N, J>
where
    N: RequestClient,
    J: JetStreamGetStream,
    JsMessageOf<J>: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    <J as JetStreamGetStream>::Stream: Send + 'static,
    <<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Messages: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::MessagesError: std::fmt::Display + Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::StreamError: std::fmt::Display + Send + 'static,
{
    pub async fn tasks_resubscribe(
        &self,
        task_id: &A2aTaskId,
        last_seq: u64,
    ) -> Result<(Task, TypedEventStream), ClientError> {
        let subject =
            self.outbound_rpc_subject(TasksResubscribeSubject::new(self.prefix(), &self.agent_id).to_string())?;
        let req_id = ReqId::new();
        let req = SubscribeToTaskRequest {
            id: task_id.as_str().to_owned(),
            tenant: None,
        };
        let snapshot: Task = send_unary(
            &self.nats,
            &subject,
            "tasks/resubscribe",
            &req,
            &req_id,
            self.operation_timeout(),
            self.gateway_caller_jwt(),
        )
        .await?;
        let stream = open_resubscribe_stream(&self.js, self.prefix(), task_id, last_seq).await?;
        Ok((snapshot, stream))
    }

    pub async fn message_stream(
        &self,
        req: &SendMessageRequest,
    ) -> Result<(SendMessageResponse, TypedEventStream), ClientError> {
        let subject = self.outbound_rpc_subject(MessageStreamSubject::new(self.prefix(), &self.agent_id).to_string())?;
        let req_id = ReqId::new();
        let ctx = StreamingRequest {
            nats: &self.nats,
            js: &self.js,
            subject: subject.as_str(),
            method: "message/stream",
            req_id: &req_id,
            prefix: self.prefix(),
            op_timeout: self.operation_timeout(),
            gateway_caller_jwt: self.gateway_caller_jwt(),
        };
        send_streaming(ctx, req).await
    }
}

#[cfg(test)]
mod tests;
