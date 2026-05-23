//! Phase 4 — Cross-binding collaboration tests ( **`a2a-bridge`** prerequisite ).
//!
//! **Status:** compile-only seam — integration tests land after **`a2a-bridge`** ships HTTPS↔NATS
//! ingress. This module documents the minimal collaboration matrix so CI can type-check the
//! contract before wiring.
//!
//! ## Cross-binding scope ([`../../../../A2A_PLAN.md`](../../../../A2A_PLAN.md))
//!
//! From [`A2A_PLAN.md`](../../../../A2A_PLAN.md) §Interop Bridge and §Auth:
//!
//! - **HTTPS in, NATS out** — terminates standard A2A HTTPS, translates JSON-RPC to NATS publish,
//!   streams SSE back from the JetStream consumer.
//! - **NATS in, HTTPS out** — registers external HTTPS agents into the NATS catalog as proxied
//!   AgentCards; gateway forwards to the HTTPS endpoint and adapts SSE → JetStream events.
//! - Auth is re-minted at the bridge in both directions. AgentCards declare both transports so
//!   polyglot clients pick.
//! - Cross-binding: when the HTTPS↔NATS bridge accepts an HTTPS A2A client, it terminates HTTP
//!   auth and re-mints a NATS **User JWT in the caller's tenant Account**. AgentCard's `security`
//!   block stays HTTPS-shaped on the outside; NATS-bound peers see the NATS-native schemes.
//!   Cross-tenant access is only possible through operator-signed Account exports — off by default.
//! - **HTTPS webhook** push targets are preserved for cross-binding interop; the gateway acts as a
//!   webhook client.
//!
//! Phase 4 delivery also tracks federated catalog exports and cross-binding agent collaboration
//! ([`A2A_PLAN.md`](../../../../A2A_PLAN.md) §Phased Delivery).

/// One planned cross-binding collaboration scenario (stub row — no runtime harness yet).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TestCaseStub {
    /// Stable scenario id for test registration and CI matrix filtering.
    pub name: &'static str,
    /// **`true`** when the scenario needs a live **`a2a-bridge`** sidecar (HTTPS↔NATS ingress).
    pub requires_bridge: bool,
    /// **`true`** when the scenario spans operator-signed Account exports (federated discovery).
    pub requires_federation: bool,
}

/// Compile-only catalog of cross-binding collaboration scenarios to implement after **`a2a-bridge`**.
pub trait CollaborationTestMatrix {
    /// Planned integration cases for this matrix version.
    fn cases(&self) -> &'static [TestCaseStub];
}

/// Minimal Phase 4 cross-binding matrix — bidirectional invoke paths plus one federated row.
///
/// | Scenario | Bridge | Federation |
/// |----------|--------|------------|
/// | HTTPS client → NATS agent (`message/send`) | yes | no |
/// | HTTPS client → NATS agent (`message/stream`) | yes | no |
/// | NATS client → HTTPS agent (`message/send`) | yes | no |
/// | NATS client → HTTPS agent (`message/stream`) | yes | no |
/// | Cross-Account invoke via signed export | yes | yes |
pub const MATRIX_V0: &[TestCaseStub] = &[
    TestCaseStub {
        name: "https_client_message_send_to_nats_agent",
        requires_bridge: true,
        requires_federation: false,
    },
    TestCaseStub {
        name: "https_client_message_stream_to_nats_agent",
        requires_bridge: true,
        requires_federation: false,
    },
    TestCaseStub {
        name: "nats_client_message_send_to_https_agent",
        requires_bridge: true,
        requires_federation: false,
    },
    TestCaseStub {
        name: "nats_client_message_stream_to_https_agent",
        requires_bridge: true,
        requires_federation: false,
    },
    TestCaseStub {
        name: "federated_cross_account_invoke",
        requires_bridge: true,
        requires_federation: true,
    },
];

/// Baseline matrix implementation — returns [`MATRIX_V0`].
#[derive(Debug, Clone, Copy, Default)]
pub struct BaselineCollaborationTestMatrix;

impl CollaborationTestMatrix for BaselineCollaborationTestMatrix {
    fn cases(&self) -> &'static [TestCaseStub] {
        MATRIX_V0
    }
}

/// Returns the landed Phase 4 minimal cross-binding collaboration matrix.
pub fn matrix_v0() -> &'static [TestCaseStub] {
    MATRIX_V0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_v0_covers_bidirectional_bridge_paths() {
        let cases = matrix_v0();
        assert_eq!(cases.len(), 5);

        assert!(cases
            .iter()
            .any(|c| c.name == "https_client_message_send_to_nats_agent"));
        assert!(cases
            .iter()
            .any(|c| c.name == "nats_client_message_send_to_https_agent"));

        for case in cases {
            assert!(
                case.requires_bridge,
                "Phase 4 cross-binding tests assume a2a-bridge: {}",
                case.name
            );
        }
    }

    #[test]
    fn baseline_matrix_trait_returns_matrix_v0() {
        let matrix = BaselineCollaborationTestMatrix;
        assert_eq!(matrix.cases(), MATRIX_V0);
    }

    #[test]
    fn federated_row_is_the_only_federation_case() {
        let federated: Vec<_> = MATRIX_V0
            .iter()
            .filter(|c| c.requires_federation)
            .collect();
        assert_eq!(federated.len(), 1);
        assert_eq!(federated[0].name, "federated_cross_account_invoke");
    }
}
