use super::*;
use trogon_sandbox_contract::{CancelCommandLaunchRequest, CancelCommandLaunchResponse};

impl BrokerCore {
    pub async fn handle_cancel_command_launch(
        &self,
        request: CancelCommandLaunchRequest,
    ) -> CancelCommandLaunchResponse {
        let record = match self.active_leases.get(&request.lease_id).await {
            Ok(Some(record)) => record,
            Ok(None) => {
                return CancelCommandLaunchResponse::Denied(LeaseError::LeaseNotFound {
                    lease_id: request.lease_id,
                });
            }
            Err(_) => {
                return CancelCommandLaunchResponse::Pending {
                    message: "the authoritative lease record is unavailable".into(),
                };
            }
        };
        if record.status != LeaseStatus::Running {
            return CancelCommandLaunchResponse::Denied(LeaseError::LeaseNotRunning {
                lease_id: request.lease_id,
            });
        }
        let currently_supported = self.command_launch_cancellation_supported(&record).await;
        let retained = if self.command_launches.cancellation_custody_support() == CancellationCustodySupport::Retained {
            match self
                .command_launches
                .cancellation_authority(&request.lease_id, &request.launch_key)
                .await
            {
                Ok(Some(authority)) if authority.matches_record(&record) => Some(authority),
                Ok(Some(_)) => return CancelCommandLaunchResponse::Unsupported,
                Ok(None) => None,
                Err(_) => {
                    return CancelCommandLaunchResponse::Pending {
                        message: "retained command cancellation authority is unavailable".into(),
                    };
                }
            }
        } else {
            None
        };
        if !currently_supported && retained.is_none() {
            return CancelCommandLaunchResponse::Unsupported;
        }
        if let Some(authority) = &retained {
            match self.active_leases.get(&request.lease_id).await {
                Ok(Some(current)) if current.status == LeaseStatus::Running && authority.matches_record(&current) => {}
                Ok(_) => return CancelCommandLaunchResponse::Unsupported,
                Err(_) => {
                    return CancelCommandLaunchResponse::Pending {
                        message: "the retained command assignment is unavailable".into(),
                    };
                }
            }
        }
        let payload = match serde_json::to_vec(&request) {
            Ok(payload) => Bytes::from(payload),
            Err(_) => {
                return CancelCommandLaunchResponse::Denied(LeaseError::InvalidSpec {
                    reason: "invalid command launch cancellation request".into(),
                });
            }
        };
        let subject =
            trogon_sandbox_protocol::subjects::provider_cancel_command_launch(record.provider, &request.lease_id);
        let scope = retained
            .as_ref()
            .map(CommandCancellationAuthority::scope)
            .or_else(|| Self::command_verb_scope(&record));
        let sent = match (self.dispatch_signer.as_ref(), scope.as_ref()) {
            (Some(signer), Some(scope)) => {
                let Ok(auth) = signer.sign_verb(&subject, &payload, scope) else {
                    return CancelCommandLaunchResponse::Pending {
                        message: "command cancellation request could not be signed".into(),
                    };
                };
                self.bus
                    .request_with_auth(&subject, &auth, payload, Some(Duration::from_secs(5)))
                    .await
            }
            _ if currently_supported => {
                self.bus
                    .request_with_timeout(&subject, payload, Duration::from_secs(5))
                    .await
            }
            _ => {
                return CancelCommandLaunchResponse::Pending {
                    message: "retained command cancellation requires exact signed authority".into(),
                };
            }
        };
        let response = match sent {
            Ok(response) => response,
            Err(_) => {
                return CancelCommandLaunchResponse::Pending {
                    message: "worker cancellation acknowledgement is unavailable".into(),
                };
            }
        };
        match serde_json::from_slice::<CancelCommandLaunchResponse>(&response) {
            Ok(response) => response,
            Err(_) => CancelCommandLaunchResponse::Pending {
                message: "worker cancellation acknowledgement could not be validated".into(),
            },
        }
    }
}
