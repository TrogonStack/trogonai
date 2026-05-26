# A2A gateway Tier-2 CEL policy

Tier-2 ingress policy evaluates [CEL](https://cel.dev/) expressions from a bundle directory before the gateway forwards JSON-RPC to agent subjects. Evaluation is **opt-in** until operators exercise bundles in production.

## Bundle layout

When `A2A_GATEWAY_POLICY_BUNDLE_DIR` is set, the gateway loads Tier-2 rules from:

```text
{A2A_GATEWAY_POLICY_BUNDLE_DIR}/tier2/*.cel
```

Each `*.cel` file is one rule. The rule name is the file stem (e.g. `deny_guests.cel` → `deny_guests`). Tier-3 redaction WASM bundles remain at the bundle root (`{skill}.wasm`) as today.

Compiled programs are cached in memory keyed by path and file mtime; edits to a `.cel` file are picked up on the next ingress evaluation without restarting the gateway.

## Environment knobs

| Variable | Default | Meaning |
|----------|---------|---------|
| `A2A_GATEWAY_POLICY_BUNDLE_DIR` | — | Enables the Wasmtime policy substrate (Tier-3 preload + Tier-2 seam). Required parent directory for `tier2/`. |
| `A2A_GATEWAY_TIER2_CEL_ENABLED` | off | When truthy (`on`, `true`, `1`, `yes`) **and** `A2A_GATEWAY_POLICY_BUNDLE_DIR` is set, loads `tier2/*.cel` and runs `RealTier2CelEvaluator`. When off, Tier-2 stays on `NoopTier2Evaluator` (always allow). |

See also [`A2A_RUNTIME_ENV.md`](./A2A_RUNTIME_ENV.md) for the full `a2a-gateway` table.

## CEL evaluation context

Each rule receives these top-level variables (bound from the ingress envelope):

| Variable | Type | Source |
|----------|------|--------|
| `request.method` | string | JSON-RPC method with slashes (e.g. `message/send`) |
| `request.params` | map | JSON-RPC `params` object (or null) |
| `caller.id` | string | NATS `X-A2a-Caller-Id` header when present |
| `agent.id` | string | Target agent id from the ingress subject |
| `task.id` | string | `params.taskId` or `params.task_id` when present |
| `headers.*` | map | Ingress NATS headers (string keys → string values) |

Example allow predicate that rejects guest roles on `message/send`:

```cel
request.method == "message/send" && request.params.message.role != "guest"
```

## Rule semantics

Rules are **allow predicates** evaluated in lexicographic order by rule name:

1. Each rule must evaluate to a **bool**.
2. First rule that evaluates to **`false`** → deny with that rule name.
3. All rules `true` (or no rules loaded) → allow.

On evaluation errors (non-bool result, CEL runtime error, bundle refresh failure), the gateway **denies closed** with rule name `evaluation_error` and audit `rules_fired` entry `gateway.tier2.evaluation_error`.

Policy denial returns JSON-RPC **`-32801`** to the caller reply inbox. Deny audit envelopes set `rules_fired` to `gateway.tier2.{rule_name}` (e.g. `gateway.tier2.deny_guests`).

Successful allow paths emit `gateway.tier2.evaluated_allow` when CEL is enabled, or `gateway.tier2.no_op_evaluated_true` when the bundle dir is set but CEL is off.

## Related docs

- [Runtime env reference](./A2A_RUNTIME_ENV.md)
- [Gateway roadmap](./A2A_GATEWAY_ROADMAP.md)
- [Architecture plan](./A2A_ARCHITECTURE.md)
