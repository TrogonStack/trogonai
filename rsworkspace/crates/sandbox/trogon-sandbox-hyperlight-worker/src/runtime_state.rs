use std::path::Path;
use std::sync::{Mutex, PoisonError};

use trogon_sandbox_contract::LeaseId;

use crate::{CleanupHandle, HandleStoreError, HyperlightError, Hypervisor, HypervisorError, RetainedHandles, WarmPool};

/// The guests and warm runtimes one worker keeps. Every method blocks, so callers run them on
/// the blocking thread pool.
pub(crate) struct RuntimeState<H: Hypervisor> {
    pub(crate) hypervisor: H,
    leases: RetainedHandles<H::Guest>,
    warm: Mutex<WarmPool<H::Warm>>,
}

impl<H: Hypervisor> RuntimeState<H> {
    pub(crate) fn new(hypervisor: H, max_warm: usize) -> Self {
        Self {
            hypervisor,
            leases: RetainedHandles::default(),
            warm: Mutex::new(WarmPool::new(max_warm)),
        }
    }

    pub(crate) fn leases(&self) -> &RetainedHandles<H::Guest> {
        &self.leases
    }

    fn warm(&self) -> Result<H::Warm, HypervisorError> {
        let kept = self.warm.lock().unwrap_or_else(PoisonError::into_inner).take();
        match kept {
            Some(warm) => Ok(warm),
            None => self.hypervisor.build_warm(),
        }
    }

    pub(crate) fn open_component(&self, component: &Path) -> Result<H::Guest, HypervisorError> {
        self.hypervisor.load_module(self.warm()?, component)
    }

    pub(crate) fn open_layout(&self, layout: &Path) -> Result<H::Guest, HypervisorError> {
        self.hypervisor.load_layout(self.warm()?, layout)
    }

    pub(crate) fn retain(&self, lease: &LeaseId, guest: H::Guest) -> Result<(), HandleStoreError> {
        self.leases.insert(lease, guest)
    }

    /// Releases the guest `lease` holds. With `recycle` the runtime goes back to the warm pool.
    /// Returns whether the lease held a guest.
    pub(crate) fn release(&self, lease: &LeaseId, recycle: bool) -> Result<bool, HyperlightError> {
        let Some(cleanup) = self.leases.take_for_cleanup(lease)? else {
            return Ok(false);
        };
        if recycle {
            self.recycle(cleanup)?;
        }
        Ok(true)
    }

    fn recycle(&self, cleanup: CleanupHandle<H::Guest>) -> Result<(), HypervisorError> {
        let mut failure = None;
        cleanup.dispose(|guest| match self.hypervisor.unload(guest) {
            Ok(warm) => self.warm.lock().unwrap_or_else(PoisonError::into_inner).give(warm),
            Err(error) => failure = Some(error),
        });
        failure.map_or(Ok(()), Err)
    }
}
