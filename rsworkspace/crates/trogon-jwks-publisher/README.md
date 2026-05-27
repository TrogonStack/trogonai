# trogon-jwks-publisher

Sidecar that publishes the mesh Security Token Service (STS) public JWKS to in-mesh and external consumers per [ADR 0006](../../../docs/adr/0006-mesh-token-signing-keys.md).

## Distribution channels

| Channel | Location | Consumers |
|---------|----------|-----------|
| NATS KV (primary) | bucket `mcp-jwks`, key `mesh/current` | Gateways and in-mesh validators with KV watch |
| HTTPS (secondary) | `/.well-known/jwks.json` | External OAuth/MCP validators |
| NATS request/reply (tertiary) | `mcp.jwks.mesh.get` | Pull-based clients without KV watch yet |

Each active signing key is also written to `mesh/{kid}` for explicit kid lookup during rotation overlap.

## Running (dev)

```bash
# Generate a dev signing key
mkdir -p /tmp/trogon-mesh-keys
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out /tmp/trogon-mesh-keys/current.pem

export TROGON_JWKS_KV_CREATE_BUCKET=true
cargo run -p trogon-jwks-publisher -- \
  --nats-url nats://127.0.0.1:4222 \
  --key-source file \
  --key-dir /tmp/trogon-mesh-keys \
  --https-listen 127.0.0.1:8080
```

Without `--tls-cert` / `--tls-key`, HTTPS listens in plaintext on loopback with a WARN log (dev only).

## Key sources

| `--key-source` | Status | Notes |
|----------------|--------|-------|
| `file` | Implemented | Reads `*.pem` private keys (RSA PKCS#8, Ed25519 PKCS#8) from `--key-dir`; watches for changes via `notify`. |
| `kms` | Stub (`TODO(prod)`) | Production adoption blocks on cloud KMS public-key export. |
| `vault` | Stub (`TODO(prod)`) | Production adoption blocks on Vault Transit public-key export. |

**Dev vs prod:** file PEM keys are for local development and CI only. Production must use KMS or Vault so private key material never lives on disk (ADR 0006).

## Key rotation runbook

### Normal rotation (overlap window)

1. **Generate** a new signing key in the custody backend (KMS/Vault) or, in dev, write a new `*.pem` into `--key-dir`.
2. **Publish overlap:** keep the previous key's PEM in the directory (dev) or ensure both key versions are active in KMS/Vault (prod). The publisher assembles a JWKS containing **both** `kid` values.
3. **Wait for propagation:** KV subscribers receive the updated document immediately; HTTPS caches honor `Cache-Control: max-age=300` (5 minutes).
4. **Overlap duration:** keep both keys in JWKS for at least **max mesh token TTL + clock skew** (ADR 0005 mesh TTL 60–300 s; default gateway leeway 60 s → plan ≥ 6 minutes, round up to operational comfort).
5. **Retire old key:** remove the previous PEM from `--key-dir` (dev) or disable the old KMS/Vault key version (prod). The publisher drops the old `kid` from JWKS; validators reject tokens signed with the retired key.

### Revoke a compromised key

1. **Immediately** remove the compromised key from the source (delete PEM, disable KMS/Vault version, or emergency `kid` removal in publisher config once prod backends exist).
2. Accept brief rejection of otherwise-valid tokens still signed with that `kid` (fail-closed).
3. Audit custody backend logs (CloudTrail / Vault audit) for signing activity on the compromised version.

### Verify rotation

```bash
# NATS KV
nats kv get mcp-jwks mesh/current

# HTTPS
curl -s http://127.0.0.1:8080/.well-known/jwks.json | jq .

# Request/reply
nats request mcp.jwks.mesh.get ''
```

## Configuration

| Flag / env | Default | Description |
|------------|---------|-------------|
| `--nats-url` / `NATS_URL` | `nats://127.0.0.1:4222` | NATS server for KV and request/reply |
| `--key-source` | `file` | `file`, `kms`, or `vault` |
| `--key-dir` | `/etc/trogon-mesh-keys` | Directory of `*.pem` keys (file source) |
| `--https-listen` | `127.0.0.1:8080` | JWKS HTTP listen address |
| `--tls-cert`, `--tls-key` | — | PEM paths for production TLS |
| `TROGON_JWKS_KV_CREATE_BUCKET` | unset | Set to `true` to create `mcp-jwks` bucket if missing (dev) |

## `kid` derivation

`kid` = first 16 hex characters of SHA-256 over the public key SPKI DER encoding. This matches the overlap-window semantics in ADR 0006: each distinct public key gets a stable `kid` used in JWS headers and KV keys `mesh/{kid}`.
