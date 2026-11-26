# JWT-SVID issuance and discovery compatibility spike (F04)

Status: local spike evidence, not provider acceptance. This note records one throwaway
experiment against the pinned SPIRE version. It does not change an accepted ADR, does not
approve a deployment artifact and does not establish support for any external provider.
F04 stays open until the residual gaps below are closed.

Run date: 2026-11-15. This note executes the F04 gate in the [workload identity federation
proposal](./federation.md#implementation-sequence-and-proof-gates). It informs, and does
not replace, the draft [ADR#0053](../../adr/0053-external-oidc-federation-surface.md)
decision on the external OIDC surface.

## Question and scope

F04 asks whether SPIRE's native JWT-SVID issuance plus its OIDC discovery helper can serve
as the external assertion issuer that
[ADR#0053](../../adr/0053-external-oidc-federation-surface.md) requires, or whether a
separately governed assertion issuer is needed. The spike measured claim shape, discovery consistency,
rotation, transport options and the privileged-API boundary on a single-host container
topology. It did not exercise any real provider, any public certificate or any Trogon
service.

## Artifacts run

The server and agent are the digests pinned by the repository's SPIRE renderer. The
discovery helper is not pinned by the repository, so the tag was resolved and its digest
recorded here.

| Component | Reference run | Notes |
| --- | --- | --- |
| SPIRE server | `ghcr.io/spiffe/spire-server@sha256:ff546f4c62dafc479a0cd79eae9a7b6df2975754d75efd31706098e0d7548837` | Reports 1.16.0. |
| SPIRE agent | `ghcr.io/spiffe/spire-agent@sha256:ab3a8ce70435374cc5d84c81eabf0278dc046ddbc0a736d59694ba9e5975b1b6` | Reports 1.16.0. |
| OIDC discovery provider | `ghcr.io/spiffe/oidc-discovery-provider:1.16.0`, index digest `sha256:455c1bce62c28b005eea5322c7860abef0350fe7def6717ebf37f76e165e45bc` | Tag resolved at run time; no repository pin exists. |

These are local spike artifacts only. The pinned 1.16.0 GHCR images currently fail release-signature verification under the repository's
renderer gate (`devops/kubernetes/spire/README.md`). This spike pulled them by
digest without that verification, so nothing here makes them an accepted deployment
artifact. The discovery provider image has never passed any signature gate.

Test tooling: a Python verifier using the PyJWT and cryptography libraries (pinned
algorithm list, required `exp`, `iat`, `sub`, `aud`, `iss`, exact issuer and audience
match, key selected by `kid` from fetched JWKS), plus `curl`, `jq` and `openssl`.

## Topology and why

- One SPIRE server per key type under test, SQLite datastore, `join_token` node attestor.
- One SPIRE agent with the `unix` workload attestor. Workloads were separate user IDs
  running the agent's own CLI inside the agent container. This was the narrowest honest way
  to exercise the Workload API: the unix attestor resolves callers through the agent's PID
  namespace, so a workload in another container is not attested unless it joins that
  namespace.
- `spire-server jwt mint` on the server's local admin socket, as the privileged issuance
  path to compare against the Workload API path.
- The discovery provider in three modes: reading the server admin socket, reading the
  agent Workload API, and serving HTTPS from a certificate file.

The trust domain, issuer and audience names below are fictional test values on the
reserved `.test` suffix. No external resolution or real certificate was involved.

### Configuration used (sanitized)

> **Lab only.** Every block below is a throwaway local configuration. `join_token`
> attestation, `insecure_bootstrap`, a world-accessible Workload API socket, a SQLite
> datastore, the `disk` key manager and `insecure_addr` listeners are all prohibited in a
> production profile by [ADR#0098](../../adr/0098-attested-workload-identity.md) and the
> [workload identity reference](../../reference/workload-identity.md). Do not copy them
> into a deployment.

Server (the RSA run; the ES256 run changed only `jwt_key_type`, the trust domain and
`ca_ttl`):

```hcl
server {
  bind_address = "0.0.0.0"
  bind_port = "8081"
  trust_domain = "spike.test"
  data_dir = "/run/spire/data"
  socket_path = "/run/spire/admin/api.sock"
  jwt_issuer = "https://issuer.spike.test"
  default_jwt_svid_ttl = "120s"
  ca_ttl = "1h"            # 6m in the automatic-rotation run
  jwt_key_type = "rsa-2048" # ec-p256 in the ES256 run
}
plugins {
  DataStore "sql" { plugin_data { database_type = "sqlite3"
                                  connection_string = "/run/spire/data/datastore.sqlite3" } }
  NodeAttestor "join_token" { plugin_data {} }
  KeyManager "disk" { plugin_data { keys_path = "/run/spire/data/keys.json" } }
}
```

Agent: `join_token` node attestor, `unix` workload attestor, in-memory key manager,
`insecure_bootstrap = true`, Workload API socket world-accessible.

Registration entries (parent is the attested agent alias): `ns/a/sa/alpha` selected by
`unix:uid:1001` with `jwtSVIDTTL 120`, `ns/a/sa/beta` selected by `unix:uid:1002` with the
server default TTL, and `oidc-helper` selected by `unix:uid:1005`.

Discovery provider variants (lab only: variants A and C serve plaintext HTTP with
`insecure_addr`, which a production profile never enables):

```hcl
# Variant A: admin socket, HTTP behind a notional proxy
domains = ["issuer.spike.test", "localhost"]
insecure_addr = ":8080"
server_api { address = "unix:///run/spire/admin/api.sock" poll_interval = "5s" }

# Variant B: pinned issuer, HTTPS from certificate files
domains = ["issuer.spike.test"]
jwt_issuer = "https://issuer.spike.test"
serving_cert_file { cert_file_path = "/tls/cert.pem" key_file_path = "/tls/key.pem" addr = ":8443" }
server_api { address = "unix:///run/spire/admin/api.sock" poll_interval = "5s" }

# Variant C: Workload API source, no admin socket
domains = ["issuer.spike.test"]
jwt_issuer = "https://issuer.spike.test"
insecure_addr = ":8080"
workload_api { socket_path = "/run/spire/sockets/agent.sock" trust_domain = "spike.test" }
```

The self-signed serving certificate and its key were generated for the run and discarded.

## Results

Evidence labels: pass, fail or not exercised. Every row is local evidence on the pinned
1.16.0 artifacts only.

| Item | Result | Evidence |
| --- | --- | --- |
| `iss` | Pass, with a condition | Token `iss` equals the server `jwt_issuer` exactly. Discovery `issuer` equals it only when the helper is pinned with `jwt_issuer` (see finding on issuer derivation). |
| Exact `sub` | Pass | Workload API token `sub` was exactly the registered SPIFFE ID of the calling uid. A uid asking for another registered identity, and an unregistered uid, received `PermissionDenied` ("no identity issued"). |
| Single `aud` | Pass, in array form | One requested audience produced `"aud": ["https://sts.example.test"]`, a one-element array, never a string. Two audiences produced a two-element array via `jwt mint`. No setting yields a string `aud`. |
| TTL | Pass | Entry TTL 120 and the server default 120 both gave `exp - iat = 120`. `jwt mint -ttl 30s` gave 30. `jwt mint -ttl 24h` was capped to the remaining key lifetime (3461 s on a 1 h key). The Workload API caller cannot choose TTL. |
| Algorithm | Pass | `jwt_key_type = rsa-2048` signs `RS256`; `ec-p256` signs `ES256`. One key type per server at a time, so one issuer cannot offer both. Header `typ` is `JWT`. |
| `kid` | Pass | Header `kid` matched a JWKS key in every run; the verifier selected the key by `kid` and verification succeeded. Claims present: `aud`, `exp`, `iat`, `iss`, `sub` only. There is no `nbf`, `jti` or `cnf`. |
| Discovery and JWKS | Pass, with caveats | `/.well-known/openid-configuration` and `/keys` served as expected; JWKS held only RSA keys in the RSA run (`kty`, `kid`, `alg` RS256, `n`, `e`) and only P-256 keys in the ES256 run. Caveats under findings. |
| Rotation and overlap | Pass | A prepared key is published in JWKS before it signs anything; old keys stay published after activation. See rotation findings. |
| Public TLS | Partial | HTTPS from a certificate file negotiated TLS 1.2 and 1.3. ACME and a real public certificate were not exercised. |
| Privileged API isolation | Partial pass | Admin socket is mode 0770 root:root; Workload API socket is world-accessible but only issues the caller's own identity. Network isolation, deployment placement and a host-owned broker path were not exercised. |
| Authority before and after issuance | Partial pass | Withdrawing a registration stopped further Workload API issuance within about 9 seconds; already issued tokens stayed valid until `exp`; `jwt mint` ignores registration. Trogon's own authority checks were not exercised. |

### Claim shape observed

Decoded from tokens minted in the run (signature and key material omitted):

```json
{"alg": "RS256", "typ": "JWT", "kid": "<key id>"}
{"aud": ["https://sts.example.test"], "exp": <iat+120>, "iat": <now>,
 "iss": "https://issuer.spike.test", "sub": "spiffe://spike.test/ns/a/sa/alpha"}
```

Discovery document (trimmed; unlisted fields are empty):

```json
{"issuer": "https://issuer.spike.test", "jwks_uri": "https://issuer.spike.test/keys",
 "authorization_endpoint": "", "response_types_supported": ["id_token"],
 "subject_types_supported": ["public"],
 "id_token_signing_alg_values_supported": ["RS256", "ES256", "ES384"]}
```

JWKS excerpt, RSA run (modulus trimmed):

```json
{"keys": [{"kty": "RSA", "kid": "<key id>", "alg": "RS256", "n": "uz-04i41a2R0...", "e": "AQAB"}]}
```

The ES256 run published `{"kty": "EC", "crv": "P-256", "kid": "<key id>", "alg": "ES256"}`
with its coordinates. Keys carry no `use` member.

## Findings

### Issuer derivation is a configuration hazard

Without `jwt_issuer` on the helper, the discovery `issuer` and `jwks_uri` were derived from
the request `Host` header (a request to `localhost` returned an issuer of
`https://localhost:18080`). A verifier given the server-minted `iss` and a helper-derived
issuer that differ failed with an invalid-issuer error in the ES256 run, where the helper's
derived host did not match the server's `jwt_issuer`. The helper logs a warning that
`jwt_issuer` also affects `jwks_uri` when `jwks_url` is unset. Pin `jwt_issuer` on the
helper to the exact server value, and treat that equality as a deployment check.

With `domains` set, discovery refused an unlisted host with HTTP 400, but `/keys` still
answered for any `Host` (HTTP 200). The domain list therefore does not restrict the JWKS
endpoint. The helper answered only GET for these paths (POST gave 405, unknown path 404),
returned `Access-Control-Allow-Origin: *` and `Cache-Control: no-cache, no-store,
must-revalidate`. How long a recipient caches the JWKS is therefore the recipient's
choice, not something the helper bounds.

The advertised `id_token_signing_alg_values_supported` is a static list that includes
RS256, ES256 and ES384 in both the RSA and ES256 runs. It does not reflect the actual key
type, so it cannot be used as proof of an RSA-only profile.

### Rotation and JWKS overlap

Manual rotation on the RSA server, observed through a helper polling every 5 seconds:

| Step | JWKS after step | Observation |
| --- | --- | --- |
| `localauthority jwt prepare` | old and new key | New key published within one poll before it signs anything. |
| `localauthority jwt activate` (authority ID required) | old and new key | New tokens carry the new `kid`; a token signed by the old key still verified. |
| An agent fetch for an audience already cached | old key `kid` | The agent served a cached JWT-SVID signed by the old key until the authority was tainted. |
| `localauthority jwt taint` of the old authority | old and new key | Agent logged removal of cached JWT-SVIDs issued by the tainted authority; JWKS unchanged. |
| `localauthority jwt revoke` of the old authority | new key only | A token signed by the old key no longer had a verifying key; the new key verified. |

Automatic rotation with `ca_ttl = 6m` (ES256 server, times relative to first key):

| Time | Event |
| --- | --- |
| 0:00 | Key A prepared and activated; lifetime equals `ca_ttl`. |
| 3:00 | Key B prepared (half of `ca_ttl`) and published immediately. |
| 5:00 | Key B activated (five sixths of `ca_ttl`). |
| 6:00 | Key A expired; key C prepared. JWKS then held A, B and C. |
| 8:19 | Expired key A was still published, about two minutes past its expiry. |

The new key is therefore visible to recipients for about a third of `ca_ttl` before it
signs, which is the window recipient JWKS caches have to refresh. It scales with `ca_ttl`,
so the overlap is configuration the platform must choose, not a fixed property. An expired
key lingered in JWKS beyond the observation window; pruning cadence was not measured. A
token with a valid signature from a lingering expired key is still refused on `exp`, but
expiry of the key does not by itself withdraw it from the published set. Early removal
needs the explicit `taint` then `revoke` sequence.

The production contract is stricter than this observation: a new key is published at least
three refresh intervals before its first signing use, so `ca_ttl`, the signing activation
point and the recipients' refresh interval must be chosen together rather than taken from
the one-third default seen here.

The JWT key lifetime equals `ca_ttl`, and `jwt_key_type` is independent of the X.509 CA
key type (the repository profile selects `ec-p256` for the CA). Choosing RS256 for
external tokens therefore does not force the X.509 SVID profile off ECDSA, but it does
change every JWT-SVID the trust domain issues, including internal ones.

### Public TLS and origin

The helper offers three serving modes. HTTP on `insecure_addr`, intended to sit behind a
TLS-terminating proxy; HTTPS from `serving_cert_file` with certificate-file reload
watching (observed negotiating TLS 1.2 and TLS 1.3; lower versions were not conclusively
tested because the client refused them); and ACME, which refuses to start unless
`tos_accepted = true` and then attempts to serve on port 443. Only the first two were
run. ACME needs public reachability for its challenge and a real DNS name; that was
deliberately not attempted. The chosen external origin, its certificate authority chain
and the public path to the helper remain unproven.

### Privileged API isolation

| Surface | Reach and authority observed |
| --- | --- |
| Server admin socket | Mode 0770, owner root:root. A process in the root group (`--user 1001`, group defaulted to 0) listed entries and could call `jwt mint`; a process as `1001:1001` could not connect. File mode and group membership are the only control in this configuration. |
| `jwt mint` | Mints for any in-domain workload path, registered or not (an identity with no entry was minted). Refused reserved `/spire/...` paths and foreign trust domains. It does not consult registration. |
| Agent Workload API | World-accessible socket. A caller receives only identities its attested selectors match. It named arbitrary audiences freely (several distinct audiences were issued to the same caller), but never another SPIFFE ID. |
| Helper in `server_api` mode | Needs the admin socket, which is the same surface that hosts `jwt mint`. |
| Helper in `workload_api` mode | Ran as an ordinary registered workload with no admin socket, and served the same JWKS. It needed the agent PID namespace to be attested. |

Consequences: a workload that can reach the agent socket can obtain its own SPIFFE
identity's JWT for any audience it names. SPIRE does not bind audience to a recipient
relationship, so recipient governance cannot live in SPIRE. This matches the
[federation proposal](./federation.md#trogon-workloads-accessing-external-resources) rule
that external-recipient issuance stays on the trusted host path. Use the helper's
`workload_api` mode so the public serving process never holds the admin socket.

### Currentness before and after issuance

Deleting the registration entry stopped Workload API issuance after roughly 9 seconds of
agent propagation, while a token issued earlier still verified against JWKS until its own
`exp`. `jwt mint` kept issuing for the withdrawn identity. So native issuance has no
platform-registration gate on the mint path and only an eventual one on the Workload API
path. An external assertion is not revoked on withdrawal beyond its TTL. This confirms the
[lifetime and revocation](./federation.md#lifetimes-caching-and-revocation) position that
platform authority must be checked by Trogon before and after issuance, not inferred from
SPIRE.

## Algorithm compatibility

RS256 is the common denominator. ES256 verified correctly end to end in this spike but is
refused by the verifiers that matter most to the proposal.

| Verifier | RS256 | ES256 | Basis |
| --- | --- | --- | --- |
| Entra federated credentials | Supported | Not supported per [ADR#0053](../../adr/0053-external-oidc-federation-surface.md); the RSA-only JWKS requirement is unconfirmed by current documentation | Provider documentation as summarized in the federation proposal; not exercised. Preserve the conservative RSA-only profile. |
| AWS IAM OIDC | Supported | Documented as supported | Documentation only; not exercised. |
| GCP workload identity federation | Documented as supported | Documented as supported | Documentation only; not exercised. |
| GitHub-style OIDC consumers | Issuer-defined; RS256 is the widely deployed profile | Verifier-defined | Not exercised; no GitHub verifier was run. |
| Trogon `a2a-auth-callout` OIDC verifier | Admitted | Refused | Code reading only; the verifier was not run against these tokens. |

The callout verifier admits only RS256, RS384, RS512 and PS256, PS384, PS512, and builds
decoding keys only from RSA JWK components, so an ES256 token whose `kid` resolves to an
EC key is refused ("OIDC JWK must be RSA"). It requires an HTTPS issuer without query or
credentials, an exact issuer match with the discovery document, a credential-free
`jwks_uri` on the issuer origin, a `kid` header, a zero-leeway `exp`, and an audience
chosen from configured values. The observed discovery document and token claims fit those
requirements on reading, including the one-element `aud` array. It also refetches the JWKS
on each verification, so JWKS overlap is exercised on every call there. None of this was
run.

With one key type per SPIRE server, a single native issuer cannot satisfy both an
RS256-only recipient and an ES256-only recipient. Entra and the callout both require RSA,
so an RSA external issuer is the profile consistent with every verifier above. The cost
is that RSA applies to all JWT-SVIDs the server issues.

## Decision recommendation

Reuse native SPIRE JWT-SVID signing and the OIDC discovery helper as the key-holding and
publication substrate, behind a Trogon-owned governed boundary, and do not build a
separate key-holding assertion issuer now. Configure `jwt_key_type = rsa-2048`, pin the
helper's `jwt_issuer`, run the helper in `workload_api` mode, and allow only the trusted
broker to obtain recipient-specific assertions.

Documented reason: the spike shows that native issuance already meets the technical claim
shape (`iss`, exact `sub`, single-valued audience, finite TTL, RS256, `kid` match) and that
rotation publishes the new key before use and keeps the old key until explicit revocation.
It does not enforce what the platform must: audience-to-recipient binding, registration
currentness on the mint path, or per-recipient key custody. Those must be enforced by the
trusted broker and Trogon authority checks. They belong in the governed boundary either
way, and a separate signer would not remove them. Revisit this recommendation and move to a
separately governed assertion issuer if any of the following turns out to be required:

- A target provider rejects a one-element `aud` array or requires a claim SPIRE cannot add
  (for example `jti`, `nbf` or a recipient-specific claim).
- The platform must give each recipient its own issuer, key or key custody (for example
  KMS-held keys), since SPIRE offers one key set per trust domain.
- RSA for every JWT-SVID in the trust domain is unacceptable to internal consumers.
- Recipient JWKS caching cannot be reconciled with the revocation window using `ca_ttl`
  and the taint-then-revoke procedure.

This is a recommendation for ADR review, not an accepted decision.

## Residual gaps keeping F04 open

- Real external TLS: the chosen public origin, ACME or proxy certificate issuance, chain
  validation by a real recipient and uptime behavior were not exercised.
- Provider acceptance: no Entra, AWS, GCP or GitHub-style verifier consumed these tokens;
  the table above is documentation and code reading only. In particular, whether Entra
  accepts a one-element `aud` array and an RSA-only JWKS was not tested.
- Trogon verifier: the callout's OIDC verifier was read, not run, against these tokens and
  this discovery document.
- Deployment profile: the repository's actual server configuration (Kubernetes
  attestation, KMS key manager, `ca_ttl` of 24 hours) was not used. JWT key behavior under
  a KMS-backed key manager, multi-replica servers and upstream authority was not tested.
  The repository profile sets no `jwt_issuer` or `jwt_key_type` today.
- Isolation in place: socket permissions were observed on one host with shared volumes.
  Network policy, pod placement, a real unprivileged broker identity and denial for
  arbitrary containers were not exercised. The unix attestor's PID-namespace requirement
  for a separate helper container needs a deployment answer.
- Authority currentness: no Trogon registration, grant or generation check was placed
  before or after issuance. Only SPIRE's own entry propagation was measured.
- JWKS pruning: the cadence that removes expired keys was not measured, and recipient
  cache behavior was not observed.
- Artifact acceptance: the 1.16.0 images used here have not passed signature verification
  and the discovery provider image is not yet pinned by the repository.
- Concurrent rotation and issuance at scale, agent restart or reattestation behavior, and
  clock-skew handling by recipients were not exercised.

## Cleanup

All spike containers, volumes and the network were removed after the run. Pulled images
were left in the local Docker cache.
