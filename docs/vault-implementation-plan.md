# Plan de implementación: Secret Management con Infisical + NATS KV

Rama: `feat/vault-agent-vault`  
Fecha: 2026-04-24

---

## Contexto y decisiones de diseño

### Source of truth: Infisical (reemplaza HashiCorp Vault)

El boss vetó HashiCorp Vault. Infisical cubre el mismo rol:
- Secrets cifrados en reposo
- API REST para read/write
- Self-hostable, MIT license en el core
- Sin coste de licencia para las operaciones CRUD que usamos

### Caché distribuido: NATS KV con AES-256-GCM

NATS KV es excelente para distribución y latencia baja, pero Jepsen (diciembre 2025,
NATS 2.12.1) documentó pérdida de writes con `sync_interval` por defecto. No se usa
como source of truth — solo como caché. Los buckets se crean con:
- `storage = File` + `sync_interval = "always"` (durabilidad máxima)
- `history = 2` (current + previous para grace period de rotación)

NATS **nunca ve plaintext** — los valores se cifran con AES-256-GCM antes de escribir.

### Human-in-the-loop: trogon-vault-approvals sobre JetStream

No se usan las approval policies nativas de Infisical (tier Pro/Enterprise). El workflow
de aprobación lo implementa trogon vía NATS JetStream, reutilizando el pipeline Slack
existente. Infisical solo recibe el write final cuando el humano aprueba.

### Lo que NO cambia

| Componente | Estado |
|---|---|
| `VaultStore` trait (4 métodos) | Solo se añade `resolve_with_previous` con default impl |
| `MemoryVault` | Sin cambios — tests siguen funcionando |
| `ApiKeyToken` formato `tok_{provider}_{env}_{id}` | Sin cambios |
| `vault_admin.rs` subjects y request/response types | Sin cambios |
| `trogon-secret-proxy` routing y proxy | Sin cambios |

---

## Arquitectura objetivo

```
┌─────────────────────────────────────────────────────┐
│  Infisical  ←  SOURCE OF TRUTH                      │
│  Self-hosted, API REST                              │
│  Path: /{project}/{env}/{id}                        │
│  InfisicalVaultStore (nuevo backend en trogon-vault)│
└──────────────────────┬──────────────────────────────┘
                       │ escribe al arranque + en cada rotación/aprobación
                       ▼
┌─────────────────────────────────────────────────────┐
│  NATS KV Bucket: vault_{name}  ←  CACHÉ DISTRIBUIDO │
│  Valores cifrados AES-256-GCM (NATS no ve plaintext)│
│  history=2, storage=File, sync_interval=always      │
└──────────────────────┬──────────────────────────────┘
                       │ watch() push — milisegundos
                       ▼
┌─────────────────────────────────────────────────────┐
│  NatsKvVault (en proceso)  ←  CACHÉ EN PROCESO      │
│  DashMap<String, RotationSlot>                      │
│  resolve() → nanosegundos, sin red                  │
└──────────────────────┬──────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────────┐
│  trogon-secret-proxy worker                         │
│  vault.resolve_with_previous(token)                 │
│  → forward con current, fallback a previous si 401  │
└─────────────────────────────────────────────────────┘
```

---

## Fase 1 — `InfisicalVaultStore` (backend nuevo en trogon-vault)

**Crate:** `trogon-vault`  
**Fichero nuevo:** `crates/trogon-vault/src/backends/infisical.rs`  
**Feature flag:** `infisical` (igual que `hashicorp-vault`)

### Mapping de paths

```
tok_anthropic_prod_abc  →  proyecto: trogon  /  env: prod  /  secret: anthropic_abc
tok_openai_staging_xyz  →  proyecto: trogon  /  env: staging  /  secret: openai_xyz
```

Implementación de `VaultStore`:

```rust
pub struct InfisicalVaultStore {
    client:      reqwest::Client,
    base_url:    String,            // INFISICAL_URL, e.g. https://infisical.internal
    project_id:  String,            // INFISICAL_PROJECT_ID
    token:       String,            // INFISICAL_SERVICE_TOKEN
}

// store()   → POST /api/v3/secrets/raw/{secretName}
// resolve() → GET  /api/v3/secrets/raw/{secretName}?workspaceId=...&environment=...
// revoke()  → DELETE /api/v3/secrets/raw/{secretName}
// rotate()  → PATCH /api/v3/secrets/raw/{secretName}
```

