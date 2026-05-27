# Agent Registry — Operator Runbook

On-call reference for the Git → controller → KV registry pipeline. Runtime lookup contract: `docs/identity/registry.md`. Control plane crate: `trogon-agent-registry-controller`.

---

## Roles

| Component | NATS identity | KV access | Writes Git? |
|---|---|---|---|
| Manifest repo | human / CI | none | yes (PR merge) |
| `trogon-agent-registry-controller` | `{org}/registry-controller` service account | **read/write** `mcp-agent-registry` | no (pull only) |
| `trogon-agent-registry` | lookup service account | **read** + watch | no |
| STS / gateway | consumer accounts | read / lookup API | no |

**ACL rule:** only the controller signer NATS account may `PUT` or `DELETE` keys in `mcp-agent-registry`. All other principals are read-only. Break-glass KV writes require dual control (see Emergency revocation).

---

## Bootstrap (fresh cluster)

1. **Provision KV bucket** `mcp-agent-registry` per tenant/account policy (JetStream KV, history ≥ 10, max value 64 KiB).
2. **Create controller NATS user/creds** with KV write on `mcp-agent-registry` only; deny publish to lookup subjects except audit.
3. **Generate dev signer** (production: KMS stub until JWKS custody lands):

   ```bash
   openssl genpkey -algorithm ED25519 -out registry-signer.pem
   ```

4. **Seed Git repo** layout:

   ```
   agents/
     acme/
       oncall-agent.toml
   ```

   See signed examples under `rsworkspace/crates/trogon-agent-registry/examples/`.

5. **Validate locally** before first merge:

   ```bash
   agctl registry sync --repo . --verify-key /path/to/registry-signer.pem
   ```

6. **Start controller** with repo path, signer key, and NATS creds (see controller README).
7. **Start lookup service** (`trogon-agent-registry`) on read-only creds.
8. **Smoke test** — request/reply on `mcp.registry.agent.lookup` returns the seeded record.

---

## Normal change workflow

1. Author a signed TOML manifest under `agents/{tenant}/{agent}.toml` (schema in `registry.md` § Signed manifest schema).
2. Open PR; CI runs `agctl registry sync --repo $REPO --verify-key $REGISTRY_VERIFY_KEY`.
3. Platform reviewer approves (first registration and `allowed_audiences` expansion need platform sign-off).
4. Merge to `main`; controller pulls within one sync interval (default 30 s).
5. Verify audit event:
   - new agent → `mcp.audit.registry.registered`
   - version bump → `mcp.audit.registry.bumped`
   - deprecation → `mcp.audit.registry.deprecated`
6. Confirm lookup returns new `agent_version` and `@latest` pointer revision increased.

---

## Signer rotation (Ed25519 file / future KMS)

### Overlap rotation (preferred)

1. Generate new key; add its public key to controller trusted set (today: replace PEM path after dual-key window — KMS backend will expose `current` + `previous` like auth-callout).
2. Re-sign all manifests with the new key (`key_id` in `[signature]` must match).
3. Deploy controller with new signer; keep old public key trusted until all manifests re-signed.
4. Remove old key from trust bundle; emit audit note in change ticket.

### Emergency single-step rotation

If old key is compromised: generate new key, re-sign manifests, deploy controller, **revoke** any agents whose manifests could have been forged (see Emergency revocation).

Production KMS rotation is tracked with JWKS custody (`docs/adr/0006-mesh-token-signing-keys.md`); do not use file PEM in production.

---

## Recovering from KV corruption

Symptoms: lookup returns stale/wrong records, `@latest` pointer mismatch, or JSON parse errors in controller logs.

1. **Stop controller** to prevent compounding writes.
2. **Export KV snapshot** (nats CLI / operator tooling) for forensics.
3. **Purge corrupted keys** in bucket `mcp-agent-registry` (versioned keys `{agent_id}/{version}` and `{agent_id}/@latest`).
4. **Verify Git** is authoritative — `git log` on manifest repo, run `agctl registry sync --repo ... --verify-key ...` (dry validation).
5. **Restart controller** — full reconcile from Git repopulates KV.
6. **Warm lookup cache** — restart `trogon-agent-registry` or wait for KV watch propagation (< 5 s target).
7. **Post-incident** — file ticket with `git_commit`, audit stream slice, and corrupted key list.

If Git and KV disagree, **Git wins**. Never hand-edit KV except break-glass revocation.

---

## Emergency revocation

**Goal:** fail-closed for a compromised or abusive `agent_id` within one sync interval (or immediately via break-glass).

### Standard path (Git)

1. Set `lifecycle_state = "revoked"` in the agent manifest (clear `allowed_workloads` if policy requires).
2. Fast-track merge; controller emits `mcp.audit.registry.revoked`.
3. Confirm STS/gateway enforce mode rejects new exchanges for that `agent_id`.

### Break-glass (controller or Git unavailable)

1. Two operators: page `owner_team` + platform on-call.
2. Direct KV put on `{agent_id}/@latest` with `lifecycle_state=revoked` and empty `allowed_workloads`, using break-glass NATS creds (pre-provisioned, rare-use).
3. Publish manual audit record to `mcp.audit.registry.revoked` with `operator=break-glass` and reason.
4. Within 24 h: restore Git manifest to match KV; re-enable controller sync.

---

## Monitoring checklist

| Signal | Action |
|---|---|
| Controller sync errors in logs | Check Git access, signature validity, monotonic version violations |
| No audit events after merge | Controller down or NATS publish ACL misconfigured |
| STS `registry_miss` / lookup NACK spike | KV unreachable or lookup service down — fail-closed in enforce mode |
| `agctl registry sync` fails in CI | Block merge; inspect manifest schema / signature |

---

## Related documents

- `docs/identity/registry.md` — entity schema, lookup API, signed manifest schema
- `rsworkspace/crates/trogon-agent-registry-controller/README.md` — controller env vars
- `docs/adr/0006-mesh-token-signing-keys.md` — signer custody pattern (KMS stub for prod)
