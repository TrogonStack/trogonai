use serde::de::DeserializeOwned;
use trogon_sandbox_contract::{LeaseError, LeaseId};
use trogon_sandbox_protocol::VerifiedVerbScope;

use super::command_owner::CommandOwner;
use super::e2b_exec::E2bExec;
use super::failure::invalid_spec;
use crate::LeaseSandbox;

impl E2bExec {
    /// A fresh keyed launch needs a dispatch fence that can be trusted, when the worker has one.
    pub(super) fn check_fresh_worker_launch(&self) -> Result<(), LeaseError> {
        if let Some(verifier) = self.worker_fence.get() {
            verifier
                .fresh_command_launch_fence()
                .map_err(|error| LeaseError::Backend {
                    message: error.to_string(),
                })?;
        }
        Ok(())
    }

    /// The assignment a verified request acts for, once the scope proves it is this very request.
    pub(super) fn command_owner<T: DeserializeOwned + PartialEq>(
        sandbox: &LeaseSandbox,
        lease: &LeaseId,
        subject: &str,
        request: &T,
        verified: &VerifiedVerbScope,
        payload: &[u8],
    ) -> Result<CommandOwner, LeaseError> {
        let same_request = serde_json::from_slice::<T>(payload).is_ok_and(|parsed| parsed == *request);
        if !verified.matches(subject, payload) || !same_request || verified.scope().lease_id != *lease {
            return Err(invalid_spec("verified command scope does not match the request"));
        }
        let scope = verified.scope();
        let owner = CommandOwner {
            tenant: scope.tenant.clone(),
            lease_id: lease.clone(),
            sandbox_id: sandbox.sandbox_id.clone(),
            worker_id: scope
                .worker_id
                .clone()
                .ok_or_else(|| invalid_spec("command custody requires an assigned worker"))?,
            attempt_id: scope
                .attempt_id
                .clone()
                .ok_or_else(|| invalid_spec("command custody requires a dispatch attempt"))?,
            epoch: scope
                .epoch
                .ok_or_else(|| invalid_spec("command custody requires an assignment epoch"))?,
        };
        owner
            .validate()
            .map_err(|_| invalid_spec("invalid command assignment authority"))?;
        Ok(owner)
    }
}
