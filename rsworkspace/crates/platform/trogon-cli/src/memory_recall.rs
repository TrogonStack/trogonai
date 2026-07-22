//! Opt-in recall of entity memories from the `SESSION_MEMORIES` NATS KV bucket
//! (`trogon-memory`).

use async_nats::jetstream::{self, kv};
use futures::StreamExt;
use trogon_memory::{EntityMemory, MemoryClient, provision::provision_kv, MEMORIES_BUCKET};

/// Parse a KV key (`{actor_type}.{sanitized_actor_key}`) into its components.
pub fn parse_entity_key(key: &str) -> Option<(&str, &str)> {
    key.split_once('.')
}

/// Filter entity keys by optional query (case-insensitive substring on the full key).
pub fn filter_entity_keys(keys: &[String], query: &str) -> Vec<String> {
    let q = query.trim();
    if q.is_empty() {
        return keys.to_vec();
    }
    let needle = q.to_lowercase();
    keys.iter()
        .filter(|k| k.to_lowercase().contains(&needle))
        .cloned()
        .collect()
}

pub fn format_entity_list(keys: &[String]) -> String {
    if keys.is_empty() {
        return format!("no stored entities in {MEMORIES_BUCKET}");
    }
    let mut out = format!("stored entities in {MEMORIES_BUCKET} ({}):\n", keys.len());
    for key in keys {
        out.push_str(&format!("  {key}\n"));
    }
    out.push_str("\nuse /recall <entity-key> to show facts");
    out
}

pub fn format_entity_detail(key: &str, memory: &EntityMemory) -> String {
    let mut out = format!("entity: {key}\n");
    out.push_str(&format!(
        "facts: {}  |  updated: {}\n\n",
        memory.facts.len(),
        memory.updated_at
    ));
    if memory.facts.is_empty() {
        out.push_str("(no facts)\n");
    } else {
        for fact in &memory.facts {
            out.push_str(&format!(
                "- [{}] {} (confidence {:.0}%, session {})\n",
                fact.category,
                fact.content,
                fact.confidence * 100.0,
                fact.source_session,
            ));
        }
    }
    out
}

pub async fn list_entity_keys(nats: &async_nats::Client) -> anyhow::Result<Vec<String>> {
    let js = jetstream::new(nats.clone());
    let store = provision_kv(&js).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut stream = kv::Store::keys(&store).await?;
    let mut keys = Vec::new();
    while let Some(item) = stream.next().await {
        keys.push(item?);
    }
    keys.sort();
    Ok(keys)
}

pub async fn fetch_entity_by_key(
    nats: &async_nats::Client,
    kv_key: &str,
) -> anyhow::Result<Option<EntityMemory>> {
    let (actor_type, actor_key) = parse_entity_key(kv_key)
        .ok_or_else(|| anyhow::anyhow!("invalid entity key: {kv_key}"))?;
    let js = jetstream::new(nats.clone());
    let store = provision_kv(&js).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    let client = MemoryClient::new(store);
    client
        .get(actor_type, actor_key)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// List or fetch entity memories. With no query, lists all keys; with a query,
/// filters keys and shows full detail when exactly one match remains.
pub async fn handle_recall(nats: &async_nats::Client, query: &str) -> anyhow::Result<String> {
    let keys = list_entity_keys(nats).await?;
    let filtered = filter_entity_keys(&keys, query);

    if filtered.is_empty() {
        return Ok(if query.trim().is_empty() {
            format!("no stored entities in {MEMORIES_BUCKET}")
        } else {
            format!("no entities matching '{}'", query.trim())
        });
    }

    if filtered.len() == 1 {
        let key = &filtered[0];
        if let Some(memory) = fetch_entity_by_key(nats, key).await? {
            return Ok(format_entity_detail(key, &memory));
        }
        return Ok(format!("entity: {key}\n(no memory data)"));
    }

    let mut out = format_entity_list(&filtered);
    if !query.trim().is_empty() {
        out.push_str("\nrefine query to a single entity key to show facts");
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_memory::types::RawFact;

    #[test]
    fn parse_entity_key_splits_on_first_dot() {
        assert_eq!(
            parse_entity_key("pr.owner_repo_456"),
            Some(("pr", "owner_repo_456"))
        );
        assert_eq!(parse_entity_key("agent"), None);
    }

    #[test]
    fn filter_entity_keys_empty_query_returns_all() {
        let keys = vec!["pr.a".into(), "pr.b".into()];
        assert_eq!(filter_entity_keys(&keys, ""), keys);
    }

    #[test]
    fn filter_entity_keys_matches_case_insensitive_substring() {
        let keys = vec!["pr.owner_repo_1".into(), "issue.trogon_42".into()];
        let out = filter_entity_keys(&keys, "OWNER");
        assert_eq!(out, vec!["pr.owner_repo_1".to_string()]);
    }

    #[test]
    fn format_entity_list_empty_reports_bucket() {
        let out = format_entity_list(&[]);
        assert!(out.contains(MEMORIES_BUCKET));
        assert!(out.contains("no stored entities"));
    }

    #[test]
    fn format_entity_list_includes_keys_and_hint() {
        let out = format_entity_list(&["pr.a".into(), "pr.b".into()]);
        assert!(out.contains("pr.a"));
        assert!(out.contains("pr.b"));
        assert!(out.contains("/recall"));
    }

    #[test]
    fn format_entity_detail_renders_facts() {
        let mut memory = EntityMemory::default();
        memory.merge(
            vec![RawFact {
                category: "preference".into(),
                content: "uses tabs".into(),
                confidence: 0.9,
            }],
            "sess-1",
        );
        let out = format_entity_detail("pr.owner_repo_1", &memory);
        assert!(out.contains("entity: pr.owner_repo_1"));
        assert!(out.contains("[preference] uses tabs"));
        assert!(out.contains("sess-1"));
    }
}
