//! Pure predicate `compactable_models(session_window)` — owns all three filters.

use crate::catalog_entry::CatalogEntry;
use crate::snapshot::CatalogSnapshot;

/// Filter inputs for the compaction model picker.
#[derive(Clone, Copy)]
pub struct CompactableFilter<'a> {
    pub catalog: &'a CatalogSnapshot,
    pub callable_providers: &'a [String],
    pub session_window: u64,
    pub margin: f64,
}

/// Returns models eligible for compaction: text-output, sufficient capacity, callable provider.
///
/// Pure function over snapshot — same input always yields the same set.
/// When `callable_providers` is empty (credential snapshot unavailable), returns empty
/// (degradation floor: builders show only Default).
pub fn compactable_models(filter: CompactableFilter<'_>) -> Vec<CatalogEntry> {
    if filter.callable_providers.is_empty() {
        return Vec::new();
    }

    let min_window = (filter.session_window as f64 * filter.margin).ceil() as u64;

    let mut out: Vec<CatalogEntry> = filter
        .catalog
        .entries
        .iter()
        .filter(|e| e.is_text_output())
        .filter(|e| e.context_window >= min_window)
        .filter(|e| filter.callable_providers.iter().any(|p| p == &e.provider))
        .cloned()
        .collect();

    out.sort_by(|a, b| (&a.provider, &a.model_id).cmp(&(&b.provider, &b.model_id)));
    out.dedup_by(|a, b| a.provider == b.provider && a.model_id == b.model_id);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog_entry::CatalogEntry;
    use crate::snapshot::CatalogSnapshot;
    use trogonai_catalog_proto::ModelModality;

    fn catalog() -> CatalogSnapshot {
        CatalogSnapshot {
            entries: vec![
                CatalogEntry {
                    model_id: "text-big".into(),
                    provider: "anthropic".into(),
                    context_window: 200_000,
                    max_output: 8192,
                    modality: ModelModality::TEXT,
                },
                CatalogEntry {
                    model_id: "text-small".into(),
                    provider: "xai".into(),
                    context_window: 50_000,
                    max_output: 4096,
                    modality: ModelModality::TEXT,
                },
                CatalogEntry {
                    model_id: "embed-model".into(),
                    provider: "openrouter".into(),
                    context_window: 200_000,
                    max_output: 0,
                    modality: ModelModality::EMBEDDING,
                },
                CatalogEntry {
                    model_id: "no-cred".into(),
                    provider: "openrouter".into(),
                    context_window: 200_000,
                    max_output: 8192,
                    modality: ModelModality::TEXT,
                },
            ],
        }
    }

    #[test]
    fn applies_all_three_filters() {
        let cat = catalog();
        let providers = vec!["anthropic".into(), "xai".into()];
        let result = compactable_models(CompactableFilter {
            catalog: &cat,
            callable_providers: &providers,
            session_window: 100_000,
            margin: 1.2,
        });
        let ids: Vec<_> = result.iter().map(|e| (&e.provider, e.model_id.as_str())).collect();
        assert!(ids.iter().any(|(p, id)| *p == "anthropic" && *id == "text-big"));
        assert!(!ids.iter().any(|(p, _)| *p == "openrouter"));
        assert!(!ids.iter().any(|(_, id)| *id == "text-small"));
        assert!(!ids.iter().any(|(_, id)| *id == "embed-model"));
    }

    #[test]
    fn same_input_same_set() {
        let cat = catalog();
        let providers = vec!["anthropic".into()];
        let f = CompactableFilter {
            catalog: &cat,
            callable_providers: &providers,
            session_window: 10_000,
            margin: 1.2,
        };
        let a = compactable_models(f);
        let b = compactable_models(f);
        assert_eq!(a, b);
    }

    #[test]
    fn empty_providers_returns_empty() {
        let cat = catalog();
        let result = compactable_models(CompactableFilter {
            catalog: &cat,
            callable_providers: &[],
            session_window: 10_000,
            margin: 1.2,
        });
        assert!(result.is_empty());
    }
}
