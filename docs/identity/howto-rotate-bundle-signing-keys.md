# How to rotate NKey signing keys for policy bundles

**Diátaxis:** how-to (goal-oriented, imperative steps).

**Audience:** platform operators rotating the NATS NKey that signs Trogon MCP gateway policy bundles. Not mesh STS signing keys, NATS account Operator keys, or SPIFFE trust-bundle PEM.

**Related:** [ADR 0010 — Bundle format](../adr/0010-bundle-format.md) · [ADR 0026 — Hot-swap and rollback](../adr/0026-bundle-hot-swap-rollback.md) · [WASM bundle format](wasm-bundle-format.md) · [How to write a bundle](howto-write-bundle.md) · [Failure mode matrix](failure-mode-matrix.md) · [MCP gateway operator overview](mcp-gateway-operator-overview.md) · [MCP gateway plan Block F](../../MCP_GATEWAY_PLAN.md)

| Placeholder | Example value |
|---|---|
| Tenant | `acme` |
| Bundle slug | `acme/github-create-issue-gate` |
| Old signing NKey public (`kid`) | `UAB…` (base32) |
| New signing NKey public (`kid`) | `UAC…` (base32) |
| Overlap window | `14d` (org policy — not pinned in ADR) |
| NATS KV config bucket | `mcp-gateway-config` |
| Trust allowlist key | `trusted_signers` **(proposed)** |

Replace placeholders before running commands.

---

## Goal

Rotate the NKey that signs policy bundles so gateways trust the new public key, in-flight traffic keeps the prior bundle revision, and you remove the retired key only after a documented overlap window — without stale signer material left in the allowlist.

---

## When to use this

- Scheduled rotation (quarterly, compliance cadence).
- Suspected or confirmed compromise of the bundle signing seed.
- Maintainer with seed custody departs; custody moves to a new team or KMS-backed pipeline.
- Promotion to production of a new org-wide publisher NKey (distinct from mesh STS keys per [ADR 0006](../adr/0006-mesh-token-signing-keys.md)).

**When not to use this:**

