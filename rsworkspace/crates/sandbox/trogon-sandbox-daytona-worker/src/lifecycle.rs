/// Daytona's three idle intervals, in the minutes its API takes.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub(crate) struct AutoIntervals {
    pub(crate) auto_stop: Option<u32>,
    pub(crate) auto_archive: Option<u32>,
    pub(crate) auto_delete: Option<u32>,
}

/// Derives Daytona's idle intervals from the lease's idle limit.
///
/// `auto_stop` follows the idle limit rounded up to whole minutes; with none it is zero, which disables Daytona's own
/// default so only the lease's end stops the sandbox. Archive and delete count from the moment a sandbox stops, and a
/// paused lease is stopped, so they stay at Daytona's defaults rather than destroy a paused lease.
pub(crate) fn auto_intervals(idle_timeout_seconds: Option<u32>) -> AutoIntervals {
    AutoIntervals {
        auto_stop: Some(idle_timeout_seconds.map_or(0, |secs| secs.div_ceil(60).max(1))),
        auto_archive: None,
        auto_delete: None,
    }
}

#[cfg(test)]
mod tests;
