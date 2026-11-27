//! Verification of signed per-lease verbs and cleanup requests.
//!
//! Their bodies are typed requests that must reach the worker unchanged, so the envelope travels in a NATS header
//! ([`AUTH_HEADER`]) instead of the body. It is the same [`DispatchAuth`] the lease dispatch carries, with the full
//! subject as its operation (which names the verb, the provider, the lease or the worker), a unique nonce, and a
//! digest of the body bytes.

use std::sync::atomic::Ordering;
use std::time::SystemTime;

use async_nats::HeaderMap;
use serde_json::Value;
use trogon_sandbox_contract::{
    CancelOwnedLeaseRequest, DispatchAttemptId, DispatchAuth, LeaseId, NativeSnapshotDeleteRequest,
};

use crate::constants::MAX_NONCES;
use crate::constants::{AUTH_HEADER, CLOCK_SKEW, TRANSFER_HEADER};
use crate::dispatch_envelope::{pointer_str, sha256_hex, unix};
use crate::subjects::provider_token;
use crate::{
    DispatchAuthError, DispatchMalformedError, DispatchVerifier, EnforcementMode, SignedSubject, VerbScope,
    VerifiedVerbScope, classify_subject,
};

fn json_error(error: serde_json::Error) -> DispatchAuthError {
    DispatchMalformedError::Json(error).into()
}

fn body_value(payload: &[u8]) -> Result<Value, DispatchAuthError> {
    serde_json::from_slice(payload).map_err(json_error)
}

impl DispatchVerifier {
    /// Checks the signed request on `subject`: signature, key, expiry, subject, body, lease, then the tenant and
    /// attempt fence and the replay check. `transferred` is true when the body is a transfer descriptor, in which
    /// case the body digest cannot be compared.
    pub fn verify_verb(
        &self,
        subject: &str,
        header: &str,
        payload: &[u8],
        transferred: bool,
        now: SystemTime,
    ) -> Result<(), DispatchAuthError> {
        self.verify_verb_scope(subject, header, payload, transferred, now)
            .map(|_| ())
    }

    /// [`verify_verb`](Self::verify_verb), returning what the signature bound the request to.
    pub fn verify_verb_scope(
        &self,
        subject: &str,
        header: &str,
        payload: &[u8],
        transferred: bool,
        now: SystemTime,
    ) -> Result<VerifiedVerbScope, DispatchAuthError> {
        let kind = classify_subject(subject).ok_or(DispatchAuthError::Missing)?;
        let auth: DispatchAuth = serde_json::from_str(header).map_err(json_error)?;
        self.check_envelope(&auth, now)?;
        if auth.operation != subject {
            return Err(DispatchAuthError::WrongOperation {
                expected: subject.to_owned(),
                found: auth.operation,
            });
        }
        let nonce = auth.nonce.clone().ok_or(DispatchMalformedError::Nonce)?;
        if !transferred && auth.body_sha256.as_str() != sha256_hex(payload) {
            return Err(DispatchAuthError::BodyMismatch);
        }
        self.check_kind(&kind, subject, &auth, payload, transferred)?;
        self.note_nonce(&nonce, auth.expires_at, now)?;
        Ok(VerifiedVerbScope {
            scope: VerbScope {
                tenant: auth.tenant,
                lease_id: auth.lease_id,
                worker_id: auth.worker_id,
                attempt_id: auth.attempt_id,
                epoch: auth.epoch,
            },
            subject: auth.operation,
            body_sha256: auth.body_sha256.as_str().to_owned(),
            expires_at: auth.expires_at,
        })
    }

    fn check_verb_fence(&self, lease: &LeaseId, auth: &DispatchAuth) -> Result<(), DispatchAuthError> {
        self.fence().check_verb(
            lease,
            &auth.tenant,
            auth.attempt_id.as_ref(),
            auth.epoch,
            self.epoch_policy(),
        )
    }

