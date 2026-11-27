use prometheus::Registry;

use crate::{
    ObservabilityArgs, ObservabilityError, TracingGuard, default_registry, init_tracing, spawn_metrics_server,
};

/// Initialize tracing and spawn the private listener, which serves the health paths and metrics.
///
/// The process reports not live and not ready until the caller marks it started through
/// [`ObservabilityArgs::health`].
///
/// Must be called from within a Tokio runtime. Keep the returned guard alive for
/// the lifetime of the process so spans are flushed on exit.
pub fn bootstrap(
    service_name: &str,
    service_port: u16,
    args: &ObservabilityArgs,
) -> Result<(TracingGuard, Registry), ObservabilityError> {
    let guard = init_tracing(service_name, args.otlp_endpoint.as_deref())?;
    let registry = default_registry(service_name)?;
    spawn_metrics_server(
        args.resolve_metrics_addr(service_port),
        registry.clone(),
        args.health().clone(),
    );
    Ok((guard, registry))
}
