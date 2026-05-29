//! RouteConfiguration translation: one route table per HTTPRoute equivalent.

use crate::config::GatewayConfigSnapshot;
use crate::proto::envoy::config::route::v3::route;
use crate::proto::envoy::config::route::v3::route_action::ClusterSpecifier;
use crate::proto::envoy::config::route::v3::route_match::PathSpecifier;
use crate::proto::envoy::config::route::v3::{Route, RouteAction, RouteConfiguration, RouteMatch, VirtualHost};

pub fn map_routes(snapshot: &GatewayConfigSnapshot) -> Vec<RouteConfiguration> {
    snapshot
        .routes
        .iter()
        .map(|http_route| RouteConfiguration {
            name: http_route.name.clone(),
            virtual_hosts: vec![VirtualHost {
                name: format!("{}-vhost", http_route.name),
                domains: if http_route.hostnames.is_empty() {
                    vec!["*".into()]
                } else {
                    http_route.hostnames.clone()
                },
                routes: vec![Route {
                    r#match: Some(RouteMatch {
                        path_specifier: Some(PathSpecifier::Prefix(
                            http_route.path_prefix.clone(),
                        )),
                        ..Default::default()
                    }),
                    action: Some(route::Action::Route(RouteAction {
                        cluster_specifier: Some(ClusterSpecifier::Cluster(
                            http_route.cluster.clone(),
                        )),
                        ..Default::default()
                    })),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::fixtures::sample_snapshot;

    #[test]
    fn route_round_trip_preserves_prefix_and_cluster() {
        let snapshot = sample_snapshot();
        let routes = map_routes(&snapshot);
        assert_eq!(routes.len(), 1);
        let route_cfg = &routes[0];
        assert_eq!(route_cfg.name, "github-route");
        let vhost = &route_cfg.virtual_hosts[0];
        assert_eq!(vhost.domains, vec!["github.example.com".to_string()]);
        let route = &vhost.routes[0];
        let prefix = route
            .r#match
            .as_ref()
            .and_then(|m| m.path_specifier.as_ref())
            .expect("prefix match");
        match prefix {
            PathSpecifier::Prefix(value) => assert_eq!(value, "/mcp/github"),
            _ => panic!("expected prefix"),
        }
        let cluster = route
            .action
            .as_ref()
            .and_then(|action| match action {
                route::Action::Route(ra) => ra.cluster_specifier.as_ref().and_then(|spec| match spec {
                    ClusterSpecifier::Cluster(name) => Some(name.as_str()),
                    ClusterSpecifier::ClusterHeader(_) => None,
                    ClusterSpecifier::WeightedClusters(_) => None,
                    ClusterSpecifier::ClusterSpecifierPlugin(_) => None,
                    ClusterSpecifier::InlineClusterSpecifierPlugin(_) => None,
                }),
                route::Action::Redirect(_) => None,
                route::Action::DirectResponse(_) => None,
                route::Action::FilterAction(_) => None,
                route::Action::NonForwardingAction(_) => None,
            })
            .expect("route action");
        assert_eq!(cluster, "github-backend");
    }
}
