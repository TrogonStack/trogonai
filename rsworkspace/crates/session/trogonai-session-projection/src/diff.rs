//! §1886 Diff de prompt projection por modelo.
//!
//! Compares two [`PromptProjection`]s — typically the same canonical session compiled for
//! two different models — over the specified projection contract (§1300: each projection
//! records `projection_id`, token estimate, included and excluded blocks; §1302: block
//! ordering is stable to ease debugging). The diff is the debugging tool that stable
//! ordering + included/excluded metadata enable: it shows which blocks a target model
//! drops, gains, or re-sizes, and which degradations differ. Pure + deterministic
//! (BTree-ordered output).

use std::collections::{BTreeMap, BTreeSet};

use trogonai_session_contracts::{DegradationMetadata, PromptProjection};

/// A block whose token estimate changed between two projections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionBlockDelta {
    pub block_id: String,
    pub from_tokens: u64,
    pub to_tokens: u64,
}

/// Structured diff between two prompt projections (§1886).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionDiff {
    pub from_model: String,
    pub to_model: String,
    pub from_token_estimate: u64,
    pub to_token_estimate: u64,
    /// Block ids included in `from` but NOT included in `to` (dropped for the target).
    pub only_in_from: Vec<String>,
    /// Block ids included in `to` but NOT in `from` (added for the target).
    pub only_in_to: Vec<String>,
    /// Blocks included in both projections but with a different token estimate.
    pub changed: Vec<ProjectionBlockDelta>,
    /// Degradations present in `to` but not in `from` (newly degraded for the target).
    pub added_degradations: Vec<String>,
    /// Degradations present in `from` but not in `to`.
    pub removed_degradations: Vec<String>,
}

impl ProjectionDiff {
    /// Whether the two projections are block-and-degradation identical.
    pub fn is_empty(&self) -> bool {
        self.only_in_from.is_empty()
            && self.only_in_to.is_empty()
            && self.changed.is_empty()
            && self.added_degradations.is_empty()
            && self.removed_degradations.is_empty()
    }
}

fn render_degradation(degradation: &DegradationMetadata) -> String {
    format!("{:?}:{}", degradation.kind.as_known(), degradation.detail)
}

/// Diff two prompt projections over their included blocks, token estimates and
/// degradation metadata (§1886 / §1300). Output ordering is stable (BTree-keyed) so the
/// diff is reproducible for debugging (§1302).
pub fn diff_projections(from: &PromptProjection, to: &PromptProjection) -> ProjectionDiff {
    let from_included: BTreeMap<&str, u64> = from
        .included_blocks
        .iter()
        .map(|block| (block.block_id.as_str(), block.token_estimate))
        .collect();
    let to_included: BTreeMap<&str, u64> = to
        .included_blocks
        .iter()
        .map(|block| (block.block_id.as_str(), block.token_estimate))
        .collect();

    let only_in_from = from_included
        .keys()
        .filter(|id| !to_included.contains_key(*id))
        .map(|id| id.to_string())
        .collect();
    let only_in_to = to_included
        .keys()
        .filter(|id| !from_included.contains_key(*id))
        .map(|id| id.to_string())
        .collect();
    let changed = from_included
        .iter()
        .filter_map(|(id, from_tokens)| {
            to_included.get(id).and_then(|to_tokens| {
                (to_tokens != from_tokens).then(|| ProjectionBlockDelta {
                    block_id: id.to_string(),
                    from_tokens: *from_tokens,
                    to_tokens: *to_tokens,
                })
            })
        })
        .collect();

    let from_degradations: BTreeSet<String> = from.degradation_metadata.iter().map(render_degradation).collect();
    let to_degradations: BTreeSet<String> = to.degradation_metadata.iter().map(render_degradation).collect();
    let added_degradations = to_degradations.difference(&from_degradations).cloned().collect();
    let removed_degradations = from_degradations.difference(&to_degradations).cloned().collect();

    ProjectionDiff {
        from_model: from.model_id.clone(),
        to_model: to.model_id.clone(),
        from_token_estimate: from.token_estimate,
        to_token_estimate: to.token_estimate,
        only_in_from,
        only_in_to,
        changed,
        added_degradations,
        removed_degradations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use buffa::EnumValue;
    use trogonai_session_contracts::{
        DegradationKind, ProjectionBlock, ProjectionBlockKind, PromptProjection,
    };

    fn block(id: &str, tokens: u64) -> ProjectionBlock {
        ProjectionBlock {
            block_id: id.to_string(),
            kind: EnumValue::Known(ProjectionBlockKind::RecentTurn),
            label: id.to_string(),
            token_estimate: tokens,
            ..ProjectionBlock::default()
        }
    }

    fn projection(model: &str, tokens: u64, included: Vec<ProjectionBlock>, degs: Vec<DegradationMetadata>) -> PromptProjection {
        PromptProjection {
            model_id: model.to_string(),
            token_estimate: tokens,
            included_blocks: included,
            degradation_metadata: degs,
            ..PromptProjection::default()
        }
    }

    #[test]
    fn identical_projections_have_empty_diff() {
        let a = projection("m", 100, vec![block("b1", 10), block("b2", 20)], vec![]);
        let b = projection("m", 100, vec![block("b1", 10), block("b2", 20)], vec![]);
        assert!(diff_projections(&a, &b).is_empty());
    }

    #[test]
    fn diff_reports_dropped_added_changed_blocks_and_degradations() {
        let from = projection(
            "claude-sonnet",
            120,
            vec![block("block_older_transcript", 50), block("block_context_twin", 30)],
            vec![],
        );
        let to = projection(
            "grok-code-fast",
            70,
            vec![block("block_context_twin", 40), block("block_recent", 30)],
            vec![DegradationMetadata {
                kind: EnumValue::Known(DegradationKind::ImagesOmitted),
                detail: "target lacks image input".to_string(),
                ..DegradationMetadata::default()
            }],
        );
        let diff = diff_projections(&from, &to);
        assert_eq!(diff.from_model, "claude-sonnet");
        assert_eq!(diff.to_model, "grok-code-fast");
        assert_eq!(diff.only_in_from, vec!["block_older_transcript".to_string()]);
        assert_eq!(diff.only_in_to, vec!["block_recent".to_string()]);
        assert_eq!(diff.changed, vec![ProjectionBlockDelta {
            block_id: "block_context_twin".to_string(),
            from_tokens: 30,
            to_tokens: 40,
        }]);
        assert_eq!(diff.added_degradations.len(), 1);
        assert!(diff.added_degradations[0].contains("target lacks image input"));
        assert!(diff.removed_degradations.is_empty());
        assert!(!diff.is_empty());
    }
}
