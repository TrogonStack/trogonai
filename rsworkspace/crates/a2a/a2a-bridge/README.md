# a2a-bridge

HTTPS ↔ NATS A2A sidecar foundation: typed caller identity value objects and the unified `BridgeError` surface that the inbound (HTTPS) and outbound (NATS / upstream HTTPS) wiring will build on in the next slices.

## Configuration

`AUTH_CALLOUT_XKEY_PUBLIC` is the public curve key of `a2a-auth-callout`. The
bridge seals every caller credential to it before the mint exchange crosses
NATS, so only the callout can read the credential. Without it the bridge
refuses every mint. Take it from the same bootstrap that hands the callout its
`AUTH_CALLOUT_XKEY_SEED`, because a credential sealed to any other key does
not open.

### AAuth proof of possession

The HTTP edge verifies the AAuth proof of possession of each request, an
RFC 9421 HTTP Message Signature, before the bridge mints a caller credential
for it
([ADR#0082](../../../../docs/adr/0082-bus-principal-from-subject-and-edge-proof-of-possession.md),
Decision 4). A blank value counts as unset, and a startup error names the
variable to fix.

| Variable | Description | Default |
|----------|-------------|---------|
| `A2A_BRIDGE_AAUTH_MODE` | `enforce`, `shadow` or `off`, in any ASCII case. | `enforce` |
| `A2A_BRIDGE_AAUTH_DEV_UNENFORCED` | Development opt-in that `shadow` and `off` refuse to start without: `true`, `1` or `yes`, in any ASCII case. Any other value fails startup in every mode. | unset |

| Mode | What the edge does |
|------|--------------------|
| `enforce` | Refuses every request whose proof does not verify. |
| `shadow` | Verifies the proof, records a failure and admits the request, except that a request that is not signed, or whose bearer names no caller, is still refused. |
| `off` | Admits every request that carries a bearer credential, without verifying a proof. |

Below `enforce` the bridge logs a startup warning that names
`A2A_BRIDGE_AAUTH_DEV_UNENFORCED`. `shadow` and `enforce` also read:

| Variable | Description | Default |
|----------|-------------|---------|
| `A2A_BRIDGE_AAUTH_RESOURCE_ISS` | The edge's resource identifier: the `aud` an auth token has to name and the issuer of the nonces the edge issues. Required. | unset |
| `A2A_BRIDGE_AAUTH_AUTHORITY` | The authority clients reach the edge at, which every signature covers as `@authority`. Required. | unset |
| `A2A_BRIDGE_AAUTH_JWKS_PATH` | File holding a JSON object that maps each trusted issuer to its JWK set. Set it or `A2A_BRIDGE_AAUTH_JWKS_DISCOVERY=true`, never both. | unset |
| `A2A_BRIDGE_AAUTH_JWKS_DISCOVERY` | `true` resolves issuer keys through well-known discovery instead of a file. `false` does not. | `false` |
| `A2A_BRIDGE_AAUTH_JWKS_TTL_SECS` | Seconds a discovered key set stays cached. Read only with discovery. | `600` |
| `A2A_BRIDGE_AAUTH_JWKS_ALLOWED_ISSUERS` | Comma-separated issuers discovery may fetch keys for. Unset lets it fetch keys for any issuer. Read only with discovery. | unset |
| `A2A_BRIDGE_AAUTH_NONCE` | `required` or `optional`, in any ASCII case. | `optional` |
| `A2A_BRIDGE_AAUTH_CHALLENGE_KEY_PATH` | PKCS#8 PEM P-256 private key the edge signs the nonces it issues with. Set together with `A2A_BRIDGE_AAUTH_CHALLENGE_KID`, and required when `A2A_BRIDGE_AAUTH_NONCE=required`. | unset |
| `A2A_BRIDGE_AAUTH_CHALLENGE_KID` | Key id the edge names its challenge key by. | unset |
| `A2A_BRIDGE_AAUTH_NONCE_TTL_SECS` | Seconds a nonce the edge issues stays usable, from 1 to 120. Read only with a challenge key. | `60` |
| `A2A_BRIDGE_AAUTH_SIGNATURE_VALIDITY_SECS` | Longest span accepted between the `created` and `expires` of a signature, from 1 to 120 seconds. | `60` |
| `A2A_BRIDGE_AAUTH_MAX_SKEW_SECS` | Clock skew tolerated on either side of the window of a signature, from 0 to 30 seconds. | `30` |
| `A2A_BRIDGE_AAUTH_LEEWAY_SECS` | How far past `exp`, or ahead of `iat`, a presented token is still accepted, from 0 to 30 seconds. | `A2A_BRIDGE_AAUTH_MAX_SKEW_SECS` |

Every signature carries `created`, `expires` and a `nonce` signature parameter
of at most 256 bytes, which the edge admits once per signing key for as long as
a signature carrying it could still be fresh. With
`A2A_BRIDGE_AAUTH_NONCE=required`, a proof fails unless the request binds a
usable nonce the edge issued to its signing key, carried in `AAuth-Nonce` and
covered as `aauth-nonce`, and the refusal hands out one for the retry, which
signs under a new `nonce` parameter. With `optional`, the edge verifies a nonce
a request binds but issues none, so an instance in `optional` honors the
nonces that an instance in `required` with the same challenge key issued.
Without a challenge key, a proof that binds a nonce fails.

The edge answers a request it refuses with:

| Refusal | Status | Header |
|---------|--------|--------|
| No bearer credential, in every mode | 401 | `WWW-Authenticate: Bearer` |
| No signature, or a proof that does not verify | 401 | `AAuth-Requirement: requirement=agent-token` |
| No usable nonce under `A2A_BRIDGE_AAUTH_NONCE=required` | 401 | `AAuth-Nonce` holding a nonce for the retry |
| A bearer that names no caller, or a caller other than the signer | 401 | `WWW-Authenticate: Bearer error="invalid_token"` |
| A replay bucket that cannot be read or written, or a nonce the edge cannot issue | 503 | none |

### AAuth replay bucket

`shadow` and `enforce` record the `nonce` parameter of every signature they
admit, and every nonce the edge issues, in the PLATFORM key-value bucket
`AAUTH_REPLAY`, which every bridge instance shares. A request one instance
admitted cannot replay through another, and an instance honors a nonce that
another one holding the same challenge key issued. The bucket keeps history 1
and a maximum age of 180 seconds, the longest signature validity plus the clock
skew on either side of it. The bridge opens the bucket and never creates it:
[Provision the A2A bridge replay bucket](../../../../devops/nats/README.md#provision-the-a2a-bridge-replay-bucket)
creates it.

| `A2A_BRIDGE_TRANSPORT` | `enforce` | `shadow` |
|------------------------|-----------|----------|
| `nats` | Refuses to start unless the bucket exists with history 1 and a maximum age of 180 seconds. | Keeps the records in the process when the bucket is unusable, with a startup warning. |
| `stub`, the default | Refuses to start, since the stub transport reaches no bucket. | Keeps the records in the process, with a startup warning. |

When the bucket cannot be read or written while serving, `enforce` refuses the
request with 503, and `shadow` records the failure and admits the request.

### Request records

The bridge logs a record for each request it handles that names the caller,
the bridge as presenter, and the `Trogon-Req-Id` the hops behind the bridge
log the request under
([ADR#0082](../../../../docs/adr/0082-bus-principal-from-subject-and-edge-proof-of-possession.md),
Decision 6).

| Message | Level | Written when |
|---------|-------|--------------|
| `refused the request at the HTTP edge before minting for it` | `WARN` | The edge refuses a request that carries a bearer credential. |
| `AAuth proof of possession failed and shadow mode admitted the request` | `WARN` | `shadow` admits a request whose proof failed. |
| `presenting the request to the gateway on the caller's behalf` | `INFO` | The bridge publishes a request to the gateway. |
| `stream payload is not a JSON-RPC envelope` | `WARN` | An event on a stream the bridge relays to the caller does not decode. |

| Field | Value |
|-------|-------|
| `caller` | The caller, as far as the bridge has verified it. |
| `caller_source` | `subject_principal` for the principal minted for the request, `edge_identity` for the signer the edge verified when the bearer names somebody else, or `unverified` when the bridge has verified nobody. |
| `presenter` | `a2a-bridge`. |
| `req_id` | The `Trogon-Req-Id` the bridge publishes the request under: its JSON-RPC `id` as text, or an id the bridge generates when that is absent, null or not a scalar. |
| `mode` | The edge mode, on the records the edge writes. |
| `reason` | Why the proof failed, on the records the edge writes. |
| `payload` | The event that does not decode, on the stream record. |

## Related

- [A2A bridge sketch (`docs/a2a/explanation/bridge-sketch.md`)](../../../../docs/a2a/explanation/bridge-sketch.md)
- [Gateway roadmap — bridge identity & policy](../../../../docs/a2a/explanation/gateway-roadmap.md)
