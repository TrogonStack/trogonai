use tracing::info;
use trogon_nats::jetstream::JetStreamContext;

use super::streams;

#[derive(Debug, thiserror::Error)]
#[error("stream provisioning failed for {stream}")]
pub struct ProvisionError<Source> {
    stream: String,
    #[source]
    source: Source,
}

pub async fn provision_streams<J: JetStreamContext>(
    js: &J,
    prefix: &crate::acp_prefix::AcpPrefix,
) -> Result<(), ProvisionError<J::Error>> {
    for config in streams::all_configs(prefix) {
        let name = config.name.clone();
        js.get_or_create_stream(config).await.map_err(|source| ProvisionError {
            stream: name.clone(),
            source,
        })?;
        info!(stream = %name, "Provisioned JetStream stream");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp_prefix::AcpPrefix;
    use std::error::Error;
    use trogon_nats::jetstream::MockJetStreamContext;

    fn p(s: &str) -> AcpPrefix {
        AcpPrefix::new(s).expect("test prefix")
    }

    #[tokio::test]
    async fn provision_creates_six_streams() {
        let ctx = MockJetStreamContext::new();
        provision_streams(&ctx, &p("acp")).await.unwrap();
        assert_eq!(ctx.created_streams().len(), 6);
    }

    #[tokio::test]
    async fn provision_creates_correct_stream_names() {
        let ctx = MockJetStreamContext::new();
        provision_streams(&ctx, &p("acp")).await.unwrap();
        let names: Vec<String> = ctx.created_streams().iter().map(|c| c.name.clone()).collect();
        assert!(names.contains(&"ACP_COMMANDS".to_string()));
        assert!(names.contains(&"ACP_RESPONSES".to_string()));
        assert!(names.contains(&"ACP_CLIENT_OPS".to_string()));
        assert!(names.contains(&"ACP_NOTIFICATIONS".to_string()));
        assert!(names.contains(&"ACP_GLOBAL".to_string()));
        assert!(names.contains(&"ACP_GLOBAL_EXT".to_string()));
    }

    #[tokio::test]
    async fn provision_with_custom_prefix() {
        let ctx = MockJetStreamContext::new();
        provision_streams(&ctx, &p("myapp")).await.unwrap();
        let names: Vec<String> = ctx.created_streams().iter().map(|c| c.name.clone()).collect();
        assert!(names.contains(&"MYAPP_COMMANDS".to_string()));
    }

    #[tokio::test]
    async fn provision_returns_error_on_failure() {
        let ctx = MockJetStreamContext::new();
        ctx.fail_next();
        let result = provision_streams(&ctx, &p("acp")).await;
        let error = result.unwrap_err();
        assert!(error.to_string().contains("ACP_COMMANDS"));
        assert!(error.source().is_some());
    }

    #[tokio::test]
    async fn provision_is_idempotent() {
        let ctx = MockJetStreamContext::new();
        provision_streams(&ctx, &p("acp")).await.unwrap();
        provision_streams(&ctx, &p("acp")).await.unwrap();
        assert_eq!(ctx.created_streams().len(), 12);
    }
}