Config desde env vars:

```
INFISICAL_URL            URL base del servidor Infisical
INFISICAL_PROJECT_ID     ID del proyecto en Infisical
INFISICAL_SERVICE_TOKEN  Service token con permisos read/write
```

**Estimado:** ~200 líneas impl + ~150 líneas tests (mockall sobre reqwest)

---

## Fase 2 — `trogon-vault-nats` (nuevo crate)

**Crate nuevo:** `rsworkspace/crates/trogon-vault-nats/`

```
Cargo.toml
src/
  lib.rs
  backend.rs   — NatsKvVault + impl VaultStore
  slot.rs      — RotationSlot { current, previous: Option<(String, Instant)> }
  crypto.rs    — AES-256-GCM encrypt/decrypt + Argon2id KDF
  bucket.rs    — ensure_vault_bucket() — provisioning idempotente
  error.rs     — NatsKvVaultError
```

### 2a — NatsKvVault core (sin crypto)

```rust
pub struct NatsKvVault {
    cache:    Arc<DashMap<String, RotationSlot>>,
    kv:       kv::Store,
    ready:    Arc<tokio::sync::Notify>,
    _watcher: tokio::task::JoinHandle<()>,
}
```

Watcher en background:

```rust
tokio::spawn(async move {
    let mut watcher = kv.watch_with_history(">").await?;
    ready.notify_waiters();  // replay inicial completado

    while let Some(Ok(entry)) = watcher.next().await {
        match entry.operation {
            Operation::Put => {
                let plaintext = crypto.decrypt(&entry.value)?;
                cache.entry(entry.key)
                    .and_modify(|slot| {
                        let old = std::mem::replace(&mut slot.current, plaintext.clone());
                        slot.previous = Some((old, Instant::now() + grace_period));
                    })
                    .or_insert(RotationSlot { current: plaintext, previous: None });
            }
            Operation::Delete | Operation::Purge => { cache.remove(&entry.key); }
        }
    }
});
```

`resolve()` es cero-latencia de red — solo lee del DashMap.

### 2b — `crypto.rs`

```rust
// Argon2id — deriva master key una vez al arranque desde VAULT_MASTER_PASSWORD
pub fn derive_key(password: &[u8], salt: &[u8; 16]) -> [u8; 32]

// AES-256-GCM — nonce aleatorio (12 bytes) por operación, prefijado al ciphertext
pub fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> Vec<u8>
pub fn decrypt(key: &[u8; 32], ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError>
```

Config desde env vars:

```
VAULT_MASTER_PASSWORD    Contraseña para derivar la master key (Argon2id)
VAULT_KEY_SALT           Salt hex de 16 bytes (fijo por deployment)
```

### 2c — Named vaults (un bucket KV por vault)

```rust
// KV_vault_prod, KV_vault_staging, KV_vault_dev
pub async fn ensure_vault_bucket(js: &JetStreamContext, vault_name: &str) -> kv::Store {
    js.create_key_value(kv::Config {
        bucket:        format!("vault_{vault_name}"),
        history:       2,
        storage:       StorageType::File,
        num_replicas:  3,     // para HA
        ..Default::default()
    }).await
}
```

Sujetos NATS extendidos (adición no-breaking en `subjects.rs`):

```
{prefix}.vault.{vault_name}.store
{prefix}.vault.{vault_name}.rotate
{prefix}.vault.{vault_name}.revoke
```

Los subjects planos existentes (`{prefix}.vault.store`, etc.) se mantienen
para compatibilidad — usan vault `"default"`.

### Dependencias nuevas (Cargo.toml del crate)

```toml
aes-gcm   = "0.10"     # MIT/Apache-2.0
argon2    = "0.5"      # MIT/Apache-2.0
dashmap   = "6"        # MIT
```

**Estimado fase 2:** ~380 líneas impl + ~350 líneas tests

---

## Fase 3 — `RotationSlot` + fallback-on-401 en worker

### Extensión del trait `VaultStore` (backward-compatible)

Fichero: `crates/trogon-vault/src/vault.rs`

```rust
// Método con implementación default — no rompe MemoryVault ni InfisicalVaultStore
fn resolve_with_previous(
    &self,
    token: &ApiKeyToken,
) -> impl Future<Output = Result<(Option<String>, Option<String>), Self::Error>> + Send {
    async { Ok((self.resolve(token).await?, None)) }
}
```

