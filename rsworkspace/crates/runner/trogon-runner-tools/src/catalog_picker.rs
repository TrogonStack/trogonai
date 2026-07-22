//! Catalog-backed compactor model picker (M3).

use agent_client_protocol::schema::v1::{SessionConfigOption, SessionConfigSelectOption};
use trogonai_catalog_client::{CatalogSnapshot, CompactableFilter, compactable_models, parse, qualify};

pub const DEFAULT_COMPACTOR_LABEL: &str = "Default (same as session model)";

/// Build select options for `compactor_model` using the shared predicate + codec.
pub fn compactor_model_select_options(
    catalog: Option<&CatalogSnapshot>,
    callable_providers: Option<&[String]>,
    session_window: u64,
    margin: f64,
) -> Vec<SessionConfigSelectOption> {
    let mut opts = vec![SessionConfigSelectOption::new("", DEFAULT_COMPACTOR_LABEL)];

    let (Some(catalog), Some(providers)) = (catalog, callable_providers) else {
        return opts;
    };
    if providers.is_empty() {
        return opts;
    }

    let entries = compactable_models(CompactableFilter {
        catalog,
        callable_providers: providers,
        session_window,
        margin,
    });

    opts.extend(entries.iter().map(|e| {
        let value = qualify(&e.provider, &e.model_id);
        let label = format!("{} ({})", e.model_id, e.provider);
        SessionConfigSelectOption::new(value, label)
    }));
    opts
}

/// Current ACP value for the compactor select (provider-qualified when both are set).
pub fn compactor_model_current_value(compactor_provider: Option<&str>, compactor_model: Option<&str>) -> String {
    match (compactor_provider, compactor_model) {
        (Some(p), Some(m)) => qualify(p, m),
        (_, Some(m)) => m.to_string(),
        _ => String::new(),
    }
}

/// Build the full `compactor_model` `SessionConfigOption`.
pub fn compactor_model_config_option(
    compactor_provider: Option<&str>,
    compactor_model: Option<&str>,
    catalog: Option<&CatalogSnapshot>,
    callable_providers: Option<&[String]>,
    session_window: u64,
    margin: f64,
) -> SessionConfigOption {
    let current = compactor_model_current_value(compactor_provider, compactor_model);
    let opts = compactor_model_select_options(catalog, callable_providers, session_window, margin);
    SessionConfigOption::select(
        "compactor_model".to_string(),
        "Compaction Model".to_string(),
        current,
        opts,
    )
}

/// Parse an ACP config value into persisted `(compactor_provider, compactor_model)`.
pub fn parse_compactor_config(value: &str) -> (Option<String>, Option<String>) {
    match parse(value) {
        None => (None, None),
        Some(q) => (Some(q.provider), Some(q.model_id)),
    }
}

/// Look up a session model's context window from the catalog snapshot.
pub fn session_window_from_catalog(
    catalog: Option<&CatalogSnapshot>,
    session_provider: &str,
    session_model: &str,
    fallback: u64,
) -> u64 {
    let Some(catalog) = catalog else {
        return fallback;
    };
    catalog
        .entries
        .iter()
        .find(|e| e.provider == session_provider && e.model_id == session_model)
        .or_else(|| catalog.entries.iter().find(|e| e.model_id == session_model))
        .map(|e| e.context_window)
        .unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogonai_catalog_client::CatalogEntry;
    use trogonai_catalog_proto::ModelModality;

    fn sample_catalog() -> CatalogSnapshot {
        CatalogSnapshot {
            entries: vec![
                CatalogEntry {
                    model_id: "grok-4".into(),
                    provider: "xai".into(),
                    context_window: 131_072,
                    max_output: 8192,
                    modality: ModelModality::TEXT,
                },
                CatalogEntry {
                    model_id: "claude-haiku".into(),
                    provider: "anthropic".into(),
                    context_window: 200_000,
                    max_output: 8192,
                    modality: ModelModality::TEXT,
                },
            ],
        }
    }

    #[test]
    fn parse_compactor_config_empty_clears_override() {
        assert_eq!(parse_compactor_config(""), (None, None));
    }

    #[test]
    fn parse_compactor_config_qualified_splits_pair() {
        let (p, m) = parse_compactor_config("anthropic::claude-haiku");
        assert_eq!(p.as_deref(), Some("anthropic"));
        assert_eq!(m.as_deref(), Some("claude-haiku"));
    }

    #[test]
    fn compactor_model_select_options_includes_default_only_without_catalog() {
        let opts = compactor_model_select_options(None, None, 100_000, 1.2);
        assert_eq!(opts.len(), 1);
        assert_eq!(opts[0].value.0.as_ref(), "");
    }

    #[test]
    fn compactor_model_select_options_lists_compactable_models() {
        let cat = sample_catalog();
        let providers = vec!["xai".into(), "anthropic".into()];
        let opts = compactor_model_select_options(Some(&cat), Some(&providers), 100_000, 1.2);
        assert!(opts.len() >= 2);
        assert!(opts.iter().any(|o| o.value.0.as_ref() == "xai::grok-4"));
    }

    #[test]
    fn compactor_model_current_value_qualifies_when_both_set() {
        assert_eq!(
            compactor_model_current_value(Some("xai"), Some("grok-4")),
            "xai::grok-4"
        );
    }

    #[test]
    fn session_window_from_catalog_uses_entry_or_fallback() {
        let cat = sample_catalog();
        assert_eq!(session_window_from_catalog(Some(&cat), "xai", "grok-4", 100), 131_072);
        assert_eq!(session_window_from_catalog(None, "xai", "grok-4", 100), 100);
        assert_eq!(session_window_from_catalog(Some(&cat), "xai", "unknown", 100), 100);
    }
}
