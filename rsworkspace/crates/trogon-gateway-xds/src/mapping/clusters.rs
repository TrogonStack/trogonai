//! Cluster and ClusterLoadAssignment translation: one cluster per backend target.

use crate::config::GatewayConfigSnapshot;
use crate::proto::envoy::config::cluster::v3::cluster::{ClusterDiscoveryType, DiscoveryType, LbPolicy};
use crate::proto::envoy::config::cluster::v3::Cluster;
use crate::proto::envoy::config::core::v3::{address, socket_address, Address, SocketAddress};
use crate::proto::envoy::config::endpoint::v3::lb_endpoint;
use crate::proto::envoy::config::endpoint::v3::{
    ClusterLoadAssignment, Endpoint, LbEndpoint, LocalityLbEndpoints,
};

pub fn map_clusters(snapshot: &GatewayConfigSnapshot) -> Vec<Cluster> {
    snapshot
        .backends
        .iter()
        .map(|backend| Cluster {
            name: backend.name.clone(),
            cluster_discovery_type: Some(ClusterDiscoveryType::Type(
                DiscoveryType::Static as i32,
            )),
            connect_timeout: Some(prost_types::Duration {
                seconds: 5,
                nanos: 0,
            }),
            lb_policy: LbPolicy::RoundRobin as i32,
            load_assignment: Some(ClusterLoadAssignment {
                cluster_name: backend.name.clone(),
                ..Default::default()
            }),
            ..Default::default()
        })
        .collect()
}

pub fn map_assignments(snapshot: &GatewayConfigSnapshot) -> Vec<ClusterLoadAssignment> {
    snapshot
        .backends
        .iter()
        .map(|backend| ClusterLoadAssignment {
            cluster_name: backend.name.clone(),
            endpoints: vec![LocalityLbEndpoints {
                lb_endpoints: backend
                    .endpoints
                    .iter()
                    .map(|endpoint| LbEndpoint {
                        host_identifier: Some(lb_endpoint::HostIdentifier::Endpoint(
                            Endpoint {
                                address: Some(Address {
                                    address: Some(address::Address::SocketAddress(
                                        SocketAddress {
                                            address: endpoint.address.clone(),
                                            port_specifier: Some(
                                                socket_address::PortSpecifier::PortValue(
                                                    endpoint.port,
                                                ),
                                            ),
                                            ..Default::default()
                                        },
                                    )),
                                }),
                                ..Default::default()
                            },
                        )),
                        ..Default::default()
                    })
                    .collect(),
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
    fn cluster_round_trip_preserves_backend_name() {
        let snapshot = sample_snapshot();
        let clusters = map_clusters(&snapshot);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].name, "github-backend");
        assert_eq!(
            clusters[0].cluster_discovery_type,
            Some(ClusterDiscoveryType::Type(DiscoveryType::Static as i32))
        );
    }

    #[test]
    fn assignment_round_trip_preserves_endpoint_socket() {
        let snapshot = sample_snapshot();
        let assignments = map_assignments(&snapshot);
        assert_eq!(assignments.len(), 1);
        let endpoint = &assignments[0].endpoints[0].lb_endpoints[0];
        let host = endpoint
            .host_identifier
            .as_ref()
            .expect("host identifier");
        match host {
            lb_endpoint::HostIdentifier::Endpoint(ep) => {
                let socket = ep
                    .address
                    .as_ref()
                    .and_then(|addr| addr.address.as_ref())
                    .expect("socket");
                match socket {
                    address::Address::SocketAddress(sa) => {
                        assert_eq!(sa.address, "10.0.0.1");
                        assert_eq!(
                            sa.port_specifier,
                            Some(socket_address::PortSpecifier::PortValue(8080))
                        );
                    }
                    address::Address::Pipe(_) | address::Address::EnvoyInternalAddress(_) => {
                        panic!("expected socket")
                    }
                }
            }
            lb_endpoint::HostIdentifier::EndpointName(_) => {
                panic!("expected inline endpoint")
            }
        }
    }
}
