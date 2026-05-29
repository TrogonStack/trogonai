use crate::schema_cache::{SchemaCacheRuntime, lookup_tool_schema};

use super::engine::{redact_with_options, RedactionOptions};
use super::outcome::{RedactionApplyResult, RedactionOutcome};
use super::registry::{RedactionDirection, RedactionRegistry};
use super::ruleset::RedactionRuleset;
use super::schema_validate::path_in_schema;
use super::path::ParsedPath;

pub struct SchemaRedactionContext<'a> {
    pub server_id: &'a str,
    pub tool_name: &'a str,
    pub direction: RedactionDirection,
    pub hash_salt: Option<&'a str>,
}

#[must_use]
pub async fn apply_schema_redaction(
    schema_runtime: Option<&SchemaCacheRuntime>,
    registry: Option<&RedactionRegistry>,
    ctx: SchemaRedactionContext<'_>,
    doc: &mut serde_json::Value,
) -> RedactionApplyResult {
    let Some(registry) = registry else {
        return RedactionApplyResult::Passthrough;
    };
    let Some(ruleset) = registry.lookup(ctx.server_id, ctx.tool_name, ctx.direction) else {
        return RedactionApplyResult::Passthrough;
    };
    if ruleset.rules().is_empty() {
        return RedactionApplyResult::Passthrough;
    }

    let schema_tool = RedactionRegistry::schema_tool_name(ctx.tool_name, ctx.direction);
    let schema = match schema_runtime {
        Some(runtime) => match lookup_tool_schema(runtime, &crate::schema_cache::ServerId::new(ctx.server_id), &schema_tool).await {
            Ok(Some(cached)) => Some(cached.schema),
            Ok(None) => None,
            Err(_) => None,
        },
        None => None,
    };

    let Some(schema) = schema else {
        return RedactionApplyResult::Skipped {
            reason: format!(
                "schema unavailable for server={} tool={} direction={:?}",
                ctx.server_id, schema_tool, ctx.direction
            ),
        };
    };

    let envelope_schema = wrap_schema_for_direction(&schema, ctx.direction);
    if !validate_ruleset_against_schema(&ruleset, &envelope_schema) {
        return RedactionApplyResult::Skipped {
            reason: format!(
                "redaction path not declared in schema for server={} tool={}",
                ctx.server_id, schema_tool
            ),
        };
    }

    let options = RedactionOptions {
        hash_salt: ctx.hash_salt,
    };
    let outcome = redact_with_options(doc, &ruleset, &options);
    RedactionApplyResult::Applied(outcome)
}

fn validate_ruleset_against_schema(ruleset: &RedactionRuleset, schema: &serde_json::Value) -> bool {
    ruleset.rules().iter().all(|rule| {
        ParsedPath::parse(rule.path.as_str())
            .map(|parsed| path_in_schema(schema, &parsed))
            .unwrap_or(false)
    })
}

fn wrap_schema_for_direction(schema: &serde_json::Value, direction: RedactionDirection) -> serde_json::Value {
    let fragment_key = match direction {
        RedactionDirection::Request => "params",
        RedactionDirection::Response => "result",
    };
    serde_json::json!({
        "type": "object",
        "properties": {
            fragment_key: schema
        }
    })
}

#[must_use]
pub fn merge_outcomes(mut base: RedactionOutcome, extra: RedactionOutcome) -> RedactionOutcome {
    base.rewrites.extend(extra.rewrites);
    base
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redaction::rule::{JsonPath, RedactionAction, RedactionRule};
    use crate::schema_cache::{SchemaCacheConfig, SchemaCacheRuntime, sniff_tools_list_reply};
    use crate::schema_cache::ServerId;

    #[tokio::test]
    async fn missing_schema_skips_redaction() {
        let registry = RedactionRegistry::new();
        registry.register(
            "srv",
            "echo",
            RedactionDirection::Request,
            RedactionRuleset::builder()
                .rule(RedactionRule {
                    path: JsonPath::parse("$.params.token").expect("path"),
                    action: RedactionAction::Hash,
                })
                .build(),
        );
        let runtime = SchemaCacheRuntime::new(SchemaCacheConfig::default());
        let mut doc = serde_json::json!({ "params": { "token": "plain" } });

        let result = apply_schema_redaction(
            Some(&runtime),
            Some(&registry),
            SchemaRedactionContext {
                server_id: "srv",
                tool_name: "echo",
                direction: RedactionDirection::Request,
                hash_salt: None,
            },
            &mut doc,
        )
        .await;

        assert!(matches!(result, RedactionApplyResult::Skipped { .. }));
        assert_eq!(doc["params"]["token"], "plain");
    }

    #[tokio::test]
    async fn cached_schema_enables_redaction() {
        let registry = RedactionRegistry::new();
        registry.register(
            "srv",
            "echo",
            RedactionDirection::Request,
            RedactionRuleset::builder()
                .rule(RedactionRule {
                    path: JsonPath::parse("$.params.token").expect("path"),
                    action: RedactionAction::Mask,
                })
                .build(),
        );
        let runtime = SchemaCacheRuntime::new(SchemaCacheConfig::default());
        let server = ServerId::new("srv");
        let list = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "tools": [{
                    "name": "echo",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "token": { "type": "string" }
                        }
                    }
                }]
            }
        });
        sniff_tools_list_reply(&runtime.cache, &runtime.config, &server, &serde_json::to_vec(&list).unwrap())
            .await
            .expect("sniff");

        let mut doc = serde_json::json!({ "params": { "token": "plain" } });
        let result = apply_schema_redaction(
            Some(&runtime),
            Some(&registry),
            SchemaRedactionContext {
                server_id: "srv",
                tool_name: "echo",
                direction: RedactionDirection::Request,
                hash_salt: None,
            },
            &mut doc,
        )
        .await;

        assert!(matches!(result, RedactionApplyResult::Applied(_)));
        assert_eq!(doc["params"]["token"], "***");
    }
}
