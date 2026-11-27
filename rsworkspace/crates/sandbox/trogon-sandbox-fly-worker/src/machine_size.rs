//! Translates the wire's machine size into a Fly Machines `guest` block.
//!
//! Fly's shared CPU kind only offers 1, 2, 4 or 8 vCPUs, and memory must be a multiple of 256 MiB between 256
//! and 2048 MiB per vCPU. A request is rounded up to the next size Fly can actually run, never down, so the
//! machine is never smaller than asked for.

use std::num::NonZeroU32;

use crate::MachineSizeError;
use crate::constants::{MAX_MIB_PER_CPU, MEMORY_STEP_MIB, MIN_MIB_PER_CPU, SHARED_CPUS};

/// A guest shape Fly accepts.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct MachineSize {
    cpus: u32,
    memory_mib: u32,
}

impl MachineSize {
    pub fn cpus(self) -> u32 {
        self.cpus
    }

    pub fn memory_mib(self) -> u32 {
        self.memory_mib
    }

    /// Picks the guest for a lease. A request that names no vCPUs keeps the default ones, widened when the
    /// memory would not fit on them.
    pub fn fit(
        vcpu_millis: Option<u32>,
        memory_mib: NonZeroU32,
        default_cpus: NonZeroU32,
    ) -> Result<Self, MachineSizeError> {
        let memory = memory_mib.get();
        let wanted_cpus = match vcpu_millis {
            Some(millis) => millis.div_ceil(1000).max(1),
            None => default_cpus.get(),
        };
        let needed_for_memory = memory.div_ceil(MAX_MIB_PER_CPU);
        let cpus = SHARED_CPUS
            .into_iter()
            .find(|cpus| *cpus >= wanted_cpus.max(needed_for_memory))
            .ok_or(MachineSizeError {
                cpus: wanted_cpus,
                memory_mib: memory,
            })?;
        let memory_mib = memory
            .div_ceil(MEMORY_STEP_MIB)
            .saturating_mul(MEMORY_STEP_MIB)
            .max(cpus * MIN_MIB_PER_CPU);
        Ok(Self { cpus, memory_mib })
    }

    pub(crate) fn exact(cpus: NonZeroU32, memory_mib: NonZeroU32) -> Self {
        Self {
            cpus: cpus.get(),
            memory_mib: memory_mib.get(),
        }
    }
}