| Need | Use instead |
|---|---|
| Mesh JWT / STS signing key rotation | [ADR 0006](../adr/0006-mesh-token-signing-keys.md), [bootstrap-day-zero.md §7](bootstrap-day-zero.md#7-migration--returning-a-production-tenant-to-day-zero-without-availability-gap), [sts-exchange.md](sts-exchange.md) |
| SPIFFE trust bundle (SVID verification) rotation | `mcp-trust-bundles/{trust_domain}` — [howto-integrate-third-party-mcp.md §5](howto-integrate-third-party-mcp.md#step-5--configure-the-trust-bundle-svid-servers-only) |
| NATS account / User JWT / auth-callout issuer rotation | [nats-callout-plugin.md](nats-callout-plugin.md), auth-callout operator docs |
| Authoring or first publish of a bundle | [howto-write-bundle.md](howto-write-bundle.md) |

**Bucket clarification:** Bundle signer allowlists live in **`mcp-gateway-config/trusted_signers`** ([ADR 0010](../adr/0010-bundle-format.md), [ADR 0026](../adr/0026-bundle-hot-swap-rollback.md)). **`mcp-trust-bundles`** holds SPIFFE anchor PEM for STS only ([reference-subject-grammar.md §7.1](reference-subject-grammar.md)) — do not store bundle NKey public halves there.

---

## Prerequisites

1. **Co-deployed Trogon mesh** with `trogon-mcp-gateway` on queue group `mcp-gateway` ([mcp-gateway-operator-overview.md](mcp-gateway-operator-overview.md)).
2. **JetStream KV** bucket `mcp-gateway-config` exists; you have put permission on `trusted_signers` and `bundle/active` (or tenant-prefixed equivalents per [ADR 0001](../adr/0001-tenancy-model.md)).
3. **Active bundle pointer** — record current digest before change:

   ```bash
   nats kv get mcp-gateway-config bundle/active
   nats kv history mcp-gateway-config bundle/active
   ```

4. **`nk` on PATH** — NATS NKeys CLI for generate/sign/verify (commands below mirror [howto-write-bundle.md §6.1](howto-write-bundle.md#61-compute-manifest-digest-and-sign); exact `nk` flags are **(proposed)** until Block G ships `agctl mcp bundle sign`).
5. **Offline bundle tree** for your tenant slug (from [howto-write-bundle.md](howto-write-bundle.md)) or CI artifact tarball.
6. **Change ticket** fields: `old_kid`, `new_kid`, overlap end date, rollback digest, operator identity.

---

## Steps

### 1. Record why you are rotating and freeze promotions

1. Open the change ticket with reason: `compromise` | `scheduled` | `custody_transfer`.
2. Pause unrelated `bundle/active` promotions until rotation completes.
3. Export current allowlist and pointer:

   ```bash
   nats kv get mcp-gateway-config trusted_signers | tee /tmp/trusted-signers-before.json
   nats kv get mcp-gateway-config bundle/active | tee /tmp/bundle-active-before.json
   ```

   **Expected:** JSON with at least one base32 NKey public; active pointer includes `digest` and `version`.

### 2. Generate the new NKey pair and record metadata

1. Generate a dedicated publisher key (do not reuse mesh STS or auth-callout material — [ADR 0010 § Signing algorithm](../adr/0010-bundle-format.md#signing-algorithm-reference)):

   ```bash
   nk -gen user -pubout > /tmp/bundle-signer-rotation.txt
   export NKEY_SEED_NEW="$(grep '# SU' /tmp/bundle-signer-rotation.txt | tr -d '# ')"
   export NKEY_PUB_NEW="$(grep '# U' /tmp/bundle-signer-rotation.txt | tr -d '# ')"
   ```

2. Store the seed in org secrets (Vault/KMS envelope); never commit. Record `kid`, `created_at`, custodian, and `replaces` (`${NKEY_PUB_OLD}`) in the ticket.

   **Expected:** `${NKEY_PUB_NEW}` is base32 with `U` prefix; seed line starts with `SU`.

### 3. Open the dual-trust window (old + new keys trusted)

Gateways verify manifest signatures against the **allowlist** in KV, not only the `signing.nkey_pub` embedded in one bundle ([ADR 0010](../adr/0010-bundle-format.md#signing-algorithm-reference)).

1. Build overlapping allowlist JSON **(proposed schema)**:

   ```bash
   cat > /tmp/trusted-signers-overlap.json <<EOF
   {
     "keys": [
       { "nkey_pub": "${NKEY_PUB_OLD}", "status": "retiring", "not_after": "$(date -u -v+14d +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u -d '+14 days' +%Y-%m-%dT%H:%M:%SZ)" },
       { "nkey_pub": "${NKEY_PUB_NEW}", "status": "active", "not_before": "$(date -u +%Y-%m-%dT%H:%M:%SZ)" }
     ],
     "rotation_ticket": "CHG-XXXXX"
   }
   EOF
   ```

   Set `NKEY_PUB_OLD` from the current `trusted_signers` document or from the active bundle's `manifest.toml` `[signing] nkey_pub`.

2. Publish overlap allowlist:

   ```bash
   nats kv put mcp-gateway-config trusted_signers @/tmp/trusted-signers-overlap.json
   ```

3. Belt-and-braces reload **(proposed — ADR 0026)**:

   ```bash
   nats pub mcp.control.bundle.reload '{}'
   ```

   **Expected:** Gateways retain the current active digest; watch handlers refresh verify cache. No `bundle_rejected` storm on the audit subject **(proposed)** `mcp.control.bundle.load_failed` ([failure-mode-matrix.md](failure-mode-matrix.md) row 4).

| Overlap policy | Guidance |
|---|---|
| Minimum duration | Cover longest bundle promotion rollback drill + drain window ([ADR 0026 §3](../adr/0026-bundle-hot-swap-rollback.md#3-in-flight-request-fencing-pinned-decision)) |
| Both keys in `trusted_signers` | Required while any **active or draining** bundle was signed with the old key |
| Dual signature on one `manifest.toml` | **Not required** — one `signatures/manifest.sig` per ADR 0010; overlap is allowlist + optional re-sign |

### 4. Sign and publish a bundle with the new key

1. Update `manifest.toml` `[signing] nkey_pub` to `${NKEY_PUB_NEW}` (policy content unchanged unless you are also revving rules).
2. Sign canonical manifest bytes ([ADR 0010](../adr/0010-bundle-format.md#signing-algorithm-reference)):

   ```bash
   cd my-bundle
   mkdir -p signatures
   nk -sign manifest.toml -out signatures/manifest.sig -seedfile <(echo -n "${NKEY_SEED_NEW}")  # (proposed -- ADR 0010)
   nk -verify manifest.toml -sig signatures/manifest.sig -pubkey "${NKEY_PUB_NEW}"
   ```

   **Proposed:** `agctl mcp bundle sign --path ./my-bundle --seed-env NKEY_SEED_NEW` (Block G).

3. Publish artifact (OCI default, KV mirror fallback) per [howto-write-bundle.md §6.2–6.3](howto-write-bundle.md#62-publish-to-oci-registry-production-default):

   ```bash
   # OCI (production default per ADR 0010)
   oras push "ghcr.io/acme/mcp-policy/${BUNDLE_SLUG}:${BUNDLE_VERSION}" \
     "/tmp/bundle.tar.gz:application/vnd.trogon.mcp-policy.bundle.v1+tar+gzip"
   DIGEST=$(oras manifest fetch "ghcr.io/acme/mcp-policy/${BUNDLE_SLUG}:${BUNDLE_VERSION}" | jq -r .digest)

   # KV blob mirror (proposed bucket -- ADR 0010)
   nats kv put mcp-policy-bundles "${DIGEST}" @/tmp/bundle.tar.gz
   ```

   **Expected:** Local `nk -verify` succeeds; CI `agctl mcp bundle validate` **(proposed)** reports `VALID`. Existing active digest signed with `${NKEY_PUB_OLD}` keeps loading until you flip the pointer (Step 5).

### 5. Hot-swap the active pointer and observe load metrics

1. Write new active pointer (digest pin is authoritative — [ADR 0026 §2](../adr/0026-bundle-hot-swap-rollback.md#2-generation-numbering-and-ordering)):

   ```bash
   cat > /tmp/bundle-active-new.json <<EOF
   {
     "source": "oci",
     "digest": "${DIGEST}",
     "name": "${BUNDLE_SLUG}",
     "version": "${BUNDLE_VERSION}",
     "target_wit": "trogon:mcp-policy@0.1.0",
     "promoted_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
     "promoted_by": "human:operator@acme.com",
     "signer_nkey": "${NKEY_PUB_NEW}"
   }
   EOF
   nats kv put mcp-gateway-config bundle/active @/tmp/bundle-active-new.json
   ```

2. Confirm activation per replica:

   ```bash
   agctl mcp bundle status --output json  # (proposed -- ADR 0026)
   curl -s localhost:8080/readyz | jq '.bundle.sha256'    # (proposed)
   ```

3. Watch load outcomes ([ADR 0026 §7](../adr/0026-bundle-hot-swap-rollback.md#7-telemetry-and-audit-on-swap)):

   | Signal | Success | Failure |
   |---|---|---|
   | `mcp_bundle_load_total{outcome="success"}` **(proposed)** | Increments after pointer write | — |
   | `mcp_bundle_load_total{outcome="rejected",reason="signature_invalid"}` **(proposed)** | — | Investigate allowlist / sig |
   | Audit `bundle_loaded` **(proposed)** | `signer_nkey` = `${NKEY_PUB_NEW}` | `bundle_rejected` |
   | `/readyz` | `ready: true`, digest matches `${DIGEST}` | `503` if no prior bundle |

4. Smoke gated traffic ([howto-write-bundle.md §7.2](howto-write-bundle.md#72-live-smoke-test)) and confirm `policy_bundle_digest` in audit matches `${DIGEST}` **(proposed field — [wasm-bundle-format.md §4.2](wasm-bundle-format.md#42-audit-envelope-fields-proposed))**.

   **Expected:** New requests evaluate under the new digest; in-flight work started before the flip completes on the prior snapshot ([ADR 0026 §3](../adr/0026-bundle-hot-swap-rollback.md#3-in-flight-request-fencing-pinned-decision)).

### 6. Decommission the old key after the overlap window

Only after **all** are true:

- Overlap end time from the ticket has passed.
- No gateway reports a draining snapshot signed with `${NKEY_PUB_OLD}` **(proposed gauge `mcp_bundle_draining_snapshots`)**.
- Rollback drill to the pre-rotation digest is no longer required (or you accept that rollback to an old-key digest will fail verification once the old key is removed).

1. Shrink allowlist to the new key only:

   ```bash
   cat > /tmp/trusted-signers-final.json <<EOF
   {
     "keys": [
       { "nkey_pub": "${NKEY_PUB_NEW}", "status": "active" }
     ]
   }
   EOF
   nats kv put mcp-gateway-config trusted_signers @/tmp/trusted-signers-final.json
   ```

2. Destroy `${NKEY_SEED_OLD}` in secrets manager; archive ticket with KV revision ids.

   **Expected:** New promotions signed with the old seed fail verify at CI/gateway; existing active digest signed only with the new key continues to load.

### 7. Understand early removal of the old public key

If you remove `${NKEY_PUB_OLD}` from `trusted_signers` while a bundle signed with that key is still **active or draining**:

| Situation | Effect |
|---|---|
| In-flight requests pinned to old snapshot | **Complete normally** — snapshot already verified at activation ([ADR 0026 §3](../adr/0026-bundle-hot-swap-rollback.md#3-in-flight-request-fencing-pinned-decision)) |
| New requests | Served by current active digest (new signer) |
| Operator rolls back pointer to old digest | **Load rejected** — `policy_bundle_signature_invalid` / `bundle_rejected` ([failure-mode-matrix.md](failure-mode-matrix.md) row 4); prior good digest if same signer, else `-32108` `no_policy` **(proposed)** |
| Hot-swap re-promotion of old signed artifact | Fails closed; gateway retains last good bundle |

This is why overlap tracks **allowlist**, not merely publishing a new bundle.

---

## Verify

Run after Step 5 and again after Step 6.

- [ ] `nats kv get mcp-gateway-config bundle/active` — `digest` and `signer_nkey` (if present) match promotion record.
- [ ] `nats kv get mcp-gateway-config trusted_signers` — contains only `${NKEY_PUB_NEW}` after decommission.
- [ ] `mcp_bundle_load_total{outcome="success"}` **(proposed)** increased; no sustained `rejected` with `signature_invalid`.
- [ ] Audit tail shows `bundle_loaded` with `signer_nkey=${NKEY_PUB_NEW}` **(proposed)**.
- [ ] Gated smoke `tools/call` returns allow/deny per policy, not `-32108` `no_policy` **(proposed)**.
- [ ] `nk -verify` on published tarball manifest passes with `${NKEY_PUB_NEW}` only after Step 6.

---

## Rollback / undo

**Bad bundle after promotion** — repoint `bundle/active` to `/tmp/bundle-active-before.json` ([howto-write-bundle.md §Rollback](howto-write-bundle.md#rollback)). Hot-swap applies the same way ([ADR 0026 §5](../adr/0026-bundle-hot-swap-rollback.md#5-rollback-api-and-operator-signals)).

**Rotation gone wrong mid-window** — restore overlap allowlist:

```bash
nats kv put mcp-gateway-config trusted_signers @/tmp/trusted-signers-before.json
nats kv put mcp-gateway-config bundle/active @/tmp/bundle-active-before.json
nats pub mcp.control.bundle.reload '{}'   # (proposed -- ADR 0026)
```

Confirm `mcp_bundle_load_total{outcome="rollback"}` **(proposed)** or `bundle_loaded` with `restored_digest` in audit.

**Compromise of new key during rotation** — treat as incident: follow [failure-mode-matrix.md](failure-mode-matrix.md) row 4, freeze promotions, generate a third key, and restart from Step 2 with a new overlap window. Do not re-enable `${NKEY_PUB_OLD}` if the old seed was also compromised.

Full fail-closed semantics: [failure-mode-matrix.md](failure-mode-matrix.md) rows 4–6; invariant 8 (signed policy only).

---

## Troubleshooting

| Symptom | Root cause | Fix |
|---|---|---|
| `bundle_rejected` / `signature_invalid` after promotion | New sig not in `trusted_signers`, or manifest `nkey_pub` mismatch | Restore overlap JSON (Step 3); align `[signing] nkey_pub` with signing seed |
| `/readyz` 503, traffic `-32108` | First load with new key failed; no prior bundle | Fix verify pipeline; restore last-good pointer from KV history |
| Metric `rejected` with `cel_compile` **(proposed)** | Unrelated policy error masked as rotation failure | Fix CEL; rollback pointer ([failure-mode-matrix.md](failure-mode-matrix.md) row 6) |
| Rollback to pre-rotation digest fails after Step 6 | Old pubkey removed from allowlist | Temporarily re-add `${NKEY_PUB_OLD}` to `trusted_signers`, rollback, then re-run decommission |
| Fleet split on digest | KV propagation lag ([ADR 0026](../adr/0026-bundle-hot-swap-rollback.md#2-generation-numbering-and-ordering)) | Compare `nats kv history`; wait for revision convergence; audit by `policy_bundle_digest` |

---

## Related

- [ADR 0010](../adr/0010-bundle-format.md) · [ADR 0026](../adr/0026-bundle-hot-swap-rollback.md) · [ADR 0006](../adr/0006-mesh-token-signing-keys.md) (mesh keys only)
- [wasm-bundle-format.md](wasm-bundle-format.md) · [howto-write-bundle.md](howto-write-bundle.md) · [failure-mode-matrix.md](failure-mode-matrix.md)
- [hierarchical-policy-merge.md §6](hierarchical-policy-merge.md#6-storage) · [bootstrap-day-zero.md §7](bootstrap-day-zero.md#7-migration--returning-a-production-tenant-to-day-zero-without-availability-gap)
- [MCP_GATEWAY_PLAN.md Block F](../../MCP_GATEWAY_PLAN.md)

---

**Open questions:** normative `trusted_signers` JSON schema; CI gate for `signing.nkey_pub` ∈ allowlist (Block G); default overlap wall-clock (ADR 0010 requires overlap but does not pin duration).

---

*Document type: Diátaxis **how-to**. Authoring: [howto-write-bundle.md](howto-write-bundle.md). SPIFFE trust: [howto-integrate-third-party-mcp.md §5](howto-integrate-third-party-mcp.md#step-5--configure-the-trust-bundle-svid-servers-only).*
