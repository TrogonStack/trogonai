use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, ensure};
use bollard::models::ContainerInspectResponse;
use trogon_sandbox_contract::{Digest, LeaseId};

use super::{ActiveDockerLease, DockerWorkerProvider};
use crate::warm_pool::WarmContainer;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Birth {
    image: String,
    init_pid: i64,
    started_at: String,
}

impl Birth {
    fn inspect(known: &KnownAllocation, actual: &ContainerInspectResponse) -> anyhow::Result<Self> {
        ensure!(
            actual.id.as_deref() == Some(known.container_id.as_str())
                && actual.state.as_ref().and_then(|state| state.running) == Some(true)
                && actual
                    .config
                    .as_ref()
                    .and_then(|config| config.labels.as_ref())
                    .is_some_and(|labels| known.labels.iter().all(|(key, value)| labels.get(key) == Some(value))),
            "native birth differs from its known host allocation"
        );
        let state = actual.state.as_ref().context("native birth state missing")?;
        Ok(Self {
            image: actual
                .image
                .clone()
                .filter(|image| {
                    image.starts_with("sha256:")
                        && image.len() == 71
                        && image[7..].bytes().all(|byte| byte.is_ascii_hexdigit())
                })
                .context("native birth immutable image missing")?,
            init_pid: state
                .pid
                .filter(|pid| *pid > 0)
                .context("native birth init PID missing")?,
            started_at: state
                .started_at
                .as_ref()
                .context("native birth StartedAt missing")?
                .to_string(),
        })
    }
}

#[derive(Clone)]
struct KnownAllocation {
    container_id: String,
    labels: BTreeMap<String, String>,
    memory_mib: u32,
    cpu_bits: u32,
    snapshot: Option<Digest>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Custody {
    Held { lease: LeaseId, registration: String },
    Retiring { lease: LeaseId, registration: String },
    Pooled { token: String },
    Disposing,
}

pub(super) struct NativeCustody {
    allocation: KnownAllocation,
    birth: Option<Birth>,
    custody: Custody,
}

pub(super) type Registry = HashMap<String, NativeCustody>;

#[derive(Clone, Debug)]
pub(crate) struct PoolClaim {
    container_id: String,
    digest: Digest,
    image_ref: String,
    token: String,
}

impl NativeCustody {
    fn matches_registration(&self, lease: &LeaseId, registration: &str, retiring: bool) -> bool {
        let expected = if retiring {
            Custody::Retiring {
                lease: lease.clone(),
                registration: registration.into(),
            }
        } else {
            Custody::Held {
                lease: lease.clone(),
                registration: registration.into(),
            }
        };
        self.custody == expected
    }