    fn check_kind(
        &self,
        kind: &SignedSubject,
        subject: &str,
        auth: &DispatchAuth,
        payload: &[u8],
        transferred: bool,
    ) -> Result<(), DispatchAuthError> {
        match kind {
            SignedSubject::WorkerCommandReadiness { worker, lease, .. } => {
                if transferred {
                    return Err(DispatchMalformedError::ReadinessTransfer.into());
                }
                if &auth.lease_id != lease {
                    return Err(DispatchAuthError::LeaseMismatch);
                }
                if auth.worker_id.as_ref() != Some(worker) {
                    return Err(DispatchAuthError::WrongWorker {
                        expected: worker.as_str().to_owned(),
                        found: auth
                            .worker_id
                            .as_ref()
                            .map(|found| found.as_str().to_owned())
                            .unwrap_or_default(),
                    });
                }
                self.check_verb_fence(lease, auth)
            }
            SignedSubject::NativeSnapshotDelete { provider } => {
                if transferred {
                    return Err(DispatchMalformedError::NativeSnapshotDeleteTransfer.into());
                }
                let request: NativeSnapshotDeleteRequest = serde_json::from_slice(payload).map_err(json_error)?;
                if request.tenant != auth.tenant {
                    return Err(DispatchAuthError::TenantMismatch);
                }
                if provider_token(request.reference.provider) != provider {
                    return Err(DispatchMalformedError::NativeSnapshotProvider.into());
                }
                if !auth.lease_id.as_str().is_empty()
                    || auth.attempt_id.is_some()
                    || auth.epoch.is_some()
                    || auth.worker_id.is_some()
                {
                    return Err(DispatchAuthError::LeaseMismatch);
                }
                Ok(())
            }
            SignedSubject::LeaseVerb { lease } => {
                if &auth.lease_id != lease {
                    return Err(DispatchAuthError::LeaseMismatch);
                }
                if subject.split('.').nth(3) == Some("cancel_owned") {
                    if transferred {
                        return Err(DispatchMalformedError::OwnedCancellationTransfer.into());
                    }
                    let request: CancelOwnedLeaseRequest = serde_json::from_slice(payload).map_err(json_error)?;
                    if request.tenant != auth.tenant {
                        return Err(DispatchAuthError::TenantMismatch);
                    }
                    if &request.lease_id != lease {
                        return Err(DispatchAuthError::LeaseMismatch);
                    }
                }
                self.check_verb_fence(lease, auth)
            }
            SignedSubject::Cleanup => {
                let body = body_value(payload)?;
                if pointer_str(&body, "/identity/lease_id") != Some(auth.lease_id.as_str()) {
                    return Err(DispatchAuthError::LeaseMismatch);
                }
                if pointer_str(&body, "/identity/target/attempt_id")
                    != auth.attempt_id.as_ref().map(DispatchAttemptId::as_str)
                {
                    return Err(DispatchAuthError::StaleAttempt);
                }
                self.fence().admit_cleanup(
                    &auth.lease_id,
                    &auth.tenant,
                    auth.attempt_id.as_ref(),
                    auth.epoch,
                    self.epoch_policy(),
                )
            }
            SignedSubject::SshSession { lease } => {
                if &auth.lease_id != lease {
                    return Err(DispatchAuthError::LeaseMismatch);
                }
                if !transferred && pointer_str(&body_value(payload)?, "/lease_id") != Some(lease.as_str()) {
                    return Err(DispatchAuthError::LeaseMismatch);
                }
                self.check_verb_fence(lease, auth)
            }
            SignedSubject::SnapshotExport => {
                if !transferred && pointer_str(&body_value(payload)?, "/lease_id") != Some(auth.lease_id.as_str()) {
                    return Err(DispatchAuthError::LeaseMismatch);
                }
                self.check_verb_fence(&auth.lease_id, auth)
            }
            SignedSubject::PurgeNamed => {
                if !transferred && pointer_str(&body_value(payload)?, "/tenant") != Some(auth.tenant.as_str()) {
                    return Err(DispatchAuthError::TenantMismatch);
                }
                Ok(())
            }
        }
    }

    /// Initial authentication admits unknown leases for typed worker refusal; a later readiness check instead
    /// requires an exact existing live fence. Callers also require independent `fresh_command_launch_fence`
    /// journal health checks before and after any provider probe.
    pub fn recheck_command_readiness_scope(
        &self,
        verified: &VerifiedVerbScope,
        now: SystemTime,
    ) -> Result<(), DispatchAuthError> {
        if unix(now) > verified.expires_at.saturating_add(CLOCK_SKEW.as_secs()) {
            return Err(DispatchAuthError::Expired);
        }
        let Some(SignedSubject::WorkerCommandReadiness { worker, lease, .. }) = classify_subject(&verified.subject)
        else {
            return Err(DispatchMalformedError::CapturedScopeNotReadiness.into());
        };
        let scope = verified.scope();
        if scope.lease_id != lease {
            return Err(DispatchAuthError::LeaseMismatch);
        }
        if scope.worker_id.as_ref() != Some(&worker) {
            return Err(DispatchAuthError::WrongWorker {
                expected: worker.as_str().to_owned(),
                found: scope
                    .worker_id
                    .as_ref()
                    .map(|found| found.as_str().to_owned())
                    .unwrap_or_default(),
            });
        }
        let attempt = scope.attempt_id.as_ref().ok_or(DispatchAuthError::StaleAttempt)?;
        let epoch = scope.epoch.ok_or(DispatchAuthError::StaleEpoch)?;
        self.fence()
            .check_command_readiness(&lease, &scope.tenant, &worker, attempt, epoch)
    }

