# Secrets Management Research

Research on secrets management solutions relevant to `trogon-secret-proxy`: HashiCorp Vault / OpenBao, 1Password, and the Kubernetes Secrets Store CSI Driver.

---

## TL;DR — Recommendation for this project

**OpenBao (Vault OSS fork) + Kubernetes CSI Driver. No 1Password.**

- `trogon-secret-proxy` is pure platform infrastructure — 100% machine-to-machine access
- The `tok_{provider}_{env}_{id}` token format maps directly to Vault KV v2 paths
- Kubernetes auth eliminates long-lived credentials in the cluster
- 1Password adds complexity (Connect Server, SaaS dependency, cost) with no benefit here

---

## Table of Contents

1. [HashiCorp Vault / OpenBao](#1-hashicorp-vault--openbao)
2. [1Password Secrets Automation](#2-1password-secrets-automation)
3. [Kubernetes Secrets Store CSI Driver](#3-kubernetes-secrets-store-csi-driver)
4. [Comparison](#4-comparison)
5. [Architecture for this project](#5-architecture-for-this-project)

---

## 1. HashiCorp Vault / OpenBao

### What it is

Vault is HashiCorp's secrets management product. In August 2023 HashiCorp relicensed all products from MPL 2.0 to BSL 1.1 (prohibits competing SaaS). **OpenBao** is the Linux Foundation MPL 2.0 fork, API-compatible and the recommended choice for open-source deployments.

In 2024 IBM acquired HashiCorp for ~$6.4B.

### Core architecture

```
Storage Backend (Raft/Consul/S3) — encrypted opaque blobs
        ↓  decrypt with master key
Vault Core — Router
  ├── Secret Engines   (KV v1/v2, Transit, Database, PKI, AWS…)
  ├── Auth Methods     (AppRole, Kubernetes, OIDC, AWS, Azure…)
  └── Audit Devices    (file, syslog, socket)
```

Everything stored in the backend is encrypted with AES-256-GCM. Vault starts **sealed** — it must be unsealed by combining Shamir key shares (threshold of 3/5 by default) or via cloud KMS auto-unseal (AWS KMS, GCP KMS, Azure Key Vault).

### Secret Engines

#### KV v2 (current standard)

Every write creates a new version. Supports soft-delete, hard-destroy, check-and-set (CAS), and custom metadata.

**Critical path distinction:**

| Operation | API path |
|---|---|
| Read / write data | `/v1/<mount>/data/<path>` |
| Version metadata | `/v1/<mount>/metadata/<path>` |
| Soft delete | `/v1/<mount>/delete/<path>` |
| Restore | `/v1/<mount>/undelete/<path>` |
| Destroy permanently | `/v1/<mount>/destroy/<path>` |

```bash
# Write
vault kv put secret/myapp/openai api_key=sk-abc123

# Read latest
vault kv get secret/myapp/openai

# Read specific version
vault kv get -version=3 secret/myapp/openai

# CAS — only write if current version is 4
curl -X PUT https://vault/v1/secret/data/myapp/openai \
  -d '{"options":{"cas":4},"data":{"api_key":"sk-new"}}'
```

#### Transit (encryption-as-a-service)

The key **never leaves Vault**. Send plaintext → get ciphertext, or vice versa.

```bash
vault secrets enable transit
vault write -f transit/keys/my-key

# Encrypt
vault write transit/encrypt/my-key plaintext=$(echo "sk-abc" | base64)
# → ciphertext: vault:v1:abc123...

# Decrypt
vault write transit/decrypt/my-key ciphertext="vault:v1:abc123..."
# → plaintext: <base64>

# Rotate key (old ciphertexts still decryptable)
vault write -f transit/keys/my-key/rotate
```

#### Dynamic Secrets

Database, AWS, PKI engines generate credentials **on demand** with a TTL. When the lease expires, Vault automatically revokes them.

```bash
# Example: database engine — each read creates unique ephemeral credentials
vault read database/creds/my-role
# → username: v-appid-xxx-20240115
# → password: A1a-xyz987
# → lease_duration: 1h   (Vault drops the user when this expires)
```

### Authentication Methods

#### AppRole (for services)

```bash
# RoleID — not secret, can be baked into config
vault read auth/approle/role/my-proxy/role-id
# role_id: db02de05-...

# SecretID — secret, generated at deploy time
vault write -f auth/approle/role/my-proxy/secret-id
# secret_id: 6a174c20-...

# Application login
POST /v1/auth/approle/login
{"role_id": "db02de05-...", "secret_id": "6a174c20-..."}
# → auth.client_token  (use as X-Vault-Token)
```

SecretID wrapping: Vault returns a single-use wrap-token instead of the SecretID directly. Only the intended recipient can unwrap it.

#### Kubernetes Auth (best for K8s workloads)

```bash
vault auth enable kubernetes
vault write auth/kubernetes/config \
  kubernetes_host="https://kubernetes.default.svc"

vault write auth/kubernetes/role/my-proxy \
  bound_service_account_names=my-proxy-sa \
  bound_service_account_namespaces=default \
  token_policies=my-proxy-policy \
  ttl=1h
```

The pod's ServiceAccount JWT is the credential — no long-lived secrets needed in the cluster.

```
Pod SA JWT → POST /v1/auth/kubernetes/login → Vault token
```

### Lease / TTL model

```
Create credential / authenticate
  → lease_id, lease_duration, renewable: true

[at ~75% of TTL] PUT /v1/sys/leases/renew
  → extends duration (cannot exceed max_ttl)

[max_ttl reached] → must re-authenticate

[lease expires or revoked] → Vault calls target system to delete credential
```

TTL is resolved from most specific to most general: request → role → mount → system default.

### Policies (HCL)

```hcl
# Read-only access to AI keys
path "ai-keys/data/+/+/+" {
  capabilities = ["read"]
}

# Provisioner — write but not read (separation of concerns)
path "ai-keys/data/+/+/+" {
  capabilities = ["create", "update"]
}
path "ai-keys/metadata/+/+/+" {
  capabilities = ["delete"]
}

# Own token management
path "auth/token/renew-self" { capabilities = ["update"] }
path "auth/token/lookup-self" { capabilities = ["read"] }
```

### Kubernetes integration patterns

#### Vault Agent Injector

A MutatingWebhookConfiguration intercepts pod creation. If annotations are present, it adds an init container + sidecar running Vault Agent.

```yaml
metadata:
  annotations:
    vault.hashicorp.com/agent-inject: "true"
    vault.hashicorp.com/role: "my-proxy"
    vault.hashicorp.com/agent-inject-secret-config: "secret/data/myapp/openai"
    vault.hashicorp.com/agent-inject-template-config: |
      {{- with secret "secret/data/myapp/openai" -}}
      export OPENAI_API_KEY="{{ .Data.data.api_key }}"
      {{- end }}
```

Secrets land at `/vault/secrets/<name>`. The sidecar continuously renews leases and re-renders templates on rotation.

#### Vault Secrets Operator (VSO)

CRD-based controller that syncs Vault secrets into native Kubernetes Secret objects.

```yaml
apiVersion: secrets.hashicorp.com/v1beta1
kind: VaultStaticSecret
metadata:
  name: openai-keys
spec:
  vaultAuthRef: vault-auth
  mount: secret
  type: kv-v2
  path: myapp/openai
  refreshAfter: 30s
  destination:
    name: openai-api-secret
    create: true
  rolloutRestartTargets:
    - kind: Deployment
      name: my-proxy
```

### OSS vs Enterprise vs HCP

| | OSS / OpenBao | Enterprise | HCP Vault |
|---|---|---|---|
| Core secret engines | ✅ | ✅ | ✅ |
| Kubernetes auth | ✅ | ✅ | ✅ |
| Namespaces | ❌ | ✅ | ✅ |
| Replication | ❌ | ✅ | ✅ |
| HSM / FIPS | ❌ | ✅ | — |
| Managed infra | ❌ | ❌ | ✅ |
| License | MPL 2.0 (OpenBao) / BSL 1.1 (Vault) | Commercial | Usage-based |

---

## 2. 1Password Secrets Automation

### What it is

1Password is primarily a password manager with a developer/automation layer: Service Accounts, Connect Server, Kubernetes Operator, and SDKs. Its key differentiator is **zero-knowledge end-to-end encryption** — 1Password's servers never see plaintext values.

### Three access paths

| Path | What it is | When to use |
|---|---|---|
| **Service Account + SDK** | `ops_...` token, calls 1password.com directly | New apps, no extra infra |
| **Connect Server** | Two self-hosted containers, local REST API | Air-gapped / no cloud runtime dependency |
| **Operator** | K8s Operator watching `OnePasswordItem` CRDs | K8s with native Secret objects |

### Secret reference format

```
op://<vault>/<item>[/<section>]/<field>

op://Production/anthropic/api-key
op://Production/Database/credentials/password
op://abc123uvwxyz/def456uvwxyz/credential   ← UUIDs preferred in automation
```

**Limitation**: references cannot be embedded mid-string. The entire value must be a reference.

### Go SDK

```go
client, err := onepassword.NewClient(ctx,
    onepassword.WithServiceAccountToken(os.Getenv("OP_SERVICE_ACCOUNT_TOKEN")),
    onepassword.WithIntegrationInfo("my-service", "1.0.0"),
)

secret, err := client.Secrets().Resolve(ctx, "op://Production/anthropic/api-key")
```

### Connect Server

```yaml
# Docker Compose
services:
  connect-api:
    image: 1password/connect-api:1.7.3
    ports: ["8080:8080"]
    volumes:
      - ./1password-credentials.json:/home/opuser/.op/1password-credentials.json:ro
      - op_data:/home/opuser/.op/data

  connect-sync:
    image: 1password/connect-sync:1.7.3
    volumes:
      - ./1password-credentials.json:/home/opuser/.op/1password-credentials.json:ro
      - op_data:/home/opuser/.op/data
```

REST API at `http://connect:8080/v1`:

```
GET /v1/vaults/{vault_uuid}/items/{item_id}
GET /v1/activity
GET /v1/heartbeat
```

### Kubernetes Operator

```yaml
apiVersion: onepassword.com/v1
kind: OnePasswordItem
metadata:
  name: anthropic-credentials
  annotations:
    operator.1password.io/auto-restart: "true"
spec:
  itemPath: "vaults/Production/items/anthropic-api"
```

Creates a native K8s Secret with one key per item field.

### CLI

```bash
# Inject secrets as env vars into a subprocess
OP_SERVICE_ACCOUNT_TOKEN="..." op run --env-file=.env.tpl -- ./server

# Inject into config file template
op inject -i config.tpl.yaml -o config.yaml

# Read a single value
op read "op://Production/stripe/api-key"
```

### Security model

- **SRP (Secure Remote Password)**: master password never sent to 1Password servers
- **Secret Key**: 128-bit random value, never leaves devices, combined with master password to derive encryption keys
- **AES-256-GCM** encryption of all item data, keys derived client-side
- 1Password's servers store only encrypted blobs — zero-knowledge

### Access control

RBAC is **vault-level only** — no per-item or per-field ACL. A service account gets view/create/edit/delete on specific vaults. For fine-grained path-level control, Vault is superior.

---

## 3. Kubernetes Secrets Store CSI Driver

### The problem with native K8s Secrets

- Stored in etcd as base64 (plain if `EncryptionConfiguration` not set)
- No automatic sync from external stores on rotation
- Coarse RBAC — any SA with `get` reads the entire Secret object
- No per-pod audit trail at the source of truth

### Architecture

```
kube-apiserver
  ├── SecretProviderClass CRD (namespaced)
  └── SecretProviderClassPodStatus CRD (auto-created per pod)

Per node (DaemonSet):
  ├── secrets-store-csi-driver
  │     ├── secrets-store       (CSI gRPC server + rotation controller)
  │     ├── node-driver-registrar
  │     └── liveness-probe
  └── <provider>-csi-provider   (vault, aws, azure, gcp, 1password)
        └── Unix socket: /etc/kubernetes/secrets-store-csi-providers/<provider>.sock
```

### Pod creation → secret on disk: full flow

```
1.  Pod created with CSI volume (driver: secrets-store.csi.k8s.io)
2.  Scheduler → Node N
3.  kubelet calls NodePublishVolume on the driver
4.  Driver reads SecretProviderClass from kube-apiserver
5.  Driver calls TokenRequest API → short-lived JWT for the pod's SA
         (audience: "vault", TTL: configurable)
6.  Driver calls provider plugin via Unix socket:
         Mount(parameters + JWT)
7.  Provider (e.g. vault-csi-provider):
         POST /v1/auth/kubernetes/login {role, jwt} → Vault token
         GET  /v1/secret/data/myapp/db              → values
         returns file contents to driver
8.  Driver creates tmpfs at targetPath, writes secret files
         (tmpfs = RAM only, never touches disk)
9.  kubelet bind-mounts targetPath into the container
         → /mnt/secrets/db-password appears in the container
10. Driver creates SecretProviderClassPodStatus
         (records pod + SPC + object versions)
11. If secretObjects defined → controller creates K8s Secret (goes to etcd)
```

### SecretProviderClass — Vault example

```yaml
apiVersion: secrets-store.csi.x-k8s.io/v1
kind: SecretProviderClass
metadata:
  name: myapp-vault-secrets
  namespace: myapp
spec:
  provider: vault
  parameters:
    vaultAddress: "http://vault.vault.svc.cluster.local:8200"
    roleName: "myapp-role"
    objects: |
      - objectName: db-password
        secretPath: secret/data/myapp/db
        secretKey: password
      - objectName: api-key
        secretPath: secret/data/myapp/openai
        secretKey: api_key

  # Optional: sync to K8s Secret for env vars (goes to etcd)
  secretObjects:
  - secretName: myapp-k8s-secret
    type: Opaque
    data:
    - objectName: db-password
      key: DB_PASSWORD
```

### Pod spec

```yaml
spec:
  serviceAccountName: myapp-sa
  volumes:
  - name: vault-secrets
    csi:
      driver: secrets-store.csi.k8s.io
      readOnly: true
      volumeAttributes:
        secretProviderClass: myapp-vault-secrets
  containers:
  - name: app
    volumeMounts:
    - name: vault-secrets
      mountPath: /mnt/secrets
      readOnly: true
    # Env var from synced K8s Secret (only if needed):
    env:
    - name: DB_PASSWORD
      valueFrom:
        secretKeyRef:
          name: myapp-k8s-secret
          key: DB_PASSWORD
```

### Workload identity — zero credentials in the cluster

```
K8s TokenRequest API
  → short-lived JWT (audience="vault", sub="system:serviceaccount:ns:sa")
  ↓
vault-csi-provider sends JWT to Vault Kubernetes auth
  ↓
Vault verifies JWT against cluster JWKS endpoint
  ↓
Vault token (scoped to policy) → read secrets
```

No long-lived credentials needed anywhere in the cluster.

### Automatic rotation

```bash
helm install csi-secrets-store secrets-store-csi-driver/secrets-store-csi-driver \
  --set enableSecretRotation=true \
  --set rotationPollInterval=2m
```

- **Files** (tmpfs): updated **in-place** without pod restart. App must re-read the file on each use.
- **Env vars** (via synced K8s Secret): K8s Secret is updated but running container env vars are **not** — pod restart required.

### Installation

```bash
# 1. CSI Driver
helm repo add secrets-store-csi-driver \
  https://kubernetes-sigs.github.io/secrets-store-csi-driver/charts

helm install csi-secrets-store \
  secrets-store-csi-driver/secrets-store-csi-driver \
  --namespace kube-system \
  --set syncSecret.enabled=true \
  --set enableSecretRotation=true \
  --set rotationPollInterval=2m

# 2. Vault CSI Provider (via Vault Helm chart)
helm install vault hashicorp/vault \
  --set server.enabled=false \
  --set injector.enabled=false \
  --set csi.enabled=true
```

### Gotchas

- **Namespace-scoped**: `SecretProviderClass` must be in the same namespace as the pod. No cluster-scoped variant.
- **Dynamic secrets (DB/PKI)**: the driver does **not** renew Vault leases — it only re-fetches on the poll interval. For short-TTL dynamic secrets, Vault Agent Injector handles lease renewal better.
- **EKS Fargate**: CSI drivers do not work (no DaemonSets on Fargate). Use External Secrets Operator instead.
- **secretObjects and etcd**: syncing to K8s Secrets re-introduces the etcd exposure. Use only when env vars are strictly required.

---

## 4. Comparison

### Vault vs 1Password

| | Vault / OpenBao | 1Password |
|---|---|---|
| Primary audience | Platform/infra engineers | Developer teams |
| Deployment | Self-hosted (or HCP managed) | SaaS + optional Connect Server |
| Dynamic secrets | ✅ DB, AWS, PKI ephemeral | ❌ static only |
| Encryption as a service | ✅ Transit engine | ❌ |
| PKI / TLS issuance | ✅ | ❌ |
| RBAC granularity | Per path (very fine) | Per vault (coarse) |
| Human UX | CLI/API only | ✅ native apps, browser extension |
| Zero-knowledge | ❌ server-side encryption | ✅ client-side |
| Kubernetes auth | ✅ native (SA JWT) | ❌ requires SA token as K8s Secret |
| Audit log | ✅ per-path, per-entity | ✅ per-item, per-account |
| Operational complexity | High (cluster, HA, unseal) | Low (2 containers) |
| License | BSL 1.1 / MPL 2.0 (OpenBao) | Commercial SaaS |

### CSI Driver vs Vault Agent Injector vs External Secrets Operator

| | CSI Driver | Vault Agent Injector | External Secrets Operator |
|---|---|---|---|
| Avoids etcd | ✅ (file mode) | ✅ (emptyDir) | ❌ always K8s Secrets |
| Multi-provider | ✅ | ❌ Vault only | ✅ 30+ providers |
| Env var support | Via secretObjects (etcd) | Via template to file | ✅ native |
| In-place rotation | ✅ files | ✅ sidecar continuous | Polling interval |
| Node-level daemon | ✅ DaemonSet | ❌ | ❌ |
| Pod spec changes required | Volume mount | Annotations only | ❌ none |
| Dynamic secret lease renewal | ❌ | ✅ | ✅ |
| EKS Fargate | ❌ | ❌ | ✅ |
| Resource overhead | Per-node DaemonSet | Per-pod sidecar | Single Deployment |

---

## 5. Architecture for this project

### Chosen stack

- **OpenBao** (Vault OSS fork, MPL 2.0) — secrets store
- **Kubernetes auth** — zero long-lived credentials in cluster
- **KV v2** — AI provider key storage
- **CSI Driver** — secret distribution to workers
- **`trogon-secret-proxy`** — HTTP abstraction layer for AI provider keys

### Token → Vault path mapping

The `tok_{provider}_{env}_{id}` token format maps directly to KV v2 paths:

```
tok_anthropic_prod_a1b2c3
    └──────────────────────→  ai-keys/data/anthropic/prod/a1b2c3
```

### VaultStore trait implementation

```rust
// VaultStore trait maps to Vault KV v2 operations:
//
// store(token, plaintext)
//   PUT /v1/ai-keys/data/{provider}/{env}/{id}
//   body: {"data": {"api_key": "<plaintext>"}}
//
// resolve(token)
//   GET /v1/ai-keys/data/{provider}/{env}/{id}
//   → .data.data.api_key
//
// revoke(token)
//   DELETE /v1/ai-keys/metadata/{provider}/{env}/{id}
//   (destroys all versions)
```

### Vault configuration

```bash
# Enable KV v2 for AI keys
vault secrets enable -path=ai-keys kv-v2

# Enable Kubernetes auth
vault auth enable kubernetes
vault write auth/kubernetes/config \
  kubernetes_host="https://kubernetes.default.svc"

# Policy for workers (read-only)
vault policy write proxy-worker-policy - <<EOF
path "ai-keys/data/+/+/+" {
  capabilities = ["read"]
}
path "auth/token/renew-self" {
  capabilities = ["update"]
}
EOF

# Policy for provisioner (write-only, cannot read)
vault policy write proxy-provisioner-policy - <<EOF
path "ai-keys/data/+/+/+" {
  capabilities = ["create", "update"]
}
path "ai-keys/metadata/+/+/+" {
  capabilities = ["delete"]
}
EOF

# K8s role for workers
vault write auth/kubernetes/role/proxy-worker \
  bound_service_account_names=trogon-worker-sa \
  bound_service_account_namespaces=production \
  token_policies=proxy-worker-policy \
  ttl=1h
```

### SecretProviderClass for the worker

```yaml
apiVersion: secrets-store.csi.x-k8s.io/v1
kind: SecretProviderClass
metadata:
  name: proxy-worker-secrets
  namespace: production
spec:
  provider: vault
  parameters:
    vaultAddress: "http://openbao.vault.svc.cluster.local:8200"
    roleName: "proxy-worker"
    objects: |
      - objectName: vault-addr
        secretPath: services/data/proxy/config
        secretKey: vault_addr
```

### What NOT to do

- ❌ Native K8s Secrets for AI keys without etcd encryption + KMS
- ❌ Secrets in environment variables hardcoded in manifests
- ❌ Secrets in code or repositories
- ❌ Shared vault across prod/staging/dev
- ❌ Write access for worker pods — read-only only
- ❌ 1Password — adds complexity (Connect Server, SaaS dependency, cost) with no benefit for this use case

### Provisioning flow

```
1. Engineer gets sk-ant-xxx from Anthropic
2. Runs provisioning CLI / CD job:
     vault kv put ai-keys/anthropic/prod/a1b2c3 api_key=sk-ant-xxx
3. Generates and delivers proxy token:
     tok_anthropic_prod_a1b2c3
     (the real key sk-ant-xxx is never shared)
4. Services use tok_anthropic_prod_a1b2c3 in Authorization header
5. trogon-secret-proxy resolves → sk-ant-xxx at request time
6. Audit log in OpenBao records every resolution
```

---

## References

- [OpenBao](https://openbao.org) — MPL 2.0 fork of HashiCorp Vault
- [HashiCorp Vault docs](https://developer.hashicorp.com/vault/docs)
- [Secrets Store CSI Driver](https://secrets-store-csi-driver.sigs.k8s.io)
- [1Password Developer docs](https://developer.1password.com)
- [External Secrets Operator](https://external-secrets.io)
- [Vault CSI Provider](https://github.com/hashicorp/vault-csi-provider)
