# Secret Management con NATS — Investigación y Diseño

## Qué es Secret Management

Secret Management es el conjunto de prácticas y herramientas para almacenar,
distribuir, rotar y revocar credenciales sensibles (contraseñas, API keys,
certificados, tokens) de forma segura en un sistema de software.

Sin secret management, los secretos terminan hardcodeados o en variables de
entorno estáticas:

```bash
ANTHROPIC_API_KEY=sk-ant-realkey123  # en .env o en el proceso
```

Esto tiene varios problemas:

- Si alguien ve el proceso o el archivo, tiene la clave
- Para cambiar la clave hay que reiniciar el servicio (downtime)
- No hay historial de quién accedió ni cuándo
- Si se filtra, no hay forma rápida de invalidarla sin romper todo

---

## Las dos credenciales del gateway proxy

El sistema distingue dos tipos de credenciales con roles completamente distintos.

### `tok_anthropic_prod_abc` — el alias (token proxy)

- Lo tiene el **servicio** que quiere llamar a Anthropic
- Es un identificador opaco — no es una clave real, no sirve fuera del gateway
- Se puede guardar en config, incluso perder — solo funciona a través del proxy
- **Nunca viaja a Anthropic**

### `sk-ant-realkey` — la clave real

- La tiene **solo el worker** del gateway
- Es la credencial que Anthropic reconoce
- Es la que se rota cuando hay un incidente
- **Nunca sale del worker** — ni hacia el servicio, ni hacia los logs del caller

### El intercambio en el momento de la request

```
Servicio                 Worker                    Anthropic
   │                       │                           │
   │  Authorization:        │                           │
   │  Bearer tok_abc  ────▶ │                           │
   │                        │  vault.resolve(tok_abc)   │
   │                        │  = "sk-ant-realkey"       │
   │                        │                           │
   │                        │  Authorization:           │
   │                        │  Bearer sk-ant-realkey ──▶│
   │                        │                           │
   │   200           ◀──────│                    200 ◀──│
```

El servicio mandó `tok_abc`, Anthropic recibió `sk-ant-realkey`. El servicio
nunca supo cuál era la clave real.

---

## Qué es rotar un secreto

**Rotar** significa reemplazar una credencial vieja por una nueva sin
interrumpir el servicio. Es como cambiar la cerradura de una puerta mientras
el edificio sigue abierto: si repartes la nueva llave antes de quitar la
vieja, nadie se queda sin entrar.

### Por qué se rota

- La clave se filtró (alguien la vio que no debía)
- Política de seguridad: rotar cada 90 días aunque no haya incidente
- El empleado que la sabía se fue de la empresa
- El proveedor la deprecó y emitió una nueva

### Sin rotación dinámica (situación actual)

```
T+0    Descubren que sk-ant-v1 se filtró
T+1    Operador cambia la env var del worker
T+2    Reinicia el worker
T+3    Servicio vuelve a funcionar

T+1 → T+3  ← TODO FALLA (downtime)
           ← Mientras tanto el atacante sigue usando sk-ant-v1
```

### Con rotación dinámica y grace period

```
T+0    Se genera sk-ant-v2 en Anthropic
       Se escribe v2 al vault
       v1 queda como "anterior" con un timer de 60s

T+0 → T+60s
       Requests nuevos   → worker usa v2  ✓
       Requests en vuelo → worker termina con v1  ✓
       Si v2 falla 401   → cae back a v1  ✓

T+60s  v1 revocada en Anthropic. Solo v2 válida.
       Los servicios no notaron nada.
       tok_abc sigue funcionando igual.
```

---

## Por qué el gateway es el lugar correcto para gestionar secretos

Si los servicios tuvieran las claves reales directamente, habría que
notificar a cada servicio cuando se rota. Con docenas de servicios eso es
inmanejable.

```
Sin gateway:
  Servicio A  ──  sk-ant-v1  ──▶  Anthropic   ← hay que avisar a A
  Servicio B  ──  sk-ant-v1  ──▶  Anthropic   ← hay que avisar a B
  Servicio C  ──  sk-ant-v1  ──▶  Anthropic   ← hay que avisar a C

Con gateway:
  Servicio A  ──  tok_abc  ──┐
  Servicio B  ──  tok_abc  ──┤──▶  Worker  ──  sk-ant-v2  ──▶  Anthropic
  Servicio C  ──  tok_abc  ──┘         ↑
                                   solo aquí se rota
```

