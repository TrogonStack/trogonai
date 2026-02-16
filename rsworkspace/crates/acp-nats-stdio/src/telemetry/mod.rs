mod log;
mod metric;
mod trace;

use acp_nats::Config;
use opentelemetry::KeyValue;
use opentelemetry::trace::TracerProvider;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use std::time::Duration;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const OTEL_SERVICE_NAME: &str = "acp-nats-stdio";

pub fn init_logger(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_thread_ids(true)
        .with_span_events(FmtSpan::CLOSE)
        .json();

    let file_layer = log::get_log_dir().ok().map(|log_dir| {
        let log_file = log_dir.join("acp-nats-stdio.log");
        let file = std::fs::File::create(&log_file).expect("failed to create log file");
        tracing::info!(log_file = %log_file.display(), "File logging enabled");
        tracing_subscriber::fmt::layer()
            .with_writer(file)
            .with_thread_ids(true)
            .with_span_events(FmtSpan::CLOSE)
            .json()
    });

    match try_init_otel(config) {
        Ok((tracer_provider, meter_provider, logger_provider)) => {
            opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

            let tracer = tracer_provider.tracer(OTEL_SERVICE_NAME);
            let otel_trace_layer = tracing_opentelemetry::layer().with_tracer(tracer);

            opentelemetry::global::set_meter_provider(meter_provider.clone());
            metric::METER_PROVIDER.set(meter_provider).ok();

            let otel_logs_layer = OpenTelemetryTracingBridge::new(&logger_provider);
            log::LOGGER_PROVIDER.set(logger_provider).ok();

            tracing_subscriber::registry()
                .with(env_filter)
                .with(stderr_layer)
                .with(file_layer)
                .with(otel_trace_layer)
                .with(otel_logs_layer)
                .init();

            tracing::info!("Logger initialized with OpenTelemetry");
        }
        Err(e) => {
            tracing_subscriber::registry()
                .with(env_filter)
                .with(stderr_layer)
                .with(file_layer)
                .init();

            tracing::warn!(
                error = %e,
                "Logger initialized without OpenTelemetry (init failed)"
            );
        }
    }

    Ok(())
}

fn try_init_otel(
    config: &Config,
) -> Result<
    (
        opentelemetry_sdk::trace::SdkTracerProvider,
        opentelemetry_sdk::metrics::SdkMeterProvider,
        opentelemetry_sdk::logs::SdkLoggerProvider,
    ),
    Box<dyn std::error::Error>,
> {
    let resource = Resource::builder()
        .with_service_name(OTEL_SERVICE_NAME)
        .with_attributes(vec![KeyValue::new("acp.prefix", config.acp_prefix.clone())])
        .build();

    let tracer_provider = trace::init_provider(&resource)?;
    let meter_provider = metric::init_provider(&resource)?;
    let logger_provider = log::init_provider(&resource)?;

    Ok((tracer_provider, meter_provider, logger_provider))
}

pub async fn shutdown_otel() {
    tracing::info!("Shutting down OpenTelemetry providers");
    tokio::time::sleep(Duration::from_millis(500)).await;

    log::shutdown();
    metric::shutdown();
    trace::shutdown();
}
