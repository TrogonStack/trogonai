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
}
