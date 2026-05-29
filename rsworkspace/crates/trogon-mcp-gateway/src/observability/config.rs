use trogon_std::env::ReadEnv;

use super::audit_bridge::SiemFormat;
use super::errors::ObservabilityError;

pub const ENV_OTEL_ENDPOINT: &str = "TROGON_OTEL_ENDPOINT";
pub const ENV_OTEL_SERVICE_NAME: &str = "TROGON_OTEL_SERVICE_NAME";
pub const ENV_SIEM_SUBJECT: &str = "TROGON_SIEM_SUBJECT";
pub const ENV_AUDIT_CONSUMER: &str = "TROGON_AUDIT_CONSUMER";
pub const ENV_SIEM_FORMAT: &str = "TROGON_SIEM_FORMAT";
pub const ENV_AUDIT_STREAM: &str = "MCP_GATEWAY_AUDIT_STREAM";
pub const ENV_MCP_PREFIX: &str = "MCP_PREFIX";

pub const DEFAULT_AUDIT_CONSUMER: &str = "trogon-siem-audit-bridge";
pub const DEFAULT_AUDIT_STREAM: &str = "MCP_AUDIT";
pub const DEFAULT_MCP_PREFIX: &str = "mcp";

/// Operator-facing observability settings (OTel export + audit→SIEM bridge).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservabilityConfig {
    pub otel_endpoint: Option<String>,
    pub otel_service_name: Option<String>,
    pub siem_subject: Option<String>,
    pub audit_consumer_durable: String,
    pub siem_format: SiemFormat,
    pub audit_stream_name: String,
    pub mcp_prefix: String,
}

impl ObservabilityConfig {
    #[must_use]
    pub fn from_env<E: ReadEnv>(env: &E) -> Self {
        let otel_endpoint = env.var(ENV_OTEL_ENDPOINT).ok().filter(|value| !value.is_empty());
        let otel_service_name = env
            .var(ENV_OTEL_SERVICE_NAME)
            .ok()
            .filter(|value| !value.is_empty());
        let siem_subject = env.var(ENV_SIEM_SUBJECT).ok().filter(|value| !value.is_empty());
        let audit_consumer_durable = env
            .var(ENV_AUDIT_CONSUMER)
            .unwrap_or_else(|_| DEFAULT_AUDIT_CONSUMER.to_string());
        let siem_format = env
            .var(ENV_SIEM_FORMAT)
            .ok()
            .as_deref()
            .map(SiemFormat::parse)
            .unwrap_or(SiemFormat::Raw);
        let audit_stream_name = env
            .var(ENV_AUDIT_STREAM)
            .unwrap_or_else(|_| DEFAULT_AUDIT_STREAM.to_string());
        let mcp_prefix = env
            .var(ENV_MCP_PREFIX)
            .unwrap_or_else(|_| DEFAULT_MCP_PREFIX.to_string());

        Self {
            otel_endpoint,
            otel_service_name,
            siem_subject,
            audit_consumer_durable,
            siem_format,
            audit_stream_name,
            mcp_prefix,
        }
    }

