//! Filter application for registry list, search, and explore.

use ard_catalog::CatalogEntry;

use crate::search_filters::SearchFilters;

/// Returns true when an entry satisfies domain search filters.
pub fn entry_matches_filters(entry: &CatalogEntry, filters: &SearchFilters) -> bool {
    if !filters.media_types().is_empty()
        && !filters
            .media_types()
            .iter()
            .any(|media_type| entry.media_type().as_str() == media_type)
    {
        return false;
    }

    let required_tags = filters.tags();
    if !required_tags.is_empty() {
        let entry_tags = entry.tags().unwrap_or(&[]);
        if !required_tags.iter().all(|tag| entry_tags.contains(tag)) {
            return false;
        }
    }

    let required_capabilities = filters.capabilities();
    if !required_capabilities.is_empty() {
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
mod tests;