El token proxy es estable. La clave real es efímera y rotable. Esa separación
es todo el valor del sistema. Es exactamente lo que hace VGS (Very Good
Security) en producción.

---

## Limitación actual del worker

El worker carga los secretos una sola vez al arrancar desde `VAULT_TOKEN_*`
env vars hacia un `MemoryVault` (`Arc<Mutex<HashMap>>`). No existe ningún
mecanismo de actualización en caliente.

```
Arranque:  VAULT_TOKEN_tok_abc=sk-ant-v1  →  HashMap fijo
           no hay forma de actualizar sin reiniciar
```

---

## NATS KV como backend de secretos

NATS JetStream incluye un **Key-Value store** (`async_nats::jetstream::kv`).
No es una infraestructura separada — es una abstracción sobre un stream
JetStream interno (`KV_<bucket>`). Ya está en el stack.

### API Rust (`async-nats 0.45`)

```rust
// Rotation coordinator — escribe la nueva clave
kv.put("tok_anthropic_prod_abc123", Bytes::from("sk-ant-v2")).await?;

// Worker al arrancar — lectura inicial
let entry = kv.entry("tok_anthropic_prod_abc123").await?;

// Worker en background — push en tiempo real
let mut watcher = kv.watch_all().await?;
while let Some(Ok(entry)) = watcher.next().await {
    match entry.operation {
        Operation::Put    => { cache.insert(entry.key, entry.value); }
        Operation::Delete => { cache.remove(&entry.key); }
        Operation::Purge  => { cache.remove(&entry.key); }
    }
}
```

### Configuración del bucket para secretos

```rust
kv::Config {
    bucket:   "secrets".to_string(),
    history:  2,          // current + 1 anterior (habilita grace period)
    max_age:  Duration::ZERO,  // 0 = sin expiración (claves de larga duración)
    storage:  StorageType::File,
    ..Default::default()
}
```

### Propiedades relevantes

| Propiedad | Valor recomendado | Por qué |
|---|---|---|
| `history` | 2 | Retiene current + 1 anterior, habilita grace period nativo |
| `max_age` | 0 (sin TTL) | Las API keys no expiran solas |
| Cifrado en reposo | `cipher: chachapoly` + `JS_KEY` | AES/ChaCha20-Poly1305 a nivel servidor |
| Subject ACLs | Workers: read-only `$KV.secrets.tok_*` | Workers no pueden escribir ni leer otros buckets |
| `num_replicas` | ≥ 3 en producción | Alta disponibilidad |

---

## Arquitectura propuesta

```
┌─────────────────────────┐
│  Rotation Coordinator   │  (servicio externo o script de operador)
│  1. genera v2 en Anthropic
│  2. kv.put(token, v2)   │
│  3. espera grace period │
│  4. revoca v1 en Anthropic
└──────────┬──────────────┘
           │ kv.put("tok_anthropic_prod_abc", "sk-ant-v2")
           ▼
┌──────────────────────────────┐  history=2
│  NATS KV Bucket: "secrets"   │  revision N-1: sk-ant-v1
│                              │  revision N:   sk-ant-v2
└──────────┬───────────────────┘
           │ watch() push — latencia de milisegundos
           ▼
┌──────────────────────────────────────────┐
│  Worker  (NatsKvVault backend)           │
│                                          │
│  DashMap<String, RotationSlot>           │
│    "tok_anthropic_prod_abc" → {          │
│      current:  "sk-ant-v2",             │
│      previous: Some(("sk-ant-v1",        │
│                 expires_at: T+60s))      │
│    }                                     │
│                                          │
│  background watcher task (tokio::spawn)  │
└──────────────────────────────────────────┘
           │
           │ vault.resolve_with_previous(tok_abc)
           │ = (Some("sk-ant-v2"), Some("sk-ant-v1"))
           ▼
     forward a Anthropic con sk-ant-v2
     si 401 → reintenta con sk-ant-v1 (grace period)
```

El worker resuelve tokens desde un `DashMap` en proceso — **cero latencia
NATS por request**. El watcher corre en background.

---

## Evolución del trait `VaultStore`

### Trait actual

```rust
pub trait VaultStore: Send + Sync {
    type Error: std::error::Error + Send + Sync;
    fn store(&self, token: &ApiKeyToken, plaintext: &str)
        -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn resolve(&self, token: &ApiKeyToken)
        -> impl Future<Output = Result<Option<String>, Self::Error>> + Send;
    fn revoke(&self, token: &ApiKeyToken)
        -> impl Future<Output = Result<(), Self::Error>> + Send;
}
```

