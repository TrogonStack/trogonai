use std::num::NonZeroU64;
use std::sync::{Arc, Mutex, Weak};
use std::time::SystemTime;

use rustls::pki_types::CertificateDer;
use rustls::{ClientConfig, ServerConfig};
use spiffe::SpiffeId;
use tokio::sync::watch;
use tokio::time::Instant;

use crate::candidate::{Candidate, leaf_profile};
use crate::constants::MAX_REGISTRATION_LIFETIME;
use crate::{
    ActivationAudit, ActivationFact, ApplicationId, IdentityConfig, MaterialError, RegistrationError, RegistrationSet,
};

#[derive(Clone)]
pub struct IdentityStore {
    pub(crate) inner: Arc<Inner>,
}

pub(crate) struct Inner {
    config: IdentityConfig,
    pub audit: Arc<dyn ActivationAudit>,
    state: Mutex<GateState>,
    pub events: watch::Sender<u64>,
}

#[derive(Default)]
struct GateState {
    epoch: u64,
    source_revision: u64,
    activation_revision: u64,
    minimum_registration: u64,
    registration: Option<RegistrationLease>,
    high_watermark: Option<RegistrationSet>,
    candidate: Option<Arc<Candidate>>,
    active: Option<Arc<ApprovedMaterial>>,
    exhausted: bool,
}

#[derive(Clone)]
struct RegistrationLease {
    registration: RegistrationSet,
    deadline: Instant,
}

struct ApprovedMaterial {
    fact: ActivationFact,
    registration: RegistrationSet,
    candidate: Arc<Candidate>,
    tls: Arc<ServerConfig>,
    deadline: Instant,
}

pub(crate) struct PendingActivation {
    observation: u64,
    material: Arc<ApprovedMaterial>,
}

#[derive(Clone)]
pub(crate) struct MaterialAuthority {
    inner: Weak<Inner>,
    activation_revision: NonZeroU64,
}

impl std::fmt::Debug for MaterialAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MaterialAuthority")
            .field("activation_revision", &self.activation_revision)
            .finish()
    }
}

impl MaterialAuthority {
    fn new(store: &IdentityStore, activation_revision: NonZeroU64) -> Self {
        Self {
            inner: Arc::downgrade(&store.inner),
            activation_revision,
        }
    }

    pub fn require_current(&self) -> Result<(), rustls::Error> {
        if self.inner.upgrade().is_some_and(|inner| {
            let store = IdentityStore { inner };
            store
                .material()
                .is_some_and(|lease| lease.fact().activation_revision() == self.activation_revision)
        }) {
            Ok(())
        } else {
            Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ))
        }
    }
}

impl PendingActivation {
    pub fn fact(&self) -> &ActivationFact {
        &self.material.fact
    }
}

#[derive(Clone)]
pub struct MaterialLease {
    store: IdentityStore,
    material: Arc<ApprovedMaterial>,
}

impl IdentityStore {
    pub fn new(config: IdentityConfig, audit: Arc<dyn ActivationAudit>) -> (Self, crate::IdentityDriver) {
        let (events, _) = watch::channel(0);
        let store = Self {
            inner: Arc::new(Inner {
                config,
                audit,
                state: Mutex::new(GateState::default()),
                events,
            }),
        };
        let driver = crate::IdentityDriver::new(store.clone());
        (store, driver)
    }
    pub fn config(&self) -> &IdentityConfig {
        &self.inner.config
    }

    pub fn replace_registration(&self, registration: RegistrationSet) -> Result<(), RegistrationError> {
        let now = SystemTime::now();
        let remaining = match registration.expires_at().duration_since(now) {
            Ok(remaining) => remaining,
            Err(_) => {
                self.withdraw_registration();
                return Err(RegistrationError::Freshness);
            }
        };
        if remaining.is_zero() || remaining > MAX_REGISTRATION_LIFETIME {
            self.withdraw_registration();
            return Err(RegistrationError::Freshness);
        }
        let local = &self.config().local;
        if registration.recovery_fence() != self.config().recovery_fence
            || !registration.bindings().contains(local)
            || registration.bindings().iter().any(|binding| {
                binding.tenant() != local.tenant()
                    || binding.project() != local.project()
                    || binding.spiffe_id().trust_domain() != local.spiffe_id().trust_domain()
            })
        {
            self.withdraw_registration();
            return Err(RegistrationError::Scope);
        }
        let mut state = self.inner.state.lock().map_err(|_| RegistrationError::Unavailable)?;
        if state.exhausted {
            return Err(RegistrationError::Unavailable);
        }
        if let Some(previous) = &state.high_watermark {
            let same = previous == &registration;
            let invalid = registration.registration_revision() < previous.registration_revision()
                || registration.trust_revision() < previous.trust_revision()
                || (registration.registration_revision() == previous.registration_revision()
                    && (!same || state.registration.is_none()))
                || (registration.approved_roots() != previous.approved_roots()
                    && registration.trust_revision() <= previous.trust_revision());
            if invalid {
                state.registration = None;
                state.active = None;
                self.signal(&mut state);
                return Err(RegistrationError::Revision);
            }
            if same {
                return Ok(());
            }
        }
        state.high_watermark = Some(registration.clone());
        state.registration = Some(RegistrationLease {
            registration,
            deadline: Instant::now() + remaining,
        });
        state.active = None;
        self.signal(&mut state);
        Ok(())
    }

