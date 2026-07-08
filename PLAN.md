# ACP Spec Catch-Up Plan

Bring the ACP-over-NATS crates (`acp-nats`, `acp-nats-agent`, `acp-nats-server`, `acp-nats-stdio`) up to date with the current Agent Client Protocol spec, and put tracking in place so we never drift silently again.

**Policy: opt in to unstable features ahead of stabilization.** The default for every unstable spec feature is to enable the flag, wire the routing, and test it, not to wait for stable. Opting out is the exception and requires a written rationale in the conformance matrix.

## Current position

| Fact | Value |
| --- | --- |
| Pinned SDK | `agent-client-protocol = "=0.10.4"` (`rsworkspace/Cargo.toml:56`, published 2026-03-31) |
| Bundled schema (our effective spec level) | 0.11.4 |
| Latest SDK | 1.2.0 (`agentclientprotocol/rust-sdk`) |
| Latest schema (spec) | 1.4.0 (`agentclientprotocol/agent-client-protocol`) |
| Wire protocol | v1 (unchanged, v2 is unstable upstream, no interop risk today) |

Key architectural fact driving this plan: the bridge decodes every message into typed SDK structs and re-serializes them (see `acp-nats/src/agent/new_session.rs`, `acp-nats/src/wire.rs`). Fields our pinned SDK does not model are silently stripped in transit, and unknown `session/update` variants fail decode. Spec lag therefore means silent data loss, not graceful passthrough.

## Known gaps

### Fixable at the current pin (SDK 0.10.4 already supports these behind flags)

1. `additionalDirectories` on `session/new` and `session/load`: flag `unstable_session_additional_directories` not enabled. Now **stable** upstream (schema 0.13.5). Currently stripped by the bridge. Highest-priority gap.
2. Elicitation: flag `unstable_elicitation` not enabled, and its client-side requests have no `ClientMethod` subject mapping in `acp-nats/src/nats/parsing.rs`.
3. NES (next edit suggestions): flag `unstable_nes` not enabled.
4. JSON-RPC request cancellation: flag `unstable_cancel_request` is enabled but zero code references its types, and no subject routing exists. Enabled-but-unwired.

### Unrepresentable at the current pin (require the SDK upgrade)

5. `session/delete`: stabilized in schema 0.13.6. No subject mapping, no handler, unroutable.
6. Stable request cancellation shape (schema 1.2.0): our flag corresponds to a pre-stable draft.
7. Newer `session/update` payloads: plan operations, current-shape usage updates, stabilized message IDs and config options. Unknown variants fail typed decode and are dropped.
8. Providers (schema 0.11.7, unstable), MCP-over-ACP message types (schema 0.13.0, unstable), stabilized boolean config options (schema 1.3.0), `model_config` option category (schema 1.1.0), elicitation enum option descriptions (schema 1.4.0, unstable).

### Inverse gap

9. `session/set_model` and `unstable_session_model`: upstream **removed** this API (schema 0.13.5); models are now session config options with the `model_config` category. We map a method the current spec no longer has.

### Flags we enable that have since stabilized upstream

`unstable_logout` (0.13.3), `unstable_session_close` and `unstable_session_resume` (0.12.2), `unstable_message_id` and `unstable_session_usage` (0.13.6), `unstable_cancel_request` (1.2.0), `unstable_boolean_config` (1.3.0). These flag names disappear or change on upgrade. Still genuinely unstable upstream: `unstable_session_fork`, `unstable_auth_methods`.

---

## Phase 0: Tracking foundation (do first, small)