### Adiciones propuestas

```rust
pub trait VaultStore: Send + Sync {
    type Error: std::error::Error + Send + Sync;

    // — existentes —
    fn store(&self, token: &ApiKeyToken, plaintext: &str)
        -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn resolve(&self, token: &ApiKeyToken)
        -> impl Future<Output = Result<Option<String>, Self::Error>> + Send;
    fn revoke(&self, token: &ApiKeyToken)
        -> impl Future<Output = Result<(), Self::Error>> + Send;

    // — nuevos —

    /// Establece new_key como current y mueve el current anterior
    /// al slot "previous" con un TTL de grace_period.
    fn rotate(&self, token: &ApiKeyToken, new_key: &str)
        -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Retorna (current_key, Option<previous_key_en_grace_period>).
    /// El worker usa esto para caer back a la clave anterior si el
    /// proveedor rechaza la nueva durante la propagación.
    fn resolve_with_previous(&self, token: &ApiKeyToken)
        -> impl Future<Output = Result<(Option<String>, Option<String>), Self::Error>> + Send;
}
```

La **lógica de fallback-on-401 vive en `worker.rs`**, no en el trait — el
trait es un key store puro:

```rust
// worker.rs — process_request (propuesto)
let (current_key, previous_key) = vault.resolve_with_previous(&token).await?;

let resp = forward_request(..., &current_key).await;
if resp.status == 401 {
    if let Some(prev) = previous_key {
        tracing::warn!("current key 401 — falling back to previous (rotation in progress)");
        return forward_request(..., &prev).await;
    }
}
```

---

## Nuevo crate: `trogon-vault-nats`

```
rsworkspace/crates/trogon-vault-nats/
  Cargo.toml          — deps: trogon-vault, async-nats, dashmap, tokio
  src/lib.rs
  src/backend.rs      — NatsKvVault + impl VaultStore
  src/slot.rs         — RotationSlot { current, previous: Option<(String, Instant)> }
  src/error.rs        — NatsKvVaultError
```

`MemoryVault` se mantiene para tests. `NatsKvVault` es el backend de
producción.

### Sketch de `NatsKvVault`

```rust
pub struct NatsKvVault {
    cache:    Arc<DashMap<String, RotationSlot>>,
    _watcher: tokio::task::JoinHandle<()>,
}

#[derive(Clone)]
struct RotationSlot {
    current:  String,
    previous: Option<(String, std::time::Instant)>, // (key, expires_at)
}

impl NatsKvVault {
    pub async fn new(kv: kv::Store, grace: Duration) -> Self {
        let cache = Arc::new(DashMap::new());
        let cache2 = cache.clone();

        let _watcher = tokio::spawn(async move {
            // watch_all() replaya los valores actuales al conectar,
            // luego hace push de cada cambio nuevo.
            let mut watcher = kv.watch_all().await.expect("KV watch failed");
            while let Some(Ok(entry)) = watcher.next().await {
                match entry.operation {
                    Operation::Put => {
                        let new_key = String::from_utf8_lossy(&entry.value).to_string();
                        cache2.entry(entry.key).and_modify(|slot| {
                            let old = slot.current.clone();
                            slot.previous = Some((old, Instant::now() + grace));
                            slot.current  = new_key.clone();
                        }).or_insert(RotationSlot {
                            current:  new_key,
                            previous: None,
                        });
                    }
                    Operation::Delete | Operation::Purge => {
                        cache2.remove(&entry.key);
                    }
                }
            }
        });

        Self { cache, _watcher }
    }
}
```

---

## Patrón de rotación paso a paso

```
1. Rotation coordinator genera sk-ant-v2 en la API de Anthropic

2. Escribe al KV bucket:
      kv.put("tok_anthropic_prod_abc123", "sk-ant-v2")
   El bucket (history=2) ahora tiene:
      revision N-1: sk-ant-v1
      revision N:   sk-ant-v2

3. Workers reciben el evento via watch() en milisegundos.
   Cada worker actualiza su DashMap:
      current  = sk-ant-v2
      previous = sk-ant-v1 (expira en T+grace_period)

4. Grace period (e.g. 60s):
   - Requests nuevos usan sk-ant-v2  ✓
   - Requests en vuelo con sk-ant-v1 terminan  ✓
   - Si sk-ant-v2 retorna 401 (lag de propagación) → fallback a sk-ant-v1  ✓

5. Pasado el grace period:
   - previous slot expira (Instant::now() > expires_at)
   - Rotation coordinator revoca sk-ant-v1 en Anthropic

6. Solo sk-ant-v2 válida. Rotación completa. Cero downtime.
```

