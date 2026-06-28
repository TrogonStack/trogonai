use ard_catalog::{
    CatalogEntryWire, CatalogManifest, CatalogManifestWire, ExploreFacetRequestWire, ExploreQueryWire,
    ExploreRequestWire, ExploreResultTypeWire, FederationMode, SPEC_VERSION, SearchQueryWire, SearchRequestWire,
};

use super::Registry;
use crate::explore_request::ValidatedExploreRequest;
use crate::list_agents_request::ValidatedListAgentsQuery;
use crate::registry_config::RegistryConfig;
use crate::search_request::ValidatedSearchRequest;
use crate::source_url::SourceUrl;

fn sample_manifest() -> CatalogManifest {
    CatalogManifestWire {
        spec_version: SPEC_VERSION.to_owned(),
        host: None,
        entries: vec![
            CatalogEntryWire {
                identifier: "urn:air:example.com:agent:alpha".to_owned(),
                display_name: "Alpha Assistant".to_owned(),
                media_type: "application/a2a-agent-card+json".to_owned(),
                url: Some("https://example.com/alpha.json".to_owned()),
                data: None,
                description: Some("Alpha coding helper".to_owned()),
                representative_queries: Some(vec!["write code".to_owned(), "debug rust".to_owned()]),
                tags: Some(vec!["coding".to_owned()]),
                capabilities: Some(vec!["chat".to_owned()]),
                version: None,
                updated_at: None,
                metadata: None,
                trust_manifest: None,
            },
            CatalogEntryWire {
                identifier: "urn:air:example.com:agent:beta".to_owned(),
                display_name: "Beta Bot".to_owned(),
                media_type: "application/mcp-server-card+json".to_owned(),
                url: Some("https://example.com/beta.json".to_owned()),
                data: None,
                description: Some("Beta MCP server".to_owned()),
                representative_queries: Some(vec!["list tools".to_owned(), "call tool".to_owned()]),
                tags: Some(vec!["tools".to_owned()]),
                capabilities: Some(vec!["tools".to_owned()]),
                version: None,
                updated_at: None,
                metadata: None,
                trust_manifest: None,
            },
        ],
    }
    .try_into()
    .unwrap()
}

fn registry() -> Registry {
    Registry::new(RegistryConfig::new(
        SourceUrl::parse("https://registry.example.com").unwrap(),
        sample_manifest(),
        vec![],
    ))
}

#[test]
fn search_ranks_best_match_first() {
    let registry = registry();
    let response = registry.search(
        ValidatedSearchRequest::try_from_wire(SearchRequestWire {
            query: SearchQueryWire {
                text: "alpha".to_owned(),
                filter: None,
            },
            page_size: Some(10),
            page_token: None,
            federation: FederationMode::None,
        })
        .unwrap(),
    );

    assert_eq!(response.results.len(), 1);
    assert_eq!(response.results[0].entry.identifier, "urn:air:example.com:agent:alpha");
    assert!(response.results[0].score > 0);
}

#[test]
fn list_agents_is_deterministic() {
    let registry = registry();
    let response = registry.list_agents(
        ValidatedListAgentsQuery::try_from_wire(ard_catalog::ListAgentsQueryWire {
            page_size: Some(1),
            page_token: None,
            filters: None,
        })
        .unwrap(),
    );

    assert_eq!(response.items.len(), 1);
    assert_eq!(response.items[0].identifier, "urn:air:example.com:agent:alpha");
    assert!(response.page_token.is_some());
}

#[test]
fn explore_returns_facet_counts() {
    let registry = registry();
    let response = registry.explore(ValidatedExploreRequest::from_wire(ard_catalog::ExploreRequestWire {
        query: None,
        result_type: ExploreResultTypeWire {
            facets: vec![
                ExploreFacetRequestWire {
                    field: "type".to_owned(),
                },
                ExploreFacetRequestWire {
                    field: "tags".to_owned(),
                },
            ],
        },
    }));

    assert_eq!(response.total_count, Some(2));
    assert_eq!(response.facets["type"].buckets.len(), 2);
    assert!(
        response.facets["tags"]
            .buckets
            .iter()
            .any(|facet| facet.value == "coding")
    );
}

#[test]
fn explore_applies_query_filter() {
    let registry = registry();
    let response = registry.explore(ValidatedExploreRequest::from_wire(ExploreRequestWire {
        query: Some(ExploreQueryWire {
            text: Some("alpha".to_owned()),
            filter: None,
        }),
        result_type: ExploreResultTypeWire {
            facets: vec![ExploreFacetRequestWire {
                field: "type".to_owned(),
            }],
        },
    }));

    assert_eq!(response.total_count, Some(1));
}

#[test]
fn auto_federation_returns_configured_referrals() {
    let referral: ard_catalog::CatalogEntry = CatalogEntryWire {
        identifier: "urn:air:peer.example:registry:main".to_owned(),
        display_name: "Peer Registry".to_owned(),
        media_type: "application/ai-registry+json".to_owned(),
        url: Some("https://peer.example/registry".to_owned()),
        data: None,
        description: Some("Peer registry".to_owned()),
        representative_queries: Some(vec!["discover agents".to_owned(), "search catalog".to_owned()]),
        tags: None,
        capabilities: None,
        version: None,
        updated_at: None,
        metadata: None,
        trust_manifest: None,
    }
    .try_into()
    .unwrap();

    let registry = Registry::new(RegistryConfig::new(
        SourceUrl::parse("https://registry.example.com").unwrap(),
        CatalogManifestWire {
            spec_version: SPEC_VERSION.to_owned(),
            host: None,
            entries: vec![],
        }
        .try_into()
        .unwrap(),
        vec![referral],
    ));

    let response = registry.search(
        ValidatedSearchRequest::try_from_wire(SearchRequestWire {
            query: SearchQueryWire {
                text: "anything".to_owned(),
                filter: None,
            },
            page_size: Some(10),
            page_token: None,
            federation: FederationMode::Auto,
        })
        .unwrap(),
    );

    assert_eq!(response.referrals.as_ref().map(Vec::len), Some(1));
}
