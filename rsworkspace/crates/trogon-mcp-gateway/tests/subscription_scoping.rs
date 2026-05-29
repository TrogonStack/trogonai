//! NATS subscription scoping integration tests (scaffold).
//!
//! The gateway authorizes which subjects a client may subscribe to (MCP notifications,
//! progress, reply inboxes, etc.) via NATS auth callout at CONNECT and on subscribe attempts.
//! Scoped templates tie `client_id` from the JWT to `mcp.client.{client_id}.notifications.>`,
//! reject cross-tenant and cross-client fan-in, cap reply inboxes, and emit subscribe audit metrics.
//!
//! Cross-references:
//! - `docs/identity/reference-reply-inboxes.md` — `_INBOX.client.{nuid}`, inbox ACL, subscribe topology
//! - `docs/adr/0011-nats-auth-callout.md` — CONNECT-time User JWT and `nats.sub` permission templates
//! - `reference-reply-inboxes.md` (reply inboxes and subscription scoping)
//!
//! Harness pattern: live NATS broker, `mcp_nats::Config`, `McpPrefix`, `trogon_nats::NatsAuth`,
//! `GatewaySettings`, auth-callout mint stub or real callout handler, client CONNECT with
//! bootstrap/mesh JWT (see `e2e_nats_forward.rs`, `jwt_validation.rs`).
//!
//! Once subscription scoping lands per ADR 0011, remove `#[ignore]` and implement Arrange / Act / Assert.

#![allow(unused_imports)]

/// Shared harness constants and subject templates (see `tests/e2e_nats_forward.rs`).
mod harness {
    use std::time::Duration;

    #[allow(dead_code)]
    pub const DEFAULT_PREFIX: &str = "mcp";
    #[allow(dead_code)]
    pub const TEST_CLIENT_ID: &str = "client-alpha";
    #[allow(dead_code)]
    pub const OTHER_CLIENT_ID: &str = "client-beta";
    #[allow(dead_code)]
    pub const TEST_MAX_INBOXES_PER_CLIENT: u32 = 8;
    #[allow(dead_code)]
    pub const JWT_SHORT_TTL: Duration = Duration::from_secs(2);
    #[allow(dead_code)]
    pub const METRIC_SUBSCRIBE_TOTAL: &str = "mcp_subscribe_total";

    #[allow(dead_code)]
    pub fn client_notifications_wildcard(client_id: &str) -> String {
        format!("{DEFAULT_PREFIX}.client.{client_id}.notifications.>")
    }

    #[allow(dead_code)]
    pub fn client_notifications_list_changed(client_id: &str) -> String {
        format!("{DEFAULT_PREFIX}.client.{client_id}.notifications.tools/list_changed")
    }

    #[allow(dead_code)]
    pub fn prefix_catchall() -> String {
        format!("{DEFAULT_PREFIX}.>")
    }
}

mod self_inbox {
    use super::harness;

