//! Per-operation `{prefix}.agents.{agent_id}.{op}` subjects.
//!
//! Each operation subject ships in its own dedicated PR so the wire contract
//! is reviewed on its own. `tasks/*` and `push/*` operations land in
//! follow-ups under sibling modules.

pub mod card;
pub mod message_send;
pub mod message_stream;
pub mod push;
pub mod tasks;

pub use card::AgentCardSubject;
pub use message_send::MessageSendSubject;
pub use message_stream::MessageStreamSubject;
pub use push::{PushDeleteSubject, PushGetSubject, PushListSubject, PushSetSubject};
pub use tasks::{TasksCancelSubject, TasksGetSubject, TasksListSubject, TasksResubscribeSubject};
