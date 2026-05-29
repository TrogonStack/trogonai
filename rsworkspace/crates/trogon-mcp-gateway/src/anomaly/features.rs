use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AnomalyFeatures {
    pub chain_depth: u32,
    pub is_novel_triple: bool,
    pub exchange_rate_per_min: u32,
    pub tenant_id: String,
    pub agent_id: String,
    pub purpose: String,
    pub target: String,
    pub request_id: String,
    pub ts_unix_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_stable_field_shape() {
        let features = AnomalyFeatures {
            chain_depth: 2,
            is_novel_triple: true,
            exchange_rate_per_min: 42,
            tenant_id: "acme".into(),
            agent_id: "agent/oncall".into(),
            purpose: "incident_response".into(),
            target: "urn:trogon:mcp:backend:acme:github".into(),
            request_id: "req-1".into(),
            ts_unix_ms: 1_700_000_000_000,
        };

        let json = serde_json::to_value(&features).expect("serialize");
        assert_eq!(json["chain_depth"], 2);
        assert_eq!(json["is_novel_triple"], true);
        assert_eq!(json["exchange_rate_per_min"], 42);
        assert_eq!(json["tenant_id"], "acme");
        assert_eq!(json["agent_id"], "agent/oncall");
        assert_eq!(json["purpose"], "incident_response");
        assert_eq!(json["target"], "urn:trogon:mcp:backend:acme:github");
        assert_eq!(json["request_id"], "req-1");
        assert_eq!(json["ts_unix_ms"], serde_json::json!(1_700_000_000_000_i64));

        let keys: Vec<_> = json.as_object().expect("object").keys().cloned().collect();
        assert_eq!(
            keys,
            vec![
                "agent_id",
                "chain_depth",
                "exchange_rate_per_min",
                "is_novel_triple",
                "purpose",
                "request_id",
                "target",
                "tenant_id",
                "ts_unix_ms",
            ]
        );
    }
}
