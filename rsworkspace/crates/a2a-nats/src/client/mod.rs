//! JSON-RPC [`Client`] facade over [`RequestClient`](trogon_nats::RequestClient) + JetStream accessors.
//!
//! By default **`Client`** publishes to **`{prefix}.agent.{agent_id}.{method}`** subjects ([`Client::new`], [`Client::routing_to_agent`]).
//! Use [`Client::routing_via_gateway_ingress`] to target **`{prefix}.gateway.{agent_id}.{method}`** subjects
//! (forwarded transparently when **`a2a-gateway`** is running). Streamed **`{prefix}.task.…`** JetStream attaches are
//! unchanged — only unary / bootstrap NATS **`request`** subjects are remapped.

pub mod error;
pub mod event_stream;
pub mod handle;
pub mod resubscribe;
pub mod streaming;
pub mod unary;
pub(crate) mod wire;

pub use error::ClientError;
pub use event_stream::TypedEventStream;
pub use handle::Client;

pub use a2a_types::{
    AgentCard, CancelTaskRequest, DeleteTaskPushNotificationConfigRequest, GetExtendedAgentCardRequest,
    GetTaskPushNotificationConfigRequest, GetTaskRequest, ListTaskPushNotificationConfigsRequest,
    ListTaskPushNotificationConfigsResponse, ListTasksRequest, ListTasksResponse, SendMessageRequest,
    SendMessageResponse, Task, TaskPushNotificationConfig,
};
