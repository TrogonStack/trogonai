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
use std::sync::Arc;

use a2a_redaction::{SkillId, WasmBundlePath};
use tracing::{error, warn};
use trogon_std::env::ReadEnv;

use crate::policy::tier2::{DenyAllTier2Evaluator, NoopTier2Evaluator, Tier2CelEvaluator};
use crate::policy::tier2_cel::{RealTier2CelEvaluator, Tier2CompiledBundle};
use crate::policy::tier3_redaction::load_tier3_manifests_from_bundle;
use crate::policy::wasmtime_substrate::{Tier2State, WasmtimeSubstrate};
use crate::policy::{NoopTier3RedactionGate, RealTier3RedactionGate, Tier3RedactionGate, Tier3SkillManifest};
use crate::runtime::env::{gateway_tier2_cel_enabled, gateway_tier3_signing_pubkey};

const ENV_POLICY_BUNDLE_DIR: &str = "A2A_GATEWAY_POLICY_BUNDLE_DIR";
const ENV_POLICY_SKILLS: &str = "A2A_GATEWAY_POLICY_SKILLS";
const ENV_TIER3_REDACTION_ENABLED: &str = "A2A_GATEWAY_TIER3_REDACTION_ENABLED";

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
    /// Stack that lets everything through. Used by tests and by
    /// deployments that haven't configured a policy bundle so the
    /// dispatch path has a stable shape to take regardless of
    /// environment.
    #[must_use]
    pub fn noop() -> Self {
        Self {
            substrate: None,
            tier3_gate: Arc::new(NoopTier3RedactionGate),
            tier3_manifests: BTreeMap::new(),
        }
    }
}

/// Boot the policy stack from environment variables.
///
/// Returns the Noop stack when no bundle directory is configured,
/// when the directory is empty, or when the Wasmtime substrate fails
/// to load -- in every failure path the gateway still serves traffic
/// but applies no policy, with a warning logged so operators can
/// diagnose the misconfiguration.
///
/// Reads:
/// - `A2A_GATEWAY_POLICY_BUNDLE_DIR` -- root of the policy bundle layout
/// - `A2A_GATEWAY_POLICY_SKILLS` -- comma-separated skill slugs to preload
/// - `A2A_GATEWAY_TIER2_CEL_ENABLED` -- toggle Tier-2 CEL evaluation
/// - `A2A_GATEWAY_TIER3_REDACTION_ENABLED` -- toggle Tier-3 redaction gate
/// - `A2A_GATEWAY_TIER3_SIGNING_PUBKEY` -- optional ed25519 bundle pubkey
pub fn gateway_policy_stack_from_env<E: ReadEnv>(env: &E) -> GatewayPolicyStack {
    let tier3_enabled = gateway_tier3_redaction_enabled_local(env);

    let Ok(raw) = env.var(ENV_POLICY_BUNDLE_DIR) else {
        if tier3_enabled {
            warn!("{ENV_TIER3_REDACTION_ENABLED}=on but {ENV_POLICY_BUNDLE_DIR} unset; tier-3 noop");
        }
        return GatewayPolicyStack::noop();
    };
    let dir = raw.trim();
    if dir.is_empty() {
        if tier3_enabled {
            warn!("{ENV_TIER3_REDACTION_ENABLED}=on but {ENV_POLICY_BUNDLE_DIR} empty; tier-3 noop");
        }
        return GatewayPolicyStack::noop();
    }

    let bundle_path = WasmBundlePath::new(dir);
    let tier3_signing_pubkey = gateway_tier3_signing_pubkey(env);
    let tier2_cel_active = gateway_tier2_cel_enabled(env);
    let tier2 = load_tier2_state(&bundle_path, tier2_cel_active);

    let substrate = match WasmtimeSubstrate::try_new_with_tier2(bundle_path.clone(), tier2, tier3_signing_pubkey) {
        Ok(layer) => Arc::new(layer),
        Err(err) => {
            warn!(
                error = %err,
                bundle_dir = dir,
                "{ENV_POLICY_BUNDLE_DIR} invalid -- Wasmtime substrate disabled",
            );
            if tier3_enabled {
                warn!("{ENV_TIER3_REDACTION_ENABLED}=on but substrate failed to load; tier-3 noop");
            }
            return GatewayPolicyStack::noop();
        }
    };

    let loaded_skills = preload_skills_from_env(env, &substrate);
    let tier3_manifests = load_tier3_manifests_from_bundle(&bundle_path, &loaded_skills);
    let tier3_gate: Arc<dyn Tier3RedactionGate> = if tier3_enabled {
        let invoker = substrate.clone() as Arc<dyn crate::policy::tier3_redaction::Tier3PartInvoker>;
        Arc::new(RealTier3RedactionGate::new(invoker))
    } else {
        Arc::new(NoopTier3RedactionGate)
    };

    GatewayPolicyStack {
        substrate: Some(substrate),
        tier3_gate,
        tier3_manifests,
    }
}

fn load_tier2_state(bundle_path: &WasmBundlePath, tier2_cel_active: bool) -> Tier2State {
    if !tier2_cel_active {
        return Tier2State::Active(Box::new(NoopTier2Evaluator) as Box<dyn Tier2CelEvaluator>);
    }
    let tier2_dir = bundle_path.as_path().join("tier2");
    match Tier2CompiledBundle::load_from_dir(&tier2_dir) {
        Ok(bundle) => Tier2State::Active(Box::new(RealTier2CelEvaluator::new(bundle)) as Box<dyn Tier2CelEvaluator>),
        Err(err) => {
            warn!(
                error = %err,
                tier2_dir = %tier2_dir.display(),
                "A2A_GATEWAY_TIER2_CEL_ENABLED=on but tier2 bundle load failed -- denying all ingress",
            );
            Tier2State::Active(Box::new(DenyAllTier2Evaluator) as Box<dyn Tier2CelEvaluator>)
        }
    }
}

fn preload_skills_from_env<E: ReadEnv>(env: &E, substrate: &Arc<WasmtimeSubstrate>) -> Vec<SkillId> {
    let Ok(slugs) = env.var(ENV_POLICY_SKILLS) else {
        return Vec::new();
    };
    let mut loaded = Vec::new();
    for slug in slugs.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let skill_id = match SkillId::new(slug) {
            Ok(id) => id,
            Err(err) => {
                error!(skill = slug, error = %err, "gateway tier-3 policy skill slug rejected");
                continue;
            }
        };
        match substrate.preload_redaction_skill(skill_id.clone()) {
            Ok(()) => loaded.push(skill_id),
            Err(err) => {
                error!(skill = slug, error = %err, "gateway tier-3 policy bundle preload failed");
            }
        }
    }
    loaded
}

fn gateway_tier3_redaction_enabled_local<E: ReadEnv>(env: &E) -> bool {
    match env.var(ENV_TIER3_REDACTION_ENABLED) {
        Ok(raw) => matches!(raw.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests;
