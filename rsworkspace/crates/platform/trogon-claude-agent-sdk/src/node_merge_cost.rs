//! The stack bytes lodash `mergeWith` spends on the Node stack.

use crate::node_merge_warmth::MergeWarmth;

/// What a merge spends for each array or object it enters, and on top of that where V8 runs some of its code for the
/// first time or runs it unoptimized.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MergeCost {
    /// Each array or object the merge enters.
    pub(crate) level: usize,
    /// An array or object assigning a member while no merge of the process assigned one before.
    pub(crate) cold_assign: usize,
    /// An array at one of the first three array levels of its path while no merge of the process finished an array
    /// before, unless it is one that `shallow_array` covers. V8 also checks for that much at the level of the first
    /// lodash `isIndex` test of the process before it compiles the regular expression to bytecode.
    pub(crate) first_array: usize,
    /// An array holding members where lodash `isIndex` compiles its regular expression to test the first index,
    /// wherever the array sits in its path.
    pub(crate) shallow_array: usize,
    /// How many bytes past the `level`s the stack holds lodash still enters an array or object: it merges the
    /// members of such a level, which run out of stack where they enter or assign anything, and runs out itself as
    /// it returns from the level, when lodash `Stack.delete` runs. A compile of the `isIndex` regular expression to
    /// machine code in such an array throws `SyntaxError` instead.
    pub(crate) descent_reach: usize,
    /// What the compile of the lodash `isIndex` regular expression to machine code at the end of the descent reach
    /// spends on top of its levels in a merge after the one that compiled it to bytecode. V8 spends it when a
    /// background compile lands while the merge runs, which no profile assumes, so every profile sets it to 0.
    pub(crate) later_compile: usize,
}

impl MergeCost {
    /// What an array at the `position`th array level of its path spends on top of `level`.
    pub(crate) fn array(self, position: usize, holds_members: bool, warmth: MergeWarmth) -> usize {
        if holds_members && warmth.compiles_index_test() {
            self.shallow_array
        } else if position <= 3 && !warmth.finished_array {
            self.first_array
        } else {
            0
        }
    }

    /// What assigning a member spends on top of the `level`s of the array or object it assigns into.
    pub(crate) fn assign(self, warmth: MergeWarmth) -> usize {
        if warmth.assigned { 0 } else { self.cold_assign }
    }
}
