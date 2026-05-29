//! Integration scaffold for multi-region MCP gateway failover (ADR 0016).
//!
//! Scenario: per-region NATS clusters with leafnode/supercluster interconnect, tenant-pinned
//! home regions, regional queue groups, KV mirror propagation, and SpiceDB read replicas.
//! Covers steady-state routing, region drain, cross-region read fallback, tenancy-pin
//! violations, leafnode partition readiness, bundle mirror lag, and ZedToken staleness.
//!
//! Cross-references:
//! - [0016-multi-region-topology.md](../../../docs/adr/0016-multi-region-topology.md)
//! - [multi-region.md](../../../docs/identity/multi-region.md)
//! - [MCP_GATEWAY_PLAN.md](../../../MCP_GATEWAY_PLAN.md) Block G multi-region section
//!
//! Harness types: `mcp_nats::Config`, `McpPrefix`, `trogon_nats::NatsAuth`,
//! `trogon_mcp_gateway::gateway::GatewaySettings` (see `e2e_nats_forward.rs`).
//!
//! Once Block G multi-region lands: remove `#[ignore]`, wire dual-region NATS fixtures,
//! and assert routing, audit `served_region`, readiness, and authz error codes.

#![allow(unused_imports)]

use std::sync::Arc;
use std::time::Duration;

use mcp_nats::{Config as McpConfig, McpPrefix};
use trogon_mcp_gateway::authz::AllowAllPermissionChecker;
use trogon_mcp_gateway::gateway::GatewaySettings;
use trogon_nats::{NatsAuth, NatsConfig};

/// Shared region IDs and harness constants for dual-cluster fixtures (TBD).
mod harness {
    use std::time::Duration;

    #[allow(dead_code)]
    pub const REGION_US_EAST: &str = "us-east";
    #[allow(dead_code)]
    pub const REGION_US_WEST: &str = "us-west";
    #[allow(dead_code)]
    pub const REGION_EU_WEST: &str = "eu-west";
    #[allow(dead_code)]
    pub const GATEWAY_QUEUE_GROUP: &str = "mcp-gateway";
    #[allow(dead_code)]
    pub const AUDIT_STREAM: &str = "MCP_AUDIT";
    #[allow(dead_code)]
    pub const NATS_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
    #[allow(dead_code)]
    pub const BUNDLE_MIRROR_LAG_POLL: Duration = Duration::from_millis(50);
    #[allow(dead_code)]
    pub const ZEDTOKEN_STALENESS_WINDOW: Duration = Duration::from_secs(30);
}

mod steady_state {
    //! Client pinned to home region; ingress and reply inbox stay region-local.
    //!
    //! Spec: [0016-multi-region-topology.md §Decision (c)](../../../docs/adr/0016-multi-region-topology.md).

    use super::harness::{GATEWAY_QUEUE_GROUP, REGION_US_EAST};

