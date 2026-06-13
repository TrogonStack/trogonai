use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::error::CapabilityError;

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
