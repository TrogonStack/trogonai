//! Read-only web dashboard for sandbox lease, snapshot, worker, and tag observability.

mod constants;
mod dashboard_args;
mod dashboard_config;
mod dashboard_error;
mod router;
mod run;
mod serve;
mod ws_gateway_url;
mod ws_gateway_url_error;

pub use dashboard_args::DashboardArgs;
pub use dashboard_config::DashboardConfig;
pub use dashboard_error::DashboardError;
pub use router::router;
pub use run::run;
pub use serve::{serve_app_with_shutdown, serve_with_shutdown, shutdown_signal};
pub use ws_gateway_url::WsGatewayUrl;
pub use ws_gateway_url_error::WsGatewayUrlError;
