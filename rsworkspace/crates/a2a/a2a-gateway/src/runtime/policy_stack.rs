//! Boot-resolved policy stack the dispatch orchestrator consumes.
//!
//! Three independently-loadable pieces — the Tier-2/Tier-3 wasmtime
//! substrate, the Tier-3 redaction gate, and the skill manifests
//! that drive Tier-3 — live behind one typed surface so the
//! dispatch path holds a single `&GatewayPolicyStack` instead of
//! three loose `Arc<_>` values it has to keep in sync.
//!
//! Stays focused on shape only; the env-driven boot helper that
//! decides which substrate/gate/manifests to instantiate is a
//! separate concern that the runtime entry point owns.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use a2a_redaction::{SkillId, SkillIdError, WasmBundlePath};
use trogon_std::env::ReadEnv;

use crate::policy::error::PolicyError;
use crate::policy::tier2::{NoopTier2Evaluator, Tier2CelEvaluator};
use crate::policy::tier2_cel::{CelCompileError, RealTier2CelEvaluator, Tier2CompiledBundle};
use crate::policy::tier3_redaction::{
    Tier3ManifestLoadError, load_tier3_manifest_from_bundle, load_tier3_manifests_from_bundle,
};
use crate::policy::wasmtime_substrate::{Tier2State, WasmtimeSubstrate};
use crate::policy::{NoopTier3RedactionGate, RealTier3RedactionGate, Tier3RedactionGate, Tier3SkillManifest};
use crate::runtime::env::{Tier3SigningKey, gateway_tier2_cel_enabled, gateway_tier3_signing_pubkey};

use crate::constants::{
    ENV_GATEWAY_TIER2_CEL_ENABLED, ENV_GATEWAY_TIER3_SIGNING_PUBKEY, ENV_POLICY_BUNDLE_DIR, ENV_POLICY_SKILLS,
    ENV_TIER3_REDACTION_ENABLED,
};

/// The dispatch path's view of the policy stack. Each field is
/// independently-replaceable in tests:
///
/// - `substrate` is `None` when no bundle is configured; the
///   orchestrator skips the Tier-2 CEL step instead of running
///   against a Noop evaluator hidden inside a phantom substrate.
/// - `tier3_gate` is always present (Noop when redaction is
///   disabled) so the dispatch flow never has to branch on
///   "do we have a gate at all" -- only on the decision the gate
///   returns.
/// - `tier3_manifests` is keyed by `SkillId` so the orchestrator
///   can stamp per-skill rewrites into the audit envelope without
///   reaching back into the gate.
pub struct GatewayPolicyStack {
    pub substrate: Option<Arc<WasmtimeSubstrate>>,
    pub tier3_gate: Arc<dyn Tier3RedactionGate>,
    pub tier3_manifests: BTreeMap<SkillId, Tier3SkillManifest>,
}

impl GatewayPolicyStack {
    /// Stack for a deployment with no policy layer turned on, and the
    /// fixture tests use. It is never a fallback for a layer that was
    /// turned on and failed to load; boot fails instead.
    #[must_use]
    pub fn noop() -> Self {
        Self {
            substrate: None,
            tier3_gate: Arc::new(NoopTier3RedactionGate),
            tier3_manifests: BTreeMap::new(),
        }
    }
}

