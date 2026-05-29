# Kubernetes controller — MCP gateway control plane

**Status:** Block G paper artifact (2026-05-28). Design only — no controller binary ships from this document.

**Document type:** Diátaxis **reference** (CRD schemas, KV mappings, RBAC, metrics) with **explanation** prose where architectural trade-offs matter.

**Related:** [Integration touch-points](integration-touchpoints.md) · [Registry operations](registry-operations.md) · [How to integrate a third-party MCP server](howto-integrate-third-party-mcp.md) · [MCP gateway plan Block G](../../MCP_GATEWAY_PLAN.md#block-g--operational-tooling) · [Bootstrap / day-zero](bootstrap-day-zero.md) · [MCP policy WIT sketch](mcp-policy-wit-sketch.md)

Unless noted **proposed**, NATS bucket names, subject prefixes, and KV key shapes match identifiers verified in the repo (`trogon-sts`, `trogon-mcp-gateway`, `trogon-agent-registry-controller`, `agctl`).

---

## 1. Goal

Production operators need a **Kubernetes-native** way to declare MCP server registration, policy bundles, and SPIFFE trust bundles without shell scripts, ad-hoc `nats kv put`, or break-glass KV writes. Block G in [MCP_GATEWAY_PLAN.md](../../MCP_GATEWAY_PLAN.md) lists an optional K8s controller as v2 operational tooling; this document pins the design before any Rust controller crate lands.

### 1.1 What the controller does

The controller is a **reconciliation loop** that projects three Custom Resource Definitions (CRDs) into the same NATS JetStream KV state that the MCP gateway, STS, and registry services already consume:

| CRD kind | Primary NATS destination | Consumers |
|---|---|---|
| `MCPServer` | KV `mcp-gateway-config` key `backend/{server_id}` | `trogon-mcp-gateway` (routing, enablement) |
| `MCPPolicyBundle` | KV `mcp-gateway-config` keys `bundle/{name}/rev/{n}`, active pointer | `trogon-mcp-gateway` (CEL / bundle loader, Phase 2+) |
| `MCPTrustBundle` | KV `mcp-trust-bundles` key `{trust_domain}` | `trogon-sts` (SVID verification) |

The controller may also emit **control-plane audit** on existing subjects (for example `mcp.audit.registry.*` patterns) and **proposed** controller-specific subjects under `mcp.control.k8s.>` when operators need SIEM correlation beyond Kubernetes Events.

### 1.2 What the controller is not

| Non-goal | Rationale |
|---|---|
| Replacement for `agctl` | `agctl` remains the operator CLI for validation, traffic inspection, and Git registry sync. The controller **is** an `agctl` consumer at the NATS boundary: it performs the same KV writes an operator would run after `agctl` validation, but driven by CRD spec changes instead of imperative commands. |
| Replacement for `trogon-agent-registry-controller` | Agent manifests (`mcp-agent-registry`) stay Git-authoritative per [registry-operations.md](registry-operations.md). `MCPServer` registers **backend routing metadata**, not agent identity records. |
| Data-plane proxy | The controller never subscribes to `mcp.gateway.request.>` or forwards JSON-RPC. |
| SpiceDB writer | Tuple seeding remains a separate operator step (or future GitOps job). The controller publishes policy **bundle pointers** only. |

### 1.3 Architectural placement

```text
                    ┌─────────────────────────────────────┐
                    │  Kubernetes API (etcd)              │
                    │  MCPServer / MCPPolicyBundle /      │
                    │  MCPTrustBundle                     │
                    └──────────────┬──────────────────────┘
                                   │ watch + reconcile
                    ┌──────────────▼──────────────────────┐
                    │  trogon-mcp-gateway-controller      │
                    │  (proposed crate name)              │
                    │  - validate CRD spec              │
                    │  - map → KV records               │
                    │  - set status conditions          │
                    │  - periodic drift detection       │
                    └──────────────┬──────────────────────┘
                                   │ NATS JetStream KV PUT
          ┌────────────────────────┼────────────────────────┐
          ▼                        ▼                        ▼
   mcp-gateway-config      mcp-trust-bundles         (optional)
   backend/*               {trust_domain}            mcp.control.*
   bundle/*                                          discovery hint
          │                        │
          ▼                        ▼
   trogon-mcp-gateway         trogon-sts
   (bundle watch)             (trust watch)
```

### 1.4 Relationship to `agctl`

Today, operators integrate third-party MCP servers by imperative KV writes documented in [howto-integrate-third-party-mcp.md](howto-integrate-third-party-mcp.md). The `agctl mcp` subcommands (`servers list`, `policies list`, …) are scaffolded in `rsworkspace/crates/agctl` but return `not yet implemented` — they are the **read path** the controller should eventually satisfy.

| Operation | Today (imperative) | With controller (declarative) |
|---|---|---|
| Register backend | `nats kv put mcp-gateway-config backend/{id} …` | `kubectl apply -f mcp-server.yaml` |
| Publish trust bundle | `trogon-sts-publish-trust-bundle` or `nats kv put mcp-trust-bundles …` | `kubectl apply -f mcp-trust-bundle.yaml` |
| Activate policy bundle | `nats kv put mcp-gateway-config bundle/active …` | `MCPPolicyBundle` with `spec.active: true` |
| Pre-flight validation | `agctl registry sync` (agents only today) | **proposed:** `agctl mcp validate -f …` calling same validators as controller |

The controller **must not** invent parallel KV layouts. Every projected record must match shapes in [integration-touchpoints.md § KV buckets](integration-touchpoints.md#kv-buckets-gateway-adjacent) and [bootstrap-day-zero.md § 1.1](bootstrap-day-zero.md#11-empty-or-missing-control-plane-artifacts).

---

## 2. Custom Resource Definitions

**API group (proposed):** `gateway.trogon.ai/v1alpha1`

**Scope:** Namespaced. One namespace per tenant is the recommended binding (see §6).

**Common status shape:** All three kinds expose a `status` subresource with:

| Field | Type | Meaning |
|---|---|---|
| `observedGeneration` | `int64` | Last reconciled `metadata.generation` |
| `lastSyncedAt` | `metav1.Time` | UTC timestamp of last successful KV write |
| `conditions[]` | `[]Condition` | Kubernetes-standard conditions; `Ready` is primary |
| `kvRevision` | `int64` | **proposed** JetStream KV revision written (for drift checks) |
| `natsKey` | `string` | Canonical KV key written (for support tickets) |

**Condition types (proposed minimum):**

| Type | True means | False reason examples |
|---|---|---|
| `Ready` | Spec validated and KV reflects desired state | `ValidationFailed`, `NatsUnreachable`, `ReferencedResourceMissing` |
| `Synced` | Last sync completed without error | `SyncError`, `KvWriteRejected` |
| `ReferencesResolved` | All object references exist | `PolicyNotFound`, `TrustBundleNotFound`, `SecretNotFound` |

### 2.1 `MCPServer`

Registers a backend MCP server for gateway routing. References a policy bundle and trust bundle by name (same namespace).

```yaml
apiVersion: gateway.trogon.ai/v1alpha1
kind: MCPServer
metadata:
  name: github
  namespace: tenant-acme
  labels:
    gateway.trogon.ai/tenant: acme
spec:
  # Immutable routing slug — matches mcp-nats subject segment (no dots).
  serverId: github

  # Soft tenancy label when NATS account is not the tenant boundary.
  tenant: acme

  # Backend transport — drives operator runbooks, not wire protocol.
  transport: stdio-sidecar   # stdio-sidecar | nats-native | http-bridge

  # Lifecycle gate — gateway must not forward production traffic until active.
  enabled: true
  lifecycleState: active     # provisioning | active | deprecated | revoked

  # SPIFFE trust domain for bridge SVID attestation (optional for stdio-only).
  trustDomain: acme.local

  # Owner team for escalation (copied into KV JSON for runbooks).
  ownerTeam: platform-sre

  # References — same namespace; deletion blocked while referrers exist (§4).
  policyRef:
    name: acme-github-smoke
  trustBundleRef:
    name: acme-local-trust

status:
  observedGeneration: 3
  lastSyncedAt: "2026-05-28T14:22:01Z"
  natsKey: backend/github
  kvRevision: 42
  conditions:
    - type: Ready
      status: "True"
      reason: Synced
      message: "KV mcp-gateway-config/backend/github matches spec"
      lastTransitionTime: "2026-05-28T14:22:01Z"
    - type: ReferencesResolved
      status: "True"
      reason: AllReferencesFound
      lastTransitionTime: "2026-05-28T14:21:58Z"
    - type: Synced
      status: "True"
      reason: KvWriteSucceeded
      lastTransitionTime: "2026-05-28T14:22:01Z"
```

**Projected KV value** (`mcp-gateway-config` / `backend/{serverId}`): JSON with `server_id`, `tenant`, `enabled`, `transport`, `trust_domain`, `owner_team`, `lifecycle_state`, `policy_bundle`, `trust_bundle`, plus **proposed** `source`, `source_uid`, `source_generation` for drift audit. Field names match [howto-integrate-third-party-mcp.md § Step 3.1](howto-integrate-third-party-mcp.md#31-register-the-backend-target-gateway-routing); gateways ignore unknown keys.

**Validation rules (admission + reconcile):**

- `serverId` matches `^[a-z][a-z0-9-]{0,62}$` — no dots (dots belong in method suffixes).
- `serverId` immutable after creation.
- `lifecycleState: revoked` forces `enabled: false`.
- `policyRef` and `trustBundleRef` must resolve to existing CRs in the same namespace.
- When `trustDomain` is set, referenced `MCPTrustBundle` must cover that domain (or `spec.trustDomain` must match).

### 2.2 `MCPPolicyBundle`

Declares a policy bundle artifact (OCI reference or inline ConfigMap) and metadata required for gateway loading.

```yaml
apiVersion: gateway.trogon.ai/v1alpha1
kind: MCPPolicyBundle
metadata:
  name: acme-github-smoke
  namespace: tenant-acme
spec:
  tenant: acme

  # Monotonic revision — must increase on spec changes that alter policy content.
  revision: 2

  # WIT package this bundle targets (Phase 3 gate).
  targetWit: trogon:mcp-policy@0.1.0

  # Minimum gateway semver that may activate this bundle.
  minGatewayVersion: "0.4.0"

  # Artifact source — exactly one of inline or oci must be set.
  inline:
    configMapRef:
      name: acme-github-smoke-bundle
      key: bundle.yaml

  # OCI artifact (production path — Phase 3 signed bundles).
  # oci:
  #   image: ghcr.io/acme/mcp-bundles/acme-github-smoke:2
  #   digest: sha256:abc123...

  # Signature verification (Phase 3 — required in production).
  signature:
    keyId: acme-release-nkey-2026-q2
    # Inline signature over canonical manifest JSON (proposed field).
    value: ""

  # When true, controller updates bundle/active pointer for this tenant.
  active: true

  # Default decision when no rule matches (Tier-1 YAML field).
  defaultDecision: deny

status:
  observedGeneration: 1
  lastSyncedAt: "2026-05-28T14:20:00Z"
  natsKey: bundle/acme-github-smoke/rev/2
  kvRevision: 17
  activePointerKey: bundle/active
  conditions:
    - type: Ready
      status: "True"
      reason: Synced
      message: "Revision 2 published; active pointer updated"
      lastTransitionTime: "2026-05-28T14:20:00Z"
    - type: Synced
      status: "True"
      reason: KvWriteSucceeded
      lastTransitionTime: "2026-05-28T14:20:00Z"
```

**Projected KV keys:**

| Key | Content |
|---|---|
| `bundle/acme-github-smoke/rev/2` | Tier-1 YAML body (from ConfigMap or OCI pull) |
| `bundle/active` | Manifest pointer JSON (when `spec.active: true`) |

Active pointer JSON matches [howto-integrate-third-party-mcp.md § 4.3](howto-integrate-third-party-mcp.md#43-publish-to-kv) (`bundle_name`, `revision`, `tenant`, `artifact_sha256`, `signed_at`) plus **proposed** `source` / `source_uid`.

**Validation rules:**

- `revision` must be strictly greater than any previously observed revision for this CR name (stored in status **proposed** field `lastPublishedRevision`).
- `targetWit` must parse as `package@semver` per [mcp-policy-wit-sketch.md](mcp-policy-wit-sketch.md).
- `minGatewayVersion` semver compared against controller-configured gateway version **(proposed** env `MCP_GATEWAY_CONTROLLER_MIN_GATEWAY_VERSION` or live health probe).
- When `oci` is set, controller pulls with **imagePullSecrets** from SA namespace; digest pin required in production.
- Only one `MCPPolicyBundle` per namespace may have `active: true` at a time; controller rejects or demotes others **(proposed** conflict policy: newest `metadata.creationTimestamp` wins, others get `Ready=False` reason `Superseded`).

### 2.3 `MCPTrustBundle`

Declares a SPIFFE trust domain and anchor PEM material for STS.

```yaml
apiVersion: gateway.trogon.ai/v1alpha1
kind: MCPTrustBundle
metadata:
  name: acme-local-trust
  namespace: tenant-acme
spec:
  # KV key in mcp-trust-bundles — must match SPIFFE trust domain.
  trustDomain: acme.local

  # Exactly one PEM source.
  inlinePem: |
    -----BEGIN CERTIFICATE-----
    MIIB…
    -----END CERTIFICATE-----

  # Alternative: reference a Kubernetes Secret.
  # secretRef:
  #   name: acme-spiffe-bundle
  #   key: bundle.pem

status:
  observedGeneration: 1
  lastSyncedAt: "2026-05-28T14:18:00Z"
  natsKey: acme.local
  kvRevision: 8
  conditions:
    - type: Ready
      status: "True"
      reason: Synced
      message: "Trust bundle published to mcp-trust-bundles/acme.local"
      lastTransitionTime: "2026-05-28T14:18:00Z"
    - type: Synced
      status: "True"
      reason: KvWriteSucceeded
      lastTransitionTime: "2026-05-28T14:18:00Z"
```

**Projected KV:** bucket `mcp-trust-bundles` (`trogon_sts::TRUST_BUNDLES_KV_BUCKET`), key `{trustDomain}`, value = PEM bytes (UTF-8).

**Validation rules:**

- `trustDomain` matches `^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?(\.[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?)*$`.
- PEM must parse as at least one X.509 certificate (`openssl x509` equivalent in controller).
- `trustDomain` immutable after creation (domain rename = new CR + migrate servers).
- `secretRef` must exist and contain non-empty `bundle.pem` (or specified key).

---

## 3. Reconciler logic

Each kind implements the same reconcile skeleton: fetch → validate → publish → status → requeue. Delete paths tombstone instead of hard-deleting KV keys (audit retention per [registry-operations.md § Recovering from KV corruption](registry-operations.md#recovering-from-kv-corruption) — Git wins for agents; KV tombstones preserve forensic history for gateway config).

### 3.1 Shared reconcile skeleton (pseudocode)

```text
function reconcile(object):
    if object.metadata.deletionTimestamp is set:
        return finalize(object)

    if object.generation == object.status.observedGeneration
       and not periodicDriftDue(object):
        return  // steady state

    result = validate(object)
    if result.failed:
        setCondition(object, Ready=False, reason=result.reason, message=result.message)
        emitK8sEvent(object, Warning, result.reason, result.message)
        requeueAfter(backoff(result))
        return

    if not referencesResolved(object):
        setCondition(object, ReferencesResolved=False, …)
        setCondition(object, Ready=False, reason=ReferencedResourceMissing, …)
        requeueAfter(30s)
        return

    try:
        kvRevision = publishToNats(object)
    catch NatsUnreachable:
        setCondition(object, Synced=False, Ready=False, reason=NatsUnreachable, …)
        emitK8sEvent(object, Warning, NatsUnreachable, …)
        requeueAfter(exponentialBackoff)
        return
    catch KvWriteRejected as e:
        setCondition(object, Synced=False, Ready=False, reason=KvWriteRejected, …)
        requeueAfter(60s)
        return

    object.status.observedGeneration = object.generation
    object.status.lastSyncedAt = now()
    object.status.kvRevision = kvRevision
    object.status.natsKey = canonicalKey(object)
    setCondition(object, Synced=True, Ready=True, …)
    setCondition(object, ReferencesResolved=True, …)
    patchStatus(object)

    if periodicDriftDue(object):
        schedulePeriodicRequeue(5m)
```

### 3.2 `MCPServer` reconciler

```text
function validateMCPServer(s):
    assert s.spec.serverId matches slug regex
    assert s.spec.lifecycleState in enum
    if s.spec.lifecycleState == revoked:
        assert s.spec.enabled == false
    load MCPPolicyBundle(s.spec.policyRef) → must exist
    load MCPTrustBundle(s.spec.trustBundleRef) → must exist
    if s.spec.trustDomain set:
        assert trustBundle.spec.trustDomain == s.spec.trustDomain
        OR trustBundle covers domain (future: federated bundles)

function publishMCPServer(s):
    json = mapSpecToBackendJson(s)
    return kvPut("mcp-gateway-config", "backend/" + s.spec.serverId, json)

function finalizeMCPServer(s):
    // Tombstone — do not DELETE key (audit retention)
    json = kvGet("mcp-gateway-config", "backend/" + s.spec.serverId)
    json.lifecycle_state = "revoked"
    json.enabled = false
    json.tombstoned_at = now()
    json.tombstone_reason = "k8s-resource-deleted"
    json.tombstone_uid = s.metadata.uid
    kvPut("mcp-gateway-config", "backend/" + s.spec.serverId, json)
    emitAudit(proposed: "mcp.control.k8s.server.tombstoned", …)
    removeFinalizer(s, "gateway.trogon.ai/mcp-server")
```

**Create/update triggers:** spec change, reference change, label change affecting tenant, manual `kubectl annotate` **proposed** `gateway.trogon.ai/force-sync=true`.

**Ready=True criteria:** KV record exists; JSON fields match spec; referenced policy and trust bundles are `Ready=True`.

### 3.3 `MCPPolicyBundle` reconciler

```text
function validateMCPPolicyBundle(p):
    assert p.spec.revision > p.status.lastPublishedRevision
    assert exactly one of inline / oci
    if inline:
        load ConfigMap → parse YAML → validate Tier-1 schema (proposed validator crate)
    if oci:
        pull artifact → verify digest → validate signature if Phase 3
    assert semverValid(p.spec.minGatewayVersion)
    assert witPackageValid(p.spec.targetWit)
    if p.spec.active:
        assert no other active bundle in namespace OR run supersede policy

function publishMCPPolicyBundle(p):
    body = loadBundleBody(p)
    revKey = "bundle/" + p.metadata.name + "/rev/" + p.spec.revision
    rev = kvPut("mcp-gateway-config", revKey, body)
    if p.spec.active:
        manifest = buildActivePointer(p, sha256(body))
        kvPut("mcp-gateway-config", "bundle/active", manifest)
        proposed: natsPub("mcp.control.bundle.reload", {"tenant": p.spec.tenant, "revision": p.spec.revision})
    p.status.lastPublishedRevision = p.spec.revision
    return rev

function finalizeMCPPolicyBundle(p):
    // Tombstone active pointer only if this bundle owns it
    active = kvGet("mcp-gateway-config", "bundle/active")
    if active.bundle_name == p.metadata.name:
        active.lifecycle_state = "tombstoned"
        active.tombstoned_at = now()
        kvPut("mcp-gateway-config", "bundle/active", active)
    // Retain bundle/{name}/rev/{n} history — no hard delete
    removeFinalizer(p, "gateway.trogon.ai/mcp-policy-bundle")
```

### 3.4 `MCPTrustBundle` reconciler

```text
function validateMCPTrustBundle(t):
    assert trustDomain regex
    pem = loadPem(t)  // inline or Secret
    assert parseX509(pem) succeeds
    assert pem byte length <= KV max value (64 KiB per registry bucket policy; trust bundles typically smaller)

function publishMCPTrustBundle(t):
    pem = loadPem(t)
    return kvPut("mcp-trust-bundles", t.spec.trustDomain, pem)

function finalizeMCPTrustBundle(t):
    // Tombstone — STS must fail-closed on attested paths when bundle removed
    tombstone = {
        "tombstoned": true,
        "tombstoned_at": now(),
        "tombstone_uid": t.metadata.uid,
        "trust_domain": t.spec.trustDomain
    }
    kvPut("mcp-trust-bundles", t.spec.trustDomain, jsonEncode(tombstone))
    // Alternative (proposed): write empty PEM with comment header — prefer explicit tombstone JSON
    // so STS can distinguish missing vs revoked
    removeFinalizer(t, "gateway.trogon.ai/mcp-trust-bundle")
```

### 3.5 Finalizers and deletion ordering

All three kinds attach finalizers on first reconcile:

| Finalizer | Purpose |
|---|---|
| `gateway.trogon.ai/mcp-server` | Tombstone backend KV before CR removal |
| `gateway.trogon.ai/mcp-policy-bundle` | Demote active pointer if owned |
| `gateway.trogon.ai/mcp-trust-bundle` | Tombstone trust KV key |

Deletion of `MCPPolicyBundle` or `MCPTrustBundle` while referenced by an `MCPServer` is **blocked** by owner-reference policy (§4) — the server CR must be deleted or refs updated first.

---

## 4. Owner references and deletion protection

`MCPServer` holds explicit references to `MCPPolicyBundle` and `MCPTrustBundle` via `spec.policyRef` and `spec.trustBundleRef`. Kubernetes garbage collection must not cascade-delete policy or trust bundles while a server still points at them.

### 4.1 Reference tracking model

**proposed** v1alpha1: **validating admission webhook** on `MCPPolicyBundle` / `MCPTrustBundle` DELETE lists referring `MCPServer` objects and rejects with `409 Conflict` if any exist. Future revision: set `ownerReferences` with `blockOwnerDeletion: true` on shared bundles when multiple servers reference one policy.

### 4.2 Referrer index

Controller maintains an in-memory index (refreshed each reconcile):

```text
referrers(trustBundleName) = list MCPServer where spec.trustBundleRef.name == trustBundleName
referrers(policyBundleName) = list MCPServer where spec.policyRef.name == policyBundleName
```

Webhook denial message:

```text
cannot delete MCPPolicyBundle/acme-github-smoke: referenced by MCPServer/github, MCPServer/gitlab
```

### 4.3 Safe decommission sequence

Matches [howto-integrate-third-party-mcp.md § Step 8](howto-integrate-third-party-mcp.md#step-8--roll-back--decommission):

1. Set `MCPServer.spec.lifecycleState: deprecated`, `enabled: false` — drain traffic.
2. Delete `MCPServer` — tombstones `backend/{server_id}`.
3. Delete `MCPPolicyBundle` if no other referrers — demotes `bundle/active` if owned.
4. Delete `MCPTrustBundle` only when no servers reference it **and** no STS workloads require the domain.

Agent registry manifests (`mcp-agent-registry`) remain a **separate** Git-driven decommission per [registry-operations.md](registry-operations.md).

---

## 5. Idempotency and drift detection

### 5.1 Idempotent writes

| Property | Mechanism |
|---|---|
| Same spec, same generation | Reconcile no-ops after `observedGeneration` match |
| KV PUT with identical payload | JetStream KV revision may still increment; controller stores `kvRevision` in status |
| Controller restart | Full informer resync replays all objects; generation comparison prevents write storms |
| Concurrent editors | Last successful write wins; drift detection surfaces manual KV edits |

Publish paths use **content-addressed hashing** (SHA-256 of canonical JSON / YAML) stored in status **proposed** field `contentHash`. Skip KV PUT when hash matches last successful publish.

### 5.2 Periodic drift detection

**proposed interval:** 5 minutes (`MCP_GATEWAY_CONTROLLER_DRIFT_INTERVAL`, default `5m`).

```text
function periodicDriftCheck(object):
    desired = materializeExpectedKvValue(object)
    actual = kvGet(bucket, key)
    if canonicalHash(desired) != canonicalHash(actual):
        emitK8sEvent(object, Warning, KvDrift, "KV diverged from spec")
        emitAudit(proposed: "mcp.control.k8s.drift", {
            "kind": object.kind,
            "name": object.metadata.name,
            "namespace": object.metadata.namespace,
            "nats_key": object.status.natsKey,
            "expected_hash": …,
            "actual_hash": …
        })
        setCondition(object, Synced=False, Ready=False, reason=KvDrift, …)
        publishToNats(object)  // self-heal
        setCondition(object, Synced=True, Ready=True, reason=DriftCorrected, …)
```

**Drift sources:**

| Source | Detection | Correction |
|---|---|---|
| Manual `nats kv put` | Hash mismatch | Overwrite with CRD spec (CRD is desired state) |
| Break-glass operator edit | Hash mismatch + `source != k8s` in JSON | Overwrite unless **proposed** annotation `gateway.trogon.ai/pause-sync=true` |
| Partial write / NATS glitch | `kvRevision` regression | Full republish |
| Tombstone vs live spec | `tombstoned_at` set but CR exists | Republish live spec (CR wins) |

### 5.3 Conflict policy: CRD vs imperative

**Default:** CRD is **desired state** — controller corrects KV drift toward spec. This matches the agent registry principle "Git wins" but inverted for K8s-native resources: **the API object wins**.

Operators who need temporary KV overrides must either pause sync (annotation) or delete the CR and manage imperatively — same ergonomics as `kubectl delete` stopping Deployment reconciliation.

---

## 6. Multi-tenant model

### 6.1 Recommendation

**One controller instance per tenant namespace** (not one cluster-wide controller): dedicated NATS creds scoped to that tenant's JetStream account, `--watch-namespace=tenant-acme`, KV buckets `mcp-gateway-config` and `mcp-trust-bundles` in that account.

**Rationale:** mirrors [registry-operations.md § ACL rule](registry-operations.md#roles) (single writer SA per bucket), aligns with NATS-account-per-tenant topology from [MCP_GATEWAY_PLAN.md Block A](../../MCP_GATEWAY_PLAN.md#block-a--strategic-decisions-paper-no-code), and prevents one compromised SA from overwriting any tenant's `bundle/active`.

**Soft tenancy (JWT claim only):** if Block A resolves to shared-account claim scoping, a cluster-wide controller requires **proposed** KV key prefix `tenant/{tenant}/…` — not in the codebase today; use separate namespaces + NATS accounts until a plan amendment lands.

CRD definitions live in platform namespace `trogon-system`; tenant CRs and controller Deployment live in `tenant-{name}/`.

---

## 7. RBAC

### 7.1 Controller ServiceAccount permissions

Minimum Kubernetes RBAC for the controller pod (`trogon-mcp-gateway-controller` **proposed** name):

```yaml
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: trogon-mcp-gateway-controller
rules:
  # CRDs — reconcile loop
  - apiGroups: ["gateway.trogon.ai"]
    resources:
      - mcpservers
      - mcppolicybundles
      - mcptrustbundles
    verbs: ["get", "list", "watch"]
  - apiGroups: ["gateway.trogon.ai"]
    resources:
      - mcpservers/status
      - mcppolicybundles/status
      - mcptrustbundles/status
    verbs: ["get", "update", "patch"]
  # Finalizers
  - apiGroups: ["gateway.trogon.ai"]
    resources:
      - mcpservers/finalizers
      - mcppolicybundles/finalizers
      - mcptrustbundles/finalizers
    verbs: ["update"]
  # Secrets — trust bundle PEM via secretRef
  - apiGroups: [""]
    resources: ["secrets"]
    verbs: ["get", "list", "watch"]
  # ConfigMaps — inline policy bundles
  - apiGroups: [""]
    resources: ["configmaps"]
    verbs: ["get", "list", "watch"]
  # Events — observability
  - apiGroups: [""]
    resources: ["events"]
    verbs: ["create", "patch"]
  # Leases — leader election when HA
  - apiGroups: ["coordination.k8s.io"]
    resources: ["leases"]
    verbs: ["get", "create", "update"]
```

**Not granted:** `delete` on CRs (users delete via kubectl; controller only removes finalizers), cluster-wide Secret/ConfigMap list (namespace-scoped RoleBinding instead of ClusterRole in production), `pods/exec`, or any workload mutation.

Bind the ClusterRole via **RoleBinding** in the tenant namespace. Split Secrets/ConfigMap access into a namespaced **Role** when hardening production. Admission webhook SA needs `get`/`list` on all three CR kinds.

### 7.2 NATS ACL (JetStream account)

Mirror [registry-operations.md § Roles](registry-operations.md#roles) for the controller NATS identity:

| NATS permission | Scope |
|---|---|
| KV get, put, purge (no delete) | `$KV.mcp-gateway-config.>` |
| KV get, put | `$KV.mcp-trust-bundles.>` |
| Deny | `$KV.mcp-agent-registry.>` (Git controller only) |
| Deny | publish `mcp.gateway.request.>`, `mcp.server.>` |
| Allow (optional) | publish `mcp.control.k8s.>`, `mcp.control.bundle.reload` |
| Allow (optional) | publish `mcp.audit.>` for control-plane audit envelope |

Production: dedicated NATS user `{org}/mcp-gateway-controller` with creds in Kubernetes Secret `nats-creds`.

---

## 8. NATS credentials

### 8.1 Options considered

| Mechanism | Fit |
|---|---|
| **NKEY + JWT user creds file** | Matches existing Trogon NATS bootstrap (`nats` CLI, `NATS_CREDS` env across crates) |
| **JWT only (inline)** | Same underlying model; creds file is the standard packaging |
| **mTLS** | Strong for leaf-node topologies; adds cert rotation burden |
| **Username/password** | Dev only; avoid in production |

### 8.2 Recommendation

**NKEY-signed JWT user credentials** mounted as a Kubernetes Secret at `/etc/nats/creds/controller.creds`, referenced by env `NATS_CREDS` — consistent with `trogon-agent-registry-controller`, `trogon-sts`, and `trogon-mcp-gateway` connection patterns.

**Justification:**

1. JetStream KV ACLs are expressed in NATS JWT permissions today ([scripts/acl-templates/](../../scripts/acl-templates/README.md)); NKEY rotation follows existing `nsc` operator workflows.
2. mTLS is appropriate for **client→NATS transport** when the cluster requires it, but identity authorization for KV writes still flows through JWT permissions — mTLS alone does not replace account-scoped KV PUT denial.
3. The controller is a long-lived service account, not an end-user; creds file + periodic rotation (quarterly) matches registry controller custody.

Mount creds as Kubernetes Secret `nats-controller-creds` (standard NATS `.creds` file layout).

**Controller env (proposed):**

| Variable | Purpose |
|---|---|
| `NATS_URL` | Cluster URL(s), comma-separated |
| `NATS_CREDS` | Path to creds file |
| `MCP_GATEWAY_CONTROLLER_KV_CONFIG_BUCKET` | Default `mcp-gateway-config` |
| `MCP_GATEWAY_CONTROLLER_KV_TRUST_BUCKET` | Default `mcp-trust-bundles` |
| `MCP_GATEWAY_CONTROLLER_DRIFT_INTERVAL` | Default `5m` |
| `MCP_GATEWAY_CONTROLLER_TENANT` | Required when watching single namespace |

**Rotation:** deploy new Secret version → rolling restart controller → revoke old JWT via `nsc` → verify `Ready=True` on all CRs.

---

## 9. Failure modes

| Failure | Symptom | Controller behavior | Operator action |
|---|---|---|---|
| **NATS unreachable** | `Synced=False`, `Ready=False`, reason `NatsUnreachable` | Exponential backoff requeue (1s → 30s cap); no KV mutation | Fix network/NATS; check creds expiry |
| **CRD validation failure** | `Ready=False`, reason `ValidationFailed` | K8s Warning event with field path; no KV write | Fix spec; `kubectl describe` |
| **Referenced Secret missing** | `ReferencesResolved=False`, reason `SecretNotFound` | Requeue 30s until Secret appears or ref changed | Create Secret or fix `secretRef` |
| **Referenced ConfigMap missing** | Same for policy inline source | Same | Create ConfigMap with bundle YAML |
| **Policy reference missing** | `ReferencedResourceMissing`, reason `PolicyNotFound` | No backend publish | Create `MCPPolicyBundle` or fix ref |
| **Trust reference missing** | reason `TrustBundleNotFound` | No backend publish | Create `MCPTrustBundle` |
| **KV write ACL denied** | `KvWriteRejected` | Event + metric increment; requeue 60s | Fix NATS JWT permissions |
| **OCI pull failure** | `ValidationFailed`, reason `ArtifactPullFailed` | No publish | Fix image pull secrets / digest |
| **Signature invalid (Phase 3)** | `ValidationFailed`, reason `SignatureInvalid` | No publish | Re-sign bundle |
| **Active bundle conflict** | `Ready=False`, reason `Superseded` | Non-active CR waits | Demote duplicate `active: true` |
| **Drift detected** | `KvDrift` event; brief `Ready=False` | Self-heal republish | Investigate manual KV editor |
| **Trust bundle PEM invalid** | `ValidationFailed`, reason `InvalidPem` | No KV write | Fix PEM in spec or Secret |

NATS errors increment `mcp_gateway_controller_nats_errors_total` and requeue with exponential backoff (1s cap 30s). Gateway and STS keep serving last-known KV until sync resumes. Missing Secrets never produce partial PEM writes — STS retains the previous bundle until the reference resolves or the CR is deleted.

---

## 10. Observability

### 10.1 Prometheus metrics (proposed)

| Metric | Type | Labels | Purpose |
|---|---|---|---|
| `mcp_gateway_controller_reconcile_total` | Counter | `kind`, `result` (`success`, `error`, `requeue`) | Reconcile throughput |
| `mcp_gateway_controller_reconcile_duration_seconds` | Histogram | `kind` | Latency SLO |
| `mcp_gateway_controller_nats_errors_total` | Counter | `operation` (`kv_put`, `kv_get`, `connect`) | NATS health |
| `mcp_gateway_controller_kv_publish_total` | Counter | `bucket`, `key_prefix` | Write volume |
| `mcp_gateway_controller_drift_detected_total` | Counter | `kind` | Manual intervention signal |
| `mcp_gateway_controller_drift_corrected_total` | Counter | `kind` | Self-heal success |
| `mcp_gateway_controller_ready` | Gauge | `kind` | Objects with `Ready=True` |
| `mcp_gateway_controller_not_ready` | Gauge | `kind`, `reason` | Alert on validation/NATS failures |

Expose on `:8080/metrics` (separate from NATS). Follow [otel-name-metric conventions](https://github.com/trogonstack/trogonstack-otel) when OpenTelemetry export lands (Block G).

### 10.2 Logs and Events

Structured tracing logs include `kind`, `namespace`, `name`, `nats_key`, `kv_revision`, `duration_ms`. Never log PEM or Secret contents. Kubernetes Events on the reconciled object use reasons `Synced`, `ValidationFailed`, `NatsUnreachable`, `KvDrift`, `DriftCorrected`, `ReferencedResourceMissing`, `Superseded`. **proposed** JetStream audit subjects under `mcp.control.k8s.*` mirror sync, drift, and tombstone actions ([reference-audit-envelope.md](reference-audit-envelope.md)). Expose `/healthz`, `/readyz`, and `/metrics` on `:8080`; readiness failure does not revert KV.

---

## 11. Migration from `agctl` shell workflows

### 11.1 Current state (imperative)

Operators following [howto-integrate-third-party-mcp.md](howto-integrate-third-party-mcp.md) today:

1. `nats kv put mcp-gateway-config backend/{id}` — server registration
2. `nats kv put mcp-gateway-config bundle/...` — policy
3. `trogon-sts-publish-trust-bundle` or `nats kv put mcp-trust-bundles …` — trust
4. `agctl registry sync` — agent manifests (unchanged by this controller)
5. Git merge → `trogon-agent-registry-controller` — registry KV

### 11.2 Target state (declarative)

Same NATS KV layout; CRDs become desired state. `agctl` shifts to validation and inspection:

| Imperative step | Declarative equivalent |
|---|---|
| Manual backend KV put | `MCPServer` CR |
| Manual bundle KV put | `MCPPolicyBundle` CR + ConfigMap |
| `publish-trust-bundle` | `MCPTrustBundle` CR |
| `agctl mcp servers list` (future) | Reads same KV; optional direct K8s API list |

### 11.3 Cutover procedure (zero policy drop)

1. **Export** existing KV (`backend/{id}`, `bundle/active`, trust PEM) to Git for rollback.
2. **Install** CRDs + controller; create `MCPTrustBundle`, `MCPPolicyBundle` (+ ConfigMap), then `MCPServer` with matching refs; wait for `Ready=True`.
3. **Verify** with smoke tests from [howto § Step 6](howto-integrate-third-party-mcp.md#step-6--promote-to-active); compare KV hashes (**proposed** `agctl mcp diff --namespace …`).
4. **Enforce** CRD ownership: enable drift detection; retire break-glass KV creds for routine ops.
5. **Registry unchanged:** continue Git → `trogon-agent-registry-controller` for `mcp-agent-registry`; align `allowed_audiences` when adding servers.

**Rollback:** scale controller to zero — KV and gateway unchanged; imperative edits resume. **Coexistence:** CR wins over manual KV (drift heal within 5m); orphan KV without a CR stays until import; **proposed** `gateway.trogon.ai/pause-sync=true` skips drift correction.

---

## 12. Cross-references

| Document | Relevance to controller |
|---|---|
| [integration-touchpoints.md](integration-touchpoints.md) | KV bucket names (`mcp-gateway-config`, `mcp-trust-bundles`), gateway watch behavior, trust bundle ownership (`trogon-sts`) |
| [registry-operations.md](registry-operations.md) | ACL pattern for controller NATS identity; tombstone vs hard delete; Git-authoritative registry (out of scope) |
| [howto-integrate-third-party-mcp.md](howto-integrate-third-party-mcp.md) | Imperative workflow this controller replaces; backend JSON shape; promotion lifecycle |
| [bootstrap-day-zero.md](bootstrap-day-zero.md) | Tenant readiness predicate; KV key shapes; day-zero posture interaction |
| [mcp-policy-wit-sketch.md](mcp-policy-wit-sketch.md) | `targetWit` validation on `MCPPolicyBundle` |
| [mcp-gateway-operator-overview.md](mcp-gateway-operator-overview.md) | Data-plane topology; where controller sits in ops mental model |
| [failure-mode-matrix.md](failure-mode-matrix.md) | Gateway/STS fail modes when KV stale (rows 5, 15) |
| [reference-audit-envelope.md](reference-audit-envelope.md) | Target shape for **proposed** control-plane audit events |

**Apply order for a new tenant stack:** `MCPTrustBundle` → `MCPPolicyBundle` (+ ConfigMap) → `MCPServer`. See §2 YAML for field-level examples.

---

*Document type: Diátaxis **reference** + **explanation**. For imperative integration steps without Kubernetes, read [howto-integrate-third-party-mcp.md](howto-integrate-third-party-mcp.md). For wire-level KV and audit contracts, read [integration-touchpoints.md](integration-touchpoints.md).*
