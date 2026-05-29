//! Integration tests for anomaly side-stream emission (`mcp.anomaly.features.>`) and scaffold
//! for adaptive-access risk features exposed to CEL as `risk.*`.
//!
//! The previously merged adaptive-access branch added risk evaluator hooks. Once Pin 8
//! `risk.*` variables land, these tests drive the live NATS gateway harness and assert
//! sliding-window counters, geo-derived signals, per-caller isolation, TTL expiry, and
//! audit enrichment via `risk.score`.
//!
//! Scenarios:
//! 1. `risk.request_rate_1m_per_caller` — sliding 1-minute window; CEL `> 100` denies abusers.
//! 2. `risk.distinct_tools_5m` — many distinct tools in 5 minutes signals credential theft.
//! 3. `risk.geo_velocity_km_per_h` — impossible travel from consecutive `nats.client_ip` geo.
//! 4. `risk.failed_authz_recent` — recent `-32100` policy_deny count for this caller.
//! 5. `risk.new_device_score` — 1.0 when `device_id` claim is unseen for `jwt.sub`, else 0.0.
//! 6. Feature TTL — expired features evaluate as 0/null in CEL.
//! 7. Per-caller isolation — no cross-caller or cross-tenant aggregate pollution.
//! 8. Audit field — anomaly counters increment `risk.score` on each request audit envelope.
//!
//! Cross-references:
//! - Previously merged `adaptive-access` branch (`docs/identity/adaptive-access.md`)
//! - `reference-cel-variables.md` (CEL variable namespace; proposed `risk.*` root)
//! - `docs/identity/reference-cel-variables.md` (Pin 8 roots and host ABI)
//! - `docs/adr/0008-policy-dsl.md` (CEL policy DSL; `risk_adjunct` tier)
//!
//! Harness: same types as `tests/e2e_nats_forward.rs` — `mcp_nats::Config`, `McpPrefix`,
//! `trogon_nats::NatsAuth`, `GatewaySettings`, `AllowAllPermissionChecker`, JWT ingress.

#![allow(unused_imports)]

use std::sync::Arc;
use std::time::Duration;

use mcp_nats::{Config as McpConfig, McpPrefix};
use trogon_mcp_gateway::authz::AllowAllPermissionChecker;
use trogon_mcp_gateway::gateway::GatewaySettings;
use trogon_mcp_gateway::rpc_codes;
use futures::StreamExt;
use trogon_mcp_gateway::anomaly::{
    AnomalyEmitter, AnomalyFeatures, NoveltyTracker, RateTracker, subject_for_tenant,
};
use trogon_nats::{NatsAuth, NatsConfig, connect};

const POLICY_DENY: i32 = rpc_codes::POLICY_DENY;

async fn connect_nats_or_skip() -> Option<(async_nats::Client, async_nats::Client)> {
    let url = std::env::var("NATS_URL").unwrap_or_else(|_| "nats://127.0.0.1:4222".into());
    let connect = async move {
        let publisher = async_nats::connect(url.clone()).await.ok()?;
        let subscriber = async_nats::connect(url).await.ok()?;
        Some((publisher, subscriber))
    };
    match tokio::time::timeout(Duration::from_secs(2), connect).await {
        Ok(Some(clients)) => Some(clients),
        _ => {
            eprintln!("skip: NATS unavailable (set NATS_URL or start nats-server)");
            None
        }
    }
}

mod side_stream {
    use super::*;

