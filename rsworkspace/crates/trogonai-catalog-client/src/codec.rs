//! Provider-qualified codec: `qualify` / `parse` / `resolve`.
//!
//! Owns the default sentinel: `parse("") → None`; `qualify` never emits `Some("")`.

const SEPARATOR: &str = "::";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualifiedModel {
    pub provider: String,
    pub model_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodecError {
    #[error("ambiguous model id '{id}' — qualify as provider::model")]
    Ambiguous { id: String, providers: Vec<String> },
    #[error("unknown model id '{id}'")]
    Unknown { id: String },
    #[error("invalid qualified value '{value}'")]
    InvalidFormat { value: String },
}

/// Produce the ACP value for a provider-qualified model.
pub fn qualify(provider: &str, model_id: &str) -> String {
    format!("{provider}{SEPARATOR}{model_id}")
}

/// Parse an ACP value into an optional override. Empty string = default (session model).
pub fn parse(value: &str) -> Option<QualifiedModel> {
    if value.is_empty() {
        return None;
    }
    let (provider, model_id) = value.split_once(SEPARATOR)?;
    if provider.is_empty() || model_id.is_empty() {
        return None;
    }
    Some(QualifiedModel {
        provider: provider.to_string(),
        model_id: model_id.to_string(),
    })
}

/// Resolve a bare model id against the catalog snapshot.
pub fn resolve(catalog: &super::CatalogSnapshot, bare_id: &str) -> Result<QualifiedModel, CodecError> {
    let matches: Vec<_> = catalog.entries.iter().filter(|e| e.model_id == bare_id).collect();

    match matches.len() {
        0 => Err(CodecError::Unknown {
            id: bare_id.to_string(),
        }),
        1 => Ok(QualifiedModel {
            provider: matches[0].provider.clone(),
            model_id: matches[0].model_id.clone(),
        }),
        _ => {
            let providers: Vec<String> = matches
                .iter()
                .map(|e| e.provider.clone())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            Err(CodecError::Ambiguous {
                id: bare_id.to_string(),
                providers,
            })
        }
    }
}

/// Outcome of a C4 compactor-provider backfill attempt.
///
/// Typed result (repo policy: enums over `bool` at the edges) so call sites can
/// distinguish a successful resolution from the cases that must NOT be silent —
/// an ambiguous or unknown bare model — versus the genuine no-ops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackfillOutcome {
    /// The bare model resolved to a single catalog entry; the provider was set.
    Resolved,
    /// The bare model maps to more than one provider — left untouched (no guess).
    Ambiguous,
    /// The bare model is absent from the catalog — left untouched (no guess).
    Unknown,
    /// No `compactor_model` was set; nothing to backfill.
    NoModel,
    /// `compactor_provider` was already set; nothing to do.
    AlreadyResolved,
}

/// C4 migration: backfill the compactor provider for a session persisted before
/// M3 with only a bare `compactor_model` (no provider). Resolves the provider from
/// the catalog and fills `compactor_provider` in place when it resolves uniquely.
///
/// Returns a [`BackfillOutcome`] so the caller can warn (and flag the session for
/// retry) on `Ambiguous`/`Unknown` instead of silently degrading: the pair is left
/// untouched in those cases rather than guessing a provider.
pub fn backfill_compactor_provider(
    compactor_provider: &mut Option<String>,
    compactor_model: Option<&str>,
    catalog: &super::CatalogSnapshot,
) -> BackfillOutcome {
    if compactor_provider.is_some() {
        return BackfillOutcome::AlreadyResolved;
    }
    let Some(model) = compactor_model else {
        return BackfillOutcome::NoModel;
    };
    match resolve(catalog, model) {
        Ok(qualified) => {
            *compactor_provider = Some(qualified.provider);
            BackfillOutcome::Resolved
        }
        Err(CodecError::Ambiguous { .. }) => BackfillOutcome::Ambiguous,
        Err(_) => BackfillOutcome::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog_entry::CatalogEntry;
    use crate::snapshot::CatalogSnapshot;
    use trogonai_catalog_proto::ModelModality;

    fn sample_catalog() -> CatalogSnapshot {
        CatalogSnapshot {
            entries: vec![
                CatalogEntry {
                    model_id: "claude-haiku".into(),
                    provider: "anthropic".into(),
                    context_window: 200_000,
                    max_output: 8192,
                    modality: ModelModality::TEXT,
                },
                CatalogEntry {
                    model_id: "claude-haiku".into(),
                    provider: "openrouter".into(),
                    context_window: 200_000,
                    max_output: 8192,
                    modality: ModelModality::TEXT,
                },
                CatalogEntry {
                    model_id: "grok-2".into(),
                    provider: "xai".into(),
                    context_window: 131_072,
                    max_output: 8192,
                    modality: ModelModality::TEXT,
                },
            ],
        }
    }

    #[test]
    fn backfill_resolves_unique_model_only() {
        let catalog = sample_catalog();

        // Unique model → provider backfilled, Resolved.
        let mut provider = None;
        assert_eq!(
            backfill_compactor_provider(&mut provider, Some("grok-2"), &catalog),
            BackfillOutcome::Resolved
        );
        assert_eq!(provider.as_deref(), Some("xai"));

        // Ambiguous model → Ambiguous, left untouched (no guess).
        let mut provider = None;
        assert_eq!(
            backfill_compactor_provider(&mut provider, Some("claude-haiku"), &catalog),
            BackfillOutcome::Ambiguous
        );
        assert!(provider.is_none());

        // Unknown model → Unknown, left untouched (no guess).
        let mut provider = None;
        assert_eq!(
            backfill_compactor_provider(&mut provider, Some("not-in-catalog"), &catalog),
            BackfillOutcome::Unknown
        );
        assert!(provider.is_none());

        // Already set → AlreadyResolved, no-op.
        let mut provider = Some("anthropic".to_string());
        assert_eq!(
            backfill_compactor_provider(&mut provider, Some("grok-2"), &catalog),
            BackfillOutcome::AlreadyResolved
        );
        assert_eq!(provider.as_deref(), Some("anthropic"));

        // No model → NoModel, no-op.
        let mut provider = None;
        assert_eq!(
            backfill_compactor_provider(&mut provider, None, &catalog),
            BackfillOutcome::NoModel
        );
        assert!(provider.is_none());
    }

    #[test]
    fn parse_empty_returns_none() {
        assert!(parse("").is_none());
    }

    #[test]
    fn qualify_never_emits_empty_string() {
        let q = qualify("anthropic", "claude-haiku");
        assert!(!q.is_empty());
        assert_eq!(q, "anthropic::claude-haiku");
    }

    #[test]
    fn parse_qualified_round_trips() {
        let parsed = parse("anthropic::claude-haiku-4-5").unwrap();
        assert_eq!(parsed.provider, "anthropic");
        assert_eq!(parsed.model_id, "claude-haiku-4-5");
    }

    #[test]
    fn resolve_unique_model() {
        let cat = sample_catalog();
        let resolved = resolve(&cat, "grok-2").unwrap();
        assert_eq!(resolved.provider, "xai");
    }

    #[test]
    fn resolve_ambiguous_errors() {
        let cat = sample_catalog();
        let err = resolve(&cat, "claude-haiku").unwrap_err();
        assert!(matches!(err, CodecError::Ambiguous { .. }));
    }
}
