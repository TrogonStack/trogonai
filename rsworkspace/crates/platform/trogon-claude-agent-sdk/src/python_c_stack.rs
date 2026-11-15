//! How many CPython C recursion checks are already open where the Python SDK calls `json.loads`, which sets how
//! deeply the text it reads may nest.
//!
//! CPython 3.12's JSON scanner opens one C recursion check for each array and object it enters, and raises
//! `RecursionError` when one would put more than [`PYTHON_C_RECURSION_LIMIT`] under way. The checks the caller
//! already holds count against that limit, so the deepest nesting `json.loads` reads depends on where it runs. A
//! call between two Python functions opens none, while the C layers that drive coroutines do: the event loop
//! running a handle, the task stepping its coroutine and an async generator resuming its body each hold one or
//! more. Neither the event loop (`asyncio` or `anyio`) nor a first call that fills the interpreter's caches moves
//! the count.

use crate::constants::PYTHON_C_RECURSION_LIMIT;

/// The C recursion checks open where `json.loads` runs, for one way of reaching it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PythonCStack {
    held: usize,
}

impl PythonCStack {
    /// A plain function or module-level code: 3 checks, so `json.loads` reads 9997 levels.
    pub const MODULE: Self = Self { held: 3 };
    /// `ClaudeSDKClient.connect`, awaited directly or through `async with`, under `asyncio.run` or `anyio.run`:
    /// 7 checks, so 9993 levels.
    pub const CLIENT_CONNECT: Self = Self { held: 7 };
    /// `query()`, with a `str` or an async iterable prompt, under `asyncio.run` or `anyio.run`: 13 checks, so
    /// 9987 levels.
    pub const QUERY: Self = Self { held: 13 };

    /// How many arrays and objects `json.loads` enters here before it raises `RecursionError`.
    pub fn json_depth(self) -> usize {
        PYTHON_C_RECURSION_LIMIT.saturating_sub(self.held)
    }
}

#[cfg(test)]
#[path = "python_c_stack_tests.rs"]
mod tests;
