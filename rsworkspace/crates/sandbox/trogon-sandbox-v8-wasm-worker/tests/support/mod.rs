#![allow(dead_code)]

use tokio::sync::{Semaphore, SemaphorePermit};

/// Bounds how many CPU-bound guests run at once so parallel tests do not starve each other.
static CPU_HEAVY_TEST_GUARD: Semaphore = Semaphore::const_new(3);

pub type TestResult = Result<(), Box<dyn std::error::Error>>;

pub async fn cpu_permit() -> Result<SemaphorePermit<'static>, tokio::sync::AcquireError> {
    CPU_HEAVY_TEST_GUARD.acquire().await
}

pub mod guests;
pub mod lease;
pub mod mock_provider;
