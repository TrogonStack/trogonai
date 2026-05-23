//! Compile-time scaffolding for roadmap items tracked in **`A2A_PLAN.md`** / **`A2A_TODO.md`**.
//!
//! Modules here are **`planned`/`stub` placeholders** until the gateway and bridge crates grow real
//! implementations. Keeping them compiling exercises module boundaries ahead of wiring into
//! [`crate::runtime`].
//!
//! | Module | Roughly maps to roadmap |
//! |--------|--------------------------|
//! | [`gw_pull_backpressure`] | §5 task-event egress consumer + flow control |
//! | [`gw_unary_deadline`] | §6 gateway-enforced unary timeout story |
//! | [`bridge_identity_auth_callout`] | §7 `a2a-bridge` + auth-callout identity |
//! | [`federated_discovery_export`] | §8 operator-signed `a2a.discover.>` exports |
//! | [`gateway_user_push_acl`] | NATS push auth binding (`a2a.push.>` ACL for gateway User) |
//! | [`nats_auth_callout_sys`] | Auth-callout **`$SYS.REQ.USER.AUTH`** subscriber sketch |
//! | [`policy_wasmtime_substrate`] | Gateway Wasmtime policy seam |
//! | [`spicedb_gateway_client`] | SpiceDB client seam for ingress checks |
//! | [`bridge_https_sidecar`] | `a2a-bridge` HTTPS sidecar cargo surface |
//! | **[`batch2`](batch2)** | Detailed Phase 0–4 backlog items (**`A2A_TODO`** line-by-line compile seams) |

#![allow(dead_code)]
#![allow(missing_docs)]

pub mod batch2;
pub mod bridge_identity_auth_callout;
pub mod federated_discovery_export;
pub mod gateway_user_push_acl;
pub mod gw_pull_backpressure;
pub mod gw_unary_deadline;
pub mod nats_auth_callout_sys;
pub mod policy_wasmtime_substrate;
pub mod spicedb_gateway_client;
