//! Admission control for command execution.
//!
//! Per [ADR#0028](https://github.com/TrogonStack/trogonai/blob/main/docs/adr/0028-decider-admission-control-and-backpressure.md),
//! this module is a seam, not a policy. It owns the permit contract, the
//! [`AdmissionLimit`] value object, the [`OverloadedError`] rejection, and one
//! semaphore-shaped implementation callers may reuse. It deliberately does
//! not own a bound: the number worth choosing is
//! `admission_limit x max_memory_bytes` against a specific host's memory
//! budget, which is a deployment fact rather than a property of any decider.
//!
//! Execution is therefore unbounded unless a caller opts in with
//! `with_admission`. What the seam adds over a semaphore in each caller's own
//! dispatch loop is a shareable limiter type: two consumers in one process
//! can hold clones of one [`ConcurrencyAdmission`] and share a single host
//! budget.

use std::num::NonZeroUsize;
use std::sync::Arc;

/// Number of command executions an admission limiter allows to run at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AdmissionLimit(NonZeroUsize);

impl AdmissionLimit {
    /// Wraps an already validated non-zero admission limit.
    pub const fn new(value: NonZeroUsize) -> Self {
        Self(value)
    }

    /// Creates an admission limit after rejecting zero.
    ///
    /// Zero is rejected because a limiter that admits nothing is a disabled
    /// host, not a configured one, and it fails every command identically
    /// however much capacity the host actually has.
    pub const fn try_new(value: usize) -> Result<Self, AdmissionLimitError> {
        match NonZeroUsize::new(value) {
            Some(value) => Ok(Self(value)),
            None => Err(AdmissionLimitError { value }),
        }
    }

    /// Returns the limit as a plain integer.
    pub const fn as_usize(self) -> usize {
        self.0.get()
    }

    /// Returns the limit as a non-zero integer.
    pub const fn as_non_zero(self) -> NonZeroUsize {
        self.0
    }
}

impl TryFrom<usize> for AdmissionLimit {
    type Error = AdmissionLimitError;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
}

impl From<AdmissionLimit> for usize {
    fn from(value: AdmissionLimit) -> Self {
        value.as_usize()
    }
}

impl std::fmt::Display for AdmissionLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_usize().fmt(f)
    }
}

/// Error returned when constructing an invalid [`AdmissionLimit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("admission limit must be greater than zero, got {value}")]
pub struct AdmissionLimitError {
    value: usize,
}

impl AdmissionLimitError {
    /// Returns the rejected admission limit value.
    pub const fn value(self) -> usize {
        self.value
    }
}

/// Rejection returned when a limiter has no capacity for another command.
///
/// Distinct from every other command failure: nothing was read, decided, or
/// appended, and the same command submitted later may well succeed. Callers
/// translate it into a retry with backoff or a shed-load response rather than
/// treating it as a domain rejection or a storage fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("command shed by admission control: all {limit} execution slots are in use")]
pub struct OverloadedError {
    limit: AdmissionLimit,
}

impl OverloadedError {
    /// Names the limit that was already fully committed.
    pub const fn new(limit: AdmissionLimit) -> Self {
        Self { limit }
    }

    /// Returns the limit the rejecting limiter enforces.
    pub const fn limit(self) -> AdmissionLimit {
        self.limit
    }
}

/// Gates how many command executions may run concurrently.
///
/// Acquired once at the top of an execution, before any stream read, guest
/// instantiation, or append, and released when the execution ends.
///
/// [`admit`](Self::admit) is deliberately synchronous. An admission decision
/// that can await is a queue, and an unbounded queue relocates the memory
/// pressure a limiter exists to bound while hiding it behind latency instead
/// of surfacing it as a distinguishable error. A host that wants a bounded
/// wait before shedding owns that wait in its own dispatch loop, before it
/// builds the execution.
pub trait CommandAdmission {
    /// Capacity held for the duration of one execution.
    ///
    /// Released by dropping it, so an execution that panics or returns early
    /// still gives its slot back.
    type Permit: Send;

    /// Reserves capacity for one command execution, or sheds it immediately.
    fn admit(&self) -> Result<Self::Permit, OverloadedError>;
}

impl<A> CommandAdmission for &A
where
    A: CommandAdmission + ?Sized,
{
    type Permit = A::Permit;

    fn admit(&self) -> Result<Self::Permit, OverloadedError> {
        (*self).admit()
    }
}

impl<A> CommandAdmission for Arc<A>
where
    A: CommandAdmission + ?Sized,
{
    type Permit = A::Permit;

    fn admit(&self) -> Result<Self::Permit, OverloadedError> {
        self.as_ref().admit()
    }
}

/// The default: every command is admitted, exactly as it was before
/// admission control existed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WithoutAdmission;

impl CommandAdmission for WithoutAdmission {
    type Permit = ();

    fn admit(&self) -> Result<Self::Permit, OverloadedError> {
        Ok(())
    }
}

/// Admits up to [`AdmissionLimit`] concurrent executions and sheds the rest.
///
/// Clone this to share one budget: clones count against the same permits, so
/// every consumer in a process can be bounded together rather than each
/// bounding itself in isolation. Cloning is cheap, it only bumps a reference
/// count.
///
/// Size the limit against the memory a single execution can hold rather than
/// on its own. For the WASM path the product that matters is
/// `limit x WasmEngineConfig::max_memory_bytes`: the engine's ceiling bounds
/// one guest store's worst case, this limit bounds how many worst cases can
/// exist at once.
#[derive(Debug, Clone)]
pub struct ConcurrencyAdmission {
    permits: Arc<tokio::sync::Semaphore>,
    limit: AdmissionLimit,
}

impl ConcurrencyAdmission {
    /// Creates a limiter with `limit` free slots.
    ///
    /// A limit past what the underlying semaphore can represent is clamped to
    /// that maximum, which is large enough that reaching it means the host ran
    /// out of memory long before it ran out of slots.
    pub fn new(limit: AdmissionLimit) -> Self {
        const MAX_PERMITS: NonZeroUsize = match NonZeroUsize::new(tokio::sync::Semaphore::MAX_PERMITS) {
            Some(permits) => permits,
            None => NonZeroUsize::MIN,
        };

        let limit = AdmissionLimit::new(limit.as_non_zero().min(MAX_PERMITS));
        Self {
            permits: Arc::new(tokio::sync::Semaphore::new(limit.as_usize())),
            limit,
        }
    }

    /// Returns the concurrency this limiter enforces.
    pub const fn limit(&self) -> AdmissionLimit {
        self.limit
    }

    /// Returns how many executions may still be admitted right now.
    pub fn available(&self) -> usize {
        self.permits.available_permits()
    }

    /// Returns how many executions currently hold a permit.
    pub fn in_flight(&self) -> usize {
        self.limit.as_usize().saturating_sub(self.available())
    }
}

impl CommandAdmission for ConcurrencyAdmission {
    type Permit = tokio::sync::OwnedSemaphorePermit;

    fn admit(&self) -> Result<Self::Permit, OverloadedError> {
        Arc::clone(&self.permits)
            .try_acquire_owned()
            .map_err(|_| OverloadedError::new(self.limit))
    }
}

#[cfg(test)]
mod tests;
