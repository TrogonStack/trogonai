use crate::access::Access;
use crate::exclusion_kind::ExclusionKind;
use crate::relation::Relation;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExclusionConflict {
    pub path: String,
    pub exclusion: String,
    pub kind: ExclusionKind,
    pub access: Access,
    pub relation: Relation,
    /// Set on an `Above` conflict with a wildcard exclusion when the path
    /// could not be examined for what the exclusion covers beneath it.
    pub unexamined: Option<String>,
}
