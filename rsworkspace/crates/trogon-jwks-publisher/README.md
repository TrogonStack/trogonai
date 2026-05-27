# trogon-jwks-publisher

Sidecar that publishes the mesh Security Token Service (STS) public JWKS to in-mesh and external consumers per [ADR 0006](../../../docs/adr/0006-mesh-token-signing-keys.md).

## Dev vs production

| Environment | `--key-source` | Cargo features | Private key location |
|-------------|----------------|----------------|----------------------|
| **Development / CI** | `file` (default) | `file-pem` only | `*.pem` under `--key-dir` on disk |
| **Production** | `kms` or `vault` | `kms-aws` and/or `vault` | AWS KMS or Vault Transit — never file PEM |

Do not run `--key-source file` in production. The default binary feature set is `file-pem` so local builds do not pull cloud SDKs.

## Distribution channels

| Channel | Location | Consumers |
|---------|----------|-----------|
| NATS KV (primary) | bucket `mcp-jwks`, key `mesh/current` | Gateways and in-mesh validators with KV watch |
| HTTPS (secondary) | `/.well-known/jwks.json` | External OAuth/MCP validators |
| NATS request/reply (tertiary) | `mcp.jwks.mesh.get` | Pull-based clients without KV watch yet |

Each active signing key is also written to `mesh/{kid}` for explicit kid lookup during rotation overlap.

## Cargo features

```toml
default = ["file-pem"]
file-pem = []
kms-aws = ["aws-sdk-kms", "aws-config"]
vault   = ["reqwest"]
```

Build production binaries with explicit features, for example:

```bash
cargo build -p trogon-jwks-publisher --no-default-features --features kms-aws,vault
```

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

## AWS KMS (production)

Requires `--features kms-aws` and `--key-source kms`.

| Variable / flag | Required | Description |
|-----------------|----------|-------------|
| `TROGON_JWKS_KMS_KEY_ARN` / `--kms-key-arn` | yes | Asymmetric KMS key ARN (`RSA_2048` primary per ADR 0006) |
| `TROGON_JWKS_KMS_REGION` / `--kms-region` | no | AWS region (defaults to SDK config chain) |
| `TROGON_JWKS_KMS_PREVIOUS_KEY_VERSION` | no | Key version id for rotation overlap (e.g. `abc-123`) |
| `MCP_STS_MESH_ISSUER` / `--mesh-issuer` | no | Mesh JWT `iss` (default `urn:trogon:sts:mesh`) |

IAM actions on the key:

- `kms:Sign` — RSASSA-PKCS1-v1_5 with SHA-256 for mesh JWTs (`RS256`)
- `kms:GetPublicKey` — assemble JWKS `n`/`e` and `kid` from key version

Signing uses `MessageType=RAW` over the JWT signing input (`base64url(header).base64url(payload)`). JWKS refresh polls every 60 s.

## Vault Transit (production)

Requires `--features vault` and `--key-source vault`.

| Variable / flag | Required | Description |
|-----------------|----------|-------------|
| `TROGON_JWKS_VAULT_ADDR` | no | Vault API base URL (default `http://127.0.0.1:8200`) |
| `TROGON_JWKS_VAULT_TRANSIT_MOUNT` | no | Transit mount (default `transit`) |
| `TROGON_JWKS_VAULT_TRANSIT_KEY` / `--vault-transit-key` | yes | Transit key name |
| `TROGON_JWKS_VAULT_TOKEN` / `--vault-token` | yes | Vault token with `update` on the key |
| `MCP_STS_MESH_ISSUER` | no | Mesh JWT `iss` (default `urn:trogon:sts:mesh`) |

APIs used:

- `GET /v1/{mount}/keys/{name}` — export public keys for all versions from `min_decryption_version` through `latest_version`
- `POST /v1/{mount}/sign/{name}` — sign JWT input (base64-encoded raw signing input)

Token sourcing: inject via env/secret store in Kubernetes or systemd (`VAULT_TOKEN` pattern). Do not bake tokens into images.

Live integration test: `cargo test -p trogon-jwks-publisher --features vault vault_transit_sign_and_publish_jwks -- --ignored`.

## Key sources summary

| `--key-source` | Feature | Status |
|----------------|---------|--------|
| `file` | `file-pem` | Reads `*.pem` private keys (RSA PKCS#8, Ed25519 PKCS#8); `current.pem` is the active signer; watches directory via `notify`. |
| `kms` | `kms-aws` | AWS KMS `GetPublicKey` + `Sign` for JWKS publication and mesh JWT signing. |
| `vault` | `vault` | Vault Transit keys + sign for JWKS publication and mesh JWT signing. |

## Key rotation runbook

### Normal rotation (overlap window)

1. **Generate** a new signing key in the custody backend (KMS/Vault) or, in dev, add a new `*.pem` into `--key-dir` (keep `current.pem` as the active signer until cutover).
2. **Publish overlap:** keep the previous KMS key version (`TROGON_JWKS_KMS_PREVIOUS_KEY_VERSION`) or previous Vault version enabled; in dev, keep `previous.pem` alongside `current.pem`. The publisher assembles a JWKS containing **both** `kid` values.
3. **Wait for propagation:** KV subscribers receive the updated document immediately; HTTPS caches honor `Cache-Control: max-age=300` (5 minutes).
4. **Overlap duration:** keep both keys in JWKS for at least **max mesh token TTL + clock skew** (300 s + 60 s = 360 s minimum; see `MIN_ROTATION_OVERLAP_SECS` in the crate).
5. **Retire old key:** remove `previous.pem` (dev) or drop the previous KMS/Vault version from JWKS config after the overlap window. Validators reject tokens signed with the retired `kid`.

### Revoke a compromised key

1. **Immediately** disable the compromised KMS/Vault version or remove its PEM from `--key-dir`.
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

Offline overlap test (no NATS/AWS/Vault): `cargo test -p trogon-jwks-publisher --test rotation_e2e`.

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

`kid` = first 16 hex characters of SHA-256 over the public key SPKI DER encoding (file/PEM keys). AWS KMS uses the key version id from `GetPublicKey`. Vault uses `vault-{hash}-v{version}`. This matches overlap-window semantics in ADR 0006: each distinct public key gets a stable `kid` used in JWS headers and KV keys `mesh/{kid}`.

## Mesh signer API

The crate exports `MeshSigner` (`KmsSigner`, `VaultSigner`, `FileMeshSigner`) for STS and tests: `sign()` mints RS256 mesh JWTs with `kid` and `iss`; `active_jwks()` returns the verification set for publication.
