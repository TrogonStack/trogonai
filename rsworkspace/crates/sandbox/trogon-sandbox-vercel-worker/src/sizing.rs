//! Translation of the wire machine size into Vercel's only sizing knob, whole vCPUs.
//!
//! Vercel sandboxes take `resources.vcpus` and fix memory at 2048 MiB per vCPU. There is no disk option. A request is
//! rounded up to whole vCPUs, and lifted further when the memory it asks for needs more vCPUs to be backed.

use trogon_sandbox_contract::{LeaseError, SandboxSpec};

use crate::Resources;
use crate::constants::MIB_PER_VCPU;

/// The vCPU count to request, or `None` when the lease names no size and Vercel's default applies. A size above
/// `max_vcpus` is an error: admission already refuses it from the advertised range, so reaching here means the range
/// and the config disagree.
pub fn resources_for(spec: &SandboxSpec, max_vcpus: u32) -> Result<Option<Resources>, LeaseError> {
    if spec.disk_mib().is_some() {
        return Err(LeaseError::InvalidSpec {
            reason: "Vercel sandboxes have no disk size option".to_owned(),
        });
    }
    let from_cpu = spec.vcpu_millis().map(|millis| millis.div_ceil(1000).max(1));
    let memory = spec.memory_mib().get();
    let from_memory = (memory > MIB_PER_VCPU).then(|| memory.div_ceil(MIB_PER_VCPU));
    let vcpus = match (from_cpu, from_memory) {
        (None, None) => return Ok(None),
        (cpu, memory) => cpu.unwrap_or(0).max(memory.unwrap_or(0)),
    };
    if vcpus > max_vcpus {
        return Err(LeaseError::InvalidSpec {
            reason: format!("the requested size needs {vcpus} vCPUs, above this worker's limit of {max_vcpus}"),
        });
    }
    Ok(Some(Resources { vcpus }))
}