    #[test]
    fn payload_shape_matches_anomaly_features_schema() {
        let features = AnomalyFeatures {
            chain_depth: 1,
            is_novel_triple: false,
            exchange_rate_per_min: 3,
            tenant_id: "acme".into(),
            agent_id: "agent/oncall".into(),
            purpose: "incident_response".into(),
            target: "urn:trogon:mcp:backend:acme:github".into(),
            request_id: "req-shape".into(),
            ts_unix_ms: 1_700_000_000_000,
        };

        let json = serde_json::to_value(&features).expect("serialize features");
        for key in [
            "chain_depth",
            "is_novel_triple",
            "exchange_rate_per_min",
            "tenant_id",
            "agent_id",
            "purpose",
            "target",
            "request_id",
            "ts_unix_ms",
        ] {
            assert!(json.get(key).is_some(), "missing key {key}");
        }
        assert_eq!(json["tenant_id"], "acme");
        assert_eq!(json["is_novel_triple"], false);
    }

    #[tokio::test]
    async fn novel_tuple_emit_publishes_is_novel_triple_flag() {
        let Some((publisher, subscriber)) = connect_nats_or_skip().await else {
            return;
        };

        let tenant = format!("tenant-{}", uuid::Uuid::now_v7().as_simple());
        let subject = subject_for_tenant(&tenant);
        let mut subscription = subscriber.subscribe(subject.clone()).await.expect("subscribe");
        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut novelty = NoveltyTracker::default();
        let ts = trogon_mcp_gateway::anomaly::now_unix() * 1_000;
        let is_novel = novelty.observe(&tenant, "agent/oncall", "ops", "server/github", ts);
        assert!(is_novel);

        let features = AnomalyFeatures {
            chain_depth: 1,
            is_novel_triple: is_novel,
            exchange_rate_per_min: 1,
            tenant_id: tenant.clone(),
            agent_id: "agent/oncall".into(),
            purpose: "ops".into(),
            target: "server/github".into(),
            request_id: "req-novel".into(),
            ts_unix_ms: ts,
        };

        AnomalyEmitter::new(publisher)
            .emit(&features)
            .await
            .expect("emit novel tuple features");

        let msg = tokio::time::timeout(Duration::from_secs(3), subscription.next())
            .await
            .expect("timed out waiting for anomaly feature")
            .expect("subscription closed");
        assert_eq!(msg.subject.as_str(), subject.as_str());

        let payload: AnomalyFeatures = serde_json::from_slice(&msg.payload).expect("decode payload");
        assert!(payload.is_novel_triple);
        assert_eq!(payload.tenant_id, tenant);
        assert_eq!(payload.request_id, "req-novel");
    }

    #[tokio::test]
    async fn rate_spike_emit_publishes_elevated_exchange_rate_per_min() {
        let Some((publisher, subscriber)) = connect_nats_or_skip().await else {
            return;
        };

        let tenant = format!("tenant-{}", uuid::Uuid::now_v7().as_simple());
        let subject = subject_for_tenant(&tenant);
        let mut subscription = subscriber.subscribe(subject.clone()).await.expect("subscribe");
        tokio::time::sleep(Duration::from_millis(100)).await;

        let mut rate = RateTracker::default();
        let agent = "agent/oncall";
        let base_ms = 1_700_000_000_000_i64;
        let current_minute = base_ms / 60_000 + 60;
        for offset in 0..60 {
            let minute = current_minute - 60 + offset;
            rate.record(agent, minute * 60_000);
        }
        for _ in 0..200 {
            rate.record(agent, current_minute * 60_000);
        }
        let ts = current_minute * 60_000;
        let exchange_rate = rate.exchange_rate_per_min(agent, ts);
        assert!(rate.is_spike(agent, ts));
        assert_eq!(exchange_rate, 200);

        let features = AnomalyFeatures {
            chain_depth: 2,
            is_novel_triple: false,
            exchange_rate_per_min: exchange_rate,
            tenant_id: tenant.clone(),
            agent_id: agent.into(),
            purpose: "ops".into(),
            target: "server/github".into(),
            request_id: "req-spike".into(),
            ts_unix_ms: ts,
        };

        AnomalyEmitter::new(publisher)
            .emit(&features)
            .await
            .expect("emit rate spike features");

        let msg = tokio::time::timeout(Duration::from_secs(3), subscription.next())
            .await
            .expect("timed out waiting for anomaly feature")
            .expect("subscription closed");
        assert_eq!(msg.subject.as_str(), subject.as_str());

        let payload: AnomalyFeatures = serde_json::from_slice(&msg.payload).expect("decode payload");
        assert_eq!(payload.exchange_rate_per_min, 200);
        assert_eq!(payload.request_id, "req-spike");
    }
}

