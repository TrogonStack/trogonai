//! ARD registry application service.

use std::collections::BTreeMap;

use ard_catalog::{
    CatalogEntry, CatalogEntryWire, ExploreFacetResultWire, ExploreResponseWire, ExploreResultTypeNameWire,
    FacetCountWire, FederationMode, ListResponseWire, SearchResponseWire, SearchResultWire,
};

use crate::explore_request::ValidatedExploreRequest;
use crate::facet_field::FacetField;
use crate::filters::{entry_matches_filters, entry_matches_query};
use crate::lexical_rank::lexical_score;
use crate::list_agents_request::ValidatedListAgentsQuery;
use crate::page_token::encode_page_token;
use crate::registry_config::RegistryConfig;
use crate::search_request::ValidatedSearchRequest;

/// In-memory ARD registry runtime backed by validated catalog domain values.
#[derive(Clone, Debug)]
pub struct Registry {
    config: RegistryConfig,
}

impl Registry {
    pub fn new(config: RegistryConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &RegistryConfig {
        &self.config
    }

    pub fn manifest(&self) -> &ard_catalog::CatalogManifest {
        self.config.manifest()
    }

    pub fn search(&self, request: ValidatedSearchRequest) -> SearchResponseWire {
        let mut ranked = self
            .entries()
            .iter()
            .filter(|entry| entry_matches_filters(entry, request.filters()))
            .map(|entry| {
                let score = lexical_score(request.query(), entry);
                (entry, score)
            })
            .filter(|(_, score)| *score > 0)
            .collect::<Vec<_>>();

        ranked.sort_by(|(left_entry, left_score), (right_entry, right_score)| {
            right_score
                .partial_cmp(left_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    left_entry
                        .identifier()
                        .to_string()
                        .cmp(&right_entry.identifier().to_string())
                })
        });

        let total = ranked.len() as u64;
        let start = request.offset().min(total) as usize;
        let end = start.saturating_add(request.limit() as usize).min(ranked.len());
        let page = &ranked[start..end];

        let results = page
            .iter()
            .map(|(entry, score)| SearchResultWire {
                entry: entry_to_wire(entry),
                score: *score,
                source: self.config.source_url().to_owned(),
            })
            .collect();

        let next_offset = request.offset() + page.len() as u64;
        let page_token = if next_offset < total {
            Some(encode_page_token(next_offset))
        } else {
            None
        };

        SearchResponseWire {
            results,
            page_token,
            referrals: self.referrals_for_federation(request.federation()),
        }
    }

    pub fn list_agents(&self, query: ValidatedListAgentsQuery) -> ListResponseWire {
        let mut entries = self
            .entries()
            .iter()
            .filter(|entry| entry_matches_filters(entry, query.filters()))
            .map(entry_to_wire)
            .collect::<Vec<_>>();

        entries.sort_by(|left, right| left.identifier.cmp(&right.identifier));

        let total_count = entries.len() as u64;
        let start = query.offset().min(total_count) as usize;
        let end = start.saturating_add(query.page_size() as usize).min(entries.len());
        let page = entries[start..end].to_vec();

        let next_offset = query.offset() + page.len() as u64;
        let page_token = if next_offset < total_count {
            Some(encode_page_token(next_offset))
        } else {
            None
        };

        ListResponseWire {
            items: page,
            page_token,
            total_count: Some(total_count),
        }
    }

    pub fn explore(&self, request: ValidatedExploreRequest) -> ExploreResponseWire {
        let filtered = self
            .entries()
            .iter()
            .filter(|entry| entry_matches_filters(entry, request.filters()))
            .filter(|entry| entry_matches_query(entry, request.text()))
            .collect::<Vec<_>>();

        let mut facet_counts = BTreeMap::new();
        for facet in request.facet_fields() {
            facet_counts.insert(
                facet.as_str().to_owned(),
                ExploreFacetResultWire {
                    buckets: self.facet_values(facet, &filtered),
                    other_count: 0,
                },
            );
        }

        ExploreResponseWire {
            result_type: ExploreResultTypeNameWire::Facets,
            facets: facet_counts,
            total_count: Some(filtered.len() as u64),
        }
    }

    pub fn entries(&self) -> &[CatalogEntry] {
        self.config.manifest().entries()
    }

    fn referrals_for_federation(&self, federation: FederationMode) -> Option<Vec<CatalogEntryWire>> {
        if !federation.includes_referrals() {
            return None;
        }

        let referrals = self.config.referrals().iter().map(entry_to_wire).collect::<Vec<_>>();

        if referrals.is_empty() { None } else { Some(referrals) }
    }

    fn facet_values(&self, facet: &FacetField, entries: &[&CatalogEntry]) -> Vec<FacetCountWire> {
        let mut counts = BTreeMap::<String, u64>::new();

        match facet {
            FacetField::Type => {
                for entry in entries {
                    *counts.entry(entry.media_type().to_string()).or_default() += 1;
                }
            }
            FacetField::Tags => {
                for entry in entries {
                    if let Some(tags) = entry.tags() {
                        for tag in tags {
                            *counts.entry(tag.clone()).or_default() += 1;
                        }
                    }
                }
            }
            FacetField::Capabilities => {
                for entry in entries {
                    if let Some(capabilities) = entry.capabilities() {
                        for capability in capabilities {
                            *counts.entry(capability.clone()).or_default() += 1;
                        }
                    }
                }
            }
        }

        counts
            .into_iter()
            .map(|(value, count)| FacetCountWire { value, count })
            .collect()
    }
}

fn entry_to_wire(entry: &CatalogEntry) -> CatalogEntryWire {
    entry.clone().into_wire()
}

#[cfg(test)]
mod tests;
