//! Lexical relevance scoring for ARD registry search.

use ard_catalog::CatalogEntry;

/// Score an entry against a lexical query on a 0 to 100 scale.
pub fn lexical_score(query: &str, entry: &CatalogEntry) -> u8 {
    let query = query.trim();
    if query.is_empty() {
        return 0;
    }

    let normalized_query = query.to_lowercase();
    let terms: Vec<&str> = normalized_query.split_whitespace().collect();
    if terms.is_empty() {
        return 0;
    }

    let display_name = entry.display_name().to_lowercase();
    let description = entry.description().unwrap_or("").to_lowercase();
    let tags = entry
        .tags()
        .map(|values| values.join(" ").to_lowercase())
        .unwrap_or_default();
    let capabilities = entry
        .capabilities()
        .map(|values| values.join(" ").to_lowercase())
        .unwrap_or_default();
    let representative_queries = entry
        .representative_queries()
        .map(|values| {
            values
                .as_slice()
                .iter()
                .map(|query| query.as_ref())
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        })
        .unwrap_or_default();

    let mut earned: f64 = 0.0;
    let mut possible: f64 = 0.0;

    for term in terms {
        possible += 100.0;
        if display_name.contains(term) {
            earned += 40.0;
        }
        if description.contains(term) {
            earned += 20.0;
        }
        if tags.contains(term) {
            earned += 15.0;
        }
        if capabilities.contains(term) {
            earned += 15.0;
        }
        if representative_queries.contains(term) {
            earned += 10.0;
        }
    }

    if possible == 0.0 {
        0
    } else {
        ((earned / possible) * 100.0).clamp(0.0, 100.0).round() as u8
    }
}

#[cfg(test)]
mod tests {
    use ard_catalog::{CatalogEntry, CatalogEntryWire};

    use super::lexical_score;

    fn sample_entry() -> CatalogEntry {
        CatalogEntryWire {
            identifier: "urn:air:example.com:agent:assistant".to_owned(),
            display_name: "Assistant".to_owned(),
            media_type: "application/a2a-agent-card+json".to_owned(),
            url: Some("https://example.com/card.json".to_owned()),
            data: None,
            description: Some("Helpful coding assistant".to_owned()),
            representative_queries: Some(vec!["write code".to_owned(), "debug rust".to_owned()]),
            tags: Some(vec!["coding".to_owned()]),
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
    fn ranks_display_name_highest() {
        let entry = sample_entry();
        assert!(lexical_score("assistant", &entry) > lexical_score("chat", &entry));
    }

    #[test]
    fn returns_zero_for_no_match() {
        let entry = sample_entry();
        assert_eq!(lexical_score("nonexistent-term", &entry), 0);
    }
}