    #[tokio::test]
    #[ignore = "scaffold; implement when subscription scoping per ADR 0011 lands"]
    async fn own_notifications_wildcard_allowed_when_client_id_matches_jwt() {
        // Arrange: mint User JWT with caller_id == harness::TEST_CLIENT_ID; NATS auth callout enabled.
        // Act: client subscribes to harness::client_notifications_wildcard(TEST_CLIENT_ID).
        // Assert: subscribe succeeds; no nats.error on callout reply.
        unimplemented!(
            "subscribe {} allowed for matching JWT client_id per ADR 0011",
            harness::client_notifications_wildcard(harness::TEST_CLIENT_ID)
        );
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when subscription scoping per ADR 0011 lands"]
    async fn nats_sub_acl_template_includes_matching_notifications_tree() {
        // Arrange: capture minted User JWT from auth callout after CONNECT.
        // Assert: nats.sub permissions include mcp.client.{jwt_client_id}.notifications.> only.
        unimplemented!("reference-reply-inboxes.md client notification subscribe scope");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when subscription scoping per ADR 0011 lands"]
    async fn mismatching_client_id_segment_in_subject_denied_at_subscribe() {
        // Arrange: JWT client_id == TEST_CLIENT_ID.
        // Act: subscribe to harness::client_notifications_wildcard(harness::OTHER_CLIENT_ID) (wrong segment).
        // Assert: NATS subscribe error before any message delivery.
        unimplemented!("subject client_id segment must match JWT-derived id");
    }
}

mod cross_client_deny {
    use super::harness;

    #[tokio::test]
    #[ignore = "scaffold; implement when subscription scoping per ADR 0011 lands"]
    async fn subscribe_to_other_client_notifications_denied_at_callout() {
        // Arrange: client A CONNECT with JWT caller_id == TEST_CLIENT_ID.
        // Act: client A attempts subscribe to harness::client_notifications_wildcard(OTHER_CLIENT_ID).
        // Assert: auth callout or server ACL returns denial; no subscription interest created.
        unimplemented!("cross-client notification fan-in denied per ADR 0011");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when subscription scoping per ADR 0011 lands"]
    async fn cross_client_list_changed_subject_denied() {
        // Act: subscribe to harness::client_notifications_list_changed(OTHER_CLIENT_ID).
        // Assert: subscribe rejected at perimeter (nats.sub template mismatch).
        unimplemented!("notifications.tools/list_changed on foreign client_id");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when subscription scoping per ADR 0011 lands"]
    async fn cross_client_deny_emits_no_notification_delivery() {
        // Arrange: backend publishes list_changed to victim client's subject.
        // Assert: client A receives zero messages (subscription never established).
        unimplemented!("deny prevents interest on foreign notification subject");
    }
}

mod notifications {
    use super::harness;

    #[tokio::test]
    #[ignore = "scaffold; implement when subscription scoping per ADR 0011 lands"]
    async fn specific_list_changed_subject_allowed_for_own_client() {
        // Arrange: client subscribed to harness::client_notifications_list_changed(TEST_CLIENT_ID).
        // Act: backend (or gateway fan-out) publishes MCP notification on that exact subject.
        // Assert: client receives JSON-RPC notification envelope.
        unimplemented!("allowed narrow notification subject");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when subscription scoping per ADR 0011 lands"]
    async fn backend_emitted_list_changed_reaches_matching_subscriber() {
        // Arrange: gateway + backend stub; warm tools/list cache optional.
        // Act: server emits notifications/tools/list_changed on client notifications lane.
        // Assert: subscribed client observes method notifications/tools/list_changed.
        unimplemented!("end-to-end notification delivery on scoped subject");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when subscription scoping per ADR 0011 lands"]
    async fn wildcard_notifications_subscription_receives_child_subjects() {
        // Arrange: subscribe to harness::client_notifications_wildcard(TEST_CLIENT_ID).
        // Act: publish to child subject ...notifications.progress (or list_changed).
        // Assert: wildcard subscriber receives both child notifications.
        unimplemented!("mcp.client.{{client_id}}.notifications.> covers method-specific children");
    }
}

mod wildcard {
    use super::harness;

    #[tokio::test]
    #[ignore = "scaffold; implement when subscription scoping per ADR 0011 lands"]
    async fn prefix_catchall_mcp_gt_denied_for_agent_role() {
        // Arrange: mesh/agent JWT without admin role.
        // Act: subscribe to harness::prefix_catchall().
        // Assert: NATS auth error; nats.sub does not grant mcp.>.
        unimplemented!("non-admin cannot subscribe to entire mcp.> tree");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when subscription scoping per ADR 0011 lands"]
    async fn prefix_catchall_mcp_gt_denied_for_observer_role() {
        // Arrange: observer role token from callout template.
        // Act: subscribe harness::prefix_catchall().
        // Assert: denied at callout mint or subscribe ACL check.
        unimplemented!("observer role lacks admin wildcard subscribe");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when subscription scoping per ADR 0011 lands"]
    async fn prefix_catchall_mcp_gt_allowed_for_admin_role() {
        // Arrange: admin role in minted User JWT (nats.sub includes mcp.>).
        // Act: subscribe harness::prefix_catchall().
        // Assert: subscription succeeds; sample child subject deliverable.
        unimplemented!("admin role template grants mcp.> per ADR 0011");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when subscription scoping per ADR 0011 lands"]
    async fn agent_role_still_allowed_own_notifications_without_admin_wildcard() {
        // Arrange: agent JWT (no mcp.>).
        // Act: subscribe harness::client_notifications_wildcard(TEST_CLIENT_ID).
        // Assert: allowed — scoped tree is not the same as prefix catchall.
        unimplemented!("scoped notifications allowed without admin wildcard");
    }
}

mod draining {
    use super::harness;

    #[tokio::test]
    #[ignore = "scaffold; implement when subscription scoping per ADR 0011 lands"]
    async fn existing_subscriptions_kept_when_tenant_enters_draining() {
        // Arrange: client already subscribed to own notifications wildcard; mark tenant draining.
        // Assert: subscription interest remains; in-flight notifications still deliverable.
        unimplemented!("draining retains existing interests per tenancy boundary");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when subscription scoping per ADR 0011 lands"]
    async fn new_subscribe_denied_while_tenant_draining() {
        // Arrange: tenant draining flag set before CONNECT/subscribe.
        // Act: new client attempts subscribe to own notifications subject.
        // Assert: NATS auth denial; no new interest registered.
        unimplemented!("new subscriptions blocked during tenant drain");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when subscription scoping per ADR 0011 lands"]
    async fn reconnect_subscribe_denied_after_drain_starts() {
        // Arrange: client had session; tenant transitions to draining; client reconnects.
        // Act: post-CONNECT subscribe to notifications wildcard.
        // Assert: denied even if prior session had been subscribed before drain.
        unimplemented!("reconnect subscribe path respects draining gate");
    }
}

mod expiry_prune {
    use super::harness;

    #[tokio::test]
    #[ignore = "scaffold; implement when subscription scoping per ADR 0011 lands"]
    async fn subscriptions_torn_down_when_jwt_exp_passes() {
        // Arrange: short-lived JWT (harness::JWT_SHORT_TTL); active notification subscription.
        // Act: wait until exp + leeway; gateway/session pruner runs.
        // Assert: server reports zero subscription interest for that connection/token.
        unimplemented!("token expiry prune per reference-reply-inboxes.md session lifecycle");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when subscription scoping per ADR 0011 lands"]
    async fn pruned_client_stops_receiving_notifications_after_exp() {
        // Arrange: subscribed client; backend publishes after JWT exp.
        // Assert: zero messages delivered post-prune.
        unimplemented!("no delivery after subscription teardown on exp");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when subscription scoping per ADR 0011 lands"]
    async fn refreshed_jwt_before_exp_retains_subscriptions() {
        // Arrange: client re-CONNECT with renewed JWT before exp.
        // Assert: notification subscription can be re-established without prune gap errors.
        unimplemented!("re-auth before exp avoids prune");
    }
}

mod quota {
    use super::harness;

    #[tokio::test]
    #[ignore = "scaffold; implement when subscription scoping per ADR 0011 lands"]
    async fn exceeding_max_inboxes_per_client_returns_nats_auth_error() {
        // Arrange: config max_inboxes_per_client == harness::TEST_MAX_INBOXES_PER_CLIENT.
        // Act: mint (N+1) distinct _INBOX.client.{nuid} subscribe interests for same token.
        // Assert: (N+1)th subscribe fails with NATS authorization error.
        unimplemented!(
            "max_inboxes_per_client cap per reference-reply-inboxes.md (N={})",
            harness::TEST_MAX_INBOXES_PER_CLIENT
        );
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when subscription scoping per ADR 0011 lands"]
    async fn at_cap_exactly_max_inboxes_allowed() {
        // Arrange: cap N = harness::TEST_MAX_INBOXES_PER_CLIENT.
        // Act: subscribe N distinct client reply inbox subjects.
        // Assert: all N succeed; no auth error.
        unimplemented!("inbox quota allows up to configured cap");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when subscription scoping per ADR 0011 lands"]
    async fn releasing_inbox_frees_quota_for_new_subscribe() {
        // Arrange: client at cap; unsubscribe one _INBOX.client.{nuid}.
        // Act: subscribe a new inbox subject.
        // Assert: new subscribe succeeds.
        unimplemented!("inbox quota decrements on unsubscribe/teardown");
    }
}

mod audit {
    use super::harness;

    #[tokio::test]
    #[ignore = "scaffold; implement when subscription scoping per ADR 0011 lands"]
    async fn allowed_subscribe_increments_mcp_subscribe_total_allow() {
        // Arrange: metrics registry / Prometheus scrape hook on gateway callout path.
        // Act: successful subscribe to harness::client_notifications_wildcard(TEST_CLIENT_ID).
        // Assert: mcp_subscribe_total{outcome="allow"} increases by 1.
        unimplemented!(
            "metric {} outcome=allow",
            harness::METRIC_SUBSCRIBE_TOTAL
        );
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when subscription scoping per ADR 0011 lands"]
    async fn denied_subscribe_increments_mcp_subscribe_total_deny() {
        // Act: denied subscribe to harness::client_notifications_wildcard(OTHER_CLIENT_ID).
        // Assert: mcp_subscribe_total{outcome="deny"} increases by 1.
        unimplemented!(
            "metric {} outcome=deny",
            harness::METRIC_SUBSCRIBE_TOTAL
        );
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when subscription scoping per ADR 0011 lands"]
    async fn subscribe_audit_labels_include_subject_and_outcome_only() {
        // Arrange: capture metric sample after allow and deny attempts.
        // Assert: label set is {outcome} in {allow, deny}; no high-cardinality subject label.
        unimplemented!("mcp_subscribe_total cardinality contract");
    }
}
