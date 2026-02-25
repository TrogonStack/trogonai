# NATS HTTP Detokenization Proxy

Research on how to proxy HTTP calls through NATS to implement a tokenization/detokenization
layer similar to [Very Good Security (VGS)](https://www.verygoodsecurity.com/).

---

## The Problem

Sensitive data (credit card numbers, SSNs, PII) should never touch your application servers.
Instead, clients hold **tokens** — opaque references to the real values — and a trusted
intermediary swaps them for real data only when strictly necessary.

VGS solves this as a SaaS reverse proxy. This document covers how to build an equivalent
system using NATS as the internal message bus.

---

## Key Concepts

### Proxy
An intermediary that sits between a client and a server, intercepting and optionally
modifying traffic in both directions.

- **Forward proxy**: protects the client (e.g. VPN)
- **Reverse proxy**: protects the server (e.g. Nginx, Cloudflare, VGS)

### HTTP
Request/Response protocol. The client always initiates, the server responds. Every message
has a method (`GET`, `POST`, etc.), headers, and an optional body.

### NATS vs HTTP

| | HTTP | NATS |
|---|---|---|
| Model | Request/Response | Publish/Subscribe |
| Who initiates | Always the client | Anyone |
| Connection | Opens and closes | Persistent |
| Recipient | One specific server | Any subscriber |
| Analogy | Phone call | Radio channel |

They are complementary: HTTP for external communication, NATS for internal service-to-service.

### Vault
A secure store for sensitive data. Characteristics:
- Encryption at rest
- Access control (only authorized services can read)
- Audit log (who accessed what and when)
- TTL / secret rotation

Examples: HashiCorp Vault, AWS Secrets Manager, Redis + encryption.

---

## Architecture

```
Client (tokenized request)
    ↓  HTTP
[HTTP Gateway]
    ↓  NATS pub
[Detokenization Worker]
    ↓  lookup
[Token Vault]
    ↓  real HTTP
[Upstream Service]
    ↑
[Response flows back the same way]
```

### Security Boundaries

| Component      | Sees real data? | Reason                              |
|----------------|-----------------|-------------------------------------|
| Client         | NO              | only holds tokens                   |
| HTTP Gateway   | NO              | passes body as-is                   |
| NATS           | NO              | just transports bytes               |
| Worker         | YES             | only for milliseconds               |
| Vault          | YES             | encrypted at rest                   |
| Upstream       | YES             | needs real data to process          |

---

## Implementation: Request/Reply

The natural fit for synchronous HTTP proxying. The gateway publishes and blocks until
the worker replies.

### Step 1 — Tokenize on inbound

Before the client sends data, tokenize it and store in the vault:

```
Real value: 4111-1111-1111-1111
     ↓
POST /vault/tokenize
     ↓
Response: { "token": "tok_abc123" }

Redis: token:tok_abc123 → 4111-1111-1111-1111 (encrypted)
```

### Step 2 — HTTP Gateway

Receives the tokenized HTTP request and publishes it to NATS:

```go
type ProxyRequest struct {
    Method  string            `json:"method"`
    URL     string            `json:"url"`
    Headers map[string]string `json:"headers"`
    Body    []byte            `json:"body"`
}

type ProxyResponse struct {
    StatusCode int               `json:"status_code"`
    Headers    map[string]string `json:"headers"`
    Body       []byte            `json:"body"`
}

http.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
    body, _ := io.ReadAll(r.Body)
    req := ProxyRequest{
        Method:  r.Method,
        URL:     "https://upstream.internal" + r.RequestURI,
        Headers: flattenHeaders(r.Header),
        Body:    body,
    }
    data, _ := json.Marshal(req)

    // Publishes and waits for worker reply
    msg, err := nc.Request("proxy.http", data, 10*time.Second)
    if err != nil {
        http.Error(w, "upstream timeout", 502)
        return
    }

    var resp ProxyResponse
    json.Unmarshal(msg.Data, &resp)
    w.WriteHeader(resp.StatusCode)
    w.Write(resp.Body)
})
```

### Step 3 — Token Vault (Redis)

```go
func (v *Vault) Tokenize(ctx context.Context, token, realValue string) error {
    return v.client.Set(ctx, "token:"+token, realValue, 0).Err()
}

func (v *Vault) Resolve(ctx context.Context, token string) (string, bool) {
    val, err := v.client.Get(ctx, "token:"+token).Result()
    if err != nil {
        return "", false
    }
    return val, true
}
```

### Step 4 — Detokenization Worker

```go
var tokenPattern = regexp.MustCompile(`tok_[a-zA-Z0-9]+`)

func detokenize(data []byte, vault *Vault) []byte {
    ctx := context.Background()
    return tokenPattern.ReplaceAllFunc(data, func(tok []byte) []byte {
        if real, ok := vault.Resolve(ctx, string(tok)); ok {
            return []byte(real)
        }
        return tok
    })
}

// QueueSubscribe = multiple workers, only one handles each message
nc.QueueSubscribe("proxy.http", "detokenize-workers", func(msg *nats.Msg) {
    var req ProxyRequest
    json.Unmarshal(msg.Data, &req)

    req.Body = detokenize(req.Body, vault)
    for k, v := range req.Headers {
        req.Headers[k] = string(detokenize([]byte(v), vault))
    }

    httpReq, _ := http.NewRequest(req.Method, req.URL, bytes.NewReader(req.Body))
    for k, v := range req.Headers {
        httpReq.Header.Set(k, v)
    }

    httpResp, _ := httpClient.Do(httpReq)
    body, _ := io.ReadAll(httpResp.Body)

    resp := ProxyResponse{
        StatusCode: httpResp.StatusCode,
        Headers:    flattenResponseHeaders(httpResp.Header),
        Body:       body,
    }

    data, _ := json.Marshal(resp)
    msg.Respond(data)
})
```

### Full Data Flow Example

```
POST /charge  { "card": "tok_abc123", "amount": 100 }
    ↓
[Gateway] publishes to NATS "proxy.http"
    ↓
[Worker] receives message
    tok_abc123 → Vault → 4111-1111-1111-1111
    ↓
[Worker] calls Stripe: { "card": "4111-1111-1111-1111", "amount": 100 }
    ↓
[Stripe] responds: { "status": "approved" }
    ↓
[Worker] replies to NATS
    ↓
[Gateway] returns HTTP to client: { "status": "approved" }
```

---

## Implementation: JetStream Alternatives

Request/Reply is synchronous and ephemeral — if a worker crashes mid-flight, the message
is lost. JetStream adds persistence and guaranteed delivery.

### Option A — JetStream request + Core NATS reply (Recommended hybrid)

Request is persisted in a stream. If the worker crashes, the message is requeued.
Reply still uses Core NATS for speed.

```go
// Gateway
correlationID := uuid.New().String()
replySubject := "proxy.reply." + correlationID

sub, _ := nc.SubscribeSync(replySubject)
defer sub.Unsubscribe()

msg := &nats.Msg{
    Subject: "proxy.requests",
    Data:    data,
    Header:  nats.Header{},
}
msg.Header.Set("Reply-To", replySubject)
js.PublishMsg(msg)

reply, err := sub.NextMsg(10 * time.Second)
```

```go
// Worker
js.QueueSubscribe("proxy.requests", "workers", func(msg *nats.Msg) {
    resp := processRequest(msg.Data, vault)
    data, _ := json.Marshal(resp)

    replyTo := msg.Header.Get("Reply-To")
    nc.Publish(replyTo, data)

    msg.Ack()
})
```

### Option B — Pure JetStream with KeyValue Store

Both request and response are persisted. Useful for full audit trails.

```go
// Gateway
kv, _ := js.KeyValue("proxy-responses")
correlationID := uuid.New().String()

js.Publish("proxy.requests", data) // with correlationID header

watcher, _ := kv.Watch("response." + correlationID)
defer watcher.Stop()

select {
case entry := <-watcher.Updates():
    if entry != nil {
        var resp ProxyResponse
        json.Unmarshal(entry.Value(), &resp)
        w.Write(resp.Body)
    }
case <-time.After(10 * time.Second):
    http.Error(w, "timeout", 502)
}
```

```go
// Worker
js.Subscribe("proxy.requests", func(msg *nats.Msg) {
    correlationID := msg.Header.Get("Correlation-Id")
    resp := processRequest(msg.Data, vault)
    data, _ := json.Marshal(resp)

    kv.Put("response."+correlationID, data)
    msg.Ack()
})
```

---

## Comparison

| | Request/Reply | JetStream hybrid | JetStream KV |
|---|---|---|---|
| **Speed** | Fastest | Fast | Slower |
| **Persistence** | No | Request only | Request + Response |
| **Worker crash** | Message lost | Message requeued | Message requeued |
| **Complexity** | Low | Medium | High |
| **Best for** | High throughput, low latency | Critical reliability | Full audit trail |

---

## Scaling

Use NATS Queue Groups to run multiple worker instances without duplicate processing:

```go
nc.QueueSubscribe("proxy.http", "detokenize-workers", handler)
// or with JetStream:
js.QueueSubscribe("proxy.requests", "workers", handler)
```

Each message goes to exactly **one** worker. Adding more workers increases throughput
with no configuration changes.

---

## Production Checklist

| Concern | Solution |
|---|---|
| Encrypt vault data at rest | AES-256 on Redis values |
| mTLS between services | NATS TLS + client certs |
| Audit log | Publish to `audit.detokenize` on each resolve |
| Token expiry | Redis TTL on each token key |
| Rate limiting | NATS account limits or gateway middleware |
| Token scope | Prefix by type: `tok_card_`, `tok_ssn_`, `tok_email_` |