/// Why a policy layer the operator turned on could not be brought up.
/// Every variant is a boot failure: the gateway must not serve traffic
/// with a layer the operator believes is enforcing but is not.
#[derive(Debug, thiserror::Error)]
pub enum PolicyStackError {
    #[error("{ENV_TIER3_REDACTION_ENABLED} is enabled but skill {skill_id} has no usable manifest")]
    Tier3Manifest {
        skill_id: SkillId,
        #[source]
        source: Tier3ManifestLoadError,
    },
    #[error("{enabled_by} is enabled but {ENV_POLICY_BUNDLE_DIR} is unset or blank")]
    BundleDirRequired { enabled_by: &'static str },
    #[error("policy bundle directory {} (from {ENV_POLICY_BUNDLE_DIR}) cannot be read", bundle_dir.display())]
    BundleDirectory {
        bundle_dir: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{ENV_GATEWAY_TIER3_SIGNING_PUBKEY} is set but is not a valid ed25519 public key")]
    InvalidSigningKey,
    #[error("policy bundle directory {} (from {ENV_POLICY_BUNDLE_DIR}) could not be loaded", bundle_dir.display())]
    Substrate {
        bundle_dir: PathBuf,
        #[source]
        source: PolicyError,
    },
    #[error("{ENV_GATEWAY_TIER2_CEL_ENABLED} is enabled but tier-2 bundle {} failed to load", tier2_dir.display())]
    Tier2Load {
        tier2_dir: PathBuf,
        #[source]
        source: CelCompileError,
    },
    #[error("{ENV_GATEWAY_TIER2_CEL_ENABLED} is enabled but tier-2 bundle {} is missing or has no rules", tier2_dir.display())]
    Tier2NoRules { tier2_dir: PathBuf },
    #[error("{ENV_POLICY_SKILLS} lists invalid skill slug {slug:?}")]
    InvalidSkillSlug {
        slug: String,
        #[source]
        source: SkillIdError,
    },
    #[error("{ENV_POLICY_SKILLS} skill {slug:?} failed to preload")]
    SkillPreload {
        slug: String,
        #[source]
        source: PolicyError,
    },
}

/// Boot the policy stack from environment variables.
///
/// Returns the Noop stack only when no policy layer is turned on and no
/// bundle directory is configured. Once the operator turns a layer on
/// (`A2A_GATEWAY_TIER2_CEL_ENABLED`, `A2A_GATEWAY_TIER3_REDACTION_ENABLED`)
/// or configures a bundle directory, any failure to bring it up is a
/// [`PolicyStackError`] and the gateway refuses to start: a missing or
/// blank bundle directory, an unusable signing pubkey, a substrate that
/// cannot load, a Tier-2 bundle that fails to compile or holds no rules,
/// and a policy skill that is invalid or fails to preload. Enabled Tier-3
/// also requires every preloaded skill to have a readable JSON manifest
/// with a usable `json_path`; otherwise boot fails with the skill identifier.
///
/// Reads:
/// - `A2A_GATEWAY_POLICY_BUNDLE_DIR` -- root of the policy bundle layout
/// - `A2A_GATEWAY_POLICY_SKILLS` -- comma-separated skill slugs to preload
/// - `A2A_GATEWAY_TIER2_CEL_ENABLED` -- toggle Tier-2 CEL evaluation
/// - `A2A_GATEWAY_TIER3_REDACTION_ENABLED` -- toggle Tier-3 redaction gate
/// - `A2A_GATEWAY_TIER3_SIGNING_PUBKEY` -- optional ed25519 bundle pubkey
pub fn gateway_policy_stack_from_env<E: ReadEnv>(env: &E) -> Result<GatewayPolicyStack, PolicyStackError> {
    let tier3_enabled = gateway_tier3_redaction_enabled_local(env);
    let tier2_cel_active = gateway_tier2_cel_enabled(env);

    let dir = env.var(ENV_POLICY_BUNDLE_DIR).ok().map(|raw| raw.trim().to_owned());
    let Some(dir) = dir.filter(|dir| !dir.is_empty()) else {
        let enabled_by = if tier2_cel_active {
            Some(ENV_GATEWAY_TIER2_CEL_ENABLED)
        } else if tier3_enabled {
            Some(ENV_TIER3_REDACTION_ENABLED)
        } else {
            None
        };
        return match enabled_by {
            Some(enabled_by) => Err(PolicyStackError::BundleDirRequired { enabled_by }),
            None => Ok(GatewayPolicyStack::noop()),
        };
    };

    let _bundle_entries = std::fs::read_dir(&dir).map_err(|source| PolicyStackError::BundleDirectory {
        bundle_dir: PathBuf::from(&dir),
        source,
    })?;
    let bundle_path = WasmBundlePath::new(&dir);
    let signing_key = gateway_tier3_signing_pubkey(env);
    if matches!(signing_key, Tier3SigningKey::Invalid) {
        return Err(PolicyStackError::InvalidSigningKey);
    }
    let tier3_signing_pubkey = signing_key.into_configured();
    let tier2 = load_tier2_state(&bundle_path, tier2_cel_active)?;

    let substrate = WasmtimeSubstrate::try_new_with_tier2(bundle_path.clone(), tier2, tier3_signing_pubkey)
        .map(Arc::new)
        .map_err(|source| PolicyStackError::Substrate {
            bundle_dir: PathBuf::from(&dir),
            source,
        })?;

    let loaded_skills = preload_skills_from_env(env, &substrate)?;
    let tier3_manifests = if tier3_enabled {
        loaded_skills
            .iter()
            .map(|skill_id| {
                load_tier3_manifest_from_bundle(&bundle_path, skill_id)
                    .map(|manifest| (skill_id.clone(), manifest))
                    .map_err(|source| PolicyStackError::Tier3Manifest {
                        skill_id: skill_id.clone(),
                        source,
                    })
            })
            .collect::<Result<_, _>>()?
    } else {
        load_tier3_manifests_from_bundle(&bundle_path, &loaded_skills)
    };
    let tier3_gate: Arc<dyn Tier3RedactionGate> = if tier3_enabled {
        let invoker = substrate.clone() as Arc<dyn crate::policy::tier3_redaction::Tier3PartInvoker>;
        Arc::new(RealTier3RedactionGate::new(invoker))
    } else {
        Arc::new(NoopTier3RedactionGate)
    };

    Ok(GatewayPolicyStack {
        substrate: Some(substrate),
        tier3_gate,
        tier3_manifests,
    })
}

fn load_tier2_state(bundle_path: &WasmBundlePath, tier2_cel_active: bool) -> Result<Tier2State, PolicyStackError> {
    if !tier2_cel_active {
        return Ok(Tier2State::Active(
            Box::new(NoopTier2Evaluator) as Box<dyn Tier2CelEvaluator>
        ));
    }
    let tier2_dir = bundle_path.as_path().join("tier2");
    let bundle = Tier2CompiledBundle::load_from_dir(&tier2_dir).map_err(|source| PolicyStackError::Tier2Load {
        tier2_dir: tier2_dir.clone(),
        source,
    })?;
    if bundle.rules().next().is_none() {
        return Err(PolicyStackError::Tier2NoRules { tier2_dir });
    }
    Ok(Tier2State::Active(
        Box::new(RealTier2CelEvaluator::new(bundle)) as Box<dyn Tier2CelEvaluator>
    ))
}

fn preload_skills_from_env<E: ReadEnv>(
    env: &E,
    substrate: &Arc<WasmtimeSubstrate>,
) -> Result<Vec<SkillId>, PolicyStackError> {
    let Ok(slugs) = env.var(ENV_POLICY_SKILLS) else {
        return Ok(Vec::new());
    };
    let mut loaded = Vec::new();
    for slug in slugs.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let skill_id = SkillId::new(slug).map_err(|source| PolicyStackError::InvalidSkillSlug {
            slug: slug.to_owned(),
            source,
        })?;
        substrate
            .preload_redaction_skill(skill_id.clone())
            .map_err(|source| PolicyStackError::SkillPreload {
                slug: slug.to_owned(),
                source,
            })?;
        loaded.push(skill_id);
    }
    Ok(loaded)
}

fn gateway_tier3_redaction_enabled_local<E: ReadEnv>(env: &E) -> bool {
    match env.var(ENV_TIER3_REDACTION_ENABLED) {
        Ok(raw) => matches!(raw.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests;
