use std::time::SystemTime;

use trogon_sandbox_contract::{DispatchAuth, FileSha256};

use crate::dispatch_envelope::{sha256_digest, unix};
use crate::{DispatchSignError, DispatchSigner, VerbScope};

impl DispatchSigner {
    /// The [`AUTH_HEADER`](crate::AUTH_HEADER) value for `payload` sent to `subject`.
    pub fn sign_verb(&self, subject: &str, payload: &[u8], scope: &VerbScope) -> Result<String, DispatchSignError> {
        self.sign_verb_at(subject, payload, scope, SystemTime::now())
    }

    /// [`sign_verb`](Self::sign_verb) as of `now`.
    pub fn sign_verb_at(
        &self,
        subject: &str,
        payload: &[u8],
        scope: &VerbScope,
        now: SystemTime,
    ) -> Result<String, DispatchSignError> {
        let body_sha256: FileSha256 = sha256_digest(payload).map_err(DispatchSignError::Digest)?;
        let mut auth = DispatchAuth {
            version: 1,
            key_id: self.key_id.clone(),
            operation: subject.to_owned(),
            tenant: scope.tenant.clone(),
            lease_id: scope.lease_id.clone(),
            worker_id: scope.worker_id.clone(),
            attempt_id: scope.attempt_id.clone(),
            epoch: scope.epoch,
            nonce: Some(uuid::Uuid::new_v4().simple().to_string()),
            expires_at: unix(now + self.ttl),
            body_sha256,
            signature: String::new(),
        };
        self.seal(&mut auth);
        serde_json::to_string(&auth).map_err(DispatchSignError::Encode)
    }
}
