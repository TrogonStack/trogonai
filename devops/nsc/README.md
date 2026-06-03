# NSC operator / account / user templates

Reference material for operators bringing up a tenant NATS Account that
runs the A2A and MCP agentgateways. The full operator runbook lives at
`docs/a2a/how-to/operators/nsc-account-bootstrap.md`; this directory is
the deployable artifact tree.

## Layout

| Path | Purpose |
|------|---------|
| `operator.template.env` | Required env vars to push an operator JWT and resolver config (no operator key material lives in-tree). |
| `account.template.env` | Per-tenant Account JWT shape: JetStream limits, exports, imports. |
| `users/caller.acl` | Caller User subject ACL (mirrors `scripts/acl-templates/caller.acl`). |
| `users/gateway.acl` | Gateway service User ACL. |
| `users/registrar.acl` | Discovery / catalog registrar User ACL. |
| `bootstrap.sh` | Convenience wrapper around `scripts/a2a-nsc-bootstrap.sh`. |

The ACL files in `users/` are intentionally duplicated from
`scripts/acl-templates/`. Operators consuming a release tarball get
`devops/nsc/` as a self-contained artifact tree; the script in
`scripts/` is the canonical source consumed by CI / dev automation.

## Apply

```sh
# 1. Configure the operator (one-time per cluster).
source devops/nsc/operator.template.env
nsc add operator --generate-signing-key --sys --name "$NSC_OPERATOR_NAME"

# 2. Per-tenant Account.
source devops/nsc/account.template.env
A2A_OPERATOR_NAME="$NSC_OPERATOR_NAME" \
A2A_ACCOUNT_NAME="$A2A_ACCOUNT_NAME" \
A2A_NSC_DIR="$NSC_HOME" \
  devops/nsc/bootstrap.sh
```

## Account JWT signing key Secret shape

Every component that mints or validates Account-scoped JWTs needs the
operator signing key (`-S`/`SK…` material from `nsc generate keys -S`).
Reference Kubernetes Secret shape:

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: trogon-account-signing-key
  namespace: trogon-system
type: Opaque
stringData:
  # NSC-generated Account signing key seed (NKEY format).
  account-signing.nk: SAAH…REPLACE…
  # Operator JWT (public) used by the resolver to verify the Account.
  operator.jwt: |
    eyJ…REPLACE…
```

Consumers:

- `trogon-sts` — signs short-lived User JWTs for delegated callers.
- `trogon-jwks-publisher` — publishes the matching JWKS to the
  `mcp-jwks` KV.
- `a2a-auth-callout` — issues User JWTs in response to
  `$SYS.REQ.USER.AUTH` callouts.

Rotate by generating a new signing key with `nsc generate keys -S`,
adding it to the Account JWT (`nsc edit account --sk <pub>`), and
patching the Secret. Old key remains valid until removed.
