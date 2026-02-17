# CI/CD & Monitoring

Documentación completa del pipeline de CI/CD y sistema de monitoreo.

---

## 📋 Tabla de Contenidos

- [GitHub Actions](#-github-actions)
- [Health Checks](#-health-checks)
- [Metrics](#-metrics)
- [Monitoring](#-monitoring)
- [Release Process](#-release-process)

---

## 🔄 GitHub Actions

### Workflows Configurados

#### 1. **CI Workflow** (`.github/workflows/ci.yml`)

Se ejecuta en cada push y pull request.

**Jobs:**

**Formatting Check**
```yaml
- Verifica formato del código con rustfmt
- Falla si hay código sin formatear
- Comando: cargo fmt --check
```

**Clippy Lints**
```yaml
- Ejecuta linter de Rust (clippy)
- Configurado con -D warnings (falla en warnings)
- Cachea dependencias para velocidad
```

**Test Suite**
```yaml
- Ejecuta todos los tests del workspace
- Tests unitarios + integración
- Corre en Ubuntu latest
```

**Build**
```yaml
- Compila en Ubuntu y macOS
- Build release con optimizaciones
- Sube binarios como artifacts
- Matrix strategy para múltiples OS
```

**Security Audit**
```yaml
- Ejecuta cargo-audit
- Busca vulnerabilidades en dependencias
- Verifica CVEs conocidos
```

**Docker Build**
```yaml
- Testea build de imágenes Docker
- telegram-bot y telegram-agent
- Usa BuildKit para cache
- No hace push (solo test)
```

#### 2. **Release Workflow** (`.github/workflows/release.yml`)

Se ejecuta cuando se crea un tag `v*` (ej: v0.1.0).

**Jobs:**

**Create Release**
```yaml
- Crea GitHub release automáticamente
- Genera release notes
```

**Build Release Binaries**
```yaml
- Compila para múltiples plataformas:
  - Linux x86_64
  - macOS x86_64
  - macOS ARM64 (Apple Silicon)
- Empaqueta en .tar.gz
- Sube a GitHub release
```

**Docker Release**
```yaml
- Build multi-architecture (amd64, arm64)
- Push a Docker Hub
- Tags: latest y versión específica
- Ejemplo: trogonai/telegram-bot:0.1.0
```

### Triggers

```yaml
# CI ejecuta en:
on:
  push:
    branches: [ main, telegram, develop ]
  pull_request:
    branches: [ main, telegram ]

# Release ejecuta en:
on:
  push:
    tags:
      - 'v*'
```

### Cacheo

Para velocidad, se cachean:
- Cargo registry
- Cargo git dependencies
- Build artifacts (target/)
- Docker layers

**Resultado:** Builds subsecuentes ~3-5x más rápidos

---

## 🏥 Health Checks

### Endpoints Disponibles

El bot expone endpoints HTTP para health checks:

#### `/health` - Estado general
```json
{
  "status": "healthy",
  "uptime_seconds": 3600,
  "nats_connected": true,
  "bot_username": "my_bot"
}
```

**Status Codes:**
- `200 OK` - Sistema saludable
- `503 Service Unavailable` - NATS desconectado

#### `/ready` - Readiness probe
```
GET /ready
```

**Retorna:**
- `200 OK` - Listo para recibir tráfico
- `503` - No listo (NATS desconectado)

**Uso:** Kubernetes readiness probe

#### `/live` - Liveness probe
```
GET /live
```

**Retorna:**
- `200 OK` - Proceso vivo

**Uso:** Kubernetes liveness probe

#### `/metrics` - Métricas de aplicación
```json
{
  "messages_received": 1234,
  "messages_sent": 1200,
  "commands_processed": 456,
  "active_sessions": 12,
  "errors": 5
}
```

### Configuración

**Puerto por defecto:** `3000`

```bash
# Cambiar puerto
export HEALTH_CHECK_PORT=8080
```

**Habilitar health checks:**
```bash
# Ya está habilitado por defecto
cargo run --package telegram-bot
```

**Verificar:**
```bash
curl http://localhost:3000/health
curl http://localhost:3000/metrics
```

---

## 📊 Metrics

### Métricas Recolectadas

**Contador de Mensajes**
- `messages_received` - Mensajes recibidos de Telegram
- `messages_sent` - Mensajes enviados a Telegram
- `commands_processed` - Comandos ejecutados (/start, /help, etc.)

**Estado de Sistema**
- `active_sessions` - Sesiones activas concurrentes
- `uptime_seconds` - Tiempo desde inicio

**Errores**
- `errors` - Total de errores encontrados

### Integración con Prometheus

**Ejemplo de scrape config:**

```yaml
# prometheus.yml
scrape_configs:
  - job_name: 'trogonai-bot'
    static_configs:
      - targets: ['localhost:3000']
    metrics_path: '/metrics'
    scrape_interval: 15s
```

### Dashboards

**Métricas clave para monitorear:**

1. **Throughput**
   - Messages/second recibidos
   - Messages/second enviados
   - Command rate

2. **Latency**
   - Response time (agregar en futuro)
   - LLM API latency

3. **Errors**
   - Error rate
   - Error types

4. **Resources**
   - Active sessions
   - Memory usage
   - CPU usage

---

## 🔍 Monitoring

### Logs Estructurados

Todos los componentes usan `tracing` para logs:

```rust
use tracing::{info, error, debug, warn};

info!("Received message from chat {}", chat_id);
debug!("Processing with {} tokens", token_count);
error!("Failed to send message: {}", error);
```

**Niveles de log:**
```bash
# Development
export RUST_LOG=telegram_bot=debug,telegram_agent=debug,info

# Production
export RUST_LOG=telegram_bot=info,telegram_agent=info,warn

# Detailed
export RUST_LOG=trace
```

### Recolección de Logs

**Docker:**
```bash
# Ver logs en tiempo real
docker-compose logs -f

# Exportar logs
docker-compose logs --since 1h > logs.txt
```

**Systemd:**
```bash
# Ver logs
journalctl -u trogonai-bot -f

# Últimas 100 líneas
journalctl -u trogonai-bot -n 100
```

**Integración con ELK/Grafana Loki:**

```yaml
# docker-compose.yml para Loki
version: '3.8'
services:
  loki:
    image: grafana/loki:latest
    ports:
      - "3100:3100"

  promtail:
    image: grafana/promtail:latest
    volumes:
      - /var/log:/var/log
      - ./promtail-config.yml:/etc/promtail/config.yml

  grafana:
    image: grafana/grafana:latest
    ports:
      - "3001:3000"
```

### Alerting

**Alertas recomendadas:**

1. **NATS Desconectado**
   ```yaml
   alert: NATSDisconnected
   expr: nats_connected == 0
   for: 1m
   ```

2. **Error Rate Alto**
   ```yaml
   alert: HighErrorRate
   expr: rate(errors[5m]) > 10
   for: 5m
   ```

3. **Sin Mensajes**
   ```yaml
   alert: NoMessagesReceived
   expr: rate(messages_received[10m]) == 0
   for: 10m
   ```

### NATS Monitoring

**NATS Dashboard:**
```bash
# Con docker-compose
docker-compose --profile monitoring up -d

# Acceder a: http://localhost:7777
```

**CLI Monitoring:**
```bash
# Instalar NATS CLI
curl -sf https://binaries.nats.dev/nats-io/natscli/nats@latest | sh

# Ver stats
nats server report jetstream
nats server report connections
nats stream ls
nats consumer ls
```

---

## 🚀 Release Process

### Crear un Release

**1. Preparar release:**
```bash
# Asegurar que todo está committed
git status

# Actualizar version en Cargo.toml
# Actualizar CHANGELOG.md
```

**2. Crear tag:**
```bash
# Tag con version
git tag -a v0.1.0 -m "Release v0.1.0"

# Push tag
git push origin v0.1.0
```

**3. GitHub Actions automáticamente:**
- Ejecuta CI completo
- Compila binarios para todas las plataformas
- Crea GitHub Release
- Sube binarios a release
- Build y push de imágenes Docker
- Publica a Docker Hub

**4. Verificar:**
```bash
# Ver release en GitHub
https://github.com/TrogonStack/trogonai/releases

# Pull imagen Docker
docker pull trogonai/telegram-bot:0.1.0
docker pull trogonai/telegram-agent:0.1.0
```

### Versionado

Seguimos **Semantic Versioning** (SemVer):

```
v{MAJOR}.{MINOR}.{PATCH}

v1.0.0 - Release inicial
v1.1.0 - Nuevas features (compatible)
v1.1.1 - Bug fixes
v2.0.0 - Breaking changes
```

### Pre-releases

Para beta/RC:
```bash
git tag -a v0.1.0-beta.1 -m "Beta 1"
git push origin v0.1.0-beta.1
```

Esto crea una **pre-release** en GitHub.

---

## 🔐 Secrets Requeridos

Para que funcionen los workflows, configurar en GitHub:

**Settings → Secrets → Actions:**

```
DOCKER_USERNAME  - Tu usuario de Docker Hub
DOCKER_PASSWORD  - Token de Docker Hub
```

**Cómo crear Docker Hub token:**
1. Login en hub.docker.com
2. Account Settings → Security
3. New Access Token
4. Copiar y agregar como secret

---

## 📈 Métricas de CI/CD

**Tiempos típicos:**

```
Formatting:     ~30 segundos
Clippy:         ~2 minutos (con cache)
Tests:          ~3 minutos
Build:          ~5 minutos por OS
Docker Build:   ~10 minutos
Total CI:       ~15 minutos
Release Build:  ~25 minutos
```

**Con cache habilitado:**
- Primera ejecución: ~15 min
- Subsecuentes: ~5-7 min

---

## 🛠️ Troubleshooting CI/CD

### Build falla en CI pero pasa local

**Causa común:** Warnings tratados como errors en CI

**Solución:**
```bash
# Ejecutar localmente con flags de CI
cargo clippy --all-targets -- -D warnings
```

### Tests fallan en CI

**Verificar:**
```bash
# Ejecutar todos los tests como en CI
cargo test --workspace --all-features
```

### Docker build lento

**Mejoras:**
1. Usar cache de GitHub Actions (ya configurado)
2. Optimizar Dockerfile con multi-stage
3. Build solo cuando cambia código

### Release no se crea

**Verificar:**
1. Tag tiene formato `v*` (v0.1.0)
2. Secrets de Docker configurados
3. Ver logs del workflow en GitHub

---

## ✅ Checklist de CI/CD

- [ ] Tests pasan localmente
- [ ] Código formateado (`make format`)
- [ ] Clippy sin warnings (`make clippy`)
- [ ] Docker build funciona (`make docker-build`)
- [ ] Health checks responden
- [ ] Secrets configurados en GitHub
- [ ] Tag creado con versionado correcto
- [ ] Release notes actualizados

---

¡CI/CD completo y automatizado! 🚀
