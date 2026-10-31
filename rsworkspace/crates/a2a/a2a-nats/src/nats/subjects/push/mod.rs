//! Push dead-letter subjects (`{prefix}.v1.push.dlq.{kind}.{id}.{task_id}` and its
//! `mirror` lane), where `{kind}.{id}` is the principal of the caller whose push
//! delivery failed (ADR#0082).

mod dead_letter_tokens;
pub mod dlq;
pub mod dlq_mirror;

pub use dlq::{PushDlqSubject, PushDlqSubjectError};
pub use dlq_mirror::{PushDlqMirrorSubject, PushDlqMirrorSubjectError};