- [x] Record our spec position: canonical table in `docs/architecture/acp-conformance.md`, pointer in `rsworkspace/crates/AGENTS.md`.
- [x] Add a comment on the `agent-client-protocol` pin in `rsworkspace/Cargo.toml` linking to the tracking issue and this plan.
- [x] Create a GitHub tracking issue for the 0.10.4 to 1.2.x migration referencing this plan: https://github.com/TrogonStack/trogonai/issues/474
- [x] Add a conformance matrix doc (`docs/architecture/acp-conformance.md`): every spec method and capability with status (implemented, forwarded, dropped, not applicable) and spec stage (stable, unstable). Seed it from the "Known gaps" section above.
- [x] Add a freshness signal: `.github/workflows/acp-freshness.yml`, weekly, compares the pinned SDK and bundled schema against crates.io latest and opens/updates a drift issue embedding the upgrade ritual checklist.
- [x] Define the upgrade ritual and write it into the conformance matrix doc, so a bump is never just a version change. Every SDK/schema bump must:
  1. Diff the schema changelog between the old and new pinned versions.
  2. For each added or stabilized method: add subject mapping in `parsing.rs`, handler, and tests, or a matrix row with opt-out rationale.
  3. For each added field or `session/update` variant: add a round-trip test through the bridge (typed re-encode means unmapped fields are silently dropped, so "it compiles" proves nothing).
  4. For each new unstable flag: enable it per the opt-in policy and wire it.
  5. Update the conformance matrix and the recorded spec position in the same PR.
  The freshness CI issue template should embed this checklist so the work is scoped the moment drift is detected.

**Acceptance**: a newcomer can answer "what spec level are we on and what do we not support" from the repo alone; drift produces an automated signal within a month; and no version bump can merge without the feature-mapping checklist applied.

## Phase 1: Close the gaps available at the current pin

Do these before the big migration; they are independent, low-risk, and immediately fix silent data loss.

- [x] Enable `unstable_session_additional_directories` in `acp-nats` and `acp-nats-agent`; round-trip tests cover wire encode/decode and full dispatch for `session/new` and `session/load`.
- [x] Request cancellation interim decision: cannot be wired at 0.10.4 because the SDK's `AgentSideConnection` dispatch drops `$/cancel_request` before the bridge sees it (verified in `acp-nats-server/src/connection.rs` ingress). Any NATS-leg routing would be unreachable dead code rewritten in Phase 2. Recorded in the conformance matrix; end-to-end wiring lands with the 1.x SDK in Phase 3.
- [x] Enable `unstable_elicitation`: flag enabled with capability round-trip tests. Method routing is impossible at this pin (no SDK trait surface until 0.14.0); full wiring moves to Phase 4 with the 1.x SDK.
- [x] Enable `unstable_nes`: capability payloads round-trip with tests; NES document methods follow the same SDK-trait limitation as elicitation.
- [x] Add a bridge-level guard for unknown `session/update` variants: decode failures now record `acp` error metric with `session_update`/`decode_failure` attributes and log the drop.

**Acceptance**: every feature the pinned SDK can express, stable or unstable, traverses the bridge with tests; unknown-variant drops are observable.

## Phase 2: SDK migration 0.10.4 to 1.2.x (the big one)

The 0.11.0 release was a full SDK redesign. Official guide: `agentclientprotocol/rust-sdk` `md/migration_v0.11.x.md`. This is a rewrite of our integration layer, not a version bump. 83 files reference the crate.

- [x] Spike: bumped in a scratch worktree; 68 errors in acp-nats, all import moves plus trait-to-role-marker changes; downstream crate errors are cascades. Migration proceeds crate by crate in dependency order.
- [x] Migrate imports: message types now under `agent_client_protocol::schema::v1::*` (`ProtocolVersion` at `schema::` root; `Error`/`ErrorCode`/`Result` stay at the crate root).
- [x] Replace connection construction: all three byte-stream boundaries (server WebSocket, server HTTP duplex, stdio) now go through `acp_nats::boundary::connect_agent_boundary`, which wraps the incoming stream in an EOF detector (the 1.x SDK treats incoming EOF as graceful and would otherwise never terminate; caught by the previously hanging `run_bridge_exits_on_io_close` test).
- [x] Replace trait impls: inbound via `on_receive_*` callbacks delegating to `AgentHandler` (ADR 0017); outbound via `boundary::ConnectionClient`, a `ClientHandler` over `ConnectionTo<Client>`. `block_task()` is only called from bullard-owned dispatch tasks, never SDK callbacks.
- [x] Session management: kept raw request-level control. The bridge routes discrete requests between independent peers; `SessionBuilder`/`ActiveSession` model a client owning a session lifecycle, which the bridge deliberately does not.
- [x] Evaluate the new transport abstraction: decision recorded in ADR 0017 (keep the NATS transport, bridge-owned handler traits, SDK builders only at byte-stream boundaries).
- [x] Update feature flags: SDK flags now unstable_auth_methods, unstable_elicitation, unstable_end_turn_token_usage, unstable_mcp_over_acp, unstable_session_fork; plus a direct `agent-client-protocol-schema` dependency enabling unstable_llm_providers, unstable_nes, unstable_plan_operations (the SDK facade does not forward these; cargo feature unification applies them).
- [x] Remove `session/set_model`: mapping, handler, subjects, semconv span, and tests removed (upstream deleted the API). All consumers were in-repo; external runner repos are flagged in the PR description. Model switching goes through session config options.
- [x] Full test pass: 684 tests green across the four crates, clippy zero warnings, full workspace check clean. Interop: the server tests drive raw v1 JSON-RPC frames over a real WebSocket through the SDK 1.2 ingress (current-SDK peer), and the boundary tests round-trip 0.10.4-era-shaped frames (old peer wire compat).

