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
