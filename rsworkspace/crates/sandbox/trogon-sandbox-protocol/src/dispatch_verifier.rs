use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::SystemTime;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};
use serde_json::Value;
use trogon_sandbox_contract::{DispatchAuth, DispatchTarget, LeaseId, TenantId, WorkerId};
use trogon_std::env::ReadEnv;

use crate::constants::{
    AUTH_FIELD, CLOCK_SKEW, ENV_FENCE_EPOCH, ENV_FENCE_FILE, ENV_PUBLIC_KEYS, ENV_PUBLIC_KEYS_FILE, ENV_VERIFY_MODE,
};
use crate::dispatch_envelope::{body_digest, signing_input, string_at, unix};
use crate::dispatch_fence::DispatchFence;
use crate::dispatch_fence_location::default_fence_path;
use crate::{
    DispatchAuthError, DispatchDecodeError, DispatchMalformedError, DispatchPublicKeyError,
    DispatchVerifierConfigError, EnforcementMode, EpochMode, FreshLaunchFenceError,
};

/// The worker's verifying half.
pub struct DispatchVerifier {
    pub(crate) mode: EnforcementMode,
    epoch_mode: EpochMode,
    pub(crate) keys: HashMap<String, VerifyingKey>,
    fence: Mutex<DispatchFence>,
    pub(crate) nonces: Mutex<HashMap<String, u64>>,
    pub(crate) warned_unconfigured: AtomicBool,
}

impl DispatchVerifier {
    /// A verifier with no keys and an in-memory fence.
    pub fn new(mode: EnforcementMode) -> Self {
        Self {
            mode,
            epoch_mode: EpochMode::Off,
            keys: HashMap::new(),
            fence: Mutex::new(DispatchFence::memory()),
            nonces: Mutex::new(HashMap::new()),
            warned_unconfigured: AtomicBool::new(false),
        }
    }

    /// The enforcement mode.
    pub fn mode(&self) -> EnforcementMode {
        self.mode
    }

    pub(crate) fn fence(&self) -> MutexGuard<'_, DispatchFence> {
        self.fence.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Whether the fence can guard a fresh command launch: signatures and epochs are enforced and the persistent
    /// journal is trusted and intact.
    pub fn fresh_command_launch_fence(&self) -> Result<(), FreshLaunchFenceError> {
        if self.mode != EnforcementMode::Require {
            return Err(FreshLaunchFenceError::VerificationRequired);
        }
        if self.keys.is_empty() {
            return Err(FreshLaunchFenceError::VerificationKeysUnavailable);
        }
        if self.epoch_policy() != EpochMode::Require {
            return Err(FreshLaunchFenceError::EpochRequired);
        }
        self.fence().fresh_launch_health()
    }

    /// Sets how a lower lease epoch is treated.
    pub fn with_epoch_mode(mut self, epoch_mode: EpochMode) -> Self {
        self.epoch_mode = epoch_mode;
        self
    }

    /// The epoch mode in force: `require` needs verified signatures, so it degrades to `warn` unless the verifier
    /// itself is in `require` mode.
    pub(crate) fn epoch_policy(&self) -> EpochMode {
        match self.epoch_mode {
            EpochMode::Require if self.mode != EnforcementMode::Require => EpochMode::Warn,
            mode => mode,
        }
    }

    /// Trusts the base64 Ed25519 public key for envelopes naming `key_id`.
    pub fn with_key(mut self, key_id: impl Into<String>, public_key_b64: &str) -> Result<Self, DispatchPublicKeyError> {
        let bytes = B64
            .decode(public_key_b64.trim())
            .ok()
            .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
            .ok_or(DispatchPublicKeyError::Shape)?;
        let key = VerifyingKey::from_bytes(&bytes).map_err(DispatchPublicKeyError::Invalid)?;
        self.keys.insert(key_id.into(), key);
        Ok(self)
    }

    /// Keeps the lease fence in the journal at `path`, loading what an earlier run recorded.
    pub fn with_fence_file(self, path: &Path) -> std::io::Result<Self> {
        *self.fence() = DispatchFence::open(path)?;
        Ok(self)
    }

    /// Native adoption needs ownership recovered from an independent dispatch journal.
    pub fn recovered_dispatch_tenant(&self, lease: &LeaseId, target: &DispatchTarget) -> Option<TenantId> {
        if self.mode != EnforcementMode::Require || self.keys.is_empty() {
            return None;
        }
        self.fence().recovered_dispatch_tenant(lease, target)
    }

    /// Builds the verifier from the process environment. `warn` is the default mode; `require` with no keys is an
    /// error.
    pub fn from_env(env: &impl ReadEnv) -> Result<Self, DispatchVerifierConfigError> {
        Self::from_lookup(&|name| env.var(name).ok(), default_fence_path())
    }

