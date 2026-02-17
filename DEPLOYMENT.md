# TrogonAI Deployment Guide

Guía completa para deployar TrogonAI en diferentes entornos.

---

## 📋 Tabla de Contenidos

- [Deployment Local](#-deployment-local)
- [Deployment con Docker](#-deployment-con-docker)
- [Deployment en Producción](#-deployment-en-producción)
- [Monitoreo](#-monitoreo)
- [Troubleshooting](#-troubleshooting)

---

## 🖥️ Deployment Local

### Prerequisitos

1. **Rust** (1.70+)
2. **NATS Server**
3. **Telegram Bot Token**
4. **(Opcional)** Claude API Key

### Pasos

**1. Configurar variables de entorno:**
```bash
cp .env.example .env
# Edita .env y añade tu TELEGRAM_BOT_TOKEN
```

**2. Iniciar NATS:**
```bash
# Terminal 1
nats-server -js
```

**3. Iniciar Bot:**
```bash
# Terminal 2
cd rsworkspace
source ../.env
cargo run --release --package telegram-bot
```

**4. Iniciar Agent:**

**Modo Echo (sin API key):**
```bash
# Terminal 3
cd rsworkspace
source ../.env
cargo run --release --package telegram-agent
```

**Modo LLM (con Claude):**
```bash
# Terminal 3
cd rsworkspace
source ../.env
export ENABLE_LLM=true
cargo run --release --package telegram-agent
```

---

## 🐳 Deployment con Docker

### Quick Start

**1. Preparar configuración:**
```bash
cp .env.example .env
# Edita .env con tus tokens
```

**2. Modo Echo (sin LLM):**
```bash
docker-compose --profile echo up -d
```

**3. Modo LLM (con Claude):**
```bash
docker-compose --profile llm up -d
```

**4. Ver logs:**
```bash
docker-compose logs -f telegram-bot
docker-compose logs -f telegram-agent-llm
```

**5. Detener:**
```bash
docker-compose down
```

### Servicios Disponibles

```yaml
Services:
  - nats           # NATS Server (siempre activo)
  - telegram-bot   # Bot de Telegram (siempre activo)
  - telegram-agent-echo  # Agent modo echo (profile: echo)
  - telegram-agent-llm   # Agent modo LLM (profile: llm)
  - nats-surveyor  # Monitoring de NATS (profile: monitoring)
```

### Profiles

```bash
# Solo bot + agent echo
docker-compose --profile echo up -d

# Solo bot + agent LLM
docker-compose --profile llm up -d

# Con monitoring
docker-compose --profile llm --profile monitoring up -d
```

### Build Custom

```bash
# Build de imágenes
docker-compose build

# Build sin cache
docker-compose build --no-cache

# Build solo del bot
docker-compose build telegram-bot
```

---

## 🚀 Deployment en Producción

### Opción 1: Systemd (Linux)

**1. Crear servicio para NATS:**

`/etc/systemd/system/trogonai-nats.service`
```ini
[Unit]
Description=NATS Server for TrogonAI
After=network.target

[Service]
Type=simple
User=trogonai
ExecStart=/usr/local/bin/nats-server -js -c /etc/trogonai/nats.conf
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

**2. Crear servicio para Bot:**

`/etc/systemd/system/trogonai-bot.service`
```ini
[Unit]
Description=TrogonAI Telegram Bot
After=network.target trogonai-nats.service
Requires=trogonai-nats.service

[Service]
Type=simple
User=trogonai
WorkingDirectory=/opt/trogonai
EnvironmentFile=/opt/trogonai/.env
ExecStart=/opt/trogonai/bin/telegram-bot
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

**3. Crear servicio para Agent:**

`/etc/systemd/system/trogonai-agent.service`
```ini
[Unit]
Description=TrogonAI Agent
After=network.target trogonai-nats.service
Requires=trogonai-nats.service

[Service]
Type=simple
User=trogonai
WorkingDirectory=/opt/trogonai
EnvironmentFile=/opt/trogonai/.env
ExecStart=/opt/trogonai/bin/telegram-agent
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

**4. Habilitar y iniciar:**
```bash
sudo systemctl daemon-reload
sudo systemctl enable trogonai-nats trogonai-bot trogonai-agent
sudo systemctl start trogonai-nats trogonai-bot trogonai-agent
```

**5. Ver status:**
```bash
sudo systemctl status trogonai-nats
sudo systemctl status trogonai-bot
sudo systemctl status trogonai-agent
```

### Opción 2: Docker Swarm

**1. Crear docker-compose.prod.yml:**
```yaml
version: '3.8'

services:
  nats:
    image: nats:2.10-alpine
    command: ["-js", "-m", "8222"]
    volumes:
      - nats-data:/data
    deploy:
      replicas: 1
      restart_policy:
        condition: on-failure
    networks:
      - trogonai

  telegram-bot:
    image: trogonai/telegram-bot:latest
    environment:
      - TELEGRAM_BOT_TOKEN=${TELEGRAM_BOT_TOKEN}
      - NATS_URL=nats://nats:4222
    deploy:
      replicas: 2
      restart_policy:
        condition: on-failure
    depends_on:
      - nats
    networks:
      - trogonai

  telegram-agent:
    image: trogonai/telegram-agent:latest
    environment:
      - NATS_URL=nats://nats:4222
      - ENABLE_LLM=true
      - ANTHROPIC_API_KEY=${ANTHROPIC_API_KEY}
    deploy:
      replicas: 3
      restart_policy:
        condition: on-failure
    depends_on:
      - nats
    networks:
      - trogonai

networks:
  trogonai:
    driver: overlay

volumes:
  nats-data:
```

**2. Deploy en Swarm:**
```bash
docker stack deploy -c docker-compose.prod.yml trogonai
```

**3. Verificar:**
```bash
docker stack services trogonai
docker service logs trogonai_telegram-bot
```

### Opción 3: Kubernetes

**deployment.yaml**
```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: nats
spec:
  replicas: 1
  selector:
    matchLabels:
      app: nats
  template:
    metadata:
      labels:
        app: nats
    spec:
      containers:
      - name: nats
        image: nats:2.10-alpine
        args: ["-js", "-m", "8222"]
        ports:
        - containerPort: 4222
        - containerPort: 8222

---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: telegram-bot
spec:
  replicas: 2
  selector:
    matchLabels:
      app: telegram-bot
  template:
    metadata:
      labels:
        app: telegram-bot
    spec:
      containers:
      - name: telegram-bot
        image: trogonai/telegram-bot:latest
        env:
        - name: TELEGRAM_BOT_TOKEN
          valueFrom:
            secretKeyRef:
              name: trogonai-secrets
              key: bot-token
        - name: NATS_URL
          value: "nats://nats:4222"

---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: telegram-agent
spec:
  replicas: 3
  selector:
    matchLabels:
      app: telegram-agent
  template:
    metadata:
      labels:
        app: telegram-agent
    spec:
      containers:
      - name: telegram-agent
        image: trogonai/telegram-agent:latest
        env:
        - name: ENABLE_LLM
          value: "true"
        - name: ANTHROPIC_API_KEY
          valueFrom:
            secretKeyRef:
              name: trogonai-secrets
              key: anthropic-key
        - name: NATS_URL
          value: "nats://nats:4222"
```

---

## 📊 Monitoreo

### NATS Monitoring

**1. Dashboard web (puerto 7777):**
```bash
docker-compose --profile monitoring up -d
# Abre http://localhost:7777
```

**2. CLI monitoring:**
```bash
# Instalar nats CLI
curl -sf https://binaries.nats.dev/nats-io/natscli/nats@latest | sh

# Ver conexiones
nats server report connections

# Ver subscripciones
nats server report jetstream

# Ver streams
nats stream ls
```

### Application Logs

**Docker:**
```bash
docker-compose logs -f --tail=100 telegram-bot
docker-compose logs -f --tail=100 telegram-agent-llm
```

**Systemd:**
```bash
sudo journalctl -u trogonai-bot -f
sudo journalctl -u trogonai-agent -f
```

### Metrics

Los logs incluyen métricas útiles:
```
INFO telegram_bot: Received text message
INFO telegram_agent: Generated LLM response (234 tokens)
INFO telegram_agent: Active sessions: 5
```

---

## 🔧 Troubleshooting

### El bot no responde

**1. Verificar servicios:**
```bash
# Docker
docker-compose ps

# Systemd
systemctl status trogonai-*
```

**2. Ver logs:**
```bash
# Docker
docker-compose logs telegram-bot | tail -50

# Systemd
journalctl -u trogonai-bot -n 50
```

**3. Verificar NATS:**
```bash
# Probar conexión
nats server ping

# Ver suscripciones activas
nats server report connections
```

### Errores de conexión a NATS

```bash
# Verificar que NATS está corriendo
docker-compose ps nats
# o
systemctl status trogonai-nats

# Verificar puerto
netstat -tulpn | grep 4222

# Probar conexión
telnet localhost 4222
```

### Agent no genera respuestas (LLM mode)

**1. Verificar API key:**
```bash
# Debe empezar con sk-ant-
echo $ANTHROPIC_API_KEY
```

**2. Verificar logs:**
```bash
docker-compose logs telegram-agent-llm | grep -i error
```

**3. Verificar que ENABLE_LLM=true:**
```bash
docker-compose exec telegram-agent-llm env | grep ENABLE_LLM
```

### Rate limiting muy lento

Esto es **normal** - Telegram limita a 1 edición por segundo.

Si es un problema, ajusta la estrategia en `outbound_streaming.rs`:
```rust
const MIN_EDIT_INTERVAL: Duration = Duration::from_millis(1000);
```

---

## 🔐 Seguridad

### Best Practices

1. **Nunca commitear tokens:**
   ```bash
   # Agregar a .gitignore
   .env
   config/telegram-bot.toml
   ```

2. **Usar secrets management:**
   ```bash
   # Docker Swarm
   docker secret create bot_token ./bot_token.txt

   # Kubernetes
   kubectl create secret generic trogonai-secrets \
     --from-literal=bot-token=YOUR_TOKEN \
     --from-literal=anthropic-key=YOUR_KEY
   ```

3. **Access control:**
   - Configurar whitelist en `telegram-bot.toml`
   - Limitar grupos permitidos
   - Definir admins

4. **NATS security:**
   ```bash
   # Generar credentials
   nats server generate-credentials

   # Usar en config
   credentials_file = "/path/to/nats.creds"
   ```

---

## 📈 Scaling

### Horizontal Scaling

**Bot:**
- Múltiples instancias OK
- Cada instancia procesa diferentes updates

**Agent:**
- Múltiples instancias OK
- NATS distribuye mensajes (round-robin)

**NATS:**
- Cluster para HA
- JetStream para persistencia

### Recomendaciones

```
Carga baja:    1 bot + 1 agent + 1 NATS
Carga media:   2 bots + 3 agents + 1 NATS
Carga alta:    3 bots + 5 agents + 3 NATS (cluster)
```

---

## 🎯 Checklist de Deploy

- [ ] Configurar `.env` con tokens
- [ ] Verificar NATS está corriendo
- [ ] Build de binarios/imágenes
- [ ] Iniciar servicios
- [ ] Verificar logs (sin errores)
- [ ] Probar comando `/start` en Telegram
- [ ] Enviar mensaje de prueba
- [ ] Verificar respuesta del bot
- [ ] Configurar monitoreo
- [ ] Configurar backups (NATS data)

---

¡Deployment exitoso! 🚀
