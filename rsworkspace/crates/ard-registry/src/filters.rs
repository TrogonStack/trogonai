//! Filter application for registry list, search, and explore.

use ard_catalog::{CatalogEntry, SearchFiltersWire};

/// Returns true when an entry satisfies optional registry filters.
pub fn entry_matches_filters(entry: &CatalogEntry, filters: Option<&SearchFiltersWire>) -> bool {
    let Some(filters) = filters else {
        return true;
    };

    if let Some(media_types) = filters.media_type.as_ref()
        && !media_types
            .iter()
            .any(|media_type| entry.media_type().as_str() == media_type)
    {
        return false;
    }

    if let Some(required_tags) = filters.tags.as_ref() {
        let entry_tags = entry.tags().unwrap_or(&[]);
        if !required_tags.iter().all(|tag| entry_tags.contains(tag)) {
            return false;
        }
    }

    if let Some(required_capabilities) = filters.capabilities.as_ref() {
        let entry_capabilities = entry.capabilities().unwrap_or(&[]);
        if !required_capabilities
            .iter()
            .all(|capability| entry_capabilities.contains(capability))
        {
            return false;
        }
    }

    true
}

/// Returns true when an entry matches an optional lexical explore query.
pub fn entry_matches_query(entry: &CatalogEntry, query: Option<&str>) -> bool {
    match query.map(str::trim).filter(|value| !value.is_empty()) {
        Some(query) => crate::lexical_rank::lexical_score(query, entry) > 0,
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use ard_catalog::{CatalogEntry, CatalogEntryWire, SearchFiltersWire};

    use super::{entry_matches_filters, entry_matches_query};

    fn sample_entry() -> CatalogEntry {
        CatalogEntryWire {
            identifier: "urn:air:example.com:agent:assistant".to_owned(),
            display_name: "Assistant".to_owned(),
            media_type: "application/a2a-agent-card+json".to_owned(),
            url: Some("https://example.com/card.json".to_owned()),
            data: None,
            description: Some("Helpful agent".to_owned()),
            representative_queries: Some(vec!["help me".to_owned(), "answer questions".to_owned()]),
            tags: Some(vec!["demo".to_owned()]),
            capabilities: Some(vec!["chat".to_owned()]),
            version: None,
            updated_at: None,
            metadata: None,
            trust_manifest: None,
        }
        .try_into()
        .unwrap()
    }

    #[test]
    fn filters_by_media_type() {
        let entry = sample_entry();
        let filters = SearchFiltersWire {
            media_type: Some(vec!["application/mcp-server-card+json".to_owned()]),
            tags: None,
            capabilities: None,
        };
        assert!(!entry_matches_filters(&entry, Some(&filters)));
    }

    #[test]
    fn filters_by_query() {
        let entry = sample_entry();
        assert!(entry_matches_query(&entry, Some("assistant")));
        assert!(!entry_matches_query(&entry, Some("missing")));
    }
}
