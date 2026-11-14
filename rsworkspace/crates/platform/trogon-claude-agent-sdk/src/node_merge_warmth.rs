//! What V8 already ran of lodash `mergeWith` in a Node process. V8 compiles a function the first time it runs, on a
//! deeper stack than later runs take, so a merge spends more stack where it runs some code for the first time. It
//! compiles the regular expression lodash `isIndex` tests keys with twice: to bytecode on its first test, and to
//! machine code on its second.

/// Whether a merge of the process already assigned a member and already finished merging an array, and how many
/// keys lodash `isIndex` tested.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct MergeWarmth {
    /// lodash `assignMergeValue` ran: a merge assigned a string, number, boolean or `null`, what the settings
    /// customizer answered, or an array or object it finished.
    pub(crate) assigned: bool,
    /// A merge finished an array.
    pub(crate) finished_array: bool,
    /// How many keys lodash `isIndex` tested, counted up to [`MergeWarmth::INDEX_COMPILES`]: lodash `keysIn` tests
    /// each index of an array the merges enter onto a fresh or copied array. An array the settings customizer
    /// unions onto a held one is not entered.
    pub(crate) index_tests: usize,
}

impl MergeWarmth {
    /// How many tests of lodash `isIndex` compile its regular expression.
    pub(crate) const INDEX_COMPILES: usize = 2;

    /// A process where a merge already assigned a member and finished an array, but lodash `isIndex` tested no key.
    #[cfg(test)]
    pub(crate) const WARM: Self = Self {
        assigned: true,
        finished_array: true,
        index_tests: 0,
    };

    /// Counts the `keys` lodash `isIndex` tested where a merge entered an array holding that many members.
    pub(crate) fn test_index_keys(&mut self, keys: usize) {
        self.index_tests = self.index_tests.saturating_add(keys).min(Self::INDEX_COMPILES);
    }

    /// Whether lodash `isIndex` compiles its regular expression on its next test.
    pub(crate) fn compiles_index_test(self) -> bool {
        self.index_tests < Self::INDEX_COMPILES
    }
}
