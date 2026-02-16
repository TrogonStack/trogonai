use opentelemetry_otlp::LogExporter;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use std::path::PathBuf;
use std::sync::OnceLock;

pub(super) static LOGGER_PROVIDER: OnceLock<SdkLoggerProvider> = OnceLock::new();

pub(super) fn init_provider(
    resource: &Resource,
) -> Result<SdkLoggerProvider, Box<dyn std::error::Error>> {
    let exporter = LogExporter::builder().with_http().build()?;

    let provider = SdkLoggerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource.clone())
        .build();

    Ok(provider)
}

pub(super) fn shutdown() {
    if let Some(provider) = LOGGER_PROVIDER.get() {
        if let Err(e) = provider.shutdown() {
            eprintln!("Failed to shutdown logger provider: {e}");
        }
    }
}

pub(super) fn get_log_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Ok(dir) = std::env::var("ACP_LOG_DIR") {
        let path = PathBuf::from(dir);
        std::fs::create_dir_all(&path)?;
        return Ok(path);
    }

    let log_dir = platform_log_dir()?;
    std::fs::create_dir_all(&log_dir)?;
    Ok(log_dir)
}

fn platform_log_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    use trogon_std::dirs::{HomeDir, StateDir, SystemDirs};

    if cfg!(target_os = "macos") {
        let home = SystemDirs
            .home_dir()
            .ok_or("HOME not set")?;
        Ok(home.join("Library").join("Logs").join("acp-nats-stdio"))
    } else {
        let base = SystemDirs
            .state_dir()
            .ok_or("could not determine state directory")?;
        Ok(base.join("acp-nats-stdio"))
    }
}