`NatsKvVault` sobreescribe este método leyendo el `RotationSlot.previous` del DashMap.

### Modificación en `worker.rs`

Fichero: `crates/trogon-secret-proxy/src/worker.rs`

En `process_request()`, después de resolver el token:

```rust
let (current, previous) = vault.resolve_with_previous(&token).await?;

let Some(api_key) = current else {
    // token desconocido — mismo comportamiento actual
    return build_error_response("token not found");
};

let resp = forward_request(&client, &req, &api_key).await?;

if resp.status() == 401 {
    if let Some(prev_key) = previous {
        tracing::warn!(
            token = token.as_str(),
            "current key rejected (401), trying previous key during rotation grace period"
        );
        return forward_request(&client, &req, &prev_key).await;
    }
}
resp
```

**Estimado fase 3:** ~50 líneas impl + ~60 líneas tests

---

## Fase 4 — Audit log JetStream

Stream NATS: `vault_audit`  
Subjects: `vault.audit.{operation}.{vault_name}`  
Retention: Limits, MaxAge configurable (default 90 días)

```rust
pub enum AuditEvent {
    Store   { token: String, vault: String, actor: String },
    Resolve { token: String, vault: String, success: bool, latency_us: u64 },
    Revoke  { token: String, vault: String, actor: String },
    Rotate  { token: String, vault: String, actor: String },
    Approve { proposal_id: String, vault: String, approver: String },
    Reject  { proposal_id: String, vault: String, approver: String },
}
```

Publicación fire-and-forget desde `NatsKvVault` y desde el approval service.
El valor de la credencial **nunca aparece** en los eventos de auditoría.

**Estimado fase 4:** ~150 líneas impl + ~100 líneas tests

---

## Fase 5 — `trogon-vault-approvals` (human-in-the-loop)

**Crate nuevo:** `rsworkspace/crates/trogon-vault-approvals/`

```
Cargo.toml
src/
  lib.rs
  proposal.rs   — Proposal, ProposalStatus, ProposalId
  service.rs    — ApprovalService (consumer + publisher NATS)
  notifier.rs   — trait Notifier + SlackNotifier (reutiliza trogon-slack)
  error.rs
```

### Subjects NATS

```
vault.proposals.{vault_name}.create        ← agente publica propuesta
vault.proposals.{vault_name}.approve       ← humano aprueba (con valor cifrado)
vault.proposals.{vault_name}.reject        ← humano rechaza
vault.proposals.{vault_name}.status.{id}  ← agente consulta estado (request-reply)
```

### Flujo completo

```
1. Agente publica en vault.proposals.{vault}.create:
   {
     "id":             "prop_abc123",
     "credential_key": "tok_stripe_prod_xyz",
     "service":        "api.stripe.com",
     "message":        "necesito acceder a Stripe para procesar pagos",
     "requested_at":   "2026-04-24T10:00:00Z"
   }

2. ApprovalService:
   a. Persiste en JetStream stream "vault_proposals" (estado: Pending)
   b. Notifica por Slack: "Agente solicita tok_stripe_prod_xyz para api.stripe.com"
      con instrucciones para aprobar/rechazar

3. Humano publica en vault.proposals.{vault}.approve:
   {
     "proposal_id":    "prop_abc123",
     "approved_by":    "mario",
     "plaintext":      "sk_live_..."     ← el humano provee el valor real
   }

4. ApprovalService:
   a. Valida que la propuesta existe y está en estado Pending
   b. Cifra el plaintext con AES-256-GCM
   c. Escribe a Infisical via InfisicalVaultStore.store()
   d. Infisical → NATS KV caché (watcher actualiza DashMap en milisegundos)
   e. Publica AuditEvent::Approve
   f. Actualiza estado en JetStream → Approved
   g. Notifica por Slack: "Propuesta prop_abc123 aprobada"

5. Agente hace request-reply a vault.proposals.{vault}.status.prop_abc123
   → { "status": "approved" }
   → puede usar la credencial via el proxy
```

### Nota de seguridad

El valor de la credencial (`plaintext`) viaja por un subject NATS dentro del
trust boundary de trogon. El ApprovalService es el único proceso que lo ve y
lo cifra inmediatamente — **nunca se persiste en JetStream en plaintext**,
solo en el mensaje puntual de aprobación que es consumido y destruido.

### `service.rs` esqueleto

