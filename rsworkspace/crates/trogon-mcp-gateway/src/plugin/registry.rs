//! Plugin chain registry: which plugins run at each pipeline stage.

use std::collections::HashMap;

use serde::Deserialize;

use super::PluginStage;

/// Policy document fragment listing plugin names per pipeline hook.
///
/// Example:
///
/// ```json
/// {
///   "pre_authz": ["risk-scorer"],
///   "pre_call": ["redaction"],
///   "post_call": []
/// }
/// ```
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize)]
pub struct PluginChainSpec {
    #[serde(default)]
    pub pre_authz: Vec<String>,
    #[serde(default)]
    pub pre_call: Vec<String>,
    #[serde(default)]
    pub post_call: Vec<String>,
}

/// Ordered plugin names indexed by pipeline stage.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PluginRegistry {
    chains: HashMap<PluginStage, Vec<String>>,
}

impl PluginRegistry {
    #[must_use]
    pub fn from_policy(spec: &PluginChainSpec) -> Self {
        let mut chains = HashMap::new();
        chains.insert(PluginStage::PreAuthz, spec.pre_authz.clone());
        chains.insert(PluginStage::PreCall, spec.pre_call.clone());
        chains.insert(PluginStage::PostCall, spec.post_call.clone());
        Self { chains }
    }

    #[must_use]
    pub fn plugins_for(&self, stage: PluginStage) -> &[String] {
        self.chains
            .get(&stage)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chains.values().all(|v| v.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_parses_two_plugin_chain() {
        let spec: PluginChainSpec = serde_json::from_str(
            r#"{
                "pre_authz": ["risk-scorer"],
                "pre_call": ["redaction"],
                "post_call": []
            }"#,
        )
        .expect("parse spec");

        let registry = PluginRegistry::from_policy(&spec);
        assert_eq!(
            registry.plugins_for(PluginStage::PreAuthz),
            &["risk-scorer".to_string()]
        );
        assert_eq!(
            registry.plugins_for(PluginStage::PreCall),
            &["redaction".to_string()]
        );
        assert!(registry.plugins_for(PluginStage::PostCall).is_empty());
        assert!(!registry.is_empty());
    }

    #[test]
    fn empty_spec_yields_empty_registry() {
        let spec = PluginChainSpec::default();
        let registry = PluginRegistry::from_policy(&spec);
        assert!(registry.is_empty());
    }
}
