//! The `{kind}.{id}.{task_id}` tail every push dead-letter subject ends with, after
//! its lane: `{prefix}.v1.push.dlq.` for the authoritative record and
//! `{prefix}.v1.push.dlq.mirror.` for its copy.

/// The tail tokens of a dead-letter subject, split but not yet validated.
pub(super) struct DeadLetterTokens<'a> {
    pub(super) kind: &'a str,
    pub(super) id: &'a str,
    pub(super) task_id: &'a str,
}

impl<'a> DeadLetterTokens<'a> {
    /// `None` unless the tail holds exactly the principal and the task.
    pub(super) fn split(tail: &'a str) -> Option<Self> {
        let mut tokens = tail.split('.');
        match (tokens.next(), tokens.next(), tokens.next(), tokens.next()) {
            (Some(kind), Some(id), Some(task_id), None) => Some(Self { kind, id, task_id }),
            _ => None,
        }
    }
}