    fn accepts_claim(&self, warm: &WarmContainer, memory_mib: u32, cpu: f32) -> bool {
        let Some(claim) = warm.claim() else {
            return false;
        };
        self.birth.is_some()
            && self.allocation.container_id == warm.container_id
            && claim.container_id == warm.container_id
            && claim.digest == warm.digest
            && claim.image_ref == warm.image_ref
            && self.allocation.snapshot.as_ref() == Some(&claim.digest)
            && self.allocation.memory_mib == memory_mib
            && self.allocation.cpu_bits == cpu.to_bits()
            && self.custody
                == (Custody::Pooled {
                    token: claim.token.clone(),
                })
    }
}

impl DockerWorkerProvider {
    #[expect(
        clippy::too_many_arguments,
        reason = "each argument is a distinct fact of one allocation; a bundle struct would only rename them"
    )]
    pub(super) async fn remember_created_native(
        &self,
        lease: &LeaseId,
        registration: &str,
        container_id: &str,
        labels: BTreeMap<String, String>,
        memory_mib: u32,
        cpu: f32,
        snapshot: Option<Digest>,
    ) -> anyhow::Result<()> {
        ensure!(
            container_id.len() == 64 && container_id.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "known native allocation is not canonical"
        );
        let mut registry = self.native_custody.lock().await;
        ensure!(
            !registry.contains_key(container_id),
            "native creation receipt already has custody"
        );
        registry.insert(
            container_id.into(),
            NativeCustody {
                allocation: KnownAllocation {
                    container_id: container_id.into(),
                    labels,
                    memory_mib,
                    cpu_bits: cpu.to_bits(),
                    snapshot,
                },
                birth: None,
                custody: Custody::Held {
                    lease: lease.clone(),
                    registration: registration.into(),
                },
            },
        );
        Ok(())
    }

    pub(super) async fn pin_created_native(
        &self,
        lease: &LeaseId,
        registration: &str,
        container_id: &str,
    ) -> anyhow::Result<()> {
        let known = {
            let registry = self.native_custody.lock().await;
            let retained = registry
                .get(container_id)
                .context("native allocation receipt missing")?;
            ensure!(
                retained.matches_registration(lease, registration, false),
                "native creation registration changed"
            );
            retained.allocation.clone()
        };
        let actual = self.docker.inspect_container(container_id, None).await?;
        let birth = Birth::inspect(&known, &actual)?;
        let mut registry = self.native_custody.lock().await;
        let retained = registry
            .get_mut(container_id)
            .context("native allocation receipt disappeared")?;
        ensure!(
            retained.matches_registration(lease, registration, false)
                && retained.birth.as_ref().is_none_or(|original| original == &birth),
            "native creation proof changed"
        );
        retained.birth = Some(birth);
        Ok(())
    }

    async fn verify_native_birth(
        &self,
        lease: &LeaseId,
        registration: &str,
        container_id: &str,
        retiring: bool,
    ) -> anyhow::Result<()> {
        let (known, expected) = {
            let registry = self.native_custody.lock().await;
            let retained = registry.get(container_id).context("trusted native birth missing")?;
            ensure!(
                retained.matches_registration(lease, registration, retiring),
                "native custody registration changed"
            );
            (
                retained.allocation.clone(),
                retained.birth.clone().context("trusted native birth is unavailable")?,
            )
        };
        let actual = self.docker.inspect_container(container_id, None).await?;
        ensure!(
            Birth::inspect(&known, &actual)? == expected,
            "native immutable image or init birth changed"
        );
        let registry = self.native_custody.lock().await;
        ensure!(
            registry
                .get(container_id)
                .is_some_and(|retained| retained.matches_registration(lease, registration, retiring)
                    && retained.birth.as_ref() == Some(&expected)),
            "native custody changed during inspection"
        );
        Ok(())
    }

    pub(super) async fn verify_held_native_birth(
        &self,
        lease: &LeaseId,
        held: &ActiveDockerLease,
    ) -> anyhow::Result<()> {
        self.verify_native_birth(lease, &held.registration_id, &held.container_id, false)
            .await
    }

    pub(super) async fn verify_retiring_native_birth(
        &self,
        lease: &LeaseId,
        held: &ActiveDockerLease,
    ) -> anyhow::Result<()> {
        self.verify_native_birth(lease, &held.registration_id, &held.container_id, true)
            .await
    }

    pub(super) async fn claim_warm_native(
        &self,
        warm: &WarmContainer,
        lease: &LeaseId,
        registration: &str,
        memory_mib: u32,
        cpu: f32,
    ) -> anyhow::Result<()> {
        let mut registry = self.native_custody.lock().await;
        let retained = registry
            .get_mut(&warm.container_id)
            .context("raw warm entry has no host allocation authority")?;
        ensure!(
            retained.accepts_claim(warm, memory_mib, cpu),
            "warm claim differs from trusted native custody"
        );
        retained.custody = Custody::Held {
            lease: lease.clone(),
            registration: registration.into(),
        };
        Ok(())
    }

    pub(super) async fn begin_native_retirement(
        &self,
        lease: &LeaseId,
        held: &ActiveDockerLease,
    ) -> anyhow::Result<()> {
        let mut registry = self.native_custody.lock().await;
        let retained = registry
            .get_mut(&held.container_id)
            .context("native retirement has no creation authority")?;
        ensure!(
            retained.matches_registration(lease, &held.registration_id, false)
                || retained.matches_registration(lease, &held.registration_id, true),
            "native retirement registration changed"
        );
        retained.custody = Custody::Retiring {
            lease: lease.clone(),
            registration: held.registration_id.clone(),
        };
        Ok(())
    }

    pub(super) async fn publish_warm_native(
        &self,
        lease: &LeaseId,
        held: &ActiveDockerLease,
        digest: Digest,
        image_ref: String,
    ) -> anyhow::Result<WarmContainer> {
        self.verify_native_birth(lease, &held.registration_id, &held.container_id, true)
            .await?;
        let mut registry = self.native_custody.lock().await;
        let retained = registry
            .get_mut(&held.container_id)
            .context("retiring native custody disappeared")?;
        ensure!(
            retained.matches_registration(lease, &held.registration_id, true)
                && retained.allocation.snapshot.as_ref() == Some(&digest)
                && retained.birth.is_some(),
            "native reset cannot mint another snapshot authority"
        );
        let mut active = self.active.lock().await;
        ensure!(
            active
                .get(lease.as_str())
                .is_some_and(|current| current.registration_id == held.registration_id
                    && current.container_id == held.container_id),
            "old lease registration changed before pool publication"
        );
        let claim = PoolClaim {
            container_id: held.container_id.clone(),
            digest: digest.clone(),
            image_ref: image_ref.clone(),
            token: crate::fresh_id::fresh_id(),
        };
        active.remove(lease.as_str());
        retained.custody = Custody::Pooled {
            token: claim.token.clone(),
        };
        Ok(WarmContainer::new(held.container_id.clone(), digest, image_ref).with_claim(claim))
    }

    pub(super) async fn publish_created_warm_native(
        &self,
        lease: &LeaseId,
        registration: &str,
        container_id: &str,
        digest: Digest,
        image_ref: String,
    ) -> anyhow::Result<WarmContainer> {
        self.verify_native_birth(lease, registration, container_id, false)
            .await?;
        let mut registry = self.native_custody.lock().await;
        let retained = registry
            .get_mut(container_id)
            .context("new warm allocation custody disappeared")?;
        ensure!(
            retained.matches_registration(lease, registration, false)
                && retained.allocation.snapshot.as_ref() == Some(&digest)
                && retained.birth.is_some(),
            "new warm allocation changed before publication"
        );
        let claim = PoolClaim {
            container_id: container_id.into(),
            digest: digest.clone(),
            image_ref: image_ref.clone(),
            token: crate::fresh_id::fresh_id(),
        };
        retained.custody = Custody::Pooled {
            token: claim.token.clone(),
        };
        Ok(WarmContainer::new(container_id.into(), digest, image_ref).with_claim(claim))
    }

    pub(super) async fn native_registration_admitted(
        &self,
        lease: &LeaseId,
        registration: &str,
        container_id: &str,
    ) -> bool {
        self.native_custody
            .lock()
            .await
            .get(container_id)
            .is_none_or(|retained| retained.matches_registration(lease, registration, false))
    }

    pub(super) async fn dispose_pooled_native(&self, container_id: &str) -> anyhow::Result<()> {
        let (known, expected) = {
            let mut registry = self.native_custody.lock().await;
            let retained = registry
                .get_mut(container_id)
                .context("untrusted pool entry has no disposal authority")?;
            ensure!(
                matches!(retained.custody, Custody::Pooled { .. } | Custody::Disposing),
                "pool disposal cannot act on a current lease"
            );
            retained.custody = Custody::Disposing;
            (
                retained.allocation.clone(),
                retained
                    .birth
                    .clone()
                    .context("pool disposal has no trusted birth proof")?,
            )
        };
        let actual = match self.docker.inspect_container(container_id, None).await {
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => {
                self.native_custody.lock().await.remove(container_id);
                return Ok(());
            }
            result => result?,
        };
        ensure!(
            Birth::inspect(&known, &actual)? == expected,
            "pool disposal birth changed"
        );
        self.inner.engine().remove(container_id).await?;
        self.confirm_container_absence(container_id).await?;
        self.native_custody.lock().await.remove(container_id);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
