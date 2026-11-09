use crate::identifier::identifier_type;

identifier_type!(
    NodeId,
    "The exact identity of one hierarchy node, so that every Agent placed at or below it is covered (ADR#0071 Decision 1)."
);