    pub fn withdraw_registration(&self) {
        if let Ok(mut state) = self.inner.state.lock() {
            state.registration = None;
            state.active = None;
            self.signal(&mut state);
        }
    }

    pub fn material(&self) -> Option<MaterialLease> {
        let state = self.inner.state.lock().ok()?;
        let material = state.active.as_ref()?;
        if !self.current(&state, material) {
            return None;
        }
        Some(MaterialLease {
            store: self.clone(),
            material: Arc::clone(material),
        })
    }

    pub fn source_revision(&self) -> u64 {
        self.inner
            .state
            .lock()
            .map(|state| state.source_revision)
            .unwrap_or(u64::MAX)
    }

    pub(crate) fn observe(&self, candidate: Candidate) {
        if let Ok(mut state) = self.inner.state.lock() {
            let Some(revision) = state.source_revision.checked_add(1) else {
                state.exhausted = true;
                state.active = None;
                self.signal(&mut state);
                return;
            };
            state.source_revision = revision;
            if state
                .active
                .as_ref()
                .is_some_and(|active| active.candidate.root_hashes != candidate.root_hashes)
            {
                state.active = None;
            }
            state.candidate = Some(Arc::new(candidate));
            self.signal(&mut state);
        }
    }

    pub(crate) fn withdraw_source(&self) {
        if let Ok(mut state) = self.inner.state.lock() {
            if let Some(registration) = &state.registration {
                state.minimum_registration = state
                    .minimum_registration
                    .max(registration.registration.registration_revision().get());
            }
            if let Some(next) = state.source_revision.checked_add(1) {
                state.source_revision = next;
            } else {
                state.exhausted = true;
            }
            state.candidate = None;
            state.active = None;
            self.signal(&mut state);
        }
    }

    pub(crate) fn prepare(&self) -> Option<PendingActivation> {
        let mut state = self.inner.state.lock().ok()?;
        if state.exhausted {
            return None;
        }
        let registration = state.registration.as_ref()?;
        if !registration.current()
            || registration.registration.registration_revision().get() <= state.minimum_registration
        {
            return None;
        }
        let candidate = Arc::clone(state.candidate.as_ref()?);
        if !candidate.current() || registration.registration.approved_roots() != candidate.root_hashes {
            return None;
        }
        let registration = registration.clone();
        if state.active.as_ref().is_some_and(|active| {
            self.current(&state, active)
                && active.fact.source_revision() == state.source_revision
                && active.registration == registration.registration
        }) {
            return None;
        }
        let next = state.activation_revision.checked_add(1)?;
        let activation_revision = NonZeroU64::new(next)?;
        let source_revision = NonZeroU64::new(state.source_revision)?;
        state.activation_revision = next;
        let authority = MaterialAuthority::new(self, activation_revision);
        let tls = crate::tls::server_config(&candidate, &registration.registration, authority).ok()?;
        let fact = ActivationFact {
            binding: self.config().local.clone(),
            recovery_fence: registration.registration.recovery_fence(),
            registration_revision: registration.registration.registration_revision(),
            trust_revision: registration.registration.trust_revision(),
            source_revision,
            activation_revision,
            leaf_sha256: candidate.leaf_sha256,
            public_key_sha256: candidate.public_key_sha256,
            bundle_sha256: candidate.bundle_sha256,
            not_before: candidate.not_before,
            expires_at: candidate.expires_at,
        };
        let deadline = candidate.deadline.min(registration.deadline);
        Some(PendingActivation {
            observation: state.epoch,
            material: Arc::new(ApprovedMaterial {
                fact,
                registration: registration.registration,
                candidate,
                tls,
                deadline,
            }),
        })
    }

