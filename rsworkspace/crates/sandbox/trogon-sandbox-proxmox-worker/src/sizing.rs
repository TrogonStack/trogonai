/// How a requested vCPU share maps onto Proxmox's `cores` and `cpulimit`.
#[derive(Debug, PartialEq)]
pub(crate) struct CpuConfig {
    pub(crate) cores: u32,
    pub(crate) cpulimit: Option<f64>,
}

/// Whole cores cover the request, and a fractional request caps the guest at its share.
pub(crate) fn cpu_config(millis: u32) -> CpuConfig {
    let cores = millis.div_ceil(1000).max(1);
    let cpulimit = (!millis.is_multiple_of(1000)).then(|| f64::from(millis) / 1000.0);
    CpuConfig { cores, cpulimit }
}

#[cfg(test)]
mod tests;
