use std::time::SystemTime;

use trogon_sandbox_contract::{
    ProviderCleanupRequest, ProviderCleanupResponse, ProviderDispatchPayload, ProviderDispatchReply, ProviderKind,
    WorkerCapabilities, WorkerCapacity, WorkerDispatchProtocol, WorkerHeartbeat,
};
use trogon_sandbox_protocol::decode_verified;

use crate::SpaceProvider;

impl SpaceProvider {
    /// Answers one lease dispatch message. A message that cannot be verified or that does
    /// not name its target is answered with a refusal rather than dropped, so the sender
    /// does not wait out a timeout.
    pub async fn answer_dispatch(&self, payload: &[u8]) -> ProviderDispatchReply {
        let dispatch: ProviderDispatchPayload = match decode_verified(payload, Some(self.dispatch_ledger().worker_id()))
        {
            Ok(dispatch) => dispatch,
            Err(error) => {
                return ProviderDispatchReply::DispatchRejected {
                    decode_error: error.to_string(),
                };
            }
        };
        if dispatch.target.is_none() {
            return ProviderDispatchReply::LaunchFailed {
                provider_error: "targeted dispatch identity is required".into(),
            };
        }
        match self.lease_dispatch(&dispatch).await {
            Ok(lease) => ProviderDispatchReply::Granted(lease),
            Err(error) => ProviderDispatchReply::LaunchFailed {
                provider_error: error.to_string(),
            },
        }
    }

    /// Answers a cleanup request for a dispatch attempt.
    pub async fn answer_cleanup(&self, payload: &[u8]) -> Result<ProviderCleanupResponse, serde_json::Error> {
        let request: ProviderCleanupRequest = serde_json::from_slice(payload)?;
        Ok(self.cleanup_dispatch(&request).await)
    }

    /// The heartbeat the worker publishes.
    pub async fn heartbeat(&self) -> WorkerHeartbeat {
        let profile = self.profile();
        WorkerHeartbeat {
            worker_id: self.dispatch_ledger().worker_id().clone(),
            dispatch_protocol: WorkerDispatchProtocol::TargetedV1,
            native_volume_bindings: Vec::new(),
            provider: ProviderKind::Space,
            capacity: WorkerCapacity {
                max_concurrent_leases: self.config.max_leases.get(),
                current_leases: u32::try_from(self.current_leases().await).unwrap_or(u32::MAX),
                capabilities: WorkerCapabilities::from(&profile),
            },
            emitted_at: SystemTime::now(),
            cached_digests: Vec::new(),
            profile,
            warm_pool_population: Default::default(),
            supports_lease_epoch: Default::default(),
        }
    }
}
