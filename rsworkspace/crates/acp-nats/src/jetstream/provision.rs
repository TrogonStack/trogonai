use tracing::info;
use trogon_nats::jetstream::JetStreamContext;

use super::streams;

#[derive(Debug)]
pub struct ProvisionError(pub String);

impl std::fmt::Display for ProvisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "stream provisioning failed: {}", self.0)
    }
}

impl std::error::Error for ProvisionError {}

pub async fn provision_streams<J: JetStreamContext>(
    js: &J,
    prefix: &str,
) -> Result<(), ProvisionError> {
    for config in streams::all_configs(prefix) {
        let name = config.name.clone();
        js.get_or_create_stream(config)
            .await
            .map_err(|e| ProvisionError(format!("{name}: {e}")))?;
        info!(stream = %name, "Provisioned JetStream stream");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_nats::jetstream::MockJetStreamContext;

    #[tokio::test]
    async fn provision_creates_six_streams() {
        let ctx = MockJetStreamContext::new();
        provision_streams(&ctx, "acp").await.unwrap();
        assert_eq!(ctx.created_streams().len(), 6);
    }

    #[tokio::test]
    async fn provision_creates_correct_stream_names() {
        let ctx = MockJetStreamContext::new();
        provision_streams(&ctx, "acp").await.unwrap();
        let names: Vec<String> = ctx
            .created_streams()
            .iter()
            .map(|c| c.name.clone())
            .collect();
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
        provision_streams(&ctx, "myapp").await.unwrap();
        let names: Vec<String> = ctx
            .created_streams()
            .iter()
            .map(|c| c.name.clone())
            .collect();
        assert!(names.contains(&"MYAPP_COMMANDS".to_string()));
    }

    #[tokio::test]
    async fn provision_returns_error_on_failure() {
        let ctx = MockJetStreamContext::new();
        ctx.fail_next();
        let result = provision_streams(&ctx, "acp").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("ACP_COMMANDS"));
    }

    #[tokio::test]
    async fn provision_is_idempotent() {
        let ctx = MockJetStreamContext::new();
        provision_streams(&ctx, "acp").await.unwrap();
        provision_streams(&ctx, "acp").await.unwrap();
        assert_eq!(ctx.created_streams().len(), 12);
    }
}
