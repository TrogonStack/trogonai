//! MCP servers emit `notifications/tools/list_changed` when their tool catalog changes;
//! the gateway must drop cached schemas for that server.

use super::key::ServerId;
use super::runtime::SchemaCacheRuntime;

pub fn should_invalidate(notification_method: &str) -> bool {
    notification_method == "notifications/tools/list_changed"
}

pub fn notification_method(payload: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(payload).ok()?;
    Some(value.get("method")?.as_str()?.to_string())
}

pub fn parse_client_id_from_notification_subject(prefix: &str, subject: &str) -> Option<String> {
    let head = format!("{prefix}.client.");
    let rest = subject.strip_prefix(head.as_str())?;
    let (client_id, _) = rest.split_once('.')?;
    if client_id.is_empty() {
        return None;
    }
    Some(client_id.to_string())
}

pub async fn handle_list_changed_notification(
    runtime: &SchemaCacheRuntime,
    prefix: &str,
    subject: &str,
    payload: &[u8],
) -> Result<bool, super::errors::SchemaCacheError> {
    let Some(method) = notification_method(payload) else {
        return Ok(false);
    };
    if !should_invalidate(method.as_str()) {
        return Ok(false);
    }
    let Some(client_id) = parse_client_id_from_notification_subject(prefix, subject) else {
        return Ok(false);
    };
    let Some(server_id) = runtime.server_for_client(client_id.as_str()) else {
        return Ok(false);
    };
    runtime.invalidate_server(&server_id).await?;
    Ok(true)
}

pub async fn handle_control_invalidate(
    runtime: &SchemaCacheRuntime,
    payload: &[u8],
) -> Result<bool, super::errors::SchemaCacheError> {
    let value: serde_json::Value = serde_json::from_slice(payload).unwrap_or(serde_json::Value::Null);
    let Some(server_id_raw) = value.get("server_id").and_then(|v| v.as_str()) else {
        return Ok(false);
    };
    runtime
        .invalidate_server(&ServerId::new(server_id_raw))
        .await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema_cache::store::SchemaCache;

    #[test]
    fn should_invalidate_only_for_tools_list_changed() {
        assert!(should_invalidate("notifications/tools/list_changed"));
        assert!(!should_invalidate("notifications/tools/list_changed/"));
        assert!(!should_invalidate("tools/list_changed"));
        assert!(!should_invalidate(""));
    }

    #[test]
    fn parse_client_id_from_subject() {
        assert_eq!(
            parse_client_id_from_notification_subject(
                "mcp",
                "mcp.client.desktop.notifications.tools.list_changed"
            ),
            Some("desktop".into())
        );
    }

    #[tokio::test]
    async fn list_changed_invalidates_bound_server() {
        let runtime = SchemaCacheRuntime::new(super::super::config::SchemaCacheConfig::default());
        let server = ServerId::new("fixture");
        runtime.record_client_server("desktop", &server);
        runtime
            .cache
            .put(
                super::super::key::SchemaCacheKey {
                    server_id: server.clone(),
                    schema_hash: super::super::key::SchemaHash::from_bytes([1; 32]),
                },
                super::super::entry::CachedSchema {
                    schema: serde_json::json!({"type": "object"}),
                    fetched_at: std::time::SystemTime::now(),
                    source: super::super::entry::SchemaSource::ToolsListSniff,
                },
                "alpha",
            )
            .await
            .expect("put");
        let payload = br#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#;
        let invalidated = handle_list_changed_notification(
            &runtime,
            "mcp",
            "mcp.client.desktop.notifications.tools.list_changed",
            payload,
        )
        .await
        .expect("handle");
        assert!(invalidated);
        assert_eq!(runtime.cache.entry_count().await, 0);
    }
}
