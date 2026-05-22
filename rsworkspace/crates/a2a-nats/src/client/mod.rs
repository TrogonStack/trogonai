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