    pub(crate) fn activate(&self, pending: PendingActivation) {
        if let Ok(mut state) = self.inner.state.lock() {
            if state.epoch != pending.observation
                || state.exhausted
                || !pending.material.candidate.current()
                || pending.material.deadline <= Instant::now()
                || !state.registration.as_ref().is_some_and(|registration| {
                    registration.current() && registration.registration == pending.material.registration
                })
                || !state
                    .candidate
                    .as_ref()
                    .is_some_and(|candidate| Arc::ptr_eq(candidate, &pending.material.candidate))
            {
                return;
            }
            state.active = Some(pending.material);
            self.signal(&mut state);
        }
    }

    pub(crate) fn expire(&self) {
        if let Ok(mut state) = self.inner.state.lock()
            && state
                .active
                .as_ref()
                .is_some_and(|material| !self.current(&state, material))
        {
            state.active = None;
            self.signal(&mut state);
        }
    }

    fn current(&self, state: &GateState, material: &Arc<ApprovedMaterial>) -> bool {
        !state.exhausted
            && state
                .active
                .as_ref()
                .is_some_and(|active| Arc::ptr_eq(active, material))
            && material.deadline > Instant::now()
            && material.candidate.current()
            && state.registration.as_ref().is_some_and(|registration| {
                registration.current() && registration.registration == material.registration
            })
            && state
                .candidate
                .as_ref()
                .is_some_and(|candidate| candidate.root_hashes == material.candidate.root_hashes)
    }

    fn signal(&self, state: &mut GateState) {
        if let Some(epoch) = state.epoch.checked_add(1) {
            state.epoch = epoch;
        } else {
            state.exhausted = true;
            state.active = None;
        }
        self.inner.events.send_replace(state.epoch);
    }
}

impl RegistrationLease {
    fn current(&self) -> bool {
        self.deadline > Instant::now() && self.registration.expires_at() > SystemTime::now()
    }
}

impl MaterialLease {
    /// The TLS handshake must have authenticated this exact certificate chain.
    pub fn peer_evidence_from_certificates(&self, certificates: &[CertificateDer<'_>]) -> Result<crate::PeerEvidence, MaterialError> {
        Ok(crate::PeerEvidence { application_id: self.peer_identity_from_certificates(certificates)?, expires_at: self.expires_at() })
    }
    pub fn server_config(&self) -> Arc<ServerConfig> {
        Arc::clone(&self.material.tls)
    }
    pub fn client_config(&self, expected_peer: &SpiffeId) -> Result<Arc<ClientConfig>, MaterialError> {
        if !self.is_current() {
            return Err(MaterialError::Unavailable);
        }
        crate::tls::client_config(
            &self.material.candidate,
            &self.material.registration,
            expected_peer,
            MaterialAuthority::new(&self.store, self.fact().activation_revision()),
        )
    }
    pub fn is_current(&self) -> bool {
        self.store
            .inner
            .state
            .lock()
            .is_ok_and(|state| self.store.current(&state, &self.material))
    }
    pub async fn changed(&self) {
        let mut changes = self.store.inner.events.subscribe();
        loop {
            if !self.is_current() {
                return;
            }
            tokio::select! {
                result = changes.changed() => { if result.is_err() { return; } },
                () = tokio::time::sleep_until(self.deadline()) => return,
            }
        }
    }
    pub fn deadline(&self) -> Instant {
        self.material.deadline
    }
    pub fn expires_at(&self) -> SystemTime {
        self.material.fact.expires_at()
    }
    pub fn fact(&self) -> &ActivationFact {
        &self.material.fact
    }
    pub fn registration(&self) -> &RegistrationSet {
        &self.material.registration
    }
    pub fn application_for_peer(&self, peer: &SpiffeId) -> Option<ApplicationId> {
        if !self.is_current() {
            return None;
        }
        self.material
            .registration
            .bindings()
            .iter()
            .find(|binding| binding.spiffe_id() == peer)
            .and_then(|binding| binding.application_id().cloned())
    }
    /// The TLS handshake must have authenticated this exact certificate chain.
    pub fn peer_identity_from_certificates(
        &self,
        certificates: &[CertificateDer<'_>],
    ) -> Result<ApplicationId, MaterialError> {
        let leaf = certificates.first().ok_or(MaterialError::Identity)?;
        let (id, _, _) = leaf_profile(leaf, SystemTime::now())?;
        self.application_for_peer(&id).ok_or(MaterialError::Unavailable)
    }
}
