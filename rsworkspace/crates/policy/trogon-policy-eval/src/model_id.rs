use crate::identifier::identifier_type;

identifier_type!(
    ModelId,
    "The exact `model_id` of a catalog model. Families, names, and patterns are not identities (ADR#0071 Decision 5)."
);
