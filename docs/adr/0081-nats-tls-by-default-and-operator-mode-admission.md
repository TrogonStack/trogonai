---
number: "0081"
slug: nats-tls-by-default-and-operator-mode-admission
status: accepted
date: 2026-10-30
---

# ADR#0081: Every NATS Connection Uses TLS Unless Explicitly Disabled, and Clients Enter Operator-Mode Accounts Only Through Platform Credentials or the Auth Callout

## Context

NATS carries every protocol the platform routes
([ADR#0003](./0003-ai-protocol-transport-taxonomy.md)), tenant secret
resolution included
([ADR#0023](./0023-secret-management-and-key-custody-direction.md)), and
[ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md) makes the NATS
account the primary tenant boundary. Those decisions assume a transport that
encrypts and an account that the server chooses for the client. Neither the
clients nor the servers in the repository enforce either assumption today.

**Clients encrypt only when asked.** First-party binaries connect through
`trogon_nats::connect`, directly or through the `mcp-nats` and `acp-nats`
re-exports, except for the a2a crates described below. In
`rsworkspace/crates/platform/trogon-nats`, `NatsTls` derives `Default` with
nothing set (`src/tls.rs:35-36`), `NatsConfig::new` and `NatsConfig::from_url`
start from that default (`src/auth.rs:64-74`), and `apply_tls` returns the
options untouched unless `NATS_CERT`, `NATS_KEY` or `NATS_CA` is set
(`src/connect.rs:121-125`). Only then does it call `require_tls(true)`. The
crate README (`README.md:22`) and the compose example environment
(`devops/docker/compose/.env.example:52`) document this as "setting any TLS
variable requires TLS". Without those variables, async-nats 0.50.0 upgrades to
TLS only when the URL scheme is `tls://` or the server's INFO line asks for it
(`src/connector.rs:597-603`), and that INFO line arrives in plaintext before
any handshake. Whoever can rewrite it receives the CONNECT frame in the clear,
with any token or password in it. A forgotten variable is a silent downgrade,
not an error.

The a2a crates bypass the shared path. `a2a-auth-callout`
(`rsworkspace/crates/a2a/a2a-auth-callout/src/main.rs:182-189`) and
`a2a-bridge` (`rsworkspace/crates/a2a/a2a-bridge/src/main.rs:122-129`) build
`async_nats::ConnectOptions` themselves, with an optional user and password
and no TLS settings, and the bridge opens a connection the same way for each
caller it serves (`src/inbound.rs:220-225` and `:355-360`).

**Turning TLS on today leaves the crypto provider to the dependency graph.**
rustls 0.23.46 builds a client configuration from the process-level crypto
provider. When none is installed it infers one from crate features, and when
more than one provider is compiled in it panics instead of choosing
(`src/crypto/mod.rs:243-257`). async-nats 0.50.0 builds its configuration that
way (`src/tls.rs:99`). The workspace's own rustls dependency enables
aws-lc-rs (`rsworkspace/Cargo.toml:187`), while async-nats's `ring` feature,
which `trogon-nats` and `trogon-gateway` enable
(`rsworkspace/crates/platform/trogon-nats/Cargo.toml:12`,
`rsworkspace/crates/platform/trogon-gateway/Cargo.toml:18`), and reqwest's
`rustls-tls`, hyper-rustls and bollard enable `ring` in the same rustls. Only
`trogon-gateway` and `trogon-provider-gateway` install a provider
(`rsworkspace/crates/platform/trogon-gateway/src/main.rs:55`,
`rsworkspace/crates/provider/trogon-provider-gateway/src/main.rs:20`), through
a helper that fails when a provider is already installed
(`rsworkspace/crates/platform/trogon-std/src/tls.rs:24-28`). Every other
binary gets whatever its dependency graph implies: where the graph enables
both providers, the first TLS connection panics, and where it enables only
`ring`, NATS TLS runs on `ring` against
[ADR#0015](./0015-rust-tls-library.md)'s preference. async-nats also loads the
system roots only when no root certificate is configured, so a private CA
replaces the public roots rather than adding to them (`src/tls.rs:58-75`).

**The servers trust everything.** The compose NATS server runs
`nats:2.14.2-alpine` with `--jetstream`, `--store_dir=/data` and
`--http_port=8222` and nothing else
(`devops/docker/compose/compose.yml:4-18`): no TLS, no accounts, no
authorization, an unencrypted store, and plain HTTP monitoring. Services reach
it at `nats://nats:4222` (`compose.yml:32`). Clients fall back to
`NatsAuth::None` when no credential is configured
(`rsworkspace/crates/platform/trogon-nats/src/settings.rs:96`,
`src/connect.rs:106`), so any process that can reach the port lands in the
server's default global account. The only server configuration with accounts
and a callout is a config-mode test fixture. `base_server_config` declares
`AUTH` with a static `callout` user and password, `APP`, `SYS` and a tenant
account in the server file, under an `authorization { auth_callout { ... } }`
block
(`rsworkspace/crates/a2a/a2a-auth-callout/tests/nats_server_callout_integration.rs:285-312`),
and the tests that start it are ignored (`:638`, `:675`, `:708`).

**The callout's user JWTs fit neither server mode.** `mint_nats_user_jwt` in
`a2a-auth-callout` sets both `iss` and `issuer_account` to the callout's own
signing key and names the target account in `aud`
(`src/jwt/nats_user_jwt.rs:105`, `:133`, `:136`). nats-server 2.14.1 rejects
`issuer_account` outside operator mode, and in operator mode it places the
user in the account that `issuer_account` names and ignores `aud`
(`server/auth_callout.go:156-243`). The callout also picks the credential
scheme by which CONNECT fields are present: the `jwt` field or an opaque
password as OIDC, then a client certificate as mTLS, then a token as an API
key (`src/dispatcher.rs:49-63`). In operator mode every connection that
reaches the callout carries the sentinel's JWT in that field, so this order
would read every caller as OIDC.

**Other clients lean the right way or cannot encrypt at all.** The OpenBao
client requires TLS unless told otherwise. `openbao_transport_from_env`
returns `TlsRequired` unless `OPENBAO_ALLOW_PLAINTEXT` is `true`
(`rsworkspace/crates/platform/trogon-gateway/src/main.rs:993-1001`), and
`TlsRequired` refuses an `http` address
(`src/secret_store/openbao_secret_store.rs:251-259`). The compose stack sets
that flag (`compose.yml:33-35`). Postgres cannot use TLS at all: the workspace
sqlx dependency enables no TLS feature (`rsworkspace/Cargo.toml:118`), the
scheduler's Postgres projection takes a caller-built `PgPool`
(`rsworkspace/crates/scheduler/trogon-scheduler/src/projections/postgres/store.rs:43`),
and the compose OpenBao reaches its Postgres with `sslmode=disable`
(`compose.yml:108`).

**What this builds on.** [ADR#0015](./0015-rust-tls-library.md) already prefers rustls with aws-lc-rs, and
prefers terminating TLS at the deployment edge with in-process TLS as the
exception. [ADR#0033](./0033-key-custody-product-model.md) puts the NATS
operator and account signing seeds, the callout's signing keys, and the NATS
credentials the callout and the secrets service use for their own connections
on the platform boot path, outside the `SecretStore` and `KeyManagement`
ports. It keeps the callout's signing keys deployment-provisioned until the
callout and the secrets service connect with credentials that are not subject
to callout admission.
[ADR#0042](./0042-nats-trace-context-and-message-path-tracing.md) already
requires nats-server 2.14 or newer.
[ADR#0017](./0017-aauth-agent-authentication.md) specifies a binding in which
agents sign NATS messages themselves, so an agent can be a direct NATS client
and not only an HTTP caller.

## Decision

NATS is treated as a network an attacker can reach. Every connection is
encrypted and mutually authenticated, every client lands in an account that
the operator's keys chose, and the only way to weaken either is a switch whose
name says what it does.

### 1. TLS is the default for every NATS client, and only an explicit switch disables it

`trogon_nats::connect` is the only way first-party code connects to NATS.
`a2a-auth-callout` and `a2a-bridge` move onto it, the bridge's per-caller
connections included.

`NatsTls::default()` means TLS against the system roots, and
`require_tls(true)` is applied unconditionally, so neither the URL scheme nor
the server's INFO line can downgrade a connection. `NATS_CA` names a private
CA, which becomes the client's only trust anchor. `NATS_CERT` and `NATS_KEY`
together add a client certificate for mutual TLS, and either one without the
other is a configuration error.

`NATS_TLS_DISABLED=true` is the only way to get a plaintext connection. It
exists for tests and local debugging, and a client that uses it logs a warning
at startup naming the servers it reaches without TLS. Plaintext is a distinct
`NatsTls::Disabled` value rather than the absence of settings, so
`rg 'NATS_TLS_DISABLED|NatsTls::Disabled'` lists every plaintext path in the
repository, and that search has no hits under `devops/`.

Configuration disables TLS explicitly. Nothing "allows plaintext", and nothing
has to turn TLS on.

The same stance covers the platform's other network clients. Postgres
connections use `sslmode=verify-full` unless the connection string explicitly
says `sslmode=disable`, and no mode in between is accepted: `allow` and
`prefer` fall back to plaintext under another name, and `require` and
`verify-ca` encrypt without checking the server's name. sqlx is built with its
`tls-rustls-aws-lc-rs` feature. The OpenBao client uses `https` unless TLS is
explicitly disabled. Its existing check already enforces that, and
`OPENBAO_ALLOW_PLAINTEXT` gives way to a switch that names the disabling, in
the same form as `NATS_TLS_DISABLED`.

### 2. The servers run in operator mode with a fixed account topology

Every deployed nats-server, the dev compose stack's included, runs in
operator mode under one operator JWT. The operator's identity key signs
nothing in routine operation; an operator signing key signs the account JWTs.
The accounts are fixed in kind:

- `SYS` is the system account. No application user exists in it. Only
  operator tooling holds a `SYS` user, to push account JWTs and to monitor.
- `AUTH` hosts the auth callout. Its account JWT lists the callout service's
  user in `auth_users`, lists the tenant accounts the callout may place users
  into in `allowed_accounts`, and carries the callout's xkey where one is
  used. Every other `AUTH` user is a sentinel, a bearer user with no
  permissions whose only purpose is to trigger the callout.
- `PLATFORM` holds the shared workloads: gateways, bridges, the secrets
  service and schedulers. Each workload is its own user, signed by a
  `PLATFORM` signing key, with its own allow lists.
- Each tenant has its own account. It registers the callout signing key,
  imports what `PLATFORM` exports for it, and has no users except the ones the
  callout mints.

Account JWTs are served by the full resolver, which receives them as pushes
over the system account, keeps them on disk and synchronizes them between
servers. It runs with `allow_delete: false`, and `resolver_preload` carries
the `SYS` account JWT, which a server needs before it can accept any push.

An account can use JetStream only when its account JWT sets JetStream limits,
and a new account JWT sets none (`NewAccountClaims` and `IsJSEnabled` in
`v2/account_claims.go`, `configJetStream` in `server/jetstream.go`). The
bootstrap therefore sets them on `PLATFORM` and on any other account that owns
streams.

The server features this depends on were checked against the nats-server
2.14.1 source and the `nats-io/jwt` v2.8.1 module that release pins: callout
placement and `allowed_accounts` (`server/auth_callout.go`, and
`isAllowedAcount` in `server/accounts.go`), export authorization by account
token position for services and streams alike (`checkAuth` in
`server/accounts.go`), renaming imports with `local_subject`
(`v2/imports.go`), `default_sentinel` (`server/auth.go`), and in-place
JetStream key rotation (`server/jetstream.go`, `server/filestore.go`). [ADR#0042](./0042-nats-trace-context-and-message-path-tracing.md)
already makes 2.14 the minimum server version.

### 3. Inside PLATFORM, the tenant token is the importing account's public key

Tenant traffic reaches the shared workloads through imports, not through
shared users. `PLATFORM` exports each protocol subtree with the account token
on the first token, for example `*.mcp.v1.>` with `account_token_position: 1`.
A tenant account imports `<tenant account key>.mcp.v1.>` with
`local_subject: mcp.v1.>`, and the server authorizes that import only when the
token at that position is the importing account's own public key (`checkAuth`
in `server/accounts.go`).

Subjects inside a tenant account therefore stay bare: the prefix is the
protocol alone, as [ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md) intends when the account is the boundary. Inside
`PLATFORM`, the `{tenant}` component of [ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md)'s `[{tenant}.]{protocol}`
prefix is the tenant's account public key. A request that arrives through a
service import carries a key the server authorized only for the account it
names, so a `PLATFORM` responder reads the tenant from the subject and no
tenant can present another tenant's key. A message that `PLATFORM` publishes
for a tenant can be imported only by the account whose key leads its subject.

This amends [ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md). Inside `PLATFORM`, `{tenant}` is the 56-character
uppercase account public key, exempt from the lower_snake token rule in the
same way as the escape encoding. It costs 57 of the 256 bytes and one of the
16 tokens in [ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md)'s budget and keeps the prefix inside its 128-byte limit.
The slug form of `{tenant}` remains for single-account test fixtures. A
decider in `PLATFORM` that serves several tenants builds its
[ADR#0027](./0027-decider-multi-tenancy-primitive.md) `SubjectScope` from the
same account key token.

A stream import carries messages one way, so a request that a `PLATFORM`
workload sends to a responder inside a tenant account travels through a
service that the tenant account exports and `PLATFORM` imports. How a
responder authorizes a `PLATFORM` workload acting for a tenant is not settled
here.

### 4. Admission is PLATFORM credentials or the callout, and nothing else

Platform workloads connect with nsc-generated credentials for their
`PLATFORM` user through `NATS_CREDS`, and the deployment's NATS bootstrap is
the step that issues those users and their allow lists. The callout service
connects as the `AUTH` user named in `auth_users`, which the server exempts
from the callout, and the secrets service connects as a `PLATFORM` workload.
Both therefore use deployment-provisioned credentials that are not subject to
callout admission, which is the topology [ADR#0033](./0033-key-custody-product-model.md) names as the precondition
for moving the callout's signing keys behind `KeyManagement`. The keys stay
deployment-provisioned until that ordering is proven, as [ADR#0033](./0033-key-custody-product-model.md) requires.

Every other client is a tenant caller and enters only through the callout. It
connects as the `AUTH` sentinel, or with no NATS user JWT on servers that set
`default_sentinel`, and presents its own credential: an OIDC token, an API
key, or its client certificate. The callout answers with a user JWT whose
`issuer_account` is the tenant account and which the callout signing key
registered on that account signs, inside an authorization response signed by
an `AUTH` signing key. The server places the user in the tenant account only
if `AUTH` lists that account in `allowed_accounts` and the account lists the
signing key (`server/auth_callout.go:140-243`). The minted user carries
explicit publish and subscribe allow lists, and everything else is denied. The
callout signing key is registered unscoped, because for a scoped key
nats-server takes the permissions from the scope's template, and the jwt
library rejects a scoped user that sets permissions of its own
(`v2/signingkeys.go:94-106`).

The callout reads credentials in a new order. The connections it sees all
presented the sentinel, so the CONNECT `jwt` field always holds the sentinel's
JWT, and an OIDC token placed there never arrives, because nats-server
replaces a `jwt` value that is not a NATS user JWT with `default_sentinel` or
refuses the connection (`server/auth.go:813-834`). Under `verify: true` every
connection also presents a client certificate. A caller's OIDC token or API
key therefore travels in `auth_token`, or in `user` and `pass`, and the
callout reads those first and treats the certificate as the credential only
when nothing else is presented.

No static `authorization {}` users or passwords exist outside tests, and
application code never connects to `SYS`.

[ADR#0033](./0033-key-custody-product-model.md) describes the callout's signing keys as the keys "that admit platform
workloads to NATS". Under this decision `PLATFORM` credentials admit platform
workloads and the callout's keys admit tenant callers. The keys remain
platform boot keys under that ADR's rule.

### 5. The reference server configuration

```text
port: 4222
tls {
  cert_file: "/etc/nats/tls/server.crt"
  key_file: "/etc/nats/tls/server.key"
  ca_file: "/etc/nats/tls/ca.crt"
  verify: true
}
http: "127.0.0.1:8222"

operator: "/etc/nats/jwt/operator.jwt"
system_account: <SYS account public key>
default_sentinel: <AUTH sentinel bearer user JWT>
resolver {
  type: full
  dir: "/data/jwt"
  allow_delete: false
  interval: "2m"
  timeout: "1.9s"
}
resolver_preload {
  <SYS account public key>: <SYS account JWT>
}

jetstream {
  store_dir: "/data/jetstream"
  cipher: chacha
  key: $JS_KEY
}
```

The client listener requires TLS and a client certificate from the platform
CA on every connection. Clustered deployments give `cluster` and `leafnodes`
their own `tls` blocks with `verify: true`, so routes and leaf connections are
mutually authenticated too. Monitoring stays on loopback, or moves to
`https_port` where it must be reachable from outside the host; the HTTPS
monitor does not ask for client certificates (`getMonitoringTLSConfig` in
`server/server.go`). The client port is reachable only from the platform
network. Parties outside it reach the platform through its HTTPS edges, such
as `trogon-gateway` and `a2a-bridge`.

An agent on [ADR#0017](./0017-aauth-agent-authentication.md)'s NATS binding is a direct NATS client, and it is
admitted like every other tenant caller: it connects from the platform
network with a client certificate from the platform CA, and the callout
places it in its tenant account with its own allow lists. Whether any hop
also verifies the binding's per-message signatures is decided in
[ADR#0082](./0082-bus-principal-from-subject-and-edge-proof-of-possession.md),
which runs proof of possession at the external HTTP edge and on no NATS hop;
transport admission neither depends on those signatures nor replaces them. An agent
outside the platform network has no route to the client port and reaches the
platform through an HTTPS edge.

### 6. JetStream encrypts at rest with a key read from OpenBao

Every JetStream server sets `cipher` and `key`. The cipher is `chacha`, or
`aes` where FIPS-140 mode is required, because nats-server 2.14.1 rejects
`chacha` in that mode (`server/opts.go:2621-2633`). The key never lives in an
image, the repository or a configuration file. The deployment reads it at
startup from a platform-operator mount in OpenBao and passes it to the server
through the environment, which the configuration references as
`key: $JS_KEY`, following [ADR#0007](./0007-configuration-sources.md)'s rule
that secrets come from the environment or the deployment rather than from
configuration files. OpenBao does not depend on NATS, so this adds no cycle to
[ADR#0033](./0033-key-custody-product-model.md)'s boot chain. A JetStream server cannot restart while OpenBao is
unreachable, which is the same restart dependency
[ADR#0052](./0052-cloud-kms-production-seal.md) accepts for OpenBao's own
seal.

Rotation happens in place. Restarting with the new key in `key` and the old
one in `prev_key` makes the server open each stream with whichever key works,
regenerate its key files under the new key, and convert each message block
(`server/jetstream.go:274-317` and `:1520-1529`,
`server/filestore.go:1360-1447`). A cipher change converts the same way, and a
store written in plaintext is encrypted on its first start with a key.
`prev_key` is removed once the conversion has finished, and an old key is kept
until no copy of a store written under it remains.

Encryption at rest is server-wide. It protects disks and their copies, not one
tenant's data from another's; the account boundary does that.

### 7. The dev compose stack runs the same topology

The compose stack runs the topology above rather than a plaintext stand-in:
an operator, the full resolver, `SYS`, `AUTH`, `PLATFORM` and a development
tenant, TLS from a development CA with `verify: true`, and an encrypted store.
A one-shot bootstrap service, in the pattern `openbao-bootstrap` already
follows (`devops/docker/compose/compose.yml:128-154`), generates the
operator, the accounts and their users with nsc, along with the development
CA, the certificates and the store key, on first start, and is a no-op
afterwards. No private key, seed or credentials file is committed; each
checkout generates its own. No compose service sets `NATS_TLS_DISABLED`, and
the healthcheck runs inside the container against the loopback monitor.

### 8. What this supersedes and amends

This decision supersedes the fail-open client default and its documentation
(`rsworkspace/crates/platform/trogon-nats/README.md:22`,
`devops/docker/compose/.env.example:52`), the direct connects in the a2a
crates, the plaintext compose NATS server
(`devops/docker/compose/compose.yml:4-18`), and the "allow plaintext" framing
wherever it appears.

It amends [ADR#0015](./0015-rust-tls-library.md): the workspace has one rustls provider, aws-lc-rs, and
in-process TLS is the norm for NATS clients rather than the exception. It
amends [ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md): inside `PLATFORM`, `{tenant}` is the account public key,
exempt from the lower_snake rule. It refines [ADR#0033](./0033-key-custody-product-model.md)'s boot-key rule as the
admission decision above describes.

Under [ADR#0023](./0023-secret-management-and-key-custody-direction.md), callout-minted tenant JWTs still authenticate tenant callers
of the secrets service, and the tenant still comes from the authenticated
connection, now through the account key the server authorized in the subject.
Platform consumers connect as `PLATFORM` users, and how they resolve secrets
on a tenant's behalf is outside this decision.

## Alternatives Considered

**Keep TLS opt-in and document it.** This is today's behavior. A forgotten
variable stays a silent downgrade, and no documentation makes that failure
visible at runtime.

**Terminate NATS TLS in a sidecar or service mesh.** The server and the
callout would see the proxy instead of the client's certificate, so
`verify: true` and the certificate scheme would stop identifying anyone. A
plaintext hop would remain between the proxy and the process, and clients that
follow the server URLs a cluster advertises would bypass the proxy unless
advertising is turned off. The compose stack and self-hosted deployments have
no mesh to rely on.

**Config-mode accounts with a configuration-file callout.** Every new tenant
would need a server configuration change and reload, tenant boundaries would
live in a file that every server must agree on, and the callout user would
keep a static password.

**`allowed_accounts: ["*"]` on `AUTH`.** The callout could place users into
any account the operator has signed, and the tenant's registration of the
callout signing key would be the only check left. An explicit list keeps a
second, independent control at the cost of updating `AUTH` whenever a tenant
is onboarded.

**One shared account with `{tenant}` prefixes in production.** Isolation
would rest on permissions alone, so one permission mistake would cross
tenants. [ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md) already makes the account the primary boundary.

**A scoped callout signing key.** A scope would cap what the callout can mint
for a tenant, but nats-server would then take each user's permissions from the
scope's template, so per-caller allow lists would have to be expressed through
template tags. That trade is not taken now.

**A host-local secrets broker that mints the NATS JWTs in place.** A per-host
daemon such as Basil keeps the operator, account and callout signing seeds in
its backend, admits a caller by the kernel's uid of the socket peer, and signs
user JWTs on request, so no service process ever holds a seed. The custody
shape is the one [ADR#0023](./0023-secret-management-and-key-custody-direction.md) and [ADR#0033](./0033-key-custody-product-model.md) already chose, with the secrets service
in the broker's place; what the daemon adds is a uid as the admission
credential and a second policy language in front of the same OpenBao. A uid
identifies nothing across containers, which is the only deployment this
repository ships, and the project is pre-1.0 with an unfrozen API. The
in-place signing itself is kept as the target: once Decision 4's ordering
holds, the callout signs through `KeyManagement` and holds no seed, which is
the evolution [ADR#0033](./0033-key-custody-product-model.md) leaves open.

**Rotating the JetStream key by migrating streams.** Copying every stream into
a new store is unnecessary when the server converts in place with `prev_key`.

## Consequences

- The [ADR#0015](./0015-rust-tls-library.md) amendment lands before anything turns TLS on, so no binary can
  hit the provider panic. The TLS default in `trogon-nats`, the a2a migration
  and the compose topology then land together, so no environment needs
  `NATS_TLS_DISABLED` as a stopgap.
- The operator signing key and the account signing keys, the callout signing
  key among them, live in a platform-operator OpenBao mount that deployment
  tooling reads outside the `SecretStore` and `KeyManagement` ports, as
  [ADR#0033](./0033-key-custody-product-model.md) requires of boot keys. The operator identity key stays offline.
- JetStream servers gain a start dependency on OpenBao for their store key.
- nsc becomes a bootstrap dependency of the dev stack and of deployment
  tooling.
- Onboarding a tenant becomes a JWT push instead of a server reload: create
  the account, register the callout signing key and the imports, push it, and
  push `AUTH` with the updated `allowed_accounts`, and `PLATFORM` too when it
  imports a service from the tenant. Operator tooling does this with its `SYS`
  user; self-service onboarding is a later decision. The resolver runs with
  `allow_delete: false`, so offboarding disables or revokes an account rather
  than deleting it.
- Every NATS client needs a certificate from the platform CA, so issuing and
  rotating client certificates becomes deployment work, and each of the
  bridge's per-caller connections pays for a TLS handshake.
- `a2a-auth-callout` changes: `issuer_account` names the tenant account, the
  user JWT is signed by the callout signing key that tenant registers, the
  authorization response is signed by an `AUTH` signing key, and credential
  selection reads `auth_token`, `user` and `pass` before the certificate. Its
  server tests move to operator mode and run under
  [ADR#0010](./0010-testcontainers-for-infrastructure-tests.md)'s
  Testcontainers rule instead of staying ignored.
- `trogon-gateway` and `trogon-provider-gateway` stop installing a crypto
  provider themselves, and any other library path that builds a rustls
  configuration from the process default uses the same idempotent install as
  `trogon_nats::connect`.
- Subjects inside `PLATFORM` spend 57 bytes and one token of [ADR#0055](./0055-nats-subject-design-jsonrpc-bindings.md)'s
  budget on the account key.
- An import rewrites the subject between the exporting and the importing
  account, which is the subject rewriting that
  [ADR#0051](./0051-fully-bound-request-signing.md) Decision 6 leaves
  unsupported in front of signed subjects, so a signature over a subject does
  not survive a hop between a tenant account and `PLATFORM`.
- Monitoring moves to loopback or `https_port`, so anything that scraped the
  plain monitor port from outside the host has to change.
- A FIPS-140 deployment uses `aes`. nats-server in that mode also refuses the
  configuration-file callout and creates no server xkey
  (`server/opts.go:4555-4558`, `server/server.go:709-712`), so such a
  deployment proves the account-JWT callout in that mode before relying on it.

## References

- [ADR#0003: AI Protocol Transport Taxonomy](./0003-ai-protocol-transport-taxonomy.md)
- [ADR#0007: Configuration Sources](./0007-configuration-sources.md)
- [ADR#0010: Unit Tests First, Testcontainers Only When Necessary](./0010-testcontainers-for-infrastructure-tests.md)
- [ADR#0015: Rust TLS Library](./0015-rust-tls-library.md)
- [ADR#0017: AAuth Agent Authentication over a Trogon NATS PoP Binding](./0017-aauth-agent-authentication.md)
- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service](./0023-secret-management-and-key-custody-direction.md)
- [ADR#0027: Declared Subject Scope for Decider Stream Resolution](./0027-decider-multi-tenancy-primitive.md)
- [ADR#0033: Key Custody Product Model](./0033-key-custody-product-model.md)
- [ADR#0042: NATS Trace Context and Message Path Tracing](./0042-nats-trace-context-and-message-path-tracing.md)
- [ADR#0051: Fully Bound Per-Request Signing Contract](./0051-fully-bound-request-signing.md)
- [ADR#0052: Cloud KMS Auto-Unseal Is Mandatory for Production OpenBao](./0052-cloud-kms-production-seal.md)
- [ADR#0055: NATS Subject Design for JSON-RPC Protocol Bindings](./0055-nats-subject-design-jsonrpc-bindings.md)
- [async-nats 0.50.0 `src/tls.rs`](https://github.com/nats-io/nats.rs/blob/async-nats/v0.50.0/async-nats/src/tls.rs#L58-L99)
- [async-nats 0.50.0 `src/connector.rs`](https://github.com/nats-io/nats.rs/blob/async-nats/v0.50.0/async-nats/src/connector.rs#L597-L603)
- [rustls 0.23.46 `src/crypto/mod.rs`](https://github.com/rustls/rustls/blob/v/0.23.46/rustls/src/crypto/mod.rs#L243-L257)
- [nats-server 2.14.1 `server/auth_callout.go`](https://github.com/nats-io/nats-server/blob/v2.14.1/server/auth_callout.go#L140-L243)
- [nats-server 2.14.1 `server/auth.go`](https://github.com/nats-io/nats-server/blob/v2.14.1/server/auth.go#L813-L834)
- [nats-server 2.14.1 `server/accounts.go` `checkAuth`](https://github.com/nats-io/nats-server/blob/v2.14.1/server/accounts.go#L2771-L2778)
- [nats-server 2.14.1 `server/accounts.go` `isAllowedAcount`](https://github.com/nats-io/nats-server/blob/v2.14.1/server/accounts.go#L3357-L3371)
- [nats-server 2.14.1 `server/jetstream.go`](https://github.com/nats-io/nats-server/blob/v2.14.1/server/jetstream.go#L274-L317)
- [nats-server 2.14.1 `server/jetstream.go` `configJetStream`](https://github.com/nats-io/nats-server/blob/v2.14.1/server/jetstream.go#L797-L832)
- [nats-server 2.14.1 `server/filestore.go` `convertCipher`](https://github.com/nats-io/nats-server/blob/v2.14.1/server/filestore.go#L1360-L1447)
- [nats-server 2.14.1 `server/opts.go` `setJetStreamEkCipher`](https://github.com/nats-io/nats-server/blob/v2.14.1/server/opts.go#L2621-L2633)
- [nats-server 2.14.1 `server/server.go` `getMonitoringTLSConfig`](https://github.com/nats-io/nats-server/blob/v2.14.1/server/server.go#L3055-L3060)
- [nats-io/jwt v2.8.1 `v2/exports.go`](https://github.com/nats-io/jwt/blob/v2.8.1/v2/exports.go#L185-L198)
- [nats-io/jwt v2.8.1 `v2/imports.go`](https://github.com/nats-io/jwt/blob/v2.8.1/v2/imports.go#L35-L40)
- [nats-io/jwt v2.8.1 `v2/account_claims.go` `IsJSEnabled`](https://github.com/nats-io/jwt/blob/v2.8.1/v2/account_claims.go#L96-L107)
- [nats-io/jwt v2.8.1 `v2/account_claims.go` `ExternalAuthorization`](https://github.com/nats-io/jwt/blob/v2.8.1/v2/account_claims.go#L183-L194)
- [nats-io/jwt v2.8.1 `v2/account_claims.go` `NewAccountClaims`](https://github.com/nats-io/jwt/blob/v2.8.1/v2/account_claims.go#L342-L359)
- [nats-io/jwt v2.8.1 `v2/signingkeys.go`](https://github.com/nats-io/jwt/blob/v2.8.1/v2/signingkeys.go#L94-L106)
- [NATS ADR-26: NATS Authorization Callouts](https://github.com/nats-io/nats-architecture-and-design/blob/main/adr/ADR-26.md)
- [NATS auth callout](https://docs.nats.io/running-a-nats-service/configuration/securing_nats/auth_callout)
- [NATS decentralized JWT authentication](https://docs.nats.io/running-a-nats-service/configuration/securing_nats/jwt)
- [NATS account resolvers](https://docs.nats.io/running-a-nats-service/configuration/securing_nats/jwt/resolver)
- [NATS JetStream encryption at rest](https://docs.nats.io/running-a-nats-service/nats_admin/jetstream_admin/encryption_at_rest)
- [NATS TLS](https://docs.nats.io/running-a-nats-service/configuration/securing_nats/tls)
- [nsc](https://docs.nats.io/using-nats/nats-tools/nsc)
- [Basil documentation](https://docs.openbasil.org/)
