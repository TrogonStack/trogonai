//! The Node stack one lodash `mergeWith` call has.

use crate::node_merge_cost::MergeCost;

/// What a merge spends and the bytes it has to spend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MergeRoom {
    pub(crate) cost: MergeCost,
    pub(crate) available: usize,
}

impl MergeRoom {
    /// Whether `depth` nested arrays and objects, and `extra` bytes on top of them, fit.
    pub(crate) fn fits(self, depth: usize, extra: usize) -> bool {
        depth.saturating_mul(self.cost.level).saturating_add(extra) <= self.available
    }

    /// Whether lodash still enters an array or object at `depth` where its level does not fit, as
    /// [`MergeCost::descent_reach`] describes.
    pub(crate) fn reaches(self, depth: usize) -> bool {
        self.reaches_past(depth, 0)
    }

    /// Whether lodash still reaches `extra` bytes past `depth` nested arrays and objects that do not fit, as
    /// [`MergeCost::descent_reach`] describes.
    pub(crate) fn reaches_past(self, depth: usize, extra: usize) -> bool {
        depth.saturating_mul(self.cost.level).saturating_add(extra)
            <= self.available.saturating_add(self.cost.descent_reach)
    }
}
