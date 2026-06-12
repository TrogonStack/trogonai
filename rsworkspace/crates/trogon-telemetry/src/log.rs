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

use crate::TelemetryProviderShutdownError;

pub(crate) static LOGGER_PROVIDER: OnceLock<SdkLoggerProvider> = OnceLock::new();

pub(crate) fn init_provider(resource: &Resource) -> anyhow::Result<SdkLoggerProvider> {
    let exporter = LogExporter::builder().with_http().build()?;

    let provider = SdkLoggerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource.clone())
        .build();

    Ok(provider)
}

pub(crate) fn shutdown() -> Result<(), TelemetryProviderShutdownError> {
    if let Some(provider) = LOGGER_PROVIDER.get() {
        provider
            .shutdown()
            .map_err(|source| TelemetryProviderShutdownError::Logger { source: source.into() })?;
    }
    Ok(())
}

pub(crate) fn ensure_log_dir<E: ReadEnv, F: CreateDirAll>(
    service_name: ServiceName,
    env: &E,
    fs: &F,
) -> anyhow::Result<PathBuf> {
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
fn platform_log_dir(service_name: ServiceName) -> anyhow::Result<PathBuf> {
    let home = SystemDirs
        .home_dir()
        .ok_or_else(|| anyhow::anyhow!("could not determine home directory"))?;
    Ok(home.join("Library").join("Logs").join(service_name.as_str()))
}

#[cfg(not(target_os = "macos"))]
fn platform_log_dir(service_name: ServiceName) -> anyhow::Result<PathBuf> {
    let base = SystemDirs
        .state_dir()
        .ok_or_else(|| anyhow::anyhow!("could not determine state directory"))?;
    Ok(base.join(service_name.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::KeyValue;
    use trogon_std::env::InMemoryEnv;
    use trogon_std::fs::MemFs;

    #[test]
    fn ensure_log_dir_uses_env_override() {
        let env = InMemoryEnv::new();
        env.set("TROGON_LOG_DIR", "/custom/logs");
        let fs = MemFs::new();

        let dir = ensure_log_dir(ServiceName::AcpNatsStdio, &env, &fs).unwrap();
        assert_eq!(dir, PathBuf::from("/custom/logs"));
        assert!(fs.dir_exists(&PathBuf::from("/custom/logs")));
    }

    #[test]
    fn ensure_log_dir_ignores_legacy_acp_env_override() {
        let env = InMemoryEnv::new();
        env.set("ACP_LOG_DIR", "/legacy/logs");
        let fs = MemFs::new();

        let dir = ensure_log_dir(ServiceName::AcpNatsStdio, &env, &fs).unwrap();

        assert!(dir.ends_with(ServiceName::AcpNatsStdio.as_str()));
        assert!(!fs.dir_exists(&PathBuf::from("/legacy/logs")));
    }

    #[test]
    fn ensure_log_dir_falls_back_to_platform() {
        let env = InMemoryEnv::new();
        let fs = MemFs::new();

        let dir = ensure_log_dir(ServiceName::AcpNatsStdio, &env, &fs).unwrap();
        assert!(dir.ends_with(ServiceName::AcpNatsStdio.as_str()));
    }

    #[test]
    fn platform_log_dir_returns_path_ending_with_service_name() {
        let dir = platform_log_dir(ServiceName::AcpNatsServer).unwrap();
        assert!(dir.ends_with(ServiceName::AcpNatsServer.as_str()));
    }

    #[test]
    fn init_provider_lifecycle() {
        let resource = opentelemetry_sdk::Resource::builder()
            .with_service_name("test-log")
            .with_attributes(vec![KeyValue::new("test", "true")])
            .build();

        let provider = init_provider(&resource);
        assert!(provider.is_ok());
    }
}
