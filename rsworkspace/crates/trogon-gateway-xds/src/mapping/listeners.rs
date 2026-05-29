//! Listener translation: one `Listener` per ingress port.

use prost::Message;

use crate::config::GatewayConfigSnapshot;
use crate::mapping::rbac::RbacMapping;
use crate::proto::envoy::config::core::v3::{address, socket_address, Address, SocketAddress};
use crate::proto::envoy::config::listener::v3::filter;
use crate::proto::envoy::config::listener::v3::{Filter, FilterChain, Listener};
use crate::proto::envoy::extensions::filters::network::http_connection_manager::v3::http_connection_manager;
use crate::proto::envoy::extensions::filters::network::http_connection_manager::v3::http_filter;
use crate::proto::envoy::extensions::filters::network::http_connection_manager::v3::{
    HttpConnectionManager, HttpFilter, Rds,
};
use prost_types::Any;

pub fn map_listeners(snapshot: &GatewayConfigSnapshot) -> Vec<Listener> {
    let rbac = crate::mapping::rbac::map_policies(&snapshot.policies);
    snapshot
        .ingress_ports
        .iter()
        .map(|ingress| map_listener(ingress, snapshot, &rbac))
        .collect()
}

fn map_listener(
    ingress: &crate::config::IngressPort,
    snapshot: &GatewayConfigSnapshot,
    rbac: &RbacMapping,
) -> Listener {
    let route_config_name = snapshot
        .routes
        .first()
        .map(|route| route.name.clone())
        .unwrap_or_else(|| "default".into());

    let mut http_filters = Vec::new();

    if let Some(rbac_filter) = rbac.http_filter() {
        http_filters.push(rbac_filter);
    }
    if let Some(ext_authz) = rbac.ext_authz_filter() {
        http_filters.push(ext_authz);
    }

    http_filters.push(HttpFilter {
        name: "envoy.filters.http.router".into(),
        config_type: Some(http_filter::ConfigType::TypedConfig(Any {
            type_url: "type.googleapis.com/envoy.extensions.filters.http.router.v3.Router".into(),
            value: Vec::new(),
        })),
        ..Default::default()
    });

    let hcm = HttpConnectionManager {
        stat_prefix: format!("ingress_{}", ingress.name),
        route_specifier: Some(http_connection_manager::RouteSpecifier::Rds(Rds {
            route_config_name,
            config_source: None,
        })),
        http_filters,
        ..Default::default()
    };

    let hcm_any = Any {
        type_url:
            "type.googleapis.com/envoy.extensions.filters.network.http_connection_manager.v3.HttpConnectionManager"
                .into(),
        value: hcm.encode_to_vec(),
    };

    Listener {
        name: ingress.name.clone(),
        address: Some(Address {
            address: Some(address::Address::SocketAddress(SocketAddress {
                address: "0.0.0.0".into(),
                port_specifier: Some(socket_address::PortSpecifier::PortValue(ingress.port)),
                ..Default::default()
            })),
        }),
        filter_chains: vec![FilterChain {
            filters: vec![Filter {
                name: "envoy.filters.network.http_connection_manager".into(),
                config_type: Some(filter::ConfigType::TypedConfig(hcm_any)),
                ..Default::default()
            }],
            ..Default::default()
        }],
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::fixtures::sample_snapshot;

    #[test]
    fn listener_round_trip_preserves_name_and_port() {
        let snapshot = sample_snapshot();
        let listeners = map_listeners(&snapshot);
        assert_eq!(listeners.len(), snapshot.ingress_ports.len());
        let listener = &listeners[0];
        assert_eq!(listener.name, "http-8080");
        let socket = listener
            .address
            .as_ref()
            .and_then(|addr| addr.address.as_ref())
            .expect("socket address");
        match socket {
            address::Address::SocketAddress(sa) => {
                assert_eq!(sa.address, "0.0.0.0");
                assert_eq!(
                    sa.port_specifier,
                    Some(socket_address::PortSpecifier::PortValue(8080))
                );
            }
            address::Address::Pipe(_) | address::Address::EnvoyInternalAddress(_) => {
                panic!("expected socket address")
            }
        }
    }
}
