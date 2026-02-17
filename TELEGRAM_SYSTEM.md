# TrogonAI Telegram Integration

Sistema completo de mensajería inteligente para Telegram que conecta bots con agentes de IA a través de NATS, con soporte para streaming de respuestas en tiempo real.

---

## 📖 Tabla de Contenidos

- [¿Qué es?](#-qué-es)
- [Arquitectura](#️-arquitectura)
- [Componentes](#-componentes)
- [Flujo de Conversación](#-flujo-de-conversación-real)
- [Cómo Usar](#-cómo-usar)
- [NATS Subjects](#-nats-subjects)
- [Features](#-features-implementadas)
- [Ejemplos](#-ejemplos)
- [Troubleshooting](#️-troubleshooting)

---

## 🎯 ¿Qué es?

**TrogonAI Telegram Integration** es una plataforma que permite crear bots de Telegram potenciados por IA (como Claude) con una arquitectura desacoplada y escalable.

### ¿Por qué usar esto?

En lugar de que el bot procese directamente los mensajes de Telegram, se comunica con agentes de IA a través de NATS (un sistema de mensajería). Esto permite:

| Ventaja | Beneficio |
|---------|-----------|
| **Escalabilidad** | Múltiples agentes pueden procesar mensajes en paralelo |
| **Flexibilidad** | Fácil agregar nuevos tipos de agentes o funcionalidades |
| **Confiabilidad** | Si un agente falla, el bot sigue funcionando |
| **Streaming** | Respuestas progresivas como ChatGPT (el texto aparece mientras se genera) |
| **Auditabilidad** | Todos los eventos quedan registrados en NATS |

---

## 🏗️ Arquitectura

### Diagrama de Flujo

```
┌─────────────┐
│   Usuario   │
│  (Telegram) │
└──────┬──────┘
       │ "¿Qué es Rust?"
       ▼
┌─────────────────┐
│  Telegram Bot   │ ◄─ Recibe mensajes de Telegram
│  (telegram-bot) │    Convierte a eventos NATS
└────────┬────────┘
         │ Evento: MessageTextEvent
         ▼
    ┌────────┐
    │  NATS  │ ◄─ Sistema de mensajería (broker)
    └───┬────┘    Distribuye eventos a suscriptores
        │
        ▼
┌──────────────────┐
│  Telegram Agent  │ ◄─ Procesa con IA (Claude)
│ (telegram-agent) │    Genera respuesta inteligente
└────────┬─────────┘
         │ Comando: SendMessageCommand
         ▼
    ┌────────┐
    │  NATS  │
    └───┬────┘
        │
        ▼
┌─────────────────┐
│  Telegram Bot   │ ◄─ Recibe comando
│  (telegram-bot) │    Envía a Telegram API
└────────┬────────┘
         │ "Rust es un lenguaje..."
         ▼
┌─────────────┐
│   Usuario   │ ◄─ Ve la respuesta
└─────────────┘
```

### Componentes del Sistema

```
┌─────────────────────────────────────────────────────┐
│                  TROGONAI PLATFORM                   │
├─────────────────────────────────────────────────────┤
│                                                       │
│  ┌─────────────┐     ┌──────────┐     ┌──────────┐ │
│  │telegram-    │────▶│  NATS    │◀────│telegram- │ │
│  │bot          │     │  Server  │     │agent     │ │
│  │             │     │          │     │          │ │
│  │ • Telegram  │     │ •Events  │     │ •LLM     │ │
│  │   API       │     │ •Commands│     │ •Logic   │ │
│  │ • Handlers  │     │ •Streams │     │ •Context │ │
│  └─────────────┘     └──────────┘     └──────────┘ │
│         ▲                                      │     │
│         │                                      │     │
│         └──────────────────────────────────────┘     │
│                 Comunicación bidireccional           │
│                                                       │
└─────────────────────────────────────────────────────┘
         ▲                               ▲
         │                               │
    Telegram API                   Claude API
```

---

## 📦 Componentes

### 1. **telegram-types**
*Biblioteca de tipos compartidos*

Define todas las estructuras de datos que usan los demás componentes.

**¿Qué incluye?**
- **Eventos**: Datos que van de Telegram hacia agentes
- **Comandos**: Instrucciones que van de agentes hacia Telegram
- **Tipos comunes**: Chat, User, Message, etc.
- **Configuración**: AccessConfig, SessionId, etc.

**Ejemplo:**
```rust
// Evento cuando un usuario envía texto
MessageTextEvent {
    metadata: EventMetadata {
        event_id: "uuid-v4",
        session_id: "tg-private-123456",
        timestamp: "2024-02-16T20:00:00Z",
        update_id: 123456789
    },
    message: Message {
        message_id: 42,
        chat: Chat { id: 123456, type: "private" },
        from: User { id: 123456, username: "johndoe" },
        text: "Hola bot!"
    },
    text: "Hola bot!"
}

// Comando para enviar un mensaje
SendMessageCommand {
    chat_id: 123456,
    text: "¡Hola! ¿Cómo estás?",
    parse_mode: Some(Markdown),
    reply_to_message_id: Some(42),
    reply_markup: None
}
```

---

### 2. **telegram-nats**
*Biblioteca de comunicación NATS*

Facilita la conexión y comunicación a través de NATS.

**¿Qué hace?**
- Conecta a NATS server
- Publica eventos y comandos
- Se suscribe a subjects
- Maneja serialización JSON automáticamente
- Gestiona sesiones con JetStream KV

**Ejemplo de uso:**
```rust
// Publicar un evento
let publisher = MessagePublisher::new(client, "prod");
publisher.publish(
    "telegram.prod.bot.message.text",
    &event
).await?;

// Suscribirse a eventos
let subscriber = MessageSubscriber::new(client, "prod");
let mut stream = subscriber
    .subscribe::<MessageTextEvent>("telegram.prod.bot.message.text")
    .await?;

while let Some(Ok(event)) = stream.next().await {
    println!("Recibí: {}", event.text);
}
```

---

### 3. **telegram-bot**
*Bot de Telegram*

Aplicación que conecta con Telegram y convierte mensajes en eventos NATS.

**¿Qué recibe de Telegram?**
- ✅ Mensajes de texto
- ✅ Fotos
- ✅ Videos
- ✅ Audios
- ✅ Documentos
- ✅ Mensajes de voz
- ✅ Clicks en botones (callbacks)
- ✅ Comandos (/start, /help, etc.)

**¿Qué publica a NATS?**
```
telegram.prod.bot.message.text      → Mensaje de texto recibido
telegram.prod.bot.message.photo     → Foto recibida
telegram.prod.bot.message.video     → Video recibido
telegram.prod.bot.callback.query    → Usuario clickeó un botón
telegram.prod.bot.command.start     → Usuario envió /start
```

**¿Qué recibe de NATS (comandos)?**
```
telegram.prod.agent.message.send        → Enviar mensaje
telegram.prod.agent.message.edit        → Editar mensaje
telegram.prod.agent.message.delete      → Eliminar mensaje
telegram.prod.agent.message.stream      → Streaming progresivo ⚡
telegram.prod.agent.callback.answer     → Responder callback
telegram.prod.agent.chat.action         → Mostrar "escribiendo..."
```

**Features especiales:**

🔄 **Streaming de Mensajes**

Permite que los mensajes aparezcan progresivamente (como ChatGPT):

```
Chunk 1: "Hola! "                          → Crea mensaje en Telegram
         [espera 1 segundo - rate limit]
Chunk 2: "Hola! Estoy procesando..."       → Edita el mensaje
         [espera 1 segundo - rate limit]
Chunk 3: "Hola! Estoy procesando tu..." ✓  → Edita mensaje (final)
```

⚡ **Rate Limiting**
- Respeta límite de Telegram: 1 edición por segundo
- Espera automática si es necesario
- Previene throttling de la API

🔁 **Retry Logic**
- Hasta 3 intentos si algo falla
- Exponential backoff: 100ms → 200ms → 400ms
- Logs detallados de cada intento

---

### 4. **telegram-agent**
*Agente de IA*

Aplicación que procesa mensajes y genera respuestas inteligentes.

**Dos modos de operación:**

**Modo Echo** (sin API key):
```
Usuario: "Hola"
Bot: "You said: Hola"
```

**Modo LLM** (con Claude API):
```
Usuario: "¿Qué es Rust?"
Bot: "Rust es un lenguaje de programación de sistemas diseñado para ser
      seguro, concurrente y práctico. Se enfoca en la seguridad de memoria
      sin necesidad de un recolector de basura..."
```

**Funcionalidades:**

📝 **Gestión de Conversaciones**
- Historial de hasta 20 mensajes por sesión
- Cada chat mantiene su propio contexto
- Comando `/clear` para resetear

🤖 **Comandos**
- `/start` - Mensaje de bienvenida
- `/help` - Lista de comandos disponibles
- `/status` - Estado del bot y sesiones activas
- `/clear` - Limpiar historial de conversación

💬 **Procesamiento Inteligente**
- Indicador de "escribiendo..." mientras genera respuesta
- Respuestas contextuales basadas en historial previo
- Manejo de fotos (reconoce y responde)
- Callbacks (botones interactivos)

---

## 🔄 Flujo de Conversación Real

**Ejemplo completo de una pregunta:**

```
┌─────────────────────────────────────────────────────┐
│ Usuario escribe: "Explícame qué es la fotosíntesis"│
└────────────────────────┬────────────────────────────┘
                         │
                         ▼
        ┌────────────────────────────┐
        │ 1. Telegram API            │
        │    Envía update al bot     │
        └────────┬───────────────────┘
                 │
                 ▼
        ┌────────────────────────────┐
        │ 2. telegram-bot            │
        │    • Recibe mensaje        │
        │    • Valida acceso         │
        │    • Crea MessageTextEvent │
        └────────┬───────────────────┘
                 │
                 ▼
        ┌────────────────────────────┐
        │ 3. NATS                    │
        │    Publica a:              │
        │    telegram.prod.bot       │
        │      .message.text         │
        └────────┬───────────────────┘
                 │
                 ▼
        ┌────────────────────────────┐
        │ 4. telegram-agent          │
        │    • Recibe evento         │
        │    • Obtiene historial     │
        │    • Consulta Claude API   │
        └────────┬───────────────────┘
                 │
                 ▼
        ┌────────────────────────────┐
        │ 5. Claude API              │
        │    Genera respuesta:       │
        │    "La fotosíntesis es..." │
        └────────┬───────────────────┘
                 │
                 ▼
        ┌────────────────────────────┐
        │ 6. telegram-agent          │
        │    • Recibe respuesta      │
        │    • Divide en chunks      │
        │    • Publica streams       │
        └────────┬───────────────────┘
                 │
                 ▼
        ┌────────────────────────────┐
        │ 7. NATS                    │
        │    Publica chunks a:       │
        │    telegram.prod.agent     │
        │      .message.stream       │
        └────────┬───────────────────┘
                 │
                 ▼
        ┌────────────────────────────┐
        │ 8. telegram-bot            │
        │    • Recibe chunk 1        │
        │      → Crea mensaje        │
        │    • Recibe chunk 2        │
        │      → Edita mensaje       │
        │    • Recibe chunk 3 (final)│
        │      → Edita y cleanup     │
        └────────┬───────────────────┘
                 │
                 ▼
        ┌────────────────────────────┐
        │ 9. Telegram API            │
        │    Envía mensaje al usuario│
        └────────┬───────────────────┘
                 │
                 ▼
┌────────────────────────────────────────────┐
│ Usuario ve: "La fotosíntesis es el proceso│
│ por el cual las plantas convierten la luz │
│ solar en energía química..."              │
└────────────────────────────────────────────┘
```

**Tiempo total:** ~2-5 segundos (dependiendo de la respuesta de Claude)

---

## 🚀 Cómo Usar

### Requisitos Previos

**1. Rust (1.70+)**
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

**2. NATS Server**
```bash
# macOS
brew install nats-server

# Linux
curl -L https://github.com/nats-io/nats-server/releases/download/v2.10.7/nats-server-v2.10.7-linux-amd64.tar.gz | tar xz
sudo mv nats-server /usr/local/bin/

# Windows
choco install nats-server
```

**3. Bot de Telegram**
1. Abre Telegram
2. Busca `@BotFather`
3. Envía `/newbot`
4. Sigue las instrucciones
5. Copia el token que te da

**4. Claude API Key (opcional)**
1. Ve a https://console.anthropic.com
2. Crea una API key
3. Guárdala (empieza con `sk-ant-...`)

---

### Instalación

```bash
# Clonar repositorio
git clone <repo-url>
cd trogonai/rsworkspace

# Compilar todo
cargo build --release
```

---

### Ejecución

**Terminal 1: NATS Server**
```bash
nats-server
```

**Terminal 2: Telegram Bot**
```bash
cd rsworkspace

export TELEGRAM_BOT_TOKEN="1234567890:ABCdefGHIjklMNOpqrsTUVwxyz"
export NATS_URL="localhost:4222"
export TELEGRAM_PREFIX="prod"

cargo run --release --package telegram-bot
```

**Terminal 3: Agent (Echo Mode)**
```bash
cd rsworkspace

export NATS_URL="localhost:4222"
export TELEGRAM_PREFIX="prod"

cargo run --release --package telegram-agent
```

**O Agent (LLM Mode con Claude)**
```bash
cd rsworkspace

export NATS_URL="localhost:4222"
export TELEGRAM_PREFIX="prod"
export ANTHROPIC_API_KEY="sk-ant-api-..."
export ENABLE_LLM=true
export CLAUDE_MODEL="claude-3-5-sonnet-20241022"

cargo run --release --package telegram-agent
```

---

### Probar el Bot

1. Abre Telegram
2. Busca tu bot `@tu_bot_username`
3. Envía `/start`
4. Envía cualquier mensaje
5. ¡Recibe respuesta!

---

## 📊 NATS Subjects

### Patrón de Nombres

```
telegram.{prefix}.{direction}.{entity}.{action}
```

- **prefix**: Entorno (prod, dev, test)
- **direction**: bot (→ agentes) o agent (→ bot)
- **entity**: Tipo de entidad (message, callback, chat)
- **action**: Acción específica (send, edit, text, photo)

### Eventos (Bot → Agentes)

| Subject | Tipo | Descripción |
|---------|------|-------------|
| `telegram.{prefix}.bot.message.text` | `MessageTextEvent` | Usuario envió texto |
| `telegram.{prefix}.bot.message.photo` | `MessagePhotoEvent` | Usuario envió foto |
| `telegram.{prefix}.bot.message.video` | `MessageVideoEvent` | Usuario envió video |
| `telegram.{prefix}.bot.message.audio` | `MessageAudioEvent` | Usuario envió audio |
| `telegram.{prefix}.bot.message.document` | `MessageDocumentEvent` | Usuario envió documento |
| `telegram.{prefix}.bot.message.voice` | `MessageVoiceEvent` | Usuario envió voz |
| `telegram.{prefix}.bot.callback.query` | `CallbackQueryEvent` | Usuario clickeó botón |
| `telegram.{prefix}.bot.command.{name}` | `CommandEvent` | Usuario envió /comando |

### Comandos (Agentes → Bot)

| Subject | Tipo | Descripción |
|---------|------|-------------|
| `telegram.{prefix}.agent.message.send` | `SendMessageCommand` | Enviar mensaje |
| `telegram.{prefix}.agent.message.edit` | `EditMessageCommand` | Editar mensaje |
| `telegram.{prefix}.agent.message.delete` | `DeleteMessageCommand` | Eliminar mensaje |
| `telegram.{prefix}.agent.message.send_photo` | `SendPhotoCommand` | Enviar foto |
| `telegram.{prefix}.agent.message.stream` | `StreamMessageCommand` | Streaming ⚡ |
| `telegram.{prefix}.agent.callback.answer` | `AnswerCallbackCommand` | Responder callback |
| `telegram.{prefix}.agent.chat.action` | `SendChatActionCommand` | "Escribiendo..." |

---

## 🏆 Features Implementadas

### telegram-bot
- ✅ 8 tipos de mensajes soportados
- ✅ Control de acceso (whitelist/blacklist/admins)
- ✅ Gestión de sesiones con JetStream KV
- ✅ **Streaming con rate limiting** (1 edit/seg)
- ✅ **Retry logic** con exponential backoff
- ✅ Indicadores de actividad ("escribiendo...")
- ✅ Soporte para botones inline
- ✅ Tests unitarios (10 tests)

### telegram-agent
- ✅ **Modo Echo** (sin API key necesaria)
- ✅ **Integración con Claude API**
- ✅ Gestión de conversaciones
- ✅ Historial de 20 mensajes por sesión
- ✅ Comandos (/start, /help, /status, /clear)
- ✅ Procesamiento de callbacks
- ✅ Indicadores de typing
- ✅ Multi-sesión simultáneas

### Infraestructura
- ✅ NATS JetStream para persistencia
- ✅ Session management
- ✅ Event sourcing pattern
- ✅ Documentación completa
- ✅ Ejemplos funcionales

---

## 💡 Ejemplos

### Ejemplo 1: Bot Simple Q&A

```rust
use telegram_nats::{MessageSubscriber, MessagePublisher, subjects};
use telegram_types::events::MessageTextEvent;
use telegram_types::commands::SendMessageCommand;

#[tokio::main]
async fn main() -> Result<()> {
    // Conectar a NATS
    let client = telegram_nats::connect(&config).await?;
    let subscriber = MessageSubscriber::new(client.clone(), "prod");
    let publisher = MessagePublisher::new(client, "prod");

    // Suscribirse a mensajes de texto
    let mut stream = subscriber
        .subscribe::<MessageTextEvent>(
            &subjects::bot::message_text("prod")
        ).await?;

    // Procesar mensajes
    while let Some(Ok(event)) = stream.next().await {
        let response = if event.text.contains("hola") {
            "¡Hola! ¿Cómo estás?"
        } else {
            "No entendí, di 'hola'"
        };

        // Enviar respuesta
        let command = SendMessageCommand {
            chat_id: event.message.chat.id,
            text: response.to_string(),
            parse_mode: None,
            reply_to_message_id: Some(event.message.message_id),
            reply_markup: None,
        };

        publisher.publish(
            &subjects::agent::message_send("prod"),
            &command
        ).await?;
    }

    Ok(())
}
```

### Ejemplo 2: Bot con Botones

```rust
use telegram_types::chat::{InlineKeyboardMarkup, InlineKeyboardButton};

// Crear botones
let buttons = InlineKeyboardMarkup {
    inline_keyboard: vec![
        vec![
            InlineKeyboardButton {
                text: "Opción 1".to_string(),
                callback_data: Some("option_1".to_string()),
                url: None,
            },
            InlineKeyboardButton {
                text: "Opción 2".to_string(),
                callback_data: Some("option_2".to_string()),
                url: None,
            },
        ]
    ]
};

// Enviar mensaje con botones
let command = SendMessageCommand {
    chat_id: event.message.chat.id,
    text: "Elige una opción:".to_string(),
    parse_mode: None,
    reply_to_message_id: None,
    reply_markup: Some(buttons),
};
```

---

## 🛠️ Troubleshooting

### El bot no responde

```bash
# 1. Verificar NATS
ps aux | grep nats-server
# Si no está corriendo:
nats-server

# 2. Verificar bot
ps aux | grep telegram-bot
# Ver logs:
RUST_LOG=debug cargo run --package telegram-bot

# 3. Verificar agent
ps aux | grep telegram-agent
# Ver logs:
RUST_LOG=debug cargo run --package telegram-agent
```

### Error: "authorization violation"

NATS puede tener autenticación configurada. Para deshabilitarla:

```bash
# Iniciar NATS sin auth
nats-server -c /dev/null
```

### El streaming no funciona

```bash
# Verificar logs del agent
# Debe decir: "Publishing stream message..."

# Verificar logs del bot
# Debe decir: "Subscribed to streaming messages"

# Probar el demo
cargo run --package telegram-bot --example demo_streaming
```

### Rate limiting muy lento

El bot espera 1 segundo entre ediciones (límite de Telegram). Esto es normal y necesario.

---

## 📚 Documentación Adicional

- **[NATS_ARCHITECTURE.md](./NATS_ARCHITECTURE.md)** - Arquitectura completa de NATS
- **[STREAMING_GUIDE.md](./STREAMING_GUIDE.md)** - Guía del sistema de streaming

---

## 🎯 Casos de Uso Reales

### 1. Bot de Soporte al Cliente
```
Usuario: "¿Cuál es su horario?"
Bot: "Nuestro horario es de lunes a viernes, 9:00 a 18:00"
```

### 2. Asistente Personal
```
Usuario: "Recuérdame comprar leche"
Bot: "✅ Te recordaré comprar leche"
[Más tarde]
Bot: "🔔 Recordatorio: comprar leche"
```

### 3. Bot Educativo
```
Usuario: "Explícame la fotosíntesis"
Bot: [Respuesta detallada con explicación científica]
```

### 4. Bot de Encuestas
```
Bot: "¿Cómo calificarías nuestro servicio?"
[Botones: ⭐ ⭐⭐ ⭐⭐⭐ ⭐⭐⭐⭐ ⭐⭐⭐⭐⭐]
Usuario: [Click en ⭐⭐⭐⭐]
Bot: "¡Gracias por tu feedback!"
```

---

## 🚀 Testing

```bash
# Tests unitarios del bot
cargo test --package telegram-bot

# Demo de streaming
cargo run --package telegram-bot --example demo_streaming

# Test de integración NATS
cargo run --package telegram-bot --example test_streaming
```

---

## 📄 Licencia

MIT

---

Hecho con ❤️ usando Rust 🦀 y Claude 🤖
