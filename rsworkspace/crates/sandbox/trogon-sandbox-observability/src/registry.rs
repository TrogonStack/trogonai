use prometheus::{IntGauge, Opts, Registry, TextEncoder};

use crate::ObservabilityError;

/// Minimal registry with a build-info gauge so empty binaries still expose `/metrics`.
///
/// `_service` is accepted so call sites can name the binary; the gauge itself is
/// shared by every service.
pub fn default_registry(_service: &str) -> Result<Registry, ObservabilityError> {
    let registry = Registry::new();
    let info = IntGauge::with_opts(Opts::new(
        "sandbox_build_info",
        "Sandbox binary build marker (always 1)",
    ))
    .map_err(ObservabilityError::Metrics)?;
    info.set(1);
    registry.register(Box::new(info)).map_err(ObservabilityError::Metrics)?;
    Ok(registry)
}

/// Render every metric family in `registry` in the Prometheus text format.
pub fn gather_text(registry: &Registry) -> Result<String, ObservabilityError> {
    let mut text = String::new();
    TextEncoder::new()
        .encode_utf8(&registry.gather(), &mut text)
        .map_err(ObservabilityError::Metrics)?;
    Ok(text)
}
