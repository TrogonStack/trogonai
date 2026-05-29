pub mod clusters;
pub mod listeners;
pub mod rbac;
pub mod routes;

use crate::config::GatewayConfigSnapshot;
use crate::proto::envoy::config::cluster::v3::Cluster;
use crate::proto::envoy::config::endpoint::v3::ClusterLoadAssignment;
use crate::proto::envoy::config::listener::v3::Listener;
use crate::proto::envoy::config::route::v3::RouteConfiguration;
use crate::proto::envoy::extensions::filters::http::rbac::v3::Rbac as HttpRbacFilter;

#[derive(Clone, Debug)]
pub struct MappedResources {
    pub listeners: Vec<Listener>,
    pub routes: Vec<RouteConfiguration>,
    pub clusters: Vec<Cluster>,
    pub assignments: Vec<ClusterLoadAssignment>,
    pub rbac_filters: Vec<(String, HttpRbacFilter)>,
}

pub fn map_snapshot(snapshot: &GatewayConfigSnapshot) -> MappedResources {
    let rbac = rbac::map_policies(&snapshot.policies);
    MappedResources {
        listeners: listeners::map_listeners(snapshot),
        routes: routes::map_routes(snapshot),
        clusters: clusters::map_clusters(snapshot),
        assignments: clusters::map_assignments(snapshot),
        rbac_filters: rbac.filters,
    }
}