    /// Builds the verifier from the variables `lookup` returns. `default_fence` is used when no fence file is
    /// configured.
    pub fn from_lookup(
        lookup: &dyn Fn(&str) -> Option<String>,
        default_fence: Option<PathBuf>,
    ) -> Result<Self, DispatchVerifierConfigError> {
        let mode = match lookup(ENV_VERIFY_MODE) {
            Some(value) => EnforcementMode::parse(&value)?,
            None => EnforcementMode::Warn,
        };
        let epoch_mode = EpochMode::from_env_value(lookup(ENV_FENCE_EPOCH).as_deref())?;
        let mut verifier = Self::new(mode).with_epoch_mode(epoch_mode);
        let mut entries = String::new();
        if let Some(path) = lookup(ENV_PUBLIC_KEYS_FILE).filter(|path| !path.is_empty()) {
            entries = std::fs::read_to_string(&path)
                .map_err(|source| DispatchVerifierConfigError::KeysFile { path, source })?;
        } else if let Some(value) = lookup(ENV_PUBLIC_KEYS) {
            entries = value;
        }
        for entry in entries
            .split(['\n', ','])
            .map(str::trim)
            .filter(|entry| !entry.is_empty() && !entry.starts_with('#'))
        {
            let (id, key) = entry
                .split_once(':')
                .ok_or(DispatchVerifierConfigError::KeyEntryMalformed)?;
            verifier = verifier.with_key(id.trim(), key)?;
        }
        if mode == EnforcementMode::Require && verifier.keys.is_empty() {
            return Err(DispatchVerifierConfigError::RequireWithoutKeys);
        }
        let configured_fence = lookup(ENV_FENCE_FILE).filter(|path| !path.is_empty());
        if let Some(path) = configured_fence {
            // The operator asked for a durable fence, so a journal that cannot be opened is a startup error, never a
            // silent downgrade to a fence that forgets leases on restart.
            *verifier.fence() = DispatchFence::open(Path::new(&path))
                .map_err(|source| DispatchVerifierConfigError::FenceFile { path, source })?;
        } else if let Some(path) = default_fence {
            // The default location is best effort: a journal that cannot be read leaves the fence in memory; the
            // verifier and its keys still work.
            match DispatchFence::open(&path) {
                Ok(fence) => *verifier.fence() = fence,
                Err(error) => tracing::error!(
                    %error,
                    path = %path.display(),
                    "dispatch fence journal unavailable; the lease fence is memory only"
                ),
            }
        }
        Ok(verifier)
    }

    /// Checks version, key, signature and expiry of an envelope.
    pub(crate) fn check_envelope(&self, auth: &DispatchAuth, now: SystemTime) -> Result<(), DispatchAuthError> {
        if auth.version != 1 {
            return Err(DispatchMalformedError::UnsupportedVersion { version: auth.version }.into());
        }
        let key = self
            .keys
            .get(&auth.key_id)
            .ok_or_else(|| DispatchAuthError::UnknownKey(auth.key_id.clone()))?;
        let signature = B64
            .decode(&auth.signature)
            .ok()
            .and_then(|bytes| Signature::from_slice(&bytes).ok())
            .ok_or(DispatchMalformedError::Signature)?;
        key.verify(&signing_input(auth), &signature)
            .map_err(|_| DispatchAuthError::BadSignature)?;
        if unix(now) > auth.expires_at.saturating_add(CLOCK_SKEW.as_secs()) {
            return Err(DispatchAuthError::Expired);
        }
        Ok(())
    }

    /// Verifies a parsed dispatch and removes its envelope. On success the lease is also fenced to its tenant.
    pub fn verify_value(
        &self,
        value: &mut Value,
        operation: &str,
        own_worker: Option<&WorkerId>,
        now: SystemTime,
    ) -> Result<(), DispatchAuthError> {
        let object = value.as_object_mut().ok_or(DispatchMalformedError::NotAnObject)?;
        let auth = object.remove(AUTH_FIELD).ok_or(DispatchAuthError::Missing)?;
        let auth: DispatchAuth = serde_json::from_value(auth).map_err(DispatchMalformedError::Json)?;
        self.check_envelope(&auth, now)?;
        if auth.operation != operation {
            return Err(DispatchAuthError::WrongOperation {
                expected: operation.to_owned(),
                found: auth.operation,
            });
        }
        if let (Some(own), Some(signed)) = (own_worker, auth.worker_id.as_ref())
            && own != signed
        {
            return Err(DispatchAuthError::WrongWorker {
                expected: own.as_str().to_owned(),
                found: signed.as_str().to_owned(),
            });
        }
        if auth.body_sha256.as_str() != body_digest(object) {
            return Err(DispatchAuthError::BodyMismatch);
        }
        let body = Value::Object(object.clone());
        if string_at(&body, "/request/tenant").as_deref() != Some(auth.tenant.as_str())
            || string_at(&body, "/lease_id").as_deref() != Some(auth.lease_id.as_str())
        {
            return Err(DispatchAuthError::TenantMismatch);
        }
        self.fence().admit_dispatch(
            &auth.lease_id,
            &auth.tenant,
            auth.worker_id.as_ref(),
            auth.attempt_id.as_ref(),
            auth.epoch,
            self.epoch_policy(),
        )
    }

    /// Forgets a lease's tenant once the lease is gone, so its id can be reused.
    pub fn forget_lease(&self, lease_id: &LeaseId) {
        self.fence().forget(lease_id);
    }

    /// Decodes a dispatch, applying the enforcement mode.
    pub fn decode<T: serde::de::DeserializeOwned>(
        &self,
        payload: &[u8],
        operation: &str,
        own_worker: Option<&WorkerId>,
    ) -> Result<T, DispatchDecodeError> {
        let mut value: Value = serde_json::from_slice(payload)?;
        if self.mode != EnforcementMode::Off {
            if self.keys.is_empty() && self.mode == EnforcementMode::Warn {
                if !self.warned_unconfigured.swap(true, Ordering::Relaxed) {
                    tracing::warn!(
                        "dispatch verification is in warn mode with no public keys; dispatches are accepted unverified"
                    );
                }
            } else if let Err(error) = self.verify_value(&mut value, operation, own_worker, SystemTime::now()) {
                if self.mode == EnforcementMode::Require {
                    tracing::error!(%error, "refusing dispatch that failed verification");
                    return Err(DispatchDecodeError::Auth(error));
                }
                tracing::warn!(%error, "accepting dispatch that failed verification (warn mode)");
            }
        }
        if let Some(object) = value.as_object_mut() {
            object.remove(AUTH_FIELD);
        }
        Ok(serde_json::from_value(value)?)
    }
}
