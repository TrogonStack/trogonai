//! Second-wave roadmap seams — one submodule per open **`A2A_TODO.md`** engineering line (Phases 0–4).
//!
//! These are **compile-only** boundaries: no wiring into [`crate::runtime`] yet. Pair with sibling
//! modules at the [`planned`](crate::planned) crate root (`gw_pull_backpressure`, `gw_unary_deadline`,
//! …) for overlapping seam coverage.
//!
//! | Module | Backlog lane (`A2A_TODO.md`) |
//! |--------|----------------|
//! | [`p0_agentcard_gateway_revalidation`] | Gateway JSON-Schema re-validation outside KV |
//! | [`p0_auth_callout_service`] | Sys auth responder + OIDC / mTLS / API keys |
//! | [`p0_subject_acl_matrix`] | Account-internal caller vs gateway ACL |
//! | [`p0_nsc_org_automation`] | NSC org exports beyond bootstrap |
//! | [`p1_tier1_policies`] | Tier&nbsp;1 declarative ingress policy |
//! | [`p1_spicedb_gateway`] | SpiceDB bulk tuples + gateway hooks |
//! | [`p1_audit_decision_sites`] | Ingress `rules_fired` / deny / rewrite audit |
//! | [`p2_wasmtime_substrate`] | Shared Wasmtime host (CEL + redaction WASM) |
//! | [`p2_streaming_backpressure`] | Gateway pull consumer + `A2A_EVENTS` retention |
//! | [`p2_message_send_deadline`] | `message/send` 30s gateway story |
//! | [`p3_push_exactly_once_config`] | Exactly-once push negotiation |
//! | [`p3_dlq_caller_jwt_gateway_mirror`] | DLQ `caller_id` + gateway mirror |
//! | [`p3_tier3_redaction_wasm`] | Tier&nbsp;3 `parts[*]` redaction |
//! | [`p4_a2a_bridge_runtime`] | Full **`a2a-bridge`** process seams |
//! | [`p4_federated_discovery`] | Signed `discover.>` export + SpiceDB import |
//! | [`p4_cross_binding_tests`] | Cross-binding collaboration test matrix |

#![allow(dead_code)]
#![allow(missing_docs)]

pub mod p0_agentcard_gateway_revalidation;
pub mod p0_auth_callout_service;
pub mod p0_nsc_org_automation;
pub mod p0_subject_acl_matrix;
pub mod p1_audit_decision_sites;
pub mod p1_spicedb_gateway;
pub mod p1_tier1_policies;
pub mod p2_message_send_deadline;
pub mod p2_streaming_backpressure;
pub mod p2_wasmtime_substrate;
pub mod p3_dlq_caller_jwt_gateway_mirror;
pub mod p3_push_exactly_once_config;
pub mod p3_tier3_redaction_wasm;
pub mod p4_a2a_bridge_runtime;
pub mod p4_cross_binding_tests;
pub mod p4_federated_discovery;