/// Shared harness constants (see `tests/e2e_nats_forward.rs` for wiring pattern).
#[allow(dead_code)]
mod harness {
    use super::*;

    pub const REQUEST_RATE_WINDOW: Duration = Duration::from_secs(60);
    pub const REQUEST_RATE_DENY_THRESHOLD: u32 = 100;
    pub const DISTINCT_TOOLS_WINDOW: Duration = Duration::from_secs(300);
    pub const GEO_VELOCITY_DENY_KMH: f64 = 900.0;
    pub const FAILED_AUTHZ_WINDOW: Duration = Duration::from_secs(300);
    pub const FEATURE_TTL: Duration = Duration::from_secs(300);
    pub const CEL_REQUEST_RATE_RULE: &str = "risk.request_rate_1m_per_caller > 100";
}

mod request_rate {
    use super::*;

    #[tokio::test]
    #[ignore = "scaffold; implement when risk features per Pin 8 land"]
    async fn cel_rule_denies_when_request_rate_exceeds_100_per_minute() {
        let _ = (POLICY_DENY, harness::CEL_REQUEST_RATE_RULE, harness::REQUEST_RATE_DENY_THRESHOLD);
        // Arrange: bundle CEL deny rule `risk.request_rate_1m_per_caller > 100`; single caller JWT.
        // Act: send 101 tools/list within 60 s via gateway ingress.
        // Assert: 101st returns JSON-RPC error.code == POLICY_DENY (-32100).
        unimplemented!("sliding 1m window populates risk.request_rate_1m_per_caller");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when risk features per Pin 8 land"]
    async fn request_rate_resets_after_one_minute_window_elapses() {
        // Arrange: caller at rate 99/100 in current window.
        // Act: wait for window rollover, send one more request.
        // Assert: request succeeds; CEL sees refreshed counter near 1.
        unimplemented!("sliding window expiry resets request_rate_1m_per_caller");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when risk features per Pin 8 land"]
    async fn request_rate_under_threshold_allows_through_cel_gate() {
        // Arrange: bundle with CEL deny rule; fast backend stub.
        // Act: 50 requests from same caller within 60 s.
        // Assert: all succeed; no error.code == POLICY_DENY.
        unimplemented!("baseline allow under request_rate threshold");
    }
}

mod distinct_tools {
    use super::*;

    #[tokio::test]
    #[ignore = "scaffold; implement when risk features per Pin 8 land"]
    async fn cel_rule_flags_caller_invoking_many_distinct_tools_in_5m() {
        // Arrange: CEL rule on `risk.distinct_tools_5m` (threshold TBD in bundle).
        // Act: same caller invokes N distinct tool names within 5 minutes.
        // Assert: CEL deny fires; JSON-RPC -32100 policy_deny.
        unimplemented!("distinct_tools_5m counter tracks unique mcp.tool.name per caller");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when risk features per Pin 8 land"]
    async fn repeated_same_tool_does_not_increment_distinct_tools_counter() {
        // Arrange: caller invokes `github::list_repos` 20 times in 5 m.
        // Act: evaluate CEL context for risk.distinct_tools_5m.
        // Assert: counter == 1 (or bundle threshold not reached).
        unimplemented!("distinct_tools counts unique tool names only");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when risk features per Pin 8 land"]
    async fn distinct_tools_resets_after_five_minute_window() {
        // Arrange: caller hit distinct-tools threshold; wait for 5 m TTL/window.
        // Act: invoke a new tool name.
        // Assert: prior distinct count expired; CEL allows (or counter restarted at 1).
        unimplemented!("distinct_tools_5m window rollover");
    }
}

mod geo_velocity {
    use super::*;

    #[tokio::test]
    #[ignore = "scaffold; implement when risk features per Pin 8 land"]
    async fn cel_rule_denies_impossible_travel_between_consecutive_requests() {
        // Arrange: geo lookup from `nats.client_ip` (or NATS header); CEL on geo_velocity_km_per_h.
        // Act: request A from NYC IP, request B 1 s later from Tokyo IP (same caller).
        // Assert: risk.geo_velocity_km_per_h exceeds threshold; -32100 policy_deny.
        unimplemented!("geo_velocity derived from consecutive nats.client_ip geo fixes");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when risk features per Pin 8 land"]
    async fn geo_velocity_allows_same_region_requests() {
        // Arrange: two requests minutes apart from nearby IPs (same metro).
        // Act: evaluate CEL with geo_velocity rule.
        // Assert: risk.geo_velocity_km_per_h below threshold; request allowed.
        unimplemented!("baseline allow for plausible geo velocity");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when risk features per Pin 8 land"]
    async fn geo_velocity_skips_when_client_ip_geo_unavailable() {
        // Arrange: ingress without resolvable client IP geo.
        // Act: CEL rule references risk.geo_velocity_km_per_h.
        // Assert: feature binds 0/null; rule does not false-positive deny.
        unimplemented!("missing geo treats geo_velocity as absent/zero per Pin 8");
    }
}

mod failed_authz {
    use super::*;

    #[tokio::test]
    #[ignore = "scaffold; implement when risk features per Pin 8 land"]
    async fn failed_authz_recent_counts_policy_deny_decisions_for_caller() {
        // Arrange: SpiceDB/CEL path returns -32100 for caller on mutating tool.
        // Act: repeat until risk.failed_authz_recent > 0; send gated request.
        // Assert: CEL rule on failed_authz_recent denies subsequent high-risk call.
        unimplemented!("failed_authz_recent increments on -32100 per caller");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when risk features per Pin 8 land"]
    async fn failed_authz_recent_excludes_other_error_codes() {
        // Arrange: caller receives -32105 rate_limited and -32107 approval_required.
        // Act: inspect risk.failed_authz_recent binding.
        // Assert: counter unchanged (only -32100 policy_deny counts).
        unimplemented!("failed_authz_recent filters to POLICY_DENY only");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when risk features per Pin 8 land"]
    async fn failed_authz_recent_decays_after_configured_window() {
        // Arrange: caller accumulates failed_authz_recent > 0.
        // Act: wait for window/TTL expiry.
        // Assert: risk.failed_authz_recent == 0 in CEL context.
        unimplemented!("failed_authz_recent sliding window decay");
    }
}

mod new_device {
    use super::*;

    #[tokio::test]
    #[ignore = "scaffold; implement when risk features per Pin 8 land"]
    async fn new_device_score_is_one_for_unseen_device_id_claim() {
        // Arrange: JWT with `device_id` claim never seen for this jwt.sub.
        // Act: first request through gateway with CEL rule on risk.new_device_score.
        // Assert: CEL sees 1.0; step-up or deny per bundle.
        unimplemented!("new_device_score == 1.0 for first-seen (sub, device_id) pair");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when risk features per Pin 8 land"]
    async fn new_device_score_is_zero_for_known_device_id() {
        // Arrange: prior request registered device_id for jwt.sub.
        // Act: second request with same device_id claim.
        // Assert: risk.new_device_score == 0.0 in CEL context.
        unimplemented!("new_device_score == 0.0 for repeat device_id per sub");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when risk features per Pin 8 land"]
    async fn new_device_score_scoped_per_sub_not_global() {
        // Arrange: device_id seen for sub alice; first request for sub bob with same device_id.
        // Act: evaluate risk.new_device_score for bob.
        // Assert: score == 1.0 for bob (device registry keyed by sub).
        unimplemented!("new_device registry is per jwt.sub");
    }
}

mod ttl {
    use super::*;

    #[tokio::test]
    #[ignore = "scaffold; implement when risk features per Pin 8 land"]
    async fn expired_risk_feature_evaluates_as_zero_in_cel() {
        // Arrange: populate risk.* counter; wait past configured FEATURE_TTL.
        // Act: CEL rule reads expired feature name.
        // Assert: binding is 0 or null; deny rule does not fire on stale value.
        unimplemented!("TTL expiry zeroes risk.* CEL bindings");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when risk features per Pin 8 land"]
    async fn active_risk_feature_retains_value_before_ttl() {
        // Arrange: increment request_rate within window; TTL not elapsed.
        // Act: immediate follow-up request within same window.
        // Assert: risk.request_rate_1m_per_caller reflects live counter.
        unimplemented!("pre-TTL risk feature retains non-zero value");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when risk features per Pin 8 land"]
    async fn ttl_is_configurable_per_feature_kind() {
        // Arrange: bundle/KV sets distinct TTLs for request_rate vs distinct_tools.
        // Act: advance clock past shorter TTL only.
        // Assert: shorter-lived feature zeroed; longer-lived feature still populated.
        unimplemented!("per-feature TTL from bundle config");
    }
}

mod isolation {
    use super::*;

    #[tokio::test]
    #[ignore = "scaffold; implement when risk features per Pin 8 land"]
    async fn request_rate_is_per_caller_not_tenant_aggregate() {
        // Arrange: caller A exhausts rate budget; caller B same tenant, different jwt.sub.
        // Act: request from B.
        // Assert: B succeeds; risk.request_rate_1m_per_caller isolated per sub.
        unimplemented!("no cross-caller request_rate pollution within tenant");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when risk features per Pin 8 land"]
    async fn distinct_tools_is_per_caller_not_per_tenant() {
        // Arrange: caller A invokes many tools; caller B same tenant, fresh sub.
        // Act: B invokes one tool.
        // Assert: risk.distinct_tools_5m for B == 1, unaffected by A.
        unimplemented!("distinct_tools keyed by caller not tenant aggregate");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when risk features per Pin 8 land"]
    async fn failed_authz_recent_does_not_leak_across_callers() {
        // Arrange: caller A accumulates -32100 denials.
        // Act: request from caller B with CEL on failed_authz_recent.
        // Assert: B's counter is 0; A's denials invisible to B.
        unimplemented!("failed_authz_recent per-caller isolation");
    }
}

mod audit_field {
    use super::*;

    #[tokio::test]
    #[ignore = "scaffold; implement when risk features per Pin 8 land"]
    async fn audit_envelope_includes_risk_score_incremented_per_request() {
        // Arrange: gateway with audit publish enabled; anomaly counters active.
        // Act: single allowed ingress request; capture JetStream audit payload.
        // Assert: envelope extra or top-level `risk.score` present and > 0 when signals fire.
        unimplemented!("audit risk.score reflects anomaly counter contribution");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when risk features per Pin 8 land"]
    async fn audit_risk_score_accumulates_with_multiple_anomaly_signals() {
        // Arrange: caller triggers request_rate + distinct_tools + new_device in one request.
        // Act: capture audit envelope.
        // Assert: risk.score >= sum of contributing feature weights (bundle-defined).
        unimplemented!("risk.score aggregates multiple anomaly features per request");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when risk features per Pin 8 land"]
    async fn audit_risk_score_zero_when_no_anomaly_signals() {
        // Arrange: baseline caller, no risk CEL rules fired.
        // Act: allowed request; capture audit.
        // Assert: risk.score == 0 or field omitted per reference-audit-envelope.md.
        unimplemented!("baseline audit risk.score when no anomaly counters increment");
    }
}
