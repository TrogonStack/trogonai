use trogon_sandbox_contract::{CapabilityRequirements, Digest, ProviderCapabilityProfile, ProviderKind};

use super::heartbeat_selection::heartbeat_has_cached_digest;
use super::{
    ProviderSelection, ProviderSelectionError, WorkerRegistry, WorkerSelectionConstraints, prefers_targeted_dispatch,
    requires_targeted_dispatch,
};

fn supports_snapshot_formats(profile: &ProviderCapabilityProfile, constraints: WorkerSelectionConstraints<'_>) -> bool {
    (!constraints.native_snapshot_input
        || (profile.snapshot.supports_native_snapshots.is_supported() && profile.snapshot.can_restore.is_supported()))
        && constraints.snapshot_input.is_none_or(|artifact| {
            profile.snapshot.can_restore.is_supported() && profile.snapshot.supported_artifact_types.contains(&artifact)
        })
        && constraints.snapshot_output.is_none_or(|artifact| {
            profile.snapshot.can_snapshot.is_supported()
                && profile.snapshot.supported_artifact_types.contains(&artifact)
        })
}

/// True when the registry knows live workers for the preferred providers and
/// none of them can both restore and export `artifact`. Automatic named
/// persistence rewrites the image to a snapshot tag, which such a provider
/// rejects, so the broker skips the rewrite instead. With no live worker known
/// for any preferred provider the answer is false and the caller keeps its
/// default behaviour.
pub async fn lacks_snapshot_persistence(
    registry: &dyn WorkerRegistry,
    provider_pref: &[ProviderKind],
    artifact: trogon_sandbox_contract::ArtifactType,
) -> bool {
    let constraints = WorkerSelectionConstraints {
        snapshot_input: Some(artifact),
        snapshot_output: Some(artifact),
        ..Default::default()
    };
    let mut known = false;
    for provider in provider_pref {
        let workers = registry.eligible_for(*provider, None).await;
        if workers.is_empty() {
            continue;
        }
        known = true;
        if workers
            .iter()
            .any(|worker| supports_snapshot_formats(&worker.profile, constraints))
        {
            return false;
        }
    }
    known
}

async fn candidate(
    registry: &dyn WorkerRegistry,
    provider: ProviderKind,
    requirements: Option<&CapabilityRequirements>,
    digest: Option<&Digest>,
    cached_only: bool,
    constraints: WorkerSelectionConstraints<'_>,
) -> Option<ProviderSelection> {
    let optional_targeted = prefers_targeted_dispatch(provider);
    if requires_targeted_dispatch(provider) || optional_targeted {
        let mut workers = registry.eligible_for(provider, requirements).await;
        workers.retain(|worker| {
            worker.dispatch_protocol == trogon_sandbox_contract::WorkerDispatchProtocol::TargetedV1
                && (!optional_targeted || worker.supports_lease_epoch.is_supported())
                && supports_snapshot_formats(&worker.profile, constraints)
                && constraints.native_volume_bindings.iter().all(|required| {
                    required.storage.provider == provider && worker.native_volume_bindings.contains(required)
                })
                && digest.is_none_or(|digest| {
                    worker.profile.snapshot.can_restore.is_supported()
                        && (!cached_only || heartbeat_has_cached_digest(worker, digest))
                })
        });
        workers.sort_by(|left, right| {
            (u64::from(left.capacity.current_leases) * u64::from(right.capacity.max_concurrent_leases))
                .cmp(&(u64::from(right.capacity.current_leases) * u64::from(left.capacity.max_concurrent_leases)))
                .then_with(|| left.worker_id.cmp(&right.worker_id))
        });
        let selected = workers.first().map(|worker| ProviderSelection {
            provider,
            locality_hit: cached_only,
            worker_id: Some(worker.worker_id.clone()),
        });
        if selected.is_some() || !optional_targeted {
            return selected;
        }
        let legacy = registry.eligible_for(provider, requirements).await;
        let available = constraints.native_volume_bindings.is_empty()
            && legacy.iter().any(|worker| {
                worker.dispatch_protocol == trogon_sandbox_contract::WorkerDispatchProtocol::Legacy
                    && supports_snapshot_formats(&worker.profile, constraints)
                    && digest.is_none_or(|digest| {
                        worker.profile.snapshot.can_restore.is_supported()
                            && (!cached_only || heartbeat_has_cached_digest(worker, digest))
                    })
            });
        return available.then_some(ProviderSelection {
            provider,
            locality_hit: cached_only,
            worker_id: None,
        });
    }
    if !constraints.native_volume_bindings.is_empty() {
        return None;
    }
    if constraints.snapshot_input.is_some()
        || constraints.snapshot_output.is_some()
        || constraints.native_snapshot_input
    {
        let workers = registry.eligible_for(provider, requirements).await;
        if !workers.iter().any(|worker| {
            supports_snapshot_formats(&worker.profile, constraints)
                && digest.is_none_or(|digest| {
                    worker.profile.snapshot.can_restore.is_supported()
                        && (!cached_only || heartbeat_has_cached_digest(worker, digest))
                })
        }) {
            return None;
        }
    }
    if digest.is_some() && !registry.any_can_restore(provider, requirements).await {
        return None;
    }
    let available = if cached_only {
        registry
            .any_eligible_with_cached_digest(provider, requirements, digest?)
            .await
    } else {
        registry.any_eligible(provider, requirements).await
    };
    available.then_some(ProviderSelection {
        provider,
        locality_hit: cached_only,
        worker_id: None,
    })
}

