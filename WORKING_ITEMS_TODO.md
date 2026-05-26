# A2A working items

In-tree code work that's actionable today on `yordis/feat-a2a-nats`. Extracted from `A2A_PLAN.md` and the Phase 4 smoke-full findings.

Operator/deployment work (deployed auth-callout subscriber, NSC pipelines, signing-key custody, signed-bundle distribution, cross-Account export distribution, fleet publisher rollout) lives in [`A2A_TODO.md`](./A2A_TODO.md) — none of those need code changes here.

## Bundles (`a2a-pack` placeholders)

Source: `A2A_PLAN.md` §Implementation Status — Shipped crates / `a2a-pack`.

- [x] **Resource-tuple table bundle** — surface the `A2A_PLAN.md` §SpiceDB-resource-tuples table as a first-class `a2a-pack` module the gateway loads (rather than hard-coded in `a2a-gateway/src/policy/spicedb_tier1.rs`).
- [x] **Audit envelope extension bundle** — `a2a-pack` module for A2A-specific audit fields beyond the shipped `trace_id` / `rules_fired` / `rewrites` / `refusal_skill` / `stream_consumer` / `zed_token_snapshot` shape.
- [x] **Rate-limit profile bundle** — default per-skill-kind rate-limit profile per `A2A_PLAN.md` §Bundles.

## Audit attribution fields surfaced by Phase 4 smoke

Source: smoke-full assertions (`a2a-smoke-test --profile full`) currently fall back to `rules_fired` because dedicated decision fields don't exist.

- [ ] **`tier1_decision` field on `AuditEnvelope`** — `allow` / `deny` per Tier-1 evaluation (SpiceDB + declarative), populated from `a2a-gateway/src/policy/`.
- [ ] **`tier3_decision` field on `AuditEnvelope`** — `allow` / `refuse` / `error` per Tier-3 evaluation.
- [ ] **Distinct JSON-RPC error code for declarative `agent.card` deny** — today the path returns `-32801` (same code as engine error). Either carve out a new code per `docs/A2A_TIER1_DECLARATIVE.md` or document why `-32801` is the contract.

Once these land, the `a2a-smoke-test --profile full` assertions can read these fields directly instead of inferring from `rules_fired`.

## Gateway / discovery

Source: `A2A_PLAN.md` Phase 1 Pending.

- [ ] **Discovery shaping via `BulkCheckPermission`** — on `a2a.discover.*` response, filter AgentCards by caller permission. Tuple shape per `A2A_PLAN.md` §SpiceDB-resource-tuples (`view` on `agent:{agent_id}`).

## Streaming back-pressure

Source: `A2A_PLAN.md` §Decisions — Streaming back-pressure.

- [ ] **Gateway ingress pipe** — the egress pull consumer ships env-gated (`A2A_GATEWAY_EVENTS_PULL`); the full ingress pipe (gateway → agent flow control + agent publish never-blocks invariant) is not wired end-to-end.

## Tier-1 declarative

Source: `a2a-pack/policies/time-of-day.tier1.toml` ships an approximation.

- [x] **True time-of-day predicates** — extend the declarative evaluator to accept real time predicates (clock-aware), replacing the current approximation bundle.

## Catalog

Source: `A2A_PLAN.md` §Intentional simplifications — agent catalog.

- [ ] **KV watch ergonomics for clients** — wrap `async-nats` KV watch in an A2A-shaped helper so discovery clients can subscribe to AgentCard changes without re-implementing watch semantics.

## Doc cleanup

- [ ] **Stale line in `A2A_PLAN.md` §Working surface — Audit** — "Gateway policy attribution fields (`rules_fired`, etc.) remain future once a gateway path exists" is no longer true; gateway audit envelope already carries `rules_fired` / `rewrites` / `refusal_skill` / `stream_consumer` / `zed_token_snapshot` (§Shipped crates). Update the bullet to match reality.

## Out of scope (deferred by §Decisions)

- Digest webhook auth — currently rejected with a clear error per `A2A_PLAN.md` §Decisions.
- Unary `message/send` task-lifecycle envelope coverage — product-scope decision; callers observe post-reply transitions via `message/stream` / `tasks/resubscribe` or the task event subject.
