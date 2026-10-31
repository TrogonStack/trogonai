//! Per-operation `{prefix}.v1.agents.{agent_id}.{kind}.{id}.{op}` subjects, where
//! `{kind}.{id}` is the caller's principal position (ADR#0082), and the
//! [`AgentDeliverySubject`] an agent parses every delivered request with.
//!
//! Each operation subject ships in its own dedicated PR so the wire contract
//! is reviewed on its own. `tasks/*` and `push/*` operations land in
//! follow-ups under sibling modules.

pub mod card;
pub mod delivery;
pub mod message_send;
pub mod message_stream;
pub mod push;
pub mod tasks;

pub use card::AgentCardSubject;
pub use delivery::{AgentDeliverySubject, AgentDeliverySubjectError};
pub use message_send::MessageSendSubject;
pub use message_stream::MessageStreamSubject;
pub use push::{PushDeleteSubject, PushGetSubject, PushListSubject, PushSetSubject};
pub use tasks::{TasksCancelSubject, TasksGetSubject, TasksListSubject, TasksResubscribeSubject};