    #[tokio::test]
    #[ignore = "scaffold; implement when multi-region failover per ADR 0016 lands"]
    async fn us_east_client_routes_to_us_east_gateway_queue() {
        // Arrange: gateways in us-east and us-west; client NATS URL = us-east.
        // Act: `mcp.gateway.request.*` from tenant pinned to us-east.
        // Assert: only us-east queue member `GATEWAY_QUEUE_GROUP` receives work.
        let _ = (REGION_US_EAST, GATEWAY_QUEUE_GROUP);
        unimplemented!("home-region queue pin per ADR 0016 regional fleet");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when multi-region failover per ADR 0016 lands"]
    async fn reply_inbox_stays_on_region_local_nats_cluster() {
        // Arrange: request/reply through us-east gateway.
        // Act: backend responds on ingress reply subject.
        // Assert: reply inbox subject resolves on us-east core, not us-west.
        unimplemented!("region-local reply inbox per multi-region.md §3.1");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when multi-region failover per ADR 0016 lands"]
    async fn audit_envelope_published_to_regional_mcp_audit_stream() {
        // Arrange: successful tools/call in us-east.
        // Act: gateway emits audit envelope.
        // Assert: JetStream publish targets us-east `MCP_AUDIT` only.
        unimplemented!("regional audit stream per ADR 0016 audit decision");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when multi-region failover per ADR 0016 lands"]
    async fn core_pubsub_never_crosses_wan_on_steady_state_hot_path() {
        // Arrange: healthy leafnode; tenant home us-east.
        // Act: tools/call hot path.
        // Assert: no cross-region core publish/subscribe on ingress or egress.
        unimplemented!("core pub/sub regional-only per ADR 0016 layer (b)");
    }
}

mod drain {
    //! Region drain: new work shifts; in-flight completes in draining region.
    //!
    //! Spec: [multi-region.md](../../../docs/identity/multi-region.md) operator runbooks (drain).

    use super::harness::{REGION_US_EAST, REGION_US_WEST};

    #[tokio::test]
    #[ignore = "scaffold; implement when multi-region failover per ADR 0016 lands"]
    async fn new_requests_route_to_us_west_when_us_east_draining() {
        // Arrange: mark us-east gateway fleet draining; us-west healthy.
        // Act: new `mcp.gateway.request` after drain signal.
        // Assert: us-west queue member handles request; us-east rejects new work.
        let _ = (REGION_US_EAST, REGION_US_WEST);
        unimplemented!("drain redirects new requests per multi-region runbook");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when multi-region failover per ADR 0016 lands"]
    async fn in_flight_requests_complete_in_draining_us_east() {
        // Arrange: long-running tools/call started before drain.
        // Act: signal drain on us-east while request in flight.
        // Assert: same us-east gateway finishes; client receives JSON-RPC result.
        unimplemented!("in-flight completion in draining region per ADR 0016");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when multi-region failover per ADR 0016 lands"]
    async fn readiness_reflects_draining_region_for_cross_region_work() {
        // Arrange: us-east draining; cross-region fallback policy enabled.
        // Act: GET /readyz on us-east gateway.
        // Assert: readiness reports draining; new cross-region work not scheduled locally.
        unimplemented!("drain readiness semantics per MCP_GATEWAY_PLAN Block G");
    }
}

mod cross_region {
    //! Cross-region read fallback when bundle allows; audit records `served_region`.
    //!
    //! Spec: [0016-multi-region-topology.md](../../../docs/adr/0016-multi-region-topology.md);
    //! optional fallback in [multi-region.md](../../../docs/identity/multi-region.md).

    use super::harness::{REGION_US_EAST, REGION_US_WEST};

    #[tokio::test]
    #[ignore = "scaffold; implement when multi-region failover per ADR 0016 lands"]
    async fn request_served_from_us_west_when_us_east_unreachable() {
        // Arrange: us-east cluster blackholed; tenant bundle allows US cross-region read fallback.
        // Act: client request while home region down.
        // Assert: us-west gateway serves; JSON-RPC success.
        let _ = (REGION_US_EAST, REGION_US_WEST);
        unimplemented!("cross-region read fallback when bundle permits per multi-region.md");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when multi-region failover per ADR 0016 lands"]
    async fn audit_envelope_records_served_region_on_fallback() {
        // Arrange: fallback path from us-west for us-east-pinned tenant.
        // Act: successful tools/call via fallback.
        // Assert: audit envelope `served_region` == us-west (or canonical region id).
        unimplemented!("audit served_region field on cross-region fallback");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when multi-region failover per ADR 0016 lands"]
    async fn fallback_denied_when_bundle_disallows_cross_region_serve() {
        // Arrange: us-east down; tenant residency bundle forbids cross-region serve.
        // Act: tools/call.
        // Assert: explicit JSON-RPC / gateway error; no silent us-west serve.
        unimplemented!("fallback gated by bundle residency policy per ADR 0016");
    }
}

mod tenancy_pin {
    //! Tenancy-pin violation: pinned home region cannot be bypassed by healthy remote regions.
    //!
    //! Spec: `MCPTenant.spec.regions` in [0016-multi-region-topology.md](../../../docs/adr/0016-multi-region-topology.md).

    use super::harness::{REGION_EU_WEST, REGION_US_EAST};

    #[tokio::test]
    #[ignore = "scaffold; implement when multi-region failover per ADR 0016 lands"]
    async fn tenant_pinned_us_east_rejected_from_eu_west_when_us_east_healthy() {
        // Arrange: tenant allow-list [us-east]; eu-west gateway healthy.
        // Act: route attempt via eu-west NATS URL / queue.
        // Assert: request fails with explicit tenancy-pin / region violation error.
        let _ = (REGION_US_EAST, REGION_EU_WEST);
        unimplemented!("tenancy-pin violation error per ADR 0016 residency");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when multi-region failover per ADR 0016 lands"]
    async fn violation_error_is_explicit_not_generic_timeout() {
        // Arrange: misrouted client to eu-west for us-east-only tenant.
        // Act: tools/call.
        // Assert: stable error code/class (not NATS timeout masquerading as pin failure).
        unimplemented!("explicit tenancy-pin error surface per MCP_GATEWAY_PLAN Block G");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when multi-region failover per ADR 0016 lands"]
    async fn gateway_checks_mcp_gateway_region_against_tenant_projected_config() {
        // Arrange: `MCP_GATEWAY_REGION=us-east` on gateway; tenant projected regions = [us-east].
        // Act: request with gateway env mismatch (eu-west process).
        // Assert: fail closed before backend egress.
        unimplemented!("MCP_GATEWAY_REGION runtime check per ADR 0016 consequences");
    }
}

mod partition {
    //! Leafnode partition: cross-region readiness 503; local tenants still served.
    //!
    //! Spec: [0016-multi-region-topology.md](../../../docs/adr/0016-multi-region-topology.md) layer (b).

    use super::harness::{REGION_US_EAST, REGION_US_WEST};

    #[allow(dead_code)]
    const READYZ_PATH: &str = "/readyz";

    #[tokio::test]
    #[ignore = "scaffold; implement when multi-region failover per ADR 0016 lands"]
    async fn leafnode_partition_flips_readyz_503_for_cross_region_work() {
        // Arrange: us-east gateway; sever leafnode to hub/us-west.
        // Act: GET /readyz with cross-region dependency checks enabled.
        // Assert: status 503 for cross-region facet; local-only checks may still pass (TBD contract).
        let _ = (REGION_US_EAST, REGION_US_WEST, READYZ_PATH);
        unimplemented!("readiness 503 on leafnode partition per failure-mode matrix");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when multi-region failover per ADR 0016 lands"]
    async fn local_tenant_hot_path_still_served_when_leafnode_down() {
        // Arrange: leafnode partition; tenant home us-east with backends in us-east.
        // Act: tools/call for in-region tenant.
        // Assert: JSON-RPC success; no WAN hop on hot path.
        unimplemented!("local serve continues on leafnode partition per ADR 0016");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when multi-region failover per ADR 0016 lands"]
    async fn cross_region_kv_mirror_reads_degraded_not_served_as_authoritative() {
        // Arrange: partition blocks mirror pull from primary.
        // Act: gateway policy eval relying on mirrored bundle.
        // Assert: fail closed or serve last-known per bundle class (documented in test).
        unimplemented!("mirror lag / partition behavior per multi-region.md consistency table");
    }
}

mod bundle_lag {
    //! KV mirror propagation lag: primary rollout vs regional replica staleness.
    //!
    //! Spec: `mcp-gateway-config` mirror in [0016-multi-region-topology.md](../../../docs/adr/0016-multi-region-topology.md).

    use super::harness::{BUNDLE_MIRROR_LAG_POLL, REGION_US_EAST, REGION_US_WEST};

    #[tokio::test]
    #[ignore = "scaffold; implement when multi-region failover per ADR 0016 lands"]
    async fn us_west_serves_old_bundle_until_mirror_catches_primary() {
        // Arrange: new bundle revision written to us-east (primary); us-west mirror lag simulated.
        // Act: tools/call via us-west before mirror completes.
        // Assert: policy eval uses previous active pointer revision.
        let _ = (REGION_US_EAST, REGION_US_WEST, BUNDLE_MIRROR_LAG_POLL);
        unimplemented!("regional mirror lag serves stale bundle per ADR 0016 negative consequences");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when multi-region failover per ADR 0016 lands"]
    async fn us_west_converges_to_new_bundle_after_propagation_window() {
        // Arrange: same rollout; wait for mirror p95 target (poll `BUNDLE_MIRROR_LAG_POLL`).
        // Act: tools/call via us-west after propagation.
        // Assert: policy eval uses new active pointer.
        unimplemented!("mirror convergence per multi-region.md KV mirror SLO");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when multi-region failover per ADR 0016 lands"]
    async fn revocation_in_primary_eventually_blocks_stale_regional_serve() {
        // Arrange: agent revoked in primary KV; mirror lag on us-west.
        // Act: lookup during lag window then after catch-up.
        // Assert: brief stale allow then deny after mirror reflects revocation.
        unimplemented!("revocation vs mirror lag incident class per ADR 0016");
    }
}

mod router_failover {
    //! `RegionRouter` acceptance (ADR 0016 deterministic failover + session pin).
    //! Full gateway/NATS harness cases below remain scaffolded until `gateway.rs` wiring lands.

    use trogon_mcp_gateway::multi_region::{
        MultiRegionConfig, MultiRegionConfigError, NoopRegionAuditSink, RegionHealth,
        RegionHealthConfig, RegionRouter, RegionRouteErrorKind, RequestContext, RouteReason,
        TopologyBuildError,
    };

    fn router_from_json(json: &str) -> RegionRouter<NoopRegionAuditSink> {
        let topo = MultiRegionConfig::from_json_str(json)
            .expect("config")
            .into_topology()
            .expect("topology");
        let health = RegionHealth::new(&topo, RegionHealthConfig::default());
        RegionRouter::new(topo, health, NoopRegionAuditSink)
    }

    const TWO_REGION_JSON: &str = r#"{
        "home_region": "us-east",
        "failover": ["us-west"],
        "regions": {
            "us-east": { "nats_url": "nats://east:4222" },
            "us-west": { "nats_url": "nats://west:4222" }
        }
    }"#;

    #[test]
    fn home_region_happy_path() {
        let router = router_from_json(TWO_REGION_JSON);
        let decision = router
            .route(None, &RequestContext::default())
            .expect("route");
        assert_eq!(decision.region.as_str(), "us-east");
        assert_eq!(decision.reason, RouteReason::HomeRegion);
    }

    #[test]
    fn single_region_failover() {
        let router = router_from_json(TWO_REGION_JSON);
        let home = router.topology().home_region().clone();
        router.health().force_unreachable(&home);
        let decision = router
            .route(None, &RequestContext::default())
            .expect("route");
        assert_eq!(decision.region.as_str(), "us-west");
        assert!(matches!(decision.reason, RouteReason::Failover { .. }));
    }

    #[test]
    fn all_regions_down_error_mapping() {
        let router = router_from_json(TWO_REGION_JSON);
        for region in router.topology().route_candidates() {
            router.health().force_unreachable(&region);
        }
        let err = router
            .route(None, &RequestContext::default())
            .expect_err("route");
        assert_eq!(err.kind, RegionRouteErrorKind::AllRegionsUnreachable);
        assert_eq!(err.attempted.len(), 2);
        assert_eq!(err.attempted[0].as_str(), "us-east");
        assert_eq!(err.attempted[1].as_str(), "us-west");
    }

    #[test]
    fn session_stickiness_across_n_calls() {
        let router = router_from_json(TWO_REGION_JSON);
        let ctx = RequestContext {
            session_id: Some("integration-sess".into()),
        };
        let first = router.route(None, &ctx).expect("first");
        assert_eq!(first.reason, RouteReason::HomeRegion);
        for _ in 0..8 {
            let next = router.route(None, &ctx).expect("next");
            assert_eq!(next.region, first.region);
            assert_eq!(next.reason, RouteReason::SessionPin);
        }
    }

    #[test]
    fn topology_rejects_failover_including_home() {
        let raw = r#"{
            "home_region": "us-east",
            "failover": ["us-east"],
            "regions": { "us-east": { "nats_url": "nats://east:4222" } }
        }"#;
        let err = MultiRegionConfig::from_json_str(raw)
            .expect("config")
            .into_topology()
            .expect_err("topology");
        assert_eq!(
            err,
            MultiRegionConfigError::Topology(TopologyBuildError::FailoverIncludesHome)
        );
    }
}

mod zedtoken {
    //! ZedToken minted in one region accepted in another within staleness window.
    //!
    //! Spec: SpiceDB regional replicas in [0016-multi-region-topology.md](../../../docs/adr/0016-multi-region-topology.md).

    use super::harness::{REGION_US_EAST, REGION_US_WEST, ZEDTOKEN_STALENESS_WINDOW};

    #[tokio::test]
    #[ignore = "scaffold; implement when multi-region failover per ADR 0016 lands"]
    async fn zedtoken_minted_us_east_accepted_us_west_within_staleness_window() {
        // Arrange: CheckBulkPermissions in us-east returns ZedToken; replicate to us-west replica.
        // Act: us-west gateway authz with `at_least_as_fresh` using that token within window.
        // Assert: permission check succeeds (not -32107 stale closed).
        let _ = (REGION_US_EAST, REGION_US_WEST, ZEDTOKEN_STALENESS_WINDOW);
        unimplemented!("cross-region ZedToken acceptance per ADR 0016 SpiceDB decision");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when multi-region failover per ADR 0016 lands"]
    async fn zedtoken_outside_staleness_window_fails_closed() {
        // Arrange: aged ZedToken beyond configured staleness.
        // Act: us-west CheckBulkPermissions with `at_least_as_fresh`.
        // Assert: authz fails closed (-32107 class per integration-touchpoints).
        unimplemented!("ZedToken staleness fail-closed per ADR 0016 negative consequences");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when multi-region failover per ADR 0016 lands"]
    async fn bulk_check_cache_honors_per_region_replica_freshness() {
        // Arrange: `trogon_mcp_gateway` bulk_check cache in us-west.
        // Act: repeated check with token from us-east mint within window.
        // Assert: cache hit respects replica freshness semantics (no cross-region stale bypass).
        unimplemented!("regional replica freshness on bulk_check cache per multi-region.md");
    }
}
