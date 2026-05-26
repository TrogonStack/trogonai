# A2A NSC subject ACL templates

Plain-text equivalents of Phase 0 **per-tenant Account** NATS JWT permissions for [`nsc add user`](https://nats-io.github.io/nsc/nsc_add_user.html).

Each `.acl` file lists subject lines grouped by intended NSC flag (`--allow-pub`, `--allow-sub`, `--deny-pub`, `--deny-sub`). **Repeat the flag once per subject line.** Pass [`--allow-pub-response`](https://nats-io.github.io/nsc/nsc_add_user.html) wherever the header comment recommends it.

The automated consumer is [`scripts/a2a-nsc-bootstrap.sh`](../a2a-nsc-bootstrap.sh), which substitutes `A2A_PREFIX` and related env vars rather than rewriting these templates at runtime — keep the meanings aligned when you extend either side.

Operational background: [`docs/a2a/how-to/operators/nsc-account-bootstrap.md`](../../docs/a2a/how-to/operators/nsc-account-bootstrap.md), [`docs/a2a/reference/subject-acl-quickref.md`](../../docs/a2a/reference/subject-acl-quickref.md).

## Substitution variables

| Token | Meaning | Bootstrap / NSC source |
|------|---------|--------------------------|
| `{prefix}` | Core subject anchor (`a2a.gateway.>`, …) | Substitute **`A2A_PREFIX`** (`a2a-nsc-bootstrap.sh`; default `a2a`). |
| `{caller_id}` | Stable caller segment for inbox isolation | JWT claim / `--allow-sub "_INBOX.${id}.>"`; template placeholder for auth-callout minting (`a2a-caller-template` uses the literal substring `{caller_id}` only as documentation aid — replace before production use). |
| `{catalog_kv_bucket}` | JetStream KV bucket storing AgentCards | Matches JetStream KV name (**`A2A_AGENT_CARDS`** by default) — see **`A2A_CATALOG_KV_BUCKET`** env var on the bootstrap script. Expands KV publish subjects **`$KV.<bucket>.>`**. |

## Files

| File | Account role | Highlights |
|------|--------------|-----------|
| `caller.acl` | Caller Users | Ingress publish-only on `{prefix}.gateway.*`; replies + scoped push pulls on `_INBOX.*` / `{prefix}.push.{caller_id}.*`; explicit denies on sibling `{prefix}` branches. |
| `gateway.acl` | Gateway service | Ingress + discover subscribe; agent/task/push bidirectional ACL; denies catalog register ingress. |
| `registrar.acl` | Discovery / catalog KV | Registers + discover subscribe paths; KV data-plane publishes on **`$KV.{catalog_kv_bucket}.>`**; denies gateway/data-plane masquerading. |

## Narrow-allow + explicit deny

JWT authorization for Users is modeled as **allowlists**: anything not enumerated for publish/subscribe is rejected once those permissions exist. Explicit deny lines blacklist high-risk namespaces (defense-in depth) rather than implying a permissive default.