/// Walk `provider_pref` with optional snapshot-locality preference (two-pass).
pub async fn select_provider(
    registry: &dyn WorkerRegistry,
    provider_pref: &[ProviderKind],
    requirements: Option<&CapabilityRequirements>,
    snapshot_digest: Option<&Digest>,
) -> Result<ProviderSelection, ProviderSelectionError> {
    select_provider_with_constraints(
        registry,
        provider_pref,
        requirements,
        snapshot_digest,
        WorkerSelectionConstraints::default(),
    )
    .await
}

pub async fn select_provider_with_constraints(
    registry: &dyn WorkerRegistry,
    provider_pref: &[ProviderKind],
    requirements: Option<&CapabilityRequirements>,
    snapshot_digest: Option<&Digest>,
    constraints: WorkerSelectionConstraints<'_>,
) -> Result<ProviderSelection, ProviderSelectionError> {
    if let Some(digest) = snapshot_digest {
        let mut any_restore = false;
        for k in provider_pref {
            if candidate(registry, *k, requirements, Some(digest), false, constraints)
                .await
                .is_some()
            {
                any_restore = true;
                break;
            }
        }
        if !any_restore {
            return Err(ProviderSelectionError::NoWorkerCanRestoreSnapshot(digest.clone()));
        }

        for k in provider_pref {
            if let Some(selection) = candidate(registry, *k, requirements, Some(digest), true, constraints).await {
                return Ok(selection);
            }
        }
    }

    for k in provider_pref {
        if let Some(selection) = candidate(registry, *k, requirements, snapshot_digest, false, constraints).await {
            return Ok(selection);
        }
    }

    if let Some(req) = requirements {
        Err(ProviderSelectionError::NoWorkerSatisfiesCapabilities(req.clone()))
    } else {
        Err(ProviderSelectionError::NoProviderAvailable)
    }
}

/// Region-aware selection. Each region of `regions` is tried in order (the
/// preferred region first, then the fallbacks) and the first that has an
/// eligible worker wins; the returned region is the one that matched. When
/// `regions` is empty this is plain [`select_provider_with_constraints`] and the
/// region is `None`. When none match, the request is retried without a region:
/// success there means the only obstacle was placement, reported as
/// [`ProviderSelectionError::NoWorkerInRegion`]; failure returns that
/// region-free error, which names the real blocker.
pub async fn select_provider_in_regions(
    registry: &dyn WorkerRegistry,
    provider_pref: &[ProviderKind],
    requirements: Option<&CapabilityRequirements>,
    snapshot_digest: Option<&Digest>,
    constraints: WorkerSelectionConstraints<'_>,
    regions: &[String],
) -> Result<(ProviderSelection, Option<String>), ProviderSelectionError> {
    if regions.is_empty() {
        return select_provider_with_constraints(registry, provider_pref, requirements, snapshot_digest, constraints)
            .await
            .map(|selection| (selection, None));
    }
    for region in regions {
        let mut regional = requirements.cloned().unwrap_or_default();
        regional.region = Some(region.clone());
        if let Ok(selection) =
            select_provider_with_constraints(registry, provider_pref, Some(&regional), snapshot_digest, constraints)
                .await
        {
            return Ok((selection, Some(region.clone())));
        }
    }
    select_provider_with_constraints(registry, provider_pref, requirements, snapshot_digest, constraints).await?;
    Err(ProviderSelectionError::NoWorkerInRegion(regions.to_vec()))
}

pub async fn select_provider_for_native_volumes(
    registry: &dyn WorkerRegistry,
    provider_pref: &[ProviderKind],
    requirements: Option<&CapabilityRequirements>,
    snapshot_digest: Option<&Digest>,
    required_native_bindings: &[trogon_sandbox_contract::NativeVolumeBinding],
) -> Result<ProviderSelection, ProviderSelectionError> {
    select_provider_with_constraints(
        registry,
        provider_pref,
        requirements,
        snapshot_digest,
        WorkerSelectionConstraints {
            native_volume_bindings: required_native_bindings,
            ..Default::default()
        },
    )
    .await
}
