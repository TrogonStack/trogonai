use trogon_sandbox_contract::{ProviderLease, WorkerId};

use super::super::dispatch_reply_error::DispatchReplyError;
use super::*;

impl BrokerCore {
    /// Sends a signed dispatch to a worker and decodes its reply into the
    /// receipt it granted, or the way it refused.
    pub(super) async fn request_dispatch_receipt(
        &self,
        provider: ProviderKind,
        worker_id: &WorkerId,
        payload: Bytes,
    ) -> Result<ProviderLease, DispatchReplyError> {
        let signed = self
            .sign_dispatch(payload)
            .map_err(|error| DispatchReplyError::Transport {
                message: format!("dispatch could not be signed: {error}"),
            })?;
        let reply = self
            .bus
            .request_with_timeout(
                &subjects::worker_dispatch(provider, worker_id),
                signed,
                self.config.provider_dispatch_timeout,
            )
            .await
            .map_err(|error| DispatchReplyError::Transport {
                message: error.to_string(),
            })?;
        Ok(decode_dispatch_reply(provider, &reply)?)
    }
}
