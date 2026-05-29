//! xDS v3 interop bridge: projects gateway KV configuration into Envoy resources.

pub mod config;
pub mod mapping;
pub mod proto;
pub mod server;
pub mod state;
pub mod type_urls;

pub use config::{
    BackendEndpoint, BackendTarget, GatewayConfigSnapshot, HttpRoute, IngressPort, PolicyRule,
    PolicyEffect,
};
pub use server::{AdsServer, AdsServerOpts};
pub use state::{ConfigStore, ConfigWatcher, NodeSnapshot, SnapshotError};
