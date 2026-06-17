use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::error::CapabilityError;
use crate::probe::{ProbeKind, ProbeResult};

/// Provider certification level used by the Switch Safety Gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CertificationLevel {
    Experimental,
    Basic,
    SwitchSafe,
    Production,
}

impl CertificationLevel {
    pub fn allows_switch_without_warning(self) -> bool {
        matches!(self, Self::SwitchSafe | Self::Production)
    }

    /// Probes that MUST pass for a model to be switch-safe: structured tool use, JSON
    /// schema, and a usable context window. A model that fails any of these cannot be
    /// trusted to carry a real session, so it stays `Basic` (gate still confirms).
    const SWITCH_CRITICAL: &'static [ProbeKind] = &[ProbeKind::ToolUse, ProbeKind::JsonSchema, ProbeKind::ContextLimits];

    /// Derive a certification level from a contract-test probe battery
    /// (§ Capability Registry Freshness: *"contract tests para tool use, image input,
    /// JSON schema, context limits, streaming y compaction"* — promotion to
    /// `SwitchSafe`/`Production` must come from probes, never from a static baseline).
    ///
    /// - every probe passing → `Production`;
    /// - all switch-critical probes passing (some optional ones failing) → `SwitchSafe`;
    /// - a switch-critical probe failing, or no probes at all → `Basic`.
    pub fn from_probe_results(results: &[ProbeResult]) -> Self {
        if results.is_empty() {
            return Self::Basic;
        }
        let passed = |kind: ProbeKind| results.iter().any(|r| r.kind == kind && r.passed);
        let failed_any = results.iter().any(|r| !r.passed);
        let critical_ok = Self::SWITCH_CRITICAL.iter().all(|kind| passed(*kind));

        if !critical_ok {
            Self::Basic
        } else if failed_any {
            Self::SwitchSafe
        } else {
            Self::Production
        }
    }
}

/// One row in the provider/tool certification matrix.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderCertificationEntry {
    pub model: String,
    pub runner: String,
    pub text: bool,
    pub tool_use: bool,
    pub parallel_tools: bool,
    pub image_input: bool,
    pub json_schema: bool,
    pub long_context: bool,
    pub streaming: bool,
    pub artifact_refs: bool,
    pub mcp_tools: bool,
    pub switch_from: Vec<String>,
    pub switch_to: Vec<String>,
    pub certified_level: CertificationLevel,
    pub last_verified_at: Option<OffsetDateTime>,
}

impl ProviderCertificationEntry {
    pub fn validate(&self) -> Result<(), CapabilityError> {
        if self.model.is_empty() || self.runner.is_empty() {
            return Err(CapabilityError::InvalidCertification {
                detail: "model and runner are required".to_string(),
            });
        }
        Ok(())
    }

    /// Apply a probe battery to this entry: set the verified capability booleans from the
    /// probe results, promote (or demote) the certification level via
    /// [`CertificationLevel::from_probe_results`], and stamp `last_verified_at` so the
    /// freshness policy treats the entry as verified rather than assumed.
    pub fn apply_probe_results(&mut self, results: &[ProbeResult], now: OffsetDateTime) {
        for result in results {
            match result.kind {
                ProbeKind::ToolUse => self.tool_use = result.passed,
                ProbeKind::ImageInput => self.image_input = result.passed,
                ProbeKind::JsonSchema => self.json_schema = result.passed,
                ProbeKind::ContextLimits => self.long_context = result.passed,
                ProbeKind::Streaming => self.streaming = result.passed,
                ProbeKind::CompactionSupported => {}
            }
        }
        self.certified_level = CertificationLevel::from_probe_results(results);
        self.last_verified_at = Some(now);
    }
}

/// Provider/tool certification matrix for switch-safe routing decisions.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProviderCertificationMatrix {
    pub entries: Vec<ProviderCertificationEntry>,
}

impl ProviderCertificationMatrix {
    pub fn get(&self, model: &str, runner: &str) -> Option<&ProviderCertificationEntry> {
        self.entries
            .iter()
            .find(|entry| entry.model == model && entry.runner == runner)
    }

    pub fn certification_level(&self, model: &str, runner: &str) -> CertificationLevel {
        self.get(model, runner)
            .map(|entry| entry.certified_level)
            .unwrap_or(CertificationLevel::Experimental)
    }