    pub(crate) fn note_nonce(&self, nonce: &str, expires_at: u64, now: SystemTime) -> Result<(), DispatchAuthError> {
        let mut seen = self.nonces.lock().unwrap_or_else(|error| error.into_inner());
        if seen.contains_key(nonce) {
            return Err(DispatchAuthError::Replayed);
        }
        if seen.len() >= MAX_NONCES {
            let cutoff = unix(now).saturating_sub(CLOCK_SKEW.as_secs());
            seen.retain(|_, expiry| *expiry >= cutoff);
        }
        seen.insert(nonce.to_owned(), expires_at);
        Ok(())
    }

    fn passes_unverified(&self) -> bool {
        self.mode == EnforcementMode::Off || (self.keys.is_empty() && self.mode == EnforcementMode::Warn)
    }

    /// Checks the reassembled body of a chunked transfer against the digest its signed envelope carries. The
    /// envelope was already verified on the descriptor, so this is the only place the chunk content is bound.
    pub fn check_reassembled(&self, headers: &HeaderMap, body: &[u8]) -> Result<(), DispatchAuthError> {
        if self.passes_unverified() {
            return Ok(());
        }
        let Some(header) = headers.get(AUTH_HEADER) else {
            return Ok(());
        };
        let result = serde_json::from_str::<DispatchAuth>(header.as_str())
            .map_err(json_error)
            .and_then(|auth| {
                if auth.body_sha256.as_str() == sha256_hex(body) {
                    Ok(())
                } else {
                    Err(DispatchAuthError::BodyMismatch)
                }
            });
        match result {
            Err(error) if self.mode == EnforcementMode::Require => {
                tracing::error!(%error, "refusing chunked body that does not match its signature");
                Err(error)
            }
            Err(error) => {
                tracing::warn!(%error, "accepting chunked body that does not match its signature (warn mode)");
                Ok(())
            }
            Ok(()) => Ok(()),
        }
    }

    /// Applies the enforcement mode to one received message. `Ok` means the message may be handled; `Err` is only
    /// returned in `require` mode.
    pub fn admit_message(
        &self,
        subject: &str,
        headers: Option<&HeaderMap>,
        payload: &[u8],
    ) -> Result<(), DispatchAuthError> {
        self.admit_message_scope(subject, headers, payload).map(|_| ())
    }

    /// [`admit_message`](Self::admit_message), returning what the signature bound the request to when the message
    /// was verified.
    pub fn admit_message_scope(
        &self,
        subject: &str,
        headers: Option<&HeaderMap>,
        payload: &[u8],
    ) -> Result<Option<VerifiedVerbScope>, DispatchAuthError> {
        if self.mode == EnforcementMode::Off || classify_subject(subject).is_none() {
            return Ok(None);
        }
        if self.keys.is_empty() && self.mode == EnforcementMode::Warn {
            if !self.warned_unconfigured.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    "dispatch verification is in warn mode with no public keys; requests are accepted unverified"
                );
            }
            return Ok(None);
        }
        let transferred = headers.is_some_and(|headers| headers.get(TRANSFER_HEADER).is_some());
        let result = match headers.and_then(|headers| headers.get(AUTH_HEADER)) {
            Some(value) => self.verify_verb_scope(subject, value.as_str(), payload, transferred, SystemTime::now()),
            None => Err(DispatchAuthError::Missing),
        };
        match result {
            Ok(scope) => Ok(Some(scope)),
            Err(error) if self.mode == EnforcementMode::Require => {
                tracing::error!(%error, subject, "refusing request that failed verification");
                Err(error)
            }
            Err(error) => {
                tracing::warn!(%error, subject, "accepting request that failed verification (warn mode)");
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
#[path = "dispatch_verifier_nonce_tests.rs"]
mod nonce_tests;

#[cfg(test)]
mod tests;
