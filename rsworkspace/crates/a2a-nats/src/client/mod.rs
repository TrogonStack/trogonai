//! JSON-RPC [`A2aClient`] facade over [`RequestClient`](trogon_nats::RequestClient) + JetStream accessors.
//!
//! By default **`A2aClient`** publishes to **`{prefix}.agent.{agent_id}.{method}`** subjects ([`A2aClient::new`], [`A2aClient::routing_to_agent`]).
//! Use [`A2aClient::routing_via_gateway_ingress`] with a [`MintedUserJwt`] to target **`{prefix}.gateway.{agent_id}.{method}`**
//! subjects (forwarded transparently when **`a2a-gateway`** is running). Streamed **`{prefix}.task.…`** JetStream attaches are
//! unchanged — only unary / bootstrap NATS **`request`** subjects are remapped.

pub mod error;
pub mod event_stream;
pub(crate) mod gateway_headers;
pub mod handle;
pub mod resubscribe;
pub mod streaming;
pub mod unary;
pub(crate) mod wire;

pub use crate::catalog::{AgentCardWatchError, AgentCardWatchEvent, AgentCardWatchStream};
pub use a2a_auth_callout::{MintedUserJwt, caller_jwt_header::CALLER_JWT_HEADER_NAME};
pub use error::ClientError;
pub use event_stream::TypedEventStream;
pub use handle::A2aClient;

pub use a2a::agent_card::AgentCard;
pub use a2a::types::{
    CancelTaskRequest, DeleteTaskPushNotificationConfigRequest, GetExtendedAgentCardRequest,
    GetTaskPushNotificationConfigRequest, GetTaskRequest, ListTaskPushNotificationConfigsRequest,
    ListTaskPushNotificationConfigsResponse, ListTasksRequest, ListTasksResponse, SendMessageRequest,
    SendMessageResponse, Task, TaskPushNotificationConfig,
};
