//! Generated protobuf types and gRPC clients for xDS v3.

pub mod google {
    pub mod api {
        pub mod expr {
            pub mod v1alpha1 {
                tonic::include_proto!("google.api.expr.v1alpha1");
            }
        }
    }
    pub mod rpc {
        tonic::include_proto!("google.rpc");
    }
}

pub mod validate {
    tonic::include_proto!("validate");
}

pub mod udpa {
    pub mod annotations {
        tonic::include_proto!("udpa.annotations");
    }
}

pub mod xds {
    pub mod annotations {
        pub mod v3 {
            tonic::include_proto!("xds.annotations.v3");
        }
    }
    pub mod core {
        pub mod v3 {
            tonic::include_proto!("xds.core.v3");
        }
    }
    pub mod r#type {
        pub mod matcher {
            pub mod v3 {
                tonic::include_proto!("xds.r#type.matcher.v3");
            }
        }
    }
}

pub mod envoy {
    pub mod data {
        pub mod accesslog {
            pub mod v3 {
                tonic::include_proto!("envoy.data.accesslog.v3");
            }
        }
    }
    pub mod r#type {
        pub mod v3 {
            tonic::include_proto!("envoy.r#type.v3");
        }
        pub mod http {
            pub mod v3 {
                tonic::include_proto!("envoy.r#type.http.v3");
            }
        }
        pub mod matcher {
            pub mod v3 {
                tonic::include_proto!("envoy.r#type.matcher.v3");
            }
        }
        pub mod metadata {
            pub mod v3 {
                tonic::include_proto!("envoy.r#type.metadata.v3");
            }
        }
        pub mod tracing {
            pub mod v3 {
                tonic::include_proto!("envoy.r#type.tracing.v3");
            }
        }
    }
    pub mod config {
        pub mod accesslog {
            pub mod v3 {
                tonic::include_proto!("envoy.config.accesslog.v3");
            }
        }
        pub mod cluster {
            pub mod v3 {
                tonic::include_proto!("envoy.config.cluster.v3");
            }
        }
        pub mod common {
            pub mod mutation_rules {
                pub mod v3 {
                    tonic::include_proto!("envoy.config.common.mutation_rules.v3");
                }
            }
        }
        pub mod core {
            pub mod v3 {
                tonic::include_proto!("envoy.config.core.v3");
            }
        }
        pub mod endpoint {
            pub mod v3 {
                tonic::include_proto!("envoy.config.endpoint.v3");
            }
        }
        pub mod listener {
            pub mod v3 {
                tonic::include_proto!("envoy.config.listener.v3");
            }
        }
        pub mod rbac {
            pub mod v3 {
                tonic::include_proto!("envoy.config.rbac.v3");
            }
        }
        pub mod route {
            pub mod v3 {
                tonic::include_proto!("envoy.config.route.v3");
            }
        }
        pub mod trace {
            pub mod v3 {
                tonic::include_proto!("envoy.config.trace.v3");
            }
        }
    }
    pub mod extensions {
        pub mod filters {
            pub mod http {
                pub mod ext_authz {
                    pub mod v3 {
                        tonic::include_proto!("envoy.extensions.filters.http.ext_authz.v3");
                    }
                }
                pub mod rbac {
                    pub mod v3 {
                        tonic::include_proto!("envoy.extensions.filters.http.rbac.v3");
                    }
                }
                pub mod router {
                    pub mod v3 {
                        tonic::include_proto!("envoy.extensions.filters.http.router.v3");
                    }
                }
            }
            pub mod network {
                pub mod http_connection_manager {
                    pub mod v3 {
                        tonic::include_proto!(
                            "envoy.extensions.filters.network.http_connection_manager.v3"
                        );
                    }
                }
            }
        }
    }
    pub mod service {
        pub mod discovery {
            pub mod v3 {
                tonic::include_proto!("envoy.service.discovery.v3");
            }
        }
    }
}

pub use envoy::service::discovery::v3::aggregated_discovery_service_client::AggregatedDiscoveryServiceClient;
pub use envoy::service::discovery::v3::aggregated_discovery_service_server::{
    AggregatedDiscoveryService, AggregatedDiscoveryServiceServer,
};
pub use envoy::service::discovery::v3::{DiscoveryRequest, DiscoveryResponse};
