# How to import an OCI-distributed bundle into NATS KV

**Diátaxis:** how-to (goal-oriented, imperative steps).

**Audience:** CI engineers and platform SREs promoting signed MCP policy bundles from an OCI registry into the Trogon mesh NATS KV control plane so gateway replicas hot-swap without registry egress at runtime.

**Related:** [How to write a CEL-only policy bundle](howto-write-bundle.md) · [WASM bundle format reference](wasm-bundle-format.md) · [MCP gateway operator overview](mcp-gateway-operator-overview.md) · [Failure mode matrix](failure-mode-matrix.md) · [ADR 0010](../adr/0010-bundle-format.md) · [ADR 0017](../adr/0017-product-positioning.md) · [ADR 0026](../adr/0026-bundle-hot-swap-rollback.md) · [ADR 0028](../adr/0028-mcp-pack-bundle.md) · [ADR 0030](../adr/0030-gateway-ctl-cli-surface.md)

This guide covers the **CI artifact → KV mirror → gateway hot-swap** path. Author the bundle first in [howto-write-bundle.md](howto-write-bundle.md); return here when CI must publish to GHCR (or equivalent) and a cluster-local bridge must import bytes into JetStream KV.

| Placeholder | Example value |
|---|---|
| Tenant | `acme` |
| Bundle slug (`manifest.toml` `name`) | `acme/github-create-issue-gate` |
| Bundle semver | `1.0.0` |
| OCI registry | `ghcr.io/acme/mcp-policy` |
| Signing NKey public key | `UAB…` (base32) |
| NATS KV blob bucket | `mcp-policy-bundles` **(proposed — [ADR 0010](../adr/0010-bundle-format.md))** |
| Active pointer bucket | `mcp-gateway-config` |
| Active pointer key | `bundle/active` ([ADR 0026](../adr/0026-bundle-hot-swap-rollback.md)) |

Replace placeholders before running commands.

---

## Goal

Publish a signed policy bundle to an OCI registry from CI, mirror it into JetStream KV, and promote the active pointer so every `trogon-mcp-gateway` replica loads the new digest via KV watch and hot-swaps without restart.

When complete, gateway runtime reads bundle bytes from KV (not the registry), verifies the NKey signature, compiles policy, and serves traffic on the new digest while in-flight requests drain on the prior snapshot ([ADR 0026](../adr/0026-bundle-hot-swap-rollback.md)).

---

## When to use this

Use this procedure when:

- CI builds tenant or org policy bundles and stores release artifacts in an OCI registry ([ADR 0010](../adr/0010-bundle-format.md)).
- Production clusters must **not** depend on outbound registry access at runtime ([ADR 0017](../adr/0017-product-positioning.md) — KV-primary, OCI-secondary).
- You promote platform bundles such as `_global/mcp-pack` ([ADR 0028](../adr/0028-mcp-pack-bundle.md)) or tenant overlays after merge review.

**When not to use this:**

