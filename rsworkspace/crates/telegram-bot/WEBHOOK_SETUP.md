# Webhook vs Polling Setup Guide

## Overview

The Telegram bot supports two update modes:

- **Polling**: Bot actively fetches updates from Telegram (default)
- **Webhook**: Telegram sends updates to your bot's public HTTPS endpoint

## Comparison

| Feature | Polling | Webhook |
|---------|---------|---------|
| **Latency** | ~1-2 seconds | Real-time (<100ms) |
| **Resource Usage** | Higher (constant polling) | Lower (event-driven) |
| **Setup Complexity** | Simple | Moderate |
| **Public URL Required** | ❌ No | ✅ Yes (HTTPS) |
| **SSL Certificate** | ❌ No | ✅ Yes |
| **Works Behind NAT/Firewall** | ✅ Yes | ❌ No |
| **Best For** | Development, Testing | Production |

## Polling Mode (Default)

### Configuration

**Via Config File:**
```toml
[telegram.update_mode]
mode = "polling"
timeout = 30    # Long polling timeout in seconds
limit = 100     # Max updates per request (1-100)
```

**Via Environment Variables:**
```bash
# Polling is default if no webhook variables are set
TELEGRAM_BOT_TOKEN=your_token cargo run
```

### Pros
- ✅ Easy setup, no infrastructure required
- ✅ Works behind NAT/firewall
- ✅ Perfect for development and testing
- ✅ No SSL certificate needed

### Cons
- ❌ Higher latency (~1-2 seconds)
- ❌ More resource usage (constant polling)
- ❌ More bandwidth usage

## Webhook Mode

### Requirements

1. **Public HTTPS URL**: `https://your-domain.com`
2. **SSL Certificate**: Valid certificate (Let's Encrypt recommended)
3. **Open Port**: One of: 443, 80, 88, or 8443
4. **Server**: Accessible from internet

### Configuration

**Via Config File:**
```toml
[telegram.update_mode]
mode = "webhook"
url = "https://your-domain.com"
port = 8443
path = "/webhook"
secret_token = "your-secret-token"
bind_address = "0.0.0.0"
max_connections = 40
allowed_updates = []  # Empty = all updates
```

**Via Environment Variables:**
```bash
export TELEGRAM_BOT_TOKEN="your_token"
export TELEGRAM_WEBHOOK_URL="https://your-domain.com"
export TELEGRAM_WEBHOOK_PORT="8443"
export TELEGRAM_WEBHOOK_PATH="/webhook"
export TELEGRAM_WEBHOOK_SECRET="your-secret-token"

cargo run
```

### Pros
- ✅ Real-time updates (<100ms latency)
- ✅ More efficient (event-driven)
- ✅ Lower resource and bandwidth usage
- ✅ Better for production

### Cons
- ❌ Requires public HTTPS endpoint
- ❌ Needs SSL certificate
- ❌ More complex infrastructure
- ❌ Port must be accessible

## SSL Certificate Setup

### Option 1: Let's Encrypt (Recommended for Production)

```bash
# Install certbot
sudo apt-get install certbot

# Get certificate
sudo certbot certonly --standalone -d your-domain.com

# Certificates will be in:
# /etc/letsencrypt/live/your-domain.com/fullchain.pem
# /etc/letsencrypt/live/your-domain.com/privkey.pem
```

### Option 2: Self-Signed Certificate (For ports 8443, 88)

```bash
# Generate self-signed certificate
openssl req -newkey rsa:2048 -sha256 -nodes \
  -keyout private.key -x509 -days 365 \
  -out cert.pem -subj "/CN=your-domain.com"

# Upload public key to Telegram when setting webhook
# (Teloxide handles this automatically)
```

## Deployment Examples

### Example 1: Simple Polling Deployment

```bash
# Run with polling (no special setup needed)
TELEGRAM_BOT_TOKEN=your_token \
NATS_URL=localhost:4222 \
cargo run --release
```

### Example 2: Webhook with Nginx Reverse Proxy

**Nginx Configuration:**
```nginx
server {
    listen 443 ssl;
    server_name your-domain.com;

    ssl_certificate /etc/letsencrypt/live/your-domain.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/your-domain.com/privkey.pem;

    location /webhook {
        proxy_pass http://localhost:8443;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

**Run Bot:**
```bash
TELEGRAM_BOT_TOKEN=your_token \
TELEGRAM_WEBHOOK_URL=https://your-domain.com \
TELEGRAM_WEBHOOK_PORT=8443 \
TELEGRAM_WEBHOOK_SECRET=mysecret \
cargo run --release
```

### Example 3: Webhook Direct on Port 8443

```bash
# Bot listens directly on 8443 (self-signed cert OK)
TELEGRAM_BOT_TOKEN=your_token \
TELEGRAM_WEBHOOK_URL=https://your-domain.com \
TELEGRAM_WEBHOOK_PORT=8443 \
cargo run --release
```

### Example 4: Docker with Webhook

**docker-compose.yml:**
```yaml
version: '3.8'
services:
  telegram-bot:
    build: .
    environment:
      - TELEGRAM_BOT_TOKEN=${BOT_TOKEN}
      - TELEGRAM_WEBHOOK_URL=https://your-domain.com
      - TELEGRAM_WEBHOOK_PORT=8443
      - TELEGRAM_WEBHOOK_SECRET=${WEBHOOK_SECRET}
      - NATS_URL=nats:4222
    ports:
      - "8443:8443"
    depends_on:
      - nats
```

## Webhook Security Best Practices

1. **Always use secret_token**: Validates requests are from Telegram
2. **Use HTTPS only**: Never use HTTP for webhooks
3. **Restrict allowed_updates**: Only receive updates you need
4. **Rate limiting**: Configure max_connections appropriately
5. **Firewall rules**: Only allow traffic from Telegram IPs

### Telegram Server IP Ranges
```
149.154.160.0/20
91.108.4.0/22
```

## Troubleshooting

### Webhook Not Receiving Updates

1. **Check webhook status:**
```bash
curl https://api.telegram.org/bot${TOKEN}/getWebhookInfo
```

2. **Verify URL is accessible:**
```bash
curl -I https://your-domain.com/webhook
```

3. **Check bot logs** for error messages

4. **Verify SSL certificate:**
```bash
openssl s_client -connect your-domain.com:443
```

### Common Issues

**"Bad Gateway" errors**: Check if bot is running and port is correct

**"SSL certificate problem"**: Use valid certificate or self-signed for ports 8443/88

**"Connection refused"**: Check firewall and bind_address settings

**No updates received**: Verify secret_token matches and webhook URL is correct

## Switching Modes

### From Polling to Webhook

1. Stop the bot
2. Update configuration to webhook mode
3. Ensure SSL certificate is in place
4. Restart bot (it will set webhook automatically)

### From Webhook to Polling

1. Stop the bot
2. Delete webhook:
```bash
curl https://api.telegram.org/bot${TOKEN}/deleteWebhook
```
3. Update configuration to polling mode
4. Restart bot

## Monitoring

Check webhook status regularly:
```bash
# Get webhook info
curl "https://api.telegram.org/bot${TOKEN}/getWebhookInfo"

# Response includes:
# - url: Current webhook URL
# - pending_update_count: Queued updates
# - last_error_date: Last error timestamp
# - last_error_message: Error details
```

## Recommendations

- **Development**: Use polling mode
- **Production (low traffic)**: Either mode works
- **Production (high traffic)**: Use webhook mode
- **Behind firewall**: Use polling mode
- **Cloud deployment**: Use webhook mode
