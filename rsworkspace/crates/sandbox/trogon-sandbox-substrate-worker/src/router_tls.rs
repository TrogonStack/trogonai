//! Authenticated transport for the router's CONNECT-TLS listener.

mod constants;
mod pem;
mod router_tls_config;
mod router_tls_error;

pub use router_tls_config::RouterTlsConfig;
pub use router_tls_error::RouterTlsError;