    /// Maps Trogon-specific env vars onto standard `OTEL_*` variables before SDK init.
    pub fn apply_otel_env(&self) -> Result<(), ObservabilityError> {
        if let Some(endpoint) = &self.otel_endpoint {
            // trogon-telemetry uses OTLP/HTTP JSON (port 4318) via opentelemetry-otlp.
            unsafe {
                std::env::set_var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", endpoint);
                std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", endpoint);
            }
        }
        if let Some(service_name) = &self.otel_service_name {
            unsafe {
                std::env::set_var("OTEL_SERVICE_NAME", service_name);
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn audit_filter_subject(&self) -> String {
        format!("{}.audit.>", self.mcp_prefix)
    }

    #[must_use]
    pub fn siem_dry_run_stdout(&self) -> bool {
        self.siem_subject
            .as_deref()
            .is_some_and(|subject| subject == "-" || subject.eq_ignore_ascii_case("stdout"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use trogon_std::env::ReadEnv;

    struct TestEnv {
        values: HashMap<String, String>,
    }

    impl TestEnv {
        fn with(values: &[(&str, &str)]) -> Self {
            Self {
                values: values
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                    .collect(),
            }
        }
    }

    impl ReadEnv for TestEnv {
        fn var(&self, key: &str) -> Result<String, std::env::VarError> {
            self.values
                .get(key)
                .cloned()
                .ok_or(std::env::VarError::NotPresent)
        }
    }

    #[test]
    fn from_env_applies_defaults_when_unset() {
        let env = TestEnv { values: HashMap::new() };
        let config = ObservabilityConfig::from_env(&env);
        assert!(config.otel_endpoint.is_none());
        assert!(config.otel_service_name.is_none());
        assert!(config.siem_subject.is_none());
        assert_eq!(config.audit_consumer_durable, DEFAULT_AUDIT_CONSUMER);
        assert_eq!(config.siem_format, SiemFormat::Raw);
        assert_eq!(config.audit_stream_name, DEFAULT_AUDIT_STREAM);
        assert_eq!(config.mcp_prefix, DEFAULT_MCP_PREFIX);
    }

    #[test]
    fn from_env_reads_trogon_variables() {
        let env = TestEnv::with(&[
            (ENV_OTEL_ENDPOINT, "http://collector:4318/v1/traces"),
            (ENV_OTEL_SERVICE_NAME, "gateway-siem-sidecar"),
            (ENV_SIEM_SUBJECT, "siem.audit.events"),
            (ENV_AUDIT_CONSUMER, "siem-bridge-test"),
            (ENV_SIEM_FORMAT, "raw"),
            (ENV_AUDIT_STREAM, "AUDIT_TEST"),
            (ENV_MCP_PREFIX, "acme.mcp"),
        ]);

        let config = ObservabilityConfig::from_env(&env);
        assert_eq!(
            config.otel_endpoint.as_deref(),
            Some("http://collector:4318/v1/traces")
        );
        assert_eq!(config.otel_service_name.as_deref(), Some("gateway-siem-sidecar"));
        assert_eq!(config.siem_subject.as_deref(), Some("siem.audit.events"));
        assert_eq!(config.audit_consumer_durable, "siem-bridge-test");
        assert_eq!(config.audit_stream_name, "AUDIT_TEST");
        assert_eq!(config.mcp_prefix, "acme.mcp");
        assert_eq!(config.audit_filter_subject(), "acme.mcp.audit.>");
    }

    #[test]
    fn apply_otel_env_sets_standard_exporter_variables() {
        let config = ObservabilityConfig {
            otel_endpoint: Some("http://127.0.0.1:4318".into()),
            otel_service_name: Some("trogon-mcp-gateway".into()),
            siem_subject: None,
            audit_consumer_durable: DEFAULT_AUDIT_CONSUMER.into(),
            siem_format: SiemFormat::Raw,
            audit_stream_name: DEFAULT_AUDIT_STREAM.into(),
            mcp_prefix: DEFAULT_MCP_PREFIX.into(),
        };

        let _guard = EnvGuard::new(&[
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "OTEL_SERVICE_NAME",
        ]);
        config.apply_otel_env().expect("apply");

        assert_eq!(
            std::env::var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT").unwrap(),
            "http://127.0.0.1:4318"
        );
        assert_eq!(
            std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").unwrap(),
            "http://127.0.0.1:4318"
        );
        assert_eq!(std::env::var("OTEL_SERVICE_NAME").unwrap(), "trogon-mcp-gateway");
    }

    struct EnvGuard {
        keys: Vec<String>,
        prior: Vec<Option<String>>,
    }

    impl EnvGuard {
        fn new(keys: &[&str]) -> Self {
            let prior = keys
                .iter()
                .map(|key| std::env::var(*key).ok())
                .collect();
            Self {
                keys: keys.iter().map(|key| (*key).to_string()).collect(),
                prior,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in self.keys.iter().zip(self.prior.iter()) {
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
    }
}
