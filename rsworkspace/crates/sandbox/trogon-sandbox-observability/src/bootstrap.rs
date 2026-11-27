use prometheus::Registry;

use crate::{
    ObservabilityArgs, ObservabilityError, TracingGuard, default_registry, init_tracing, spawn_metrics_server,
};

/// Initialize tracing and spawn the default metrics listener for a sandbox binary.
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
    spawn_metrics_server(args.resolve_metrics_addr(service_port), registry.clone());
    Ok((guard, registry))
}
