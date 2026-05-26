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
| `time-of-day.tier1.toml` | Requests outside Mon–Fri 09:00–17:00 UTC | `time_of_day` match with pattern `Mon-Fri\|09:00-17:00\|UTC` and `negate = true` |

## Schema

- Match kinds: `agent_method`, `agent_id`, `caller_subject`, `nats_subject_pattern`, `time_of_day`
- `time_of_day` pattern: `{weekdays}|{HH:MM-HH:MM}|{UTC|Z|±HH:MM}` (weekdays: `Mon-Fri`, `Mon,Wed`, or `*`)
- Effects: `allow`, `deny`
- Deny returns JSON-RPC `-32801` on ingress

See [`docs/a2a/reference/policy/tier1-declarative.md`](../../../../docs/a2a/reference/policy/tier1-declarative.md).
