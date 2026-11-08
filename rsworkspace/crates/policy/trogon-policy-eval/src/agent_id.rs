use crate::identifier::identifier_type;

identifier_type!(
    AgentId,
    "The exact identity of one Agent. Never a display name, label, or pattern (ADR#0071 Decision 1)."
);