- **Authoring** policy from scratch — use [howto-write-bundle.md](howto-write-bundle.md).
- **Air-gapped clusters with no registry ever** — skip OCI entirely; see [Step 6 — Air-gapped direct KV import](#6-air-gapped-direct-kv-import-skip-oci).
- **SPIFFE trust bundle rotation** — that is `mcp-trust-bundles`, not policy bundles; see [howto-integrate-third-party-mcp.md § Step 5](howto-integrate-third-party-mcp.md#step-5--configure-the-trust-bundle-svid-servers-only).

---

## Prerequisites

1. **Trogon mesh co-deployed** — gateway, JetStream, STS, registry ([mcp-gateway-operator-overview.md](mcp-gateway-operator-overview.md), [ADR 0017](../adr/0017-product-positioning.md)).
2. **KV buckets:** `mcp-gateway-config` (pointer, `trusted_signers`); `mcp-policy-bundles` **(proposed — [ADR 0010](../adr/0010-bundle-format.md))**.
3. **Signing NKey** in CI secrets; public half in `trusted_signers` ([ADR 0010](../adr/0010-bundle-format.md)). Not mesh STS keys ([ADR 0006](../adr/0006-mesh-token-signing-keys.md)).
4. **Tooling:** `oras`, `trogon-gateway-ctl` **(proposed — [ADR 0030](../adr/0030-gateway-ctl-cli-surface.md))**, `nats` CLI with write ACL on both buckets.
5. **Bundle tree** built per [howto-write-bundle.md § Step 6](howto-write-bundle.md#step-6--sign-and-publish).

```bash
export NATS_URL=nats://nats.backend.svc:4222
export OCI_REGISTRY=ghcr.io/acme/mcp-policy
export BUNDLE_NAME=acme/github-create-issue-gate
export BUNDLE_VERSION=1.0.0
export NKEY_PUB=UAB…
```

---

## Steps

### 1. Validate the bundle offline before CI publish

Run structural validation and NKey verification before any registry push. This catches manifest errors, missing signatures, and CEL compile failures without touching the fleet.

```bash
trogon-gateway-ctl bundle validate ./my-bundle \
  --trusted-signer "${NKEY_PUB}" \
  --output json
# (proposed — ADR 0030)
```

**Expected result:** exit `0`, JSON envelope `data.result = "VALID"`. On failure: exit `5` with `capabilities.imports omits "host.rate-acquire"` or similar structural reason.

Until `trogon-gateway-ctl` ships, use the manual checklist from [howto-write-bundle.md § Step 4](howto-write-bundle.md#step-4--validate-offline) and verify `signatures/manifest.sig` with your org NKey tool.

---

### 2. Build the OCI artifact and sign the manifest

Package the bundle tree as a single-layer OCI artifact. **Authoritative signing gate is NKey over canonical `manifest.toml` bytes** ([ADR 0010](../adr/0010-bundle-format.md)); cosign supplementary attestation is optional and deferred ([ADR 0028](../adr/0028-mcp-pack-bundle.md)).

```bash
cd my-bundle

# Canonical manifest bytes — do not re-serialize after signing (ADR 0010).
nkey sign --in manifest.toml --out signatures/manifest.sig --seed "${NKEY_SEED}"

tar czf /tmp/${BUNDLE_NAME//\//-}-${BUNDLE_VERSION}.tar.gz \
  manifest.toml policies/ signatures/ README.md
# Phase 3: add components/ schemas/ when present (ADR 0010)
```

Push with ORAS. Media type is **proposed** until registered ([wasm-bundle-format.md §2.3](wasm-bundle-format.md#23-media-types-proposed)):

```bash
OCI_TAG="${OCI_REGISTRY}/${BUNDLE_NAME}:${BUNDLE_VERSION}"

oras push "${OCI_TAG}" \
  "/tmp/${BUNDLE_NAME//\//-}-${BUNDLE_VERSION}.tar.gz:application/vnd.trogon.mcp-policy.bundle.v1+tar+gzip"
# (proposed mediaType — ADR 0010 / wasm-bundle-format §2.3)

MANIFEST_DIGEST=$(oras manifest fetch "${OCI_TAG}" | jq -r .digest)
echo "OCI manifest digest: ${MANIFEST_DIGEST}"
```

**Expected result:** registry returns `sha256:…` manifest digest; local tree contains `signatures/manifest.sig` over `manifest.toml`.

| OCI expectation | Authority | Notes |
|---|---|---|
| Filesystem layer | tarball or multi-layer ([wasm-bundle-format.md §3.6](wasm-bundle-format.md#36-layer-packing-strategies)) | Single tarball is simplest for bridge import |
| Root manifest file | `manifest.toml` ([ADR 0010](../adr/0010-bundle-format.md)) | `bundle.toml` deprecated alias one release cycle |
| NKey signature | `signatures/manifest.sig` | Detached signature over SHA-256 of canonical UTF-8 manifest bytes |
| NKey in OCI annotations | **(proposed)** `dev.trogon.mcp.signer_nkey` | Optional; loader reads `signatures/manifest.sig` from layer |
| Human tag alias | `mcp-policy@<semver>+<sha256-12>` ([ADR 0010](../adr/0010-bundle-format.md)) | Audit pins digest, not tag |

---

### 3. CI gate — verify OCI artifact before promotion

CI must re-pull and verify the artifact it just pushed. Do not trust the registry alone; gateway re-verifies at load ([wasm-bundle-format.md §5.4](wasm-bundle-format.md#54-ci-vs-gateway-verification)).

```bash
oras pull "${OCI_TAG}" -o /tmp/oci-verify/
tar xzf /tmp/oci-verify/*.tar.gz -C /tmp/oci-verify/tree/

trogon-gateway-ctl bundle validate /tmp/oci-verify/tree \
  --trusted-signer "${NKEY_PUB}" \
  --output json
# (proposed — ADR 0030)

nkey verify \
  --in /tmp/oci-verify/tree/manifest.toml \
  --sig /tmp/oci-verify/tree/signatures/manifest.sig \
  --pub "${NKEY_PUB}"
```

**Expected result:** validate prints `VALID`; verify exits `0`. CI fails the pipeline on any non-zero exit — do not run the KV bridge.

**Sketch — CI stages (adapt per org):**

```text
build-bundle → validate → sign → oras push → ci-verify-pull → trigger-kv-bridge (manual approval gate optional)
```

---

### 4. Run the KV-import bridge Job

The bridge copies verified OCI bytes into `mcp-policy-bundles`. Gateways **read KV at runtime**, not the registry ([ADR 0017](../adr/0017-product-positioning.md) — KV-primary gives runtime independence from external registries, uniform watch/reload, and air-gapped operation after one import).

Run inside a Kubernetes Job, CronJob, or CI deploy stage:

```bash
oras pull "${OCI_TAG}" -o /tmp/bridge/
ARTIFACT_TAR=$(ls /tmp/bridge/*.tar.gz | head -1)
DIGEST="${MANIFEST_DIGEST}"

nats kv put mcp-policy-bundles "${DIGEST}" @"${ARTIFACT_TAR}"
# (proposed bucket — ADR 0010)
```

**Expected result:** `nats kv get mcp-policy-bundles "${DIGEST}"` returns tarball bytes matching OCI layer size. Bucket `mcp-policy-bundles` **(proposed)**, key `{digest}`, writers = bridge or break-glass operator, readers = gateway loader only.

**Open question:** ADR 0010 also documents slug keys `{tenant}/{name}` — confirm dual-key layout against your loader version.

**Emergency manual import:** `nats kv put mcp-policy-bundles "${DIGEST}" @/path/to/bundle.tar.gz` after local signature verify. Invalid signature at gateway load emits `bundle_rejected` and retains prior digest ([failure-mode-matrix.md row 4](failure-mode-matrix.md#failure-mode-matrix)).

Deploy the bridge as a Job or CronJob with `oras`, `trogon-gateway-ctl`, and `nats` in the image; pin `MANIFEST_DIGEST` from CI. Promotion stays a separate stage.

---

### 5. Promote the active pointer (triggers hot-swap)

Hot-swap follows **KV watch on the active pointer**, not the blob write alone ([ADR 0026](../adr/0026-bundle-hot-swap-rollback.md)). Separate import from promotion so CI can stage digests before change windows.

```bash
cat > /tmp/active-pointer.json <<EOF
{
  "source": "kv",
  "name": "${BUNDLE_NAME}",
  "version": "${BUNDLE_VERSION}",
  "digest": "${DIGEST}",
  "target_wit": "trogon:mcp-policy@0.1.0",
  "oci_ref": "${OCI_TAG}@${DIGEST}",
  "promoted_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "promoted_by": "ci:bundle-import-pipeline"
}
EOF

nats kv put mcp-gateway-config bundle/active @/tmp/active-pointer.json
```

Org first-party pack: key `policy/org/mcp-pack/active_digest` **(proposed — [ADR 0028](../adr/0028-mcp-pack-bundle.md))**. Soft tenancy: `{tenant}/bundle/active` ([ADR 0026](../adr/0026-bundle-hot-swap-rollback.md)).

**Ordering:** `digest` is content-addressed rollback target; `kv_revision` on the pointer key is the operator promotion order; per-replica `generation` increments after activation **(proposed — ADR 0026)**. Gateway debounces watch events (~250 ms **proposed**), runs verify-compile-pool on a candidate, then atomically activates. Failed verify retains prior bundle ([ADR 0026 §4](../adr/0026-bundle-hot-swap-rollback.md)).

```bash
nats pub mcp.control.bundle.reload '{}'
# (proposed — ADR 0026; pointer JSON remains truth)
```

---

### 6. Air-gapped direct KV import (skip OCI)

When no registry exists or egress is permanently denied:

1. Build and sign on a connected staging host per [howto-write-bundle.md](howto-write-bundle.md).
2. `trogon-gateway-ctl bundle validate ./my-bundle --trusted-signer "${NKEY_PUB}"` **(proposed — ADR 0030)**.
3. Transfer tarball via sneakernet to the cluster.
4. `nats kv put mcp-policy-bundles "${DIGEST}" @bundle.tar.gz`
5. Promote pointer (Step 5) — set `"source": "kv"` and omit `oci_ref`.

Never skip signature verification because OCI was bypassed. Unsigned bundles MUST NOT activate in production ([ADR 0010](../adr/0010-bundle-format.md), [failure-mode-matrix.md invariant 8](failure-mode-matrix.md#invariants-all-modes)).

---

## Verify

Run this checklist after promotion:

```bash
# Pointer visible
nats kv get mcp-gateway-config bundle/active | jq .

# Blob present
nats kv get mcp-policy-bundles "${DIGEST}" --raw | wc -c

# Gateway readiness (proposed fields — ADR 0026)
curl -s "http://gateway:8080/readyz" | jq '{bundle: .bundle.sha256, generation: .bundle.generation}'

# Load outcome metric (proposed)
# mcp_bundle_load_total{outcome="success"} increment after promotion

# Audit lifecycle event
nats sub "mcp.audit.>" --count 5 | jq 'select(.event_kind=="bundle_loaded")'
# (proposed audit shape — ADR 0010)
```

| Check | Pass criterion |
|---|---|
| Pointer `digest` | Matches `${DIGEST}` from CI |
| Gateway `/readyz` | `bundle.sha256` equals `${DIGEST}`; `ready: true` |
| Gated smoke call | Allow/deny matches dry-run from [howto-write-bundle.md § Step 7](howto-write-bundle.md#step-7--hot-swap) |
| Audit | `bundle_loaded` with `policy_bundle_digest`, `previous_digest`, `kv_revision` **(proposed)** |
| No `bundle_rejected` | Absent in audit stream after promotion |

---

## Rollback / undo

Revert by repointing the active pointer to the last known-good digest. Hot-swap applies rollback the same way as promotion ([ADR 0026 §5](../adr/0026-bundle-hot-swap-rollback.md)).

```bash
# Inspect history before revert
nats kv history mcp-gateway-config bundle/active

# Restore prior revision JSON (copy from history or backup)
nats kv put mcp-gateway-config bundle/active @/tmp/previous-active-pointer.json

# Or use CLI helper when shipped
trogon-gateway-ctl bundle rollback --to-digest sha256:PREVIOUS…
# (proposed — ADR 0030)
```

If promotion broke policy behavior, see [failure-mode-matrix.md row 3](failure-mode-matrix.md#failure-mode-matrix) (WASM panic) and row 4 (signature invalid at load). Failed load **automatically** retains the prior digest — no auto-rollback on `/readyz` flapping ([ADR 0026](../adr/0026-bundle-hot-swap-rollback.md)).

To remove a bad blob from KV without activating it: delete only the `mcp-policy-bundles` key if the pointer was never promoted. Never mutate tarball bytes in place; publish a fixed semver and new digest forward ([ADR 0010](../adr/0010-bundle-format.md)).

---

## Troubleshooting

| Symptom | Root cause | Fix |
|---|---|---|
| **`bundle_rejected` / `policy_bundle_signature_invalid` in audit** | Wrong NKey, tampered manifest, or signer not in `trusted_signers` | Confirm `NKEY_PUB` in KV trust list; re-sign manifest; re-import blob; see [failure-mode-matrix.md row 4](failure-mode-matrix.md#failure-mode-matrix) |
| **Pointer updated but gateway stays on old digest** | Blob missing at `${DIGEST}` key, or watch debounce / loader still compiling | `nats kv get mcp-policy-bundles "${DIGEST}"`; wait for compile; publish `mcp.control.bundle.reload` **(proposed)** |
| **CI verify passes, gateway rejects at load** | Manifest canonical bytes differ between CI re-tar and signed tree (whitespace drift) | Sign after final tarball layout; use `trogon-gateway-ctl bundle validate` on extracted tree **(proposed — ADR 0030)** |
| **`-32108 no_policy` after promotion** | Pointer deleted or digest typo; day-zero posture | Restore pointer; bootstrap org pack `_global/mcp-pack` ([ADR 0028](../adr/0028-mcp-pack-bundle.md)); see [failure-mode-matrix.md row 5](failure-mode-matrix.md#failure-mode-matrix) |
| **Replicas show different digests briefly** | Expected split-brain window during KV propagation ([ADR 0026](../adr/0026-bundle-hot-swap-rollback.md)) | Compare audit `policy_bundle_digest`; wait for `kv_revision` convergence; avoid dual promotion paths |

---

## Related

- [howto-write-bundle.md](howto-write-bundle.md) — author and sign before OCI publish
- [wasm-bundle-format.md](wasm-bundle-format.md) — OCI layout, KV import, media types
- [ADR 0010](../adr/0010-bundle-format.md) · [ADR 0017](../adr/0017-product-positioning.md) · [ADR 0026](../adr/0026-bundle-hot-swap-rollback.md) · [ADR 0028](../adr/0028-mcp-pack-bundle.md) · [ADR 0030](../adr/0030-gateway-ctl-cli-surface.md)
- [failure-mode-matrix.md](failure-mode-matrix.md) — signature invalid, missing bundle, rollback
- [mcp-gateway-operator-overview.md](mcp-gateway-operator-overview.md) · [hierarchical-policy-merge.md](hierarchical-policy-merge.md) · [bootstrap-day-zero.md](bootstrap-day-zero.md)

---

*Document type: Diátaxis **how-to**. For bundle authoring, read [howto-write-bundle.md](howto-write-bundle.md). For format and signing theory, read [ADR 0010](../adr/0010-bundle-format.md) and [wasm-bundle-format.md](wasm-bundle-format.md).*
