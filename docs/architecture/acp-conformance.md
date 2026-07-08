# ACP Conformance

This document is the single source of truth for where this repository stands relative to the Agent Client Protocol (ACP) specification. Update it in the same PR as any `agent-client-protocol` version bump or any change to the bridged method surface.

## Spec position

| Fact | Value |
| --- | --- |
| Wire protocol | v1 |
| Pinned Rust SDK | `agent-client-protocol` 0.10.4 (`rsworkspace/Cargo.toml`) |
| Bundled schema (effective spec level) | 0.11.4 |
| Latest upstream SDK at last review | 1.2.0 (2026-07-07) |
| Latest upstream schema at last review | 1.4.0 (2026-07-06) |
| Last reviewed | 2026-07-07 |

Upstream repositories: [spec/schema](https://github.com/agentclientprotocol/agent-client-protocol), [Rust SDK](https://github.com/agentclientprotocol/rust-sdk).

## Policy

Opt in to unstable spec features ahead of stabilization. The default for every unstable feature is to enable the flag, wire the routing, and test it. Opting out is the exception and requires a rationale in the matrix below.

## Why this matters here

The bridge decodes every message into typed SDK structs and re-serializes them (`acp-nats/src/wire.rs`). Fields the pinned SDK does not model are silently stripped in transit, and unknown `session/update` variants fail decode. Spec lag means silent data loss, not graceful passthrough, so this matrix must stay accurate.

## Conformance matrix

Status values: `implemented` (routed, typed, tested), `unwired` (SDK flag enabled but no routing), `dropped` (peers may send it, the bridge strips or rejects it), `unrepresentable` (pinned SDK cannot express it), `not supported` (deliberate opt-out with rationale).

### Agent-side methods (client to agent)

| Spec surface | Spec stage (schema 1.4.0) | Our status | Notes |
| --- | --- | --- | --- |
| `initialize` | stable | implemented | |
| `authenticate` | stable | implemented | `unstable_auth_methods` shapes enabled |
| `logout` | stable (0.13.3) | implemented | via 0.10.4-era `unstable_logout` |
| `session/new` | stable | implemented | includes `additionalDirectories` |
| `session/load` | stable | implemented | includes `additionalDirectories` |
| `session/list` | stable | implemented | |
| `session/prompt` | stable | implemented | |
| `session/cancel` (notification) | stable | implemented | |
| `session/set_mode` | stable | implemented | |
| `session/set_config_option` | stable | implemented | 0.10.4-era shape |
| `session/set_model` | **removed upstream** (0.13.5) | implemented | to remove during SDK migration; replaced by `model_config` config options |
| `session/fork` | unstable | implemented | |
| `session/resume` | stable (0.12.2) | implemented | via 0.10.4-era flag |
| `session/close` | stable (0.12.2) | implemented | via 0.10.4-era flag |
| `session/delete` | stable (0.13.6) | unrepresentable | requires SDK 1.x; Phase 3 of PLAN.md |
| JSON-RPC request cancellation | stable (1.2.0) | unwired, blocked at ingress | flag enabled, but the 0.10.4 SDK connection drops `$/cancel_request` before the bridge sees it, so no routing can be exercised end to end at this pin; implemented with the 1.x SDK in Phase 3 of PLAN.md |
| `ext/*` (extension methods) | stable | implemented | passthrough |

### Client-side methods (agent to client)

| Spec surface | Spec stage | Our status | Notes |
| --- | --- | --- | --- |
| `fs/read_text_file` | stable | implemented | |
| `fs/write_text_file` | stable | implemented | |
| `session/request_permission` | stable | implemented | |
| `session/update` | stable | implemented | unknown variants fail decode and are dropped with a `session_update`/`decode_failure` error metric |
| `terminal/create` | stable | implemented | |
| `terminal/output` | stable | implemented | |
| `terminal/release` | stable | implemented | |
| `terminal/wait_for_exit` | stable | implemented | |
| `terminal/kill` | stable | implemented | |
| Elicitation requests | unstable | capabilities implemented | flag enabled, capability fields round-trip; the 0.10.4 SDK trait surface has no elicitation methods (SDK support landed in 0.14.0), full wiring lands with SDK 1.x in Phase 4 of PLAN.md |
| `ext/*` | stable | implemented | passthrough, plus bullard-specific `ext/session/prompt_response` |

### Payload-level capabilities

| Spec surface | Spec stage | Our status | Notes |
| --- | --- | --- | --- |
| `additionalDirectories` (session/new, session/load) | stable (0.13.5) | implemented | round-trip tested through the bridge |
| Message IDs on chunks | stable (0.13.6) | implemented | 0.10.4-era `unstable_message_id` shape |
| Session usage updates | stable (0.13.6) | implemented | 0.10.4-era `unstable_session_usage` shape |
| Session config options | stable | implemented | 0.10.4-era shape |
| Boolean config options | stable (1.3.0) | implemented (draft shape) | stabilized shape requires SDK 1.x |
| `model_config` option category | stable (1.1.0) | unrepresentable | requires SDK 1.x; Phase 3 |
| NES (next edit suggestions) | unstable | capabilities implemented | capability payloads round-trip; NES document methods have no trait surface at this pin, full wiring lands with SDK 1.x |
| Plan operations | unstable (0.13.4) | unrepresentable | requires SDK 1.x; Phase 4 |
| Providers | unstable (0.11.7) | unrepresentable | requires SDK 1.x; Phase 4 |
| MCP-over-ACP message types | unstable (0.13.0) | unrepresentable | requires SDK 1.x; Phase 4 |
| Elicitation enum option descriptions | unstable (1.4.0) | unrepresentable | requires SDK 1.x; Phase 4 |
| Protocol v2 | unstable, heavy churn | watch-only | adopt at preview per PLAN.md Phase 4 |

## Upgrade ritual

A version bump of `agent-client-protocol` (or the schema it bundles) is never just a version change. Every bump PR must:

1. Diff the schema changelog between the old and new pinned versions ([changelog](https://github.com/agentclientprotocol/agent-client-protocol/blob/main/CHANGELOG.md)).
2. For each added or stabilized method: add subject mapping in `acp-nats/src/nats/parsing.rs`, a handler, and tests, or add a matrix row with an opt-out rationale.
3. For each added field or `session/update` variant: add a round-trip test through the bridge. Typed re-encode means unmapped fields are silently dropped, so a green compile proves nothing about coverage.
4. For each new unstable flag: enable it per the opt-in policy and wire it.
5. Update this document (matrix and spec position table) in the same PR.

The scheduled freshness workflow (`.github/workflows/acp-freshness.yml`) embeds this checklist in the issue it files when drift is detected.
