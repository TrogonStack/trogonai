use std::collections::BTreeMap;

use serde::Serialize;
use trogon_sandbox_contract::{LeaseId, OciImageRef};

/// The lease the live host check asks for: one small, networkless, ephemeral VM of the worker's image.
#[derive(Serialize)]
pub(crate) struct SpikeLeaseWire<'a> {
    tenant: &'static str,
    provider_pref: [&'static str; 1],
    spec: SpikeSpecWire<'a>,
    ttl_seconds: u32,
    idempotency_key: &'a str,
    persistent: bool,
}

impl<'a> SpikeLeaseWire<'a> {
    pub(crate) fn new(lease: &'a LeaseId, image: &'a OciImageRef) -> Self {
        Self {
            tenant: "spike",
            provider_pref: ["proxmox"],
            spec: SpikeSpecWire {
                image: SpikeImageWire {
                    kind: "registry",
                    reference: image,
                },
                workspace: SpikeWorkspaceWire { empty: EmptyWire {} },
                cpu_seconds: 60,
                network_policy: "deny_all",
                memory_mib: 512,
            },
            ttl_seconds: 60,
            idempotency_key: lease.as_str(),
            persistent: false,
        }
    }
}

#[derive(Serialize)]
struct SpikeSpecWire<'a> {
    image: SpikeImageWire<'a>,
    workspace: SpikeWorkspaceWire,
    cpu_seconds: u32,
    network_policy: &'static str,
    memory_mib: u32,
}

#[derive(Serialize)]
struct SpikeImageWire<'a> {
    kind: &'static str,
    reference: &'a OciImageRef,
}

#[derive(Serialize)]
struct SpikeWorkspaceWire {
    empty: EmptyWire,
}

#[derive(Serialize)]
struct EmptyWire {}

/// The command the live host check runs in its guest, with a marker in its environment.
#[derive(Serialize)]
pub(crate) struct SpikeCommandWire<'a> {
    lease_id: &'a str,
    command: [&'a str; 3],
    env: BTreeMap<&'static str, &'static str>,
    timeout_seconds: u32,
}

impl<'a> SpikeCommandWire<'a> {
    pub(crate) fn new(lease: &'a LeaseId, script: &'a str) -> Self {
        Self {
            lease_id: lease.as_str(),
            command: ["/bin/sh", "-c", script],
            env: BTreeMap::from([("SPIKE_MARKER", "preserved")]),
            timeout_seconds: 10,
        }
    }
}

/// One JSON line of the live host check's report.
#[derive(Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub(crate) enum SpikeReportLine<'a> {
    Granted {
        lease_id: &'a str,
        vm_id: String,
    },
    Released {
        vm_id: u32,
        lease_id: &'a str,
        clone_to_ready_ms: u128,
        total_ms: u128,
        networkless: bool,
        exec_verified: bool,
        binary_file_bytes: usize,
        destroy_verified: bool,
    },
}