    pub fn is_switch_allowed(&self, from_model: &str, from_runner: &str, to_model: &str, to_runner: &str) -> bool {
        let Some(from) = self.get(from_model, from_runner) else {
            return false;
        };
        let Some(to) = self.get(to_model, to_runner) else {
            return false;
        };
        from.switch_to.iter().any(|target| target == to_model)
            && to.switch_from.iter().any(|source| source == from_model)
    }

    pub fn push_validated(&mut self, entry: ProviderCertificationEntry) -> Result<(), CapabilityError> {
        entry.validate()?;
        self.entries.push(entry);
        Ok(())
    }

    /// Certify a model/runner from a probe battery, promoting it out of the unverified
    /// `Basic` baseline (§ Backlog: *"Promover certificacion por probes"*). Returns the
    /// resulting level. Errors if the model/runner is not in the matrix — promotion must
    /// target a known expected entry, not invent one.
    pub fn certify_from_probes(
        &mut self,
        model: &str,
        runner: &str,
        results: &[ProbeResult],
        now: OffsetDateTime,
    ) -> Result<CertificationLevel, CapabilityError> {
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.model == model && entry.runner == runner)
            .ok_or_else(|| CapabilityError::InvalidCertification {
                detail: format!("no certification entry for model={model} runner={runner}"),
            })?;
        entry.apply_probe_results(results, now);
        Ok(entry.certified_level)
    }

    /// Kernel-owned initial certification matrix (cambio-modelo.md "Open
    /// Implementation Decisions": *"Matriz inicial con dos proveedores/runners y
    /// capabilities esperadas"*).
    ///
    /// The recorded set is the first-party Claude (`trogon-acp-runner`, runner
    /// `claude`) and Grok (`trogon-xai-runner`, runner `xai`) model families, with
    /// their **expected** capabilities and the intended switch pairs (full mesh).
    ///
    /// Crucially, every entry is `CertificationLevel::Basic` with
    /// `last_verified_at: None`: the design rule *"no asumir soporte si no esta
    /// verificado"* (Capability Registry Freshness) forbids treating un-probed
    /// capabilities as switch-safe. `Basic` is **not** `allows_switch_without_warning`,
    /// so the Switch Safety Gate still asks for confirmation. Promotion to
    /// `SwitchSafe`/`Production` must come from the runner health-checks/probes and
    /// contract tests (still to be implemented), not from this static baseline.
    /// Models/runners outside this set resolve to `Experimental`.
    pub fn baseline() -> Self {
        // (runner, model). Runner ids match the runners' default `AGENT_TYPE`; model
        // ids match their advertised model lists. Capabilities are *expected*, not
        // verified, so the level stays conservative until a probe confirms them.
        let recorded: &[(&str, &str)] = &[
            ("claude", "claude-opus-4-6"),
            ("claude", "claude-sonnet-4-6"),
            ("claude", "claude-haiku-4-5-20251001"),
            ("xai", "grok-4"),
            ("xai", "grok-3"),
            ("xai", "grok-3-mini"),
            ("xai", "grok-code-fast"),
        ];
        let all_models: Vec<&str> = recorded.iter().map(|(_, model)| *model).collect();

        let mut matrix = Self::default();
        for (runner, model) in recorded {
            // Full mesh: the intended switch pairs (effective once verified).
            let others: Vec<String> = all_models
                .iter()
                .filter(|candidate| **candidate != *model)
                .map(|candidate| candidate.to_string())
                .collect();
            matrix.entries.push(ProviderCertificationEntry {
                model: model.to_string(),
                runner: runner.to_string(),
                text: true,
                tool_use: true,
                parallel_tools: true,
                // Grok lacks image input; Claude supports it.
                image_input: *runner == "claude",
                json_schema: true,
                long_context: true,
                streaming: true,
                artifact_refs: true,
                mcp_tools: true,
                switch_from: others.clone(),
                switch_to: others,
                // Expected-but-unverified -> Basic (gate still asks confirmation).
                certified_level: CertificationLevel::Basic,
                last_verified_at: None,
            });
        }
        matrix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_records_expected_pairs_as_unverified_basic() {
        let matrix = ProviderCertificationMatrix::baseline();

        // Expected-but-unverified: Basic, and NOT switch-safe-without-warning, so
        // the gate still asks confirmation ("no asumir soporte si no esta verificado").
        assert_eq!(
            matrix.certification_level("claude-sonnet-4-6", "claude"),
            CertificationLevel::Basic
        );
        assert_eq!(
            matrix.certification_level("grok-code-fast", "xai"),
            CertificationLevel::Basic
        );
        assert!(!CertificationLevel::Basic.allows_switch_without_warning());

        // Entries are unverified until a probe/contract-test confirms them.
        assert!(
            matrix
                .get("claude-sonnet-4-6", "claude")
                .unwrap()
                .last_verified_at
                .is_none()
        );

        // The intended switch pairs are recorded in both directions (full mesh),
        // so once verified they become switch-safe without a matrix gap.
        assert!(matrix.is_switch_allowed("claude-sonnet-4-6", "claude", "grok-code-fast", "xai"));
        assert!(matrix.is_switch_allowed("grok-code-fast", "xai", "claude-sonnet-4-6", "claude"));
    }

    fn probe(kind: ProbeKind, passed: bool) -> ProbeResult {
        ProbeResult {
            kind,
            passed,
            detail: None,
        }
    }

    fn full_battery(all_pass: bool) -> Vec<ProbeResult> {
        [
            ProbeKind::ToolUse,
            ProbeKind::ImageInput,
            ProbeKind::JsonSchema,
            ProbeKind::ContextLimits,
            ProbeKind::Streaming,
            ProbeKind::CompactionSupported,
        ]
        .into_iter()
        .map(|kind| probe(kind, all_pass))
        .collect()
    }

    #[test]
    fn empty_probes_stay_basic() {
        assert_eq!(CertificationLevel::from_probe_results(&[]), CertificationLevel::Basic);
    }

    #[test]
    fn all_probes_pass_is_production() {
        assert_eq!(
            CertificationLevel::from_probe_results(&full_battery(true)),
            CertificationLevel::Production
        );
    }

    #[test]
    fn critical_pass_with_optional_failure_is_switch_safe() {
        // Critical (tool_use, json_schema, context_limits) pass; image/streaming fail.
        let results = vec![
            probe(ProbeKind::ToolUse, true),
            probe(ProbeKind::JsonSchema, true),
            probe(ProbeKind::ContextLimits, true),
            probe(ProbeKind::ImageInput, false),
            probe(ProbeKind::Streaming, false),
            probe(ProbeKind::CompactionSupported, false),
        ];
        assert_eq!(
            CertificationLevel::from_probe_results(&results),
            CertificationLevel::SwitchSafe
        );
    }

    #[test]
    fn critical_failure_stays_basic() {
        let results = vec![
            probe(ProbeKind::ToolUse, false),
            probe(ProbeKind::JsonSchema, true),
            probe(ProbeKind::ContextLimits, true),
        ];
        assert_eq!(CertificationLevel::from_probe_results(&results), CertificationLevel::Basic);
    }

    #[test]
    fn certify_from_probes_promotes_baseline_entry_and_stamps_verification() {
        let mut matrix = ProviderCertificationMatrix::baseline();
        // Baseline is unverified Basic with no timestamp.
        assert_eq!(
            matrix.certification_level("grok-code-fast", "xai"),
            CertificationLevel::Basic
        );
        assert!(matrix.get("grok-code-fast", "xai").unwrap().last_verified_at.is_none());

        let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let level = matrix
            .certify_from_probes("grok-code-fast", "xai", &full_battery(true), now)
            .unwrap();

        assert_eq!(level, CertificationLevel::Production);
        let entry = matrix.get("grok-code-fast", "xai").unwrap();
        assert_eq!(entry.certified_level, CertificationLevel::Production);
        assert_eq!(entry.last_verified_at, Some(now));
        assert!(entry.certified_level.allows_switch_without_warning());
    }

    #[test]
    fn certify_from_probes_errors_for_unknown_entry() {
        let mut matrix = ProviderCertificationMatrix::baseline();
        let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let err = matrix.certify_from_probes("mystery", "nobody", &full_battery(true), now);
        assert!(err.is_err());
    }

    #[test]
    fn baseline_treats_unknown_models_as_experimental() {
        let matrix = ProviderCertificationMatrix::baseline();
        assert_eq!(
            matrix.certification_level("mystery/model-x", "unknown"),
            CertificationLevel::Experimental
        );
        assert!(!matrix.is_switch_allowed("mystery/model-x", "unknown", "grok-4", "xai"));
    }

    #[test]
    fn baseline_grok_lacks_image_input() {
        let matrix = ProviderCertificationMatrix::baseline();
        assert!(!matrix.get("grok-4", "xai").unwrap().image_input);
        assert!(matrix.get("claude-opus-4-6", "claude").unwrap().image_input);
    }
}