---

## Comparación: NATS KV vs alternativas

| | NATS KV | HashiCorp Vault | AWS Secrets Manager |
|---|---|---|---|
| **Ya en el stack** | ✅ | ❌ | ❌ |
| **Cliente Rust** | First-party `async-nats` | Community `vaultrs` | AWS SDK |
| **Actualizaciones push** | ✅ `watch()` | Agent polling | EventBridge |
| **Rotación automática** | ❌ hay que implementar | ✅ built-in engines | ✅ Lambda |
| **Audit log** | ❌ | ✅ | CloudTrail |
| **Secretos dinámicos** | ❌ | ✅ (DB, PKI, etc.) | ✅ (AWS only) |
| **Cifrado por clave** | ❌ global | ✅ | ✅ KMS |
| **Coste** | Solo infraestructura | HCP o self-hosted | ~$0.40/secreto/mes |
| **Historial de versiones** | ✅ hasta 64 revisiones | ✅ | ✅ |

**Recomendación:** NATS KV es la elección correcta para este stack ahora —
sin infraestructura nueva, API Rust de primera clase, notificación push en
tiempo real para propagación de rotaciones. HashiCorp Vault vale la pena
considerar si el sistema crece y requiere rotación automática, audit log de
compliance, o generación de secretos dinámicos (credenciales de base de datos
efímeras, certificados PKI).

---

## Consideraciones de seguridad

### Cifrado en reposo

Configurar en el servidor NATS:

```
jetstream {
    store_dir: /var/nats/store
    cipher: chachapoly
    key: $JS_KEY
}
```

Es cifrado global — no por bucket. Para mayor seguridad, cifrar el valor a
nivel de aplicación antes de escribir al KV (application-layer encryption).

### Subject ACLs

- Workers: permiso de **solo lectura** en `$KV.secrets.tok_*`
- Rotation coordinator: permiso de **solo escritura** en `$KV.secrets.*`
- Ningún componente tiene acceso a los demás buckets

### Sin audit log nativo

NATS KV no registra quién leyó un secreto. Si se requiere auditoría, añadir
un shim en el vault backend que publique un evento a un subject de auditoría
en cada `resolve()` call (async, fire-and-forget).

---

## Qué hay que construir

| Componente | Descripción |
|---|---|
| `trogon-vault-nats` | Nuevo crate: `NatsKvVault` con watcher background y `DashMap` cache |
| `VaultStore` trait | Añadir `rotate()` y `resolve_with_previous()` |
| `MemoryVault` | Implementar las nuevas funciones del trait (para tests) |
| `worker.rs` | Lógica de fallback-on-401: intentar current, si 401 intentar previous |
| `ensure_kv_bucket()` | Análogo a `ensure_stream()` — provisiona el bucket al arrancar |
| Rotation coordinator | Servicio o script que: genera v2 → escribe KV → espera grace → revoca v1 |
| Startup order | Worker espera a que el watcher haga replay inicial antes de consumir JetStream |

---

## Fuentes

- [NATS Key/Value Store — NATS Docs](https://docs.nats.io/nats-concepts/jetstream/key-value-store)
- [NATS KV ADR-8](https://github.com/nats-io/nats-architecture-and-design/blob/main/adr/ADR-8.md)
- [async_nats::jetstream::kv — Rust Docs](https://docs.rs/async-nats/latest/async_nats/jetstream/index.html)
- [NATS by Example — Key-Value Intro (Rust)](https://natsbyexample.com/examples/kv/intro/rust)
- [Encryption at Rest — NATS Docs](https://docs.nats.io/running-a-nats-service/nats_admin/jetstream_admin/encryption_at_rest)
- [Zero Downtime Secrets Rotation — Doppler](https://www.doppler.com/blog/10-step-secrets-rotation-guide)
- [API Key Rotation Best Practices — GitGuardian](https://blog.gitguardian.com/api-key-rotation-best-practices/)
- [AWS Secrets Manager vs HashiCorp Vault — Infisical](https://infisical.com/blog/aws-secrets-manager-vs-hashicorp-vault)
- [wasmCloud Secrets with NATS KV](https://wasmcloud.com/blog/2024-12-17-wasmclouds-fresh-approach-to-secrets/)
- [VGS Vault Technology](https://docs.verygoodsecurity.com/vault/technology/vault)
