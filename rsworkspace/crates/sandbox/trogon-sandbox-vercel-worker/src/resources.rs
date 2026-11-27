use serde::Serialize;

/// The machine size sent to Vercel: whole vCPUs, with memory fixed per vCPU.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
pub struct Resources {
    pub vcpus: u32,
}