```rust
pub struct ApprovalService<V: VaultStore, N: Notifier> {
    vault:    Arc<V>,
    notifier: Arc<N>,
    js:       JetStreamContext,
    nats:     Client,
}

impl<V, N> ApprovalService<V, N> {
    pub async fn run(self) -> anyhow::Result<()> {
        // Tres consumers JetStream en paralelo:
        // - proposals.*.create   → handle_create()
        // - proposals.*.approve  → handle_approve()
        // - proposals.*.reject   → handle_reject()
        // Request-reply sobre proposals.*.status.* via nats.subscribe()
        tokio::try_join!(
            self.consume_creates(),
            self.consume_approvals(),
            self.consume_rejections(),
            self.serve_status(),
        )?;
        Ok(())
    }
}
```

**Estimado fase 5:** ~500 líneas impl + ~300 líneas tests

---

## Resumen de trabajo

| Fase | Componente | Impl | Tests | Total |
|---|---|---|---|---|
| 1 | `InfisicalVaultStore` | ~200 | ~150 | ~350 |
| 2 | `trogon-vault-nats` (NatsKvVault + crypto + named vaults) | ~380 | ~350 | ~730 |
| 3 | Fallback-on-401 + `resolve_with_previous` | ~50 | ~60 | ~110 |
| 4 | Audit log JetStream | ~150 | ~100 | ~250 |
| 5 | `trogon-vault-approvals` (human-in-the-loop) | ~500 | ~300 | ~800 |
| **Total** | | **~1,280** | **~960** | **~2,240** |

---

## Dependencias nuevas en el workspace

| Crate | Versión | Licencia | Uso |
|---|---|---|---|
| `aes-gcm` | 0.10 | MIT/Apache-2.0 | AES-256-GCM cifrado |
| `argon2` | 0.5 | MIT/Apache-2.0 | KDF para master key |
| `dashmap` | 6 | MIT | Caché concurrente en proceso |

`reqwest`, `async-nats`, `tokio`, `serde_json` ya están en el workspace.

---

## Variables de entorno nuevas

| Variable | Componente | Descripción |
|---|---|---|
| `INFISICAL_URL` | InfisicalVaultStore | URL base del servidor Infisical |
| `INFISICAL_PROJECT_ID` | InfisicalVaultStore | ID del proyecto en Infisical |
| `INFISICAL_SERVICE_TOKEN` | InfisicalVaultStore | Service token con permisos read/write |
| `VAULT_MASTER_PASSWORD` | trogon-vault-nats | Contraseña para Argon2id KDF |
| `VAULT_KEY_SALT` | trogon-vault-nats | Salt hex 16 bytes (fijo por deployment) |
| `VAULT_GRACE_PERIOD_SECS` | trogon-vault-nats | Duración del grace period de rotación (default: 30) |

---

## Orden de implementación recomendado

```
Fase 1: InfisicalVaultStore
  └── Independiente, se puede testear con Infisical real o mock HTTP

Fase 2: trogon-vault-nats
  ├── 2a: NatsKvVault core sin crypto (valores en texto, tests con testcontainers NATS)
  ├── 2b: crypto.rs — AES-256-GCM + Argon2id, integrar con NatsKvVault
  └── 2c: Named vaults — parametrizar bucket, extender subjects.rs

Fase 3: Fallback-on-401
  └── Requiere RotationSlot (fase 2a) y trait extension

Fase 4: Audit log
  └── Se puede añadir al final de fase 2 o 3, es independiente

Fase 5: trogon-vault-approvals
  └── Requiere InfisicalVaultStore (fase 1) y NatsKvVault (fase 2)
      Es la parte más grande — implementar last
```

---

## Features resueltas al terminar

| Feature | Cómo |
|---|---|
| Named vaults | Un bucket KV por vault (`vault_{name}`) |
| AES-256-GCM en reposo | `crypto.rs` — NATS nunca ve plaintext |
| Argon2id KDF | Master key derivada al arranque desde `VAULT_MASTER_PASSWORD` |
| Human-in-the-loop | `trogon-vault-approvals` sobre JetStream + Slack |
| Rotación sin downtime | `RotationSlot` + fallback-on-401 en worker |
| Audit log inmutable | JetStream stream `vault_audit` |
| Source of truth durable | Infisical — probado, mantenido, self-hostable |
| Sin licencia de pago | Solo usamos CRUD de secrets (tier gratuito) |