**Acceptance**: workspace builds on SDK 1.2.x with no `=` pin surprises, all tests green, interop with both an old (v1, 0.10.4-era) peer and a current 1.2.x peer verified, ADR recorded for the transport decision.

## Phase 3: Map newly stable spec surface (post-upgrade)

- [ ] `session/delete`: add `SessionAgentMethod::Delete`, subject, handler in `acp-nats/src/agent/`, JetStream/stream wiring, metrics span name in `trogon-semconv`, and tests.
- [ ] Stable request cancellation: adopt the 1.x shape end to end (supersedes the Phase 1 interim decision).
- [ ] New `session/update` variants (plan operations, usage updates, stabilized message IDs): confirm they round-trip through the bridge with tests per variant.
- [ ] Stable boolean config options and `model_config` category: round-trip tests through `set_config_option`.
- [ ] `Acp-Protocol-Version` header validation in `acp-nats-server`: confirm negotiation still matches the 1.x transport spec wording.
- [ ] Update the conformance matrix to the new position (target: schema 1.4.0, everything stable either implemented or explicitly recorded as not supported).

**Acceptance**: conformance matrix shows no "dropped" status for any stable spec feature.

## Phase 4: Unstable feature adoption and forward watch (ongoing)

Per the opt-in policy, adopt every unstable feature the 1.x SDK exposes. Each one gets its flag enabled, subject mapping where it adds methods, round-trip tests, and a conformance matrix row. Opting out requires a written rationale.

- [ ] Elicitation enum option descriptions (schema 1.4.0).
- [ ] Plan operations (schema 0.13.4).
- [ ] Providers (schema 0.11.7).
- [ ] MCP-over-ACP message types (schema 0.13.0).
- [ ] Session fork and auth methods (carried over from our current flags, under their 1.x names).
- [ ] New unstable features as upstream ships them: the freshness CI job from Phase 0 is the trigger; each new flag becomes a task here.
- [ ] Protocol v2: heavy `unstable-v2` churn in every recent schema release. Track it actively and plan early adoption once upstream marks it preview; until then it is watch-only since the wire format is still changing under our feet.
- [ ] Revisit the typed-decode-vs-passthrough architecture: consider lossless forwarding (raw `serde_json::Value` passthrough with typed validation only where the bridge needs to read fields) so future spec additions degrade to "forwarded" instead of "dropped". If adopted, record as an ADR; this materially reduces the cost of every future spec bump.

## Sequencing and dependencies

```
Phase 0 (tracking)        independent, do immediately
Phase 1 (current-pin fixes) independent of Phase 2, do before it
Phase 2 (SDK 1.2.x)       blocks Phases 3 and 4
Phase 3 (new stable surface) after Phase 2
Phase 4 (unstable adoption, watch) after Phase 3, ongoing
```

## References

- Rust SDK repo and migration guide: https://github.com/agentclientprotocol/rust-sdk (see `md/migration_v0.11.x.md`)
- Spec/schema repo and changelog: https://github.com/agentclientprotocol/agent-client-protocol
- Crate family on crates.io: `agent-client-protocol`, `-schema`, `-http`, `-rmcp`, `-tokio`, `-polyfill`, `-derive`
- Our method routing inventory: `rsworkspace/crates/acp-nats/src/nats/parsing.rs`
- Typed re-encode (lossiness source): `rsworkspace/crates/acp-nats/src/wire.rs`
