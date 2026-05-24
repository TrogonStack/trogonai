# Tier-1 declarative reference bundles

Reference `*.tier1.toml` profiles for [`Tier1DeclarativeConfig`](../../a2a-gateway/src/policy/tier1_declarative/evaluator.rs). Copy one file (or directory containing it) into your bundle dir.

## Enable

| Variable | Value |
|----------|-------|
| `A2A_GATEWAY_TIER1_DECLARATIVE_ENABLED` | `on` |
| `A2A_GATEWAY_TIER1_BUNDLE_DIR` | Path to directory holding the chosen `*.tier1.toml` |

SpiceDB Tier-1 (`A2A_GATEWAY_TIER1_SPICEDB_ENABLED`) remains the authz floor; declarative rules add optional deny/annotate layers.

## Bundles

| File | Denies | Notes |
|------|--------|-------|
| `per-method-allowlist.tier1.toml` | Any `agent_method` other than `message/send`, `tasks/get`, `tasks/list` | Catch-all `agent_method *` deny at priority 100 |
| `per-agent-allowlist.tier1.toml` | `caller_subject` not in `user/alice`, `user/bob`, `service/internal-*` | Maps gateway caller slug → `user/{slug}` SpiceDB subject |
| `time-of-day.tier1.toml` | All methods (`agent_method *`) when this after-hours profile is deployed | No time predicates in schema; swap bundle dir on schedule until `time_utc_between` (or similar) exists |

## Schema

- Match kinds: `agent_method`, `agent_id`, `caller_subject`, `nats_subject_pattern`
- Effects: `allow`, `deny`
- Deny returns JSON-RPC `-32801` on ingress

See [`docs/A2A_TIER1_DECLARATIVE.md`](../../../../docs/A2A_TIER1_DECLARATIVE.md).
