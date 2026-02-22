use opentelemetry_otlp::LogExporter;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use std::path::PathBuf;
use std::sync::OnceLock;
use trogon_std::dirs::{HomeDir, StateDir, SystemDirs};
use trogon_std::env::ReadEnv;
use trogon_std::fs::CreateDirAll;

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

pub(super) fn force_flush() {
    if let Some(provider) = LOGGER_PROVIDER.get()
        && let Err(e) = provider.force_flush()
    {
        eprintln!("Failed to flush logger provider: {e}");
    }
}

pub(super) fn shutdown() {
    if let Some(provider) = LOGGER_PROVIDER.get()
        && let Err(e) = provider.shutdown()
    {
        eprintln!("Failed to shutdown logger provider: {e}");
    }
}

pub(super) fn ensure_log_dir<E: ReadEnv, F: CreateDirAll>(
    env: &E,
    fs: &F,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Ok(dir) = env.var("ACP_LOG_DIR") {
        let path = PathBuf::from(dir);
        fs.create_dir_all(&path)?;
        return Ok(path);
    }

    let log_dir = platform_log_dir()?;
    fs.create_dir_all(&log_dir)?;
    Ok(log_dir)
}

#[cfg(target_os = "macos")]
fn platform_log_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(home) = SystemDirs.home_dir() {
        Ok(home.join("Library").join("Logs").join("acp-nats-stdio"))
    } else {
        Ok(SystemDirs
            .state_dir()
            .ok_or("could not determine home or state directory")?
            .join("acp-nats-stdio"))
    }
}

#[cfg(not(target_os = "macos"))]
fn platform_log_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let base = SystemDirs
        .state_dir()
        .ok_or("could not determine state directory")?;
    Ok(base.join("acp-nats-stdio"))
}
