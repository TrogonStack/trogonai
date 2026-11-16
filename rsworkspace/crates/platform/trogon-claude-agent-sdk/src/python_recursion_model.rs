//! Where CPython's recursion limit sits for one `jsonschema.validate` call: how deep the caller already is, how
//! many frames the interpreter allows and whether the `isinstance` caches of the ABCs `jsonschema` checks against
//! are already filled.

/// Whether the negative and positive caches of `collections.abc` and `numbers` ABCs are filled.
///
/// The first `isinstance(value, Sequence)` of a type walks `ABCMeta.__instancecheck__` and `__subclasscheck__`
/// frames; once one walk completes, later checks of that type stop at `__instancecheck__`. A walk that overflows
/// fills nothing, so the next call walks again.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum CacheState {
    /// Every type was checked against every ABC before, as in a server that already handled a call.
    #[default]
    Warm,
    /// No type was checked yet, as in the first call of a fresh process.
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "the bridge models a warm server; the tests replay cold runs")
    )]
    Cold,
}

/// The fields of a [`RecursionModel`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "the bridge uses the default model; the tests build others")
)]
pub(crate) struct RecursionModelOptions {
    /// The frame depth of `jsonschema.validate`, counting the module frame as 1.
    pub(crate) caller_depth: usize,
    pub(crate) cache: CacheState,
    /// `sys.getrecursionlimit()`.
    pub(crate) limit: usize,
}

/// Why a [`RecursionModel`] cannot be built.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "the bridge uses the default model; the tests build others")
)]
pub(crate) enum RecursionModelError {
    /// CPython raises before `jsonschema.validate` itself runs.
    #[error("a caller at depth {caller_depth} is already past the recursion limit {limit}")]
    CallerPastLimit { caller_depth: usize, limit: usize },
}

/// The recursion limit as one `jsonschema.validate` call meets it. The default is the in-process MCP path of the
/// Python SDK: `asyncio.run` from module level puts `jsonschema.validate` at depth 17, the limit is CPython's
/// default 1000, and the server already handled a call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RecursionModel {
    caller_depth: usize,
    cache: CacheState,
    limit: usize,
}

impl Default for RecursionModel {
    fn default() -> Self {
        Self {
            caller_depth: 17,
            cache: CacheState::Warm,
            limit: 1000,
        }
    }
}

impl RecursionModel {
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "the bridge uses the default model; the tests build others")
    )]
    pub(crate) fn new(options: RecursionModelOptions) -> Result<Self, RecursionModelError> {
        let RecursionModelOptions {
            caller_depth,
            cache,
            limit,
        } = options;
        if caller_depth > limit {
            return Err(RecursionModelError::CallerPastLimit { caller_depth, limit });
        }
        Ok(Self {
            caller_depth,
            cache,
            limit,
        })
    }

    /// The depth of the first frame `jsonschema.validate` pushes: `check_schema`, then `best_match`.
    pub(crate) fn first_frame(&self) -> usize {
        self.caller_depth + 1
    }

    pub(crate) fn cache(&self) -> CacheState {
        self.cache
    }

    /// The depth of the `__str__` frame of an exception the caller's handler turns into text: the handler calls it
    /// from the frame that called `jsonschema.validate`, so it sits where `jsonschema.validate` did.
    pub(crate) fn handler_frame(&self) -> usize {
        self.caller_depth
    }

    /// Whether a frame at `depth` makes CPython raise `RecursionError`.
    pub(crate) fn exceeded(&self, depth: usize) -> bool {
        depth > self.limit
    }

    /// How many frames above `depth` CPython still allows.
    pub(crate) fn room(&self, depth: usize) -> usize {
        self.limit.saturating_sub(depth)
    }
}

#[cfg(test)]
#[path = "python_recursion_model_tests.rs"]
mod tests;
