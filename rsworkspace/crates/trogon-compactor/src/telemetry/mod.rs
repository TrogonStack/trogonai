//! OpenTelemetry instruments for the compactor (ADR 0008). Setup (provider, OTLP
//! export, `service.name`) is owned by `trogon-telemetry`; this module only emits.
pub mod metrics;
