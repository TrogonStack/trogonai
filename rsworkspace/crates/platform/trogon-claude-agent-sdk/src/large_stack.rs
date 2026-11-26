//! Work that recurses as deep as CPython does needs more native stack than a default thread has, so it runs on a
//! thread of its own whose stack is as large as the system grants.

/// `work()` on a thread whose stack is the first of `sizes` the system grants. Where it grants none, `inline()`
/// answers on the calling thread; `panicked` answers for a thread that panicked.
pub(crate) fn on_large_stack<T: Send>(
    sizes: &[usize],
    work: impl Fn() -> T + Sync,
    inline: impl FnOnce() -> T,
    panicked: T,
) -> T {
    std::thread::scope(|scope| {
        let spawned = sizes.iter().find_map(|&size| {
            std::thread::Builder::new()
                .stack_size(size)
                .spawn_scoped(scope, &work)
                .ok()
        });
        match spawned {
            Some(handle) => handle.join().unwrap_or(panicked),
            None => inline(),
        }
    })
}

#[cfg(test)]
#[path = "large_stack_tests.rs"]
mod tests;
