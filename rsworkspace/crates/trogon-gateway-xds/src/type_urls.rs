//! Envoy xDS v3 type URLs served by this bridge.

pub const LISTENER: &str = "type.googleapis.com/envoy.config.listener.v3.Listener";
pub const ROUTE_CONFIGURATION: &str = "type.googleapis.com/envoy.config.route.v3.RouteConfiguration";
pub const CLUSTER: &str = "type.googleapis.com/envoy.config.cluster.v3.Cluster";
pub const CLUSTER_LOAD_ASSIGNMENT: &str =
    "type.googleapis.com/envoy.config.endpoint.v3.ClusterLoadAssignment";
pub const RBAC: &str = "type.googleapis.com/envoy.extensions.filters.http.rbac.v3.RBAC";

pub const ALL: &[&str] = &[
    LISTENER,
    ROUTE_CONFIGURATION,
    CLUSTER,
    CLUSTER_LOAD_ASSIGNMENT,
    RBAC,
];
