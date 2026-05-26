# A2A gateway Tier-1 declarative policy

Tier-1 declarative policy evaluates static allow/deny matrices from TOML bundle files on the gateway ingress path. It runs **after** the SpiceDB Tier-1 gate and **before** Tier-2 CEL. SpiceDB remains the authoritative authz floor; declarative rules add an optional deny layer or fast-path allow annotations.

Evaluation is **opt-in** (`A2A_GATEWAY_TIER1_DECLARATIVE_ENABLED`, default off). Operators author bundle files; the gateway loads them at startup.

## Bundle layout

Load rules from `*.tier1.toml` files under `A2A_GATEWAY_TIER1_BUNDLE_DIR`:

```text
{A2A_GATEWAY_TIER1_BUNDLE_DIR}/
  deny-guests.tier1.toml
  allow-internal.tier1.toml
```

Each file may define one or more `[[rule]]` tables. All files are merged into a single bundle sorted by descending `priority` at load time. Parse errors refuse gateway startup (closed-fail).

## Bundle schema

```toml
[[rule]]
id = "deny-guest-planner"
priority = 100
effect = "deny"

[[rule.matches]]
kind = "agent_id"
pattern = "planner"

[[rule.matches]]
kind = "agent_method"
pattern = "message/send"

[[rule.matches]]
kind = "caller_subject"
pattern = "user/guest-*"
negate = false

[[rule.matches]]
kind = "nats_subject_pattern"
pattern = "a2a.gateway.*.message.send"
```

| Field | Meaning |
|-------|---------|
| `id` | Stable rule identifier (audit `rules_fired` suffix) |
| `priority` | Higher values win; first fully matching rule decides |
| `effect` | `allow` or `deny` |
| `matches` | AND group — all must hit for the rule to fire |

### Match kinds

| `kind` | Context field | Example pattern |
|--------|---------------|-----------------|
| `agent_method` | A2A method with slashes (`message/send`) | `message/send` |
| `agent_id` | Target agent id from ingress subject | `planner` |
| `caller_subject` | SpiceDB subject from caller principal (`user/alice`); empty when anonymous | `user/alice` |
| `nats_subject_pattern` | Full ingress NATS subject | `a2a.gateway.*.message.send` |

### Pattern syntax

- **Exact match** when the pattern contains no `*`.
- **Glob match** when `*` appears — `*` matches zero or more characters (standard left-to-right glob).
- **`negate = true`** inverts the match result for that condition.

## Evaluation order

Gateway dispatch on ingress:

1. Unary deadline guard (`message.send`)
2. **Tier-1 SpiceDB** (`A2A_GATEWAY_TIER1_SPICEDB_ENABLED`)
3. **Tier-1 declarative** (this layer)
4. Tier-2 CEL (`A2A_GATEWAY_TIER2_CEL_ENABLED`)
5. Forward to agent subject

Declarative semantics:

1. Walk rules in **descending priority**.
2. First rule whose `matches` all hit → return that rule's `effect`.
3. No rule hits → **default allow** (SpiceDB already enforced the deny floor).

## Environment knobs

| Variable | Default | Meaning |
|----------|---------|---------|
| `A2A_GATEWAY_TIER1_DECLARATIVE_ENABLED` | off | Truthy loads the declarative evaluator; off uses noop (always allow) |
| `A2A_GATEWAY_TIER1_BUNDLE_DIR` | — | Directory of `*.tier1.toml` bundle files (required when enabled) |

See [`A2A_RUNTIME_ENV.md`](../runtime-env.md) for the full `a2a-gateway` table.

## Audit `rules_fired`

| Outcome | `rules_fired` entry |
|---------|----------------------|
| Deny | `gateway.tier1.declarative.denied.{rule_id}` |
| Allow (explicit rule) | `gateway.tier1.declarative.allowed.{rule_id}` |
| Allow (no match) | `gateway.tier1.declarative.no_match_default_allow` |
| Layer disabled | `gateway.tier1.declarative.layer_disabled` |

Deny returns JSON-RPC **`-32803`** (`tier1_declarative_denied`) to the caller reply inbox. SpiceDB and Tier-2 denials remain **`-32801`** (`policy_denied`); Tier-3 engine errors also use **`-32801`**.

## Example rules

Deny a specific agent for all methods:

```toml
[[rule]]
id = "deny-blocked-agent"
priority = 200
effect = "deny"

[[rule.matches]]
kind = "agent_id"
pattern = "blocked-agent"
```

Fast-path allow annotation for internal callers (does not bypass SpiceDB):

```toml
[[rule]]
id = "annotate-internal"
priority = 50
effect = "allow"

[[rule.matches]]
kind = "caller_subject"
pattern = "service/internal-*"
```

## Related docs

- [`A2A_RUNTIME_ENV.md`](../runtime-env.md) — env reference
- [`A2A_TIER2_CEL.md`](tier2-cel.md) — Tier-2 CEL bundle layout (design template)
- [`A2A_GATEWAY_ROADMAP.md`](../../explanation/gateway-roadmap.md) — gateway policy roadmap
