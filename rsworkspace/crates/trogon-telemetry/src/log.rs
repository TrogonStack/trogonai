use crate::ServiceName;
use opentelemetry_otlp::LogExporter;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use std::path::PathBuf;
use std::sync::OnceLock;
#[cfg(target_os = "macos")]
use trogon_std::dirs::HomeDir;
#[cfg(not(target_os = "macos"))]
use trogon_std::dirs::StateDir;
use trogon_std::dirs::SystemDirs;
use trogon_std::env::ReadEnv;
use trogon_std::fs::CreateDirAll;

pub(crate) static LOGGER_PROVIDER: OnceLock<SdkLoggerProvider> = OnceLock::new();

pub(crate) fn init_provider(resource: &Resource) -> Result<SdkLoggerProvider, Box<dyn std::error::Error>> {
    let exporter = LogExporter::builder().with_http().build()?;

    let provider = SdkLoggerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource.clone())
        .build();

    Ok(provider)
}

pub(crate) fn shutdown() -> Result<(), String> {
    if let Some(provider) = LOGGER_PROVIDER.get() {
        provider
            .shutdown()
            .map_err(|e| format!("failed to shutdown logger provider: {e}"))?;
    }
    Ok(())
}

pub(crate) fn ensure_log_dir<E: ReadEnv, F: CreateDirAll>(
    service_name: ServiceName,
    env: &E,
    fs: &F,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Ok(dir) = env.var("TROGON_LOG_DIR") {
        let path = PathBuf::from(dir);
        fs.create_dir_all(&path)?;
        return Ok(path);
    }

    let log_dir = platform_log_dir(service_name)?;
    fs.create_dir_all(&log_dir)?;
    Ok(log_dir)
}

#[cfg(target_os = "macos")]
fn platform_log_dir(service_name: ServiceName) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let home = SystemDirs.home_dir().ok_or("could not determine home directory")?;
    Ok(home.join("Library").join("Logs").join(service_name.as_str()))
}

#[cfg(not(target_os = "macos"))]
fn platform_log_dir(service_name: ServiceName) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let base = SystemDirs.state_dir().ok_or("could not determine state directory")?;
    Ok(base.join(service_name.as_str()))
}

#[cfg(test)]
mod tests;
