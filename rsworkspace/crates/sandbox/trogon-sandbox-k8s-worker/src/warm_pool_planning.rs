use std::collections::HashMap;

use trogon_sandbox_contract::{ArtifactType, Digest, OciArtifactRef};
use trogon_sandbox_providers::{ANNOTATION_SOURCE_REGISTRY, ANNOTATION_SOURCE_REPOSITORY};

use crate::WarmPoolConfig;
use crate::constants::WARM_RESTORE_SECRET_PREFIX;

/// Drops sources whose digest left the cache, then resolves `digest` to its
/// recorded source or to the fallback registry and repository.
pub(crate) fn pick_source(
    sources: &mut HashMap<Digest, OciArtifactRef>,
    cached: &[Digest],
    digest: &Digest,
    fallback_registry: &str,
    fallback_repository: &str,
) -> OciArtifactRef {
    sources.retain(|known, _| cached.contains(known));
    sources.get(digest).cloned().unwrap_or_else(|| OciArtifactRef {
        registry: fallback_registry.to_string(),
        repository: fallback_repository.to_string(),
        digest: digest.clone(),
        artifact_type: ArtifactType::SandboxRootfsV1,
    })
}

/// The artifact a surviving warm pod was built from, read from its
/// annotations; `None` for a pod created before the source was recorded.
pub(crate) fn recovered_source(
    annotations: Option<&std::collections::BTreeMap<String, String>>,
    digest: &Digest,
) -> Option<OciArtifactRef> {
    let annotations = annotations?;
    let registry = annotations.get(ANNOTATION_SOURCE_REGISTRY)?;
    let repository = annotations.get(ANNOTATION_SOURCE_REPOSITORY)?;
    if registry.is_empty() || repository.is_empty() {
        return None;
    }
    Some(OciArtifactRef {
        registry: registry.clone(),
        repository: repository.clone(),
        digest: digest.clone(),
        artifact_type: ArtifactType::SandboxRootfsV1,
    })
}

pub(crate) fn warm_pod_fits(warm_cpu: f32, warm_memory_mib: u32, vcpu_millis: Option<u32>, memory_mib: u32) -> bool {
    let cpu_fits = vcpu_millis.is_none_or(|millis| millis as f32 <= warm_cpu * 1000.0);
    cpu_fits && memory_mib <= warm_memory_mib
}

/// How many pods to create for each digest this pass. Pods that exist but are
/// still starting count as present, creation stops at the total cap, the
/// warming cap and the per-pass pace, and digests are served in the order given.
pub(crate) fn plan_refill(
    config: &WarmPoolConfig,
    digests: &[Digest],
    ready: &HashMap<Digest, u32>,
    warming: &HashMap<Digest, u32>,
) -> Vec<(Digest, u32)> {
    let mut total: u32 = ready.values().chain(warming.values()).sum();
    let mut warming_total: u32 = warming.values().sum();
    let mut budget = config.refill_per_pass;
    let mut plan = Vec::new();
    for digest in digests {
        let have = ready.get(digest).copied().unwrap_or(0) + warming.get(digest).copied().unwrap_or(0);
        let want = config
            .max_pool_size_per_digest
            .saturating_sub(have)
            .min(config.max_total_warm.saturating_sub(total))
            .min(config.max_warming.saturating_sub(warming_total))
            .min(budget);
        if want == 0 {
            continue;
        }
        total += want;
        warming_total += want;
        budget -= want;
        plan.push((digest.clone(), want));
    }
    plan
}

/// A fresh name for a warm pod's restore Secret. Object names must be
/// lowercase, and a simple UUID is.
pub(crate) fn new_warm_restore_secret_name() -> String {
    format!("{WARM_RESTORE_SECRET_PREFIX}{}", uuid::Uuid::new_v4().simple())
}

#[cfg(test)]
mod tests;
