use std::io::Write;
use std::time::Instant;

use trogon_sandbox_contract::{
    LeaseId, LeaseRequest, OciImageRef, ReadFileRequest, RunCommandRequest, SandboxFileWrite, WriteFilesRequest,
};

use crate::check::ensure;
use crate::constants::{SPIKE_BODY_TIMEOUT, SPIKE_FILE_BYTES, SPIKE_PATH, SPIKE_SCRIPT};
use crate::spike_wire::{SpikeCommandWire, SpikeLeaseWire, SpikeReportLine};
use crate::{ProxmoxProvider, SpikeError, VmId};

impl ProxmoxProvider {
    /// Leases one networkless VM, runs a command, round-trips a binary file through it and destroys it again,
    /// writing one JSON line to `report` once the VM is granted and another once its removal is confirmed.
    ///
    /// The VM is destroyed whether or not the guest checks pass.
    pub async fn spike(&self, report: &mut impl Write) -> Result<(), SpikeError> {
        let lease = LeaseId::new(format!("spike-{}", uuid::Uuid::new_v4().simple()));
        let image = OciImageRef::parse_unqualified(&self.config.image);
        let request: LeaseRequest = serde_json::from_value(serde_json::to_value(SpikeLeaseWire::new(&lease, &image))?)?;
        let start = Instant::now();
        self.lease_inner(&request, &lease, None).await?;
        let ready_ms = start.elapsed().as_millis();
        let vm = self.record(&lease).await?.vm;
        let granted = serde_json::to_string(&SpikeReportLine::Granted {
            lease_id: lease.as_str(),
            vm_id: vm.to_string(),
        })?;
        let execution = match writeln!(report, "{granted}") {
            Ok(()) => tokio::time::timeout(SPIKE_BODY_TIMEOUT, self.spike_body(&lease))
                .await
                .unwrap_or(Err(SpikeError::TimedOut {
                    seconds: SPIKE_BODY_TIMEOUT.as_secs(),
                })),
            Err(error) => Err(error.into()),
        };
        self.release(&lease).await.map_err(SpikeError::Cleanup)?;
        execution?;
        self.spike_confirm_removed(vm).await?;
        let released = serde_json::to_string(&SpikeReportLine::Released {
            vm_id: u32::from(vm),
            lease_id: lease.as_str(),
            clone_to_ready_ms: ready_ms,
            total_ms: start.elapsed().as_millis(),
            networkless: true,
            exec_verified: true,
            binary_file_bytes: SPIKE_FILE_BYTES,
            destroy_verified: true,
        })?;
        writeln!(report, "{released}")?;
        Ok(())
    }

    async fn spike_body(&self, lease: &LeaseId) -> Result<(), SpikeError> {
        let command: RunCommandRequest =
            serde_json::from_value(serde_json::to_value(SpikeCommandWire::new(lease, SPIKE_SCRIPT))?)?;
        let output = self.run_command(&command).await?;
        ensure(
            output.exit_code == 7 && output.stdout == b"spike-stdout\n" && output.stderr == b"spike-stderr\n",
            SpikeError::Mismatch("guest execution, root identity, networkless state, environment or cwd did not match"),
        )?;
        let bytes: Vec<u8> = (0..SPIKE_FILE_BYTES).map(|index| (index % 256) as u8).collect();
        self.write_files(&WriteFilesRequest {
            lease_id: lease.clone(),
            files: vec![SandboxFileWrite {
                path: SPIKE_PATH.to_owned(),
                content: bytes.clone(),
                mode: Some(0o600),
            }],
            as_user: None,
        })
        .await?;
        let read = self
            .read_file(&ReadFileRequest {
                lease_id: lease.clone(),
                path: SPIKE_PATH.to_owned(),
                as_user: None,
            })
            .await?;
        ensure(
            read == bytes,
            SpikeError::Mismatch("guest binary file roundtrip differed"),
        )
    }

    async fn spike_confirm_removed(&self, vm: VmId) -> Result<(), SpikeError> {
        let present = self.api.config_if_present(vm).await?;
        ensure(
            present.is_none(),
            SpikeError::Mismatch("clone still exists after cancellation"),
        )
    }
}
