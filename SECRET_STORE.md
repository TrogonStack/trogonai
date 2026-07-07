# Secret Store Context

## Context

`trogon-gateway` currently treats integration credentials as deployment
configuration. Source config lives in TOML, and credential fields accept either a
literal string or an explicit environment reference such as:

```toml
webhook_secret = { env = "GITHUB_ACME_MAIN_WEBHOOK_SECRET" }
```

That model is useful for operator-managed deployments. It is not enough for
self-serve integrations where an end user registers GitHub, Slack, Telegram,
Discord, Linear, Sentry, or another provider without an operator creating
environment variables or Kubernetes Secrets for each integration.

The target model should not be "no secrets". The target model should be "no
user-managed infrastructure secrets". Users register integrations through the
product, and the platform owns credential storage, rotation, retrieval,
redaction, and audit behavior.

This note is not an ADR. It records the implementation context and likely
interface shape for a future secret store.

## Conversation Decisions

The current design direction is:

- **OpenBao first.** The first production-grade self-hosted/FOSS backend should
  be OpenBao.
- **Infisical as product reference, not default dependency.** Infisical is a
  strong reference for developer experience, environments, access-control UX,
  delivery workflows, and secret lifecycle screens. Do not make it the default
  backend unless the product decision is to outsource the whole secrets
  management surface instead of building the smaller integration-credential
  control plane.
- **AWS second.** AWS Secrets Manager or AWS SSM Parameter Store are useful
  managed cloud adapters, but they are not the primary design default.
- **Valkey is not the secret store.** Valkey may be considered later for
  cache-like or infrastructure-adjacent use only if there is a separate reason.
- **Environment variables are a delivery mechanism, not the whole product
  model.** A platform can expose secrets to workloads as environment variables
  while still storing and managing those secrets in a control-plane secret
  system.
- **OpenBao should not be on every webhook request path.** OpenBao is the
  durable source of truth; the gateway should cache resolved credential material
  for short periods.
- **OpenBao sits behind the domain layer.** The public API should expose
  integration and credential lifecycle commands, not generic OpenBao paths,
  mounts, policies, or storage operations.
- **Trusted services touch plaintext only at narrow boundaries.** The management
  API, OAuth/provider callback workers, and gateway/runtime may handle plaintext
  briefly when they must write or use credential material. Browsers, API
  responses after one-time display, the application database, NATS messages,
  logs, traces, metrics, outbox messages, workflow state, and saga payloads must
  not contain plaintext.
- **Do not store raw secrets in the application database by default.** The DB
  stores integration state, credential metadata, `CredentialRef` values,
  fingerprints, tombstones, cleanup state, and outbox events. OpenBao stores raw
  credential material.
- **There is no distributed transaction between the DB and OpenBao.** Use
  command-handler writes, idempotency, deterministic OpenBao paths,
  transactional outbox, state machines, and reconciliation. If OpenBao does not
  have the secret, a background worker cannot recreate it; the user or provider
  must resubmit material.
- **Cleanup is logical first, physical second.** DB state and gateway projection
  stop runtime use before OpenBao/provider cleanup. Physical cleanup can lag and
  retry because runtime safety must come from domain state, cache invalidation,
  projection refresh, and OpenBao read policy.

The intended backend order is:

```text
1. StaticConfigSecretStore
   Current TOML/env behavior for local development and operator-managed
   deployments.

2. OpenBaoSecretStore
   First real self-hosted/FOSS production backend.

3. AwsSecretsManagerStore / SsmParameterStoreSecretStore
   Managed cloud adapters for AWS deployments.

4. Valkey
   Not a source-of-truth secret store. Possible future cache/infrastructure
   helper only if justified.
```

## Vercel Platform Comparison

Vercel is a useful comparison because it has user-facing secret management. Users
can add project or team environment variables through Vercel's dashboard, CLI,
or API. The values are encrypted at rest, and Vercel also supports sensitive
environment variables whose values are non-readable after creation.

The important distinction is that the environment variable is what the workload
receives. The platform still needs a control-plane storage and management layer
behind the dashboard/API:

```text
user-facing management UI/API
  -> platform secret metadata and encrypted secret storage
  -> deployment/build/runtime environment variables
  -> application reads process environment
```

For Vercel native integrations, the integration provider can return resource
secrets such as database URLs or API keys, and Vercel creates environment
variables for connected projects. During rotation, Vercel updates those
environment variables so future deployments receive the new values.

Public Vercel documentation does not expose the internal storage backend behind
the console. The useful architectural signal is the product boundary:

```text
dashboard / CLI / REST API
  -> environment-variable records scoped to team, project, environment, branch,
     custom environment, or integration resource
  -> encrypted or sensitive value storage
  -> deployment/build/runtime delivery
  -> redeploy or runtime refresh boundary before new values are observed
```

The console is therefore not "just editing `.env`." It is editing platform-owned
metadata and secret material. The environment variable is the final delivery
shape for the workload.

For a Vercel-like product model, this project should use OpenBao behind the
console/API:

```text
Trogonai console / API
  -> application database records for integrations, scopes, environments, and
     delivery rules
  -> OpenBao for raw credential material
  -> CredentialRef links between the two
  -> gateway runtime projection and cache
```

So the answer is yes: when building something like Vercel's dashboard-managed
secret experience, OpenBao is the internal secret backend. It is not the product
surface users interact with. Users interact with integrations, environments,
resources, and delivery controls; the platform stores secret material in
OpenBao.

### Vercel Add Environment Variable Modal

The Vercel modal maps UI fields to a platform-owned environment-variable
resource:

```text
Key
  -> public-ish identifier used by the workload, such as API_TOKEN

Value
  -> secret material submitted once through the console/API
  -> encrypted/sensitive storage behind the platform boundary

Note
  -> non-secret operator metadata for support and rotation context

Sensitive
  -> value cannot be read back from dashboard/CLI after creation
  -> still delivered to build/runtime when authorized

Environments
  -> deployment targets such as Production, Preview, Development, or custom envs

Link to Projects
  -> delivery scope: which projects receive this variable

Import / Add Another
  -> batch creation over the same resource model

Save
  -> create one or more scoped variable records and store the value securely
```

That means Vercel likely treats the modal as a command over two categories of
data:

```text
EnvironmentVariableMetadata
  id
  team_id
  project_ids
  key
  targets
  branch_or_custom_environment
  type: encrypted | sensitive
  note/comment
  created_by
  updated_by
  created_at
  updated_at

EnvironmentVariableSecret
  variable_id
  encrypted_value_or_secret_ref
  version
  sensitivity
```

For Trogonai, the equivalent shape should be:

```text
CredentialDeliveryRule
  owner_id
  integration_id
  credential_ref
  environments
  runtime_services
  allowed_hosts
  display_name
  note

OpenBao
  raw credential material
```

The screenshot's important lesson is that the console edits a scoped delivery
resource, not a raw secret path. The value is only one field in a richer domain
command.

The Vercel-like pieces to copy are:

- team/project/environment scoping;
- optional branch/custom-environment targeting;
- dashboard, CLI, and API management paths over the same underlying resource;
- sensitive values that cannot be read back after creation;
- metadata-only listing and filtering;
- clear delivery boundary between stored value and workload-visible value;
- redeploy/refresh requirement after a value changes;
- integration resources that create/update secrets for connected projects;
- production-only or environment-restricted resources for higher-risk
  credentials.

The pieces not to copy directly are:

- assuming environment variables are the only runtime delivery shape;
- making a redeploy mandatory for every gateway credential change;
- exposing every integration secret as process environment if runtime lookup and
  cache invalidation are a better fit;
- treating dashboard visibility as equivalent to runtime authorization.

For `trogon-gateway`, the equivalent "somewhere" is OpenBao plus application
metadata:

```text
user-facing integration setup
  -> integration metadata in application storage
  -> credential material in OpenBao
  -> CredentialRef stored with the integration
  -> gateway resolves and caches material at runtime
  -> source handler receives rich typed config
```

So this platform should not copy Vercel as "just env vars." A better mapping is:

```text
static/operator secrets
  -> StaticConfigSecretStore
  -> typed source config at startup

self-serve integration secrets
  -> OpenBaoSecretStore
  -> gateway cache
  -> typed source config at runtime
```

Environment variables remain valid for operator-managed deployments and local
development. They are not enough for self-serve integrations because users
should not have to create deployment infrastructure secrets for every provider
connection.

## FOSS Reference Systems

These projects are useful references, but they solve different parts of the
problem.

### OpenBao

Use this as the closest backend reference.

- Stores arbitrary key/value secrets through secrets engines.
- Supports path-oriented policy and versioned KV storage.
- Provides the durable source of truth for credential material.
- Fits the `OpenBaoSecretStore` adapter shape.

What to copy:

- path-oriented credential addressing;
- version-aware reads and writes;
- audit-oriented access model;
- policy separation between read, write, rotate, and revoke workflows.

### OpenBao Versus Vault Enterprise

Do not assume OpenBao is "Vault Enterprise, but open source."

OpenBao is an open-source, community-governed fork of Vault. It gives this
project a FOSS-friendly Vault-style backend and has added some capabilities that
were historically associated with Vault Enterprise, such as namespaces for
multi-tenancy. That is useful, but it does not mean every Vault Enterprise
feature, support model, replication mode, operational workflow, or commercial
integration is present and equivalent.

Treat OpenBao as:

- the closest FOSS Vault-style secret manager;
- good enough to evaluate first for KV credentials, policy, audit, identity,
  leases, revocation, namespaces, transit, transform, PKI, and dynamic secrets;
- a backend whose exact production feature set must be verified against the
  specific OpenBao version we deploy.

Do not treat OpenBao as:

- a drop-in substitute for every Vault Enterprise deployment architecture;
- a guarantee of Enterprise-equivalent disaster recovery or performance
  replication;
- a way to avoid designing Trogonai's own integration metadata, runtime
  projection, credential lifecycle, and gateway cache behavior.

For this project, the important question is narrower than "does it replace every
Vault Enterprise SKU?" The useful question is:

```text
Can OpenBao store and protect integration credential material,
enforce gateway/control-plane access policy,
support revocation/versioning,
and operate reliably enough for our deployment profile?
```

### Enterprise Feature Need Assessment

For the current `trogon-gateway` secret-store use case, the expected need for
Vault Enterprise-style features is low at the beginning.

The project does need production operations:

- highly available OpenBao deployment;
- TLS everywhere;
- strict gateway/control-plane policies;
- audit logging;
- backups and restore drills;
- unseal/key custody plan;
- health checks and alerting;
- short-lived gateway-side cache with fail-closed behavior.

Those are production requirements, not automatically Enterprise requirements.
OpenBao can be evaluated for this first because the gateway should not call the
secret backend on every webhook request. Runtime cache and DB projection should
absorb normal request volume.

Enterprise-class features become important only under stronger conditions:

- **Multi-region disaster recovery.** Needed if a whole region/cluster loss must
  fail over quickly with low data loss and minimal manual work.
- **Secret-backend read scalability.** Needed if cache misses, runtime refreshes,
  transit operations, or dynamic secret generation become high-volume enough that
  a single active OpenBao node is the bottleneck.
- **Central platform tenancy.** Needed if many internal teams or customers need
  delegated administration of their own isolated secret spaces. OpenBao
  namespaces may cover this, but the exact version and limits must be verified.
- **Enterprise governance.** Needed if policy approvals, multi-party
  authorization, advanced policy engines, or formal separation-of-duty workflows
  are compliance requirements.
- **Commercial support and SLA.** Needed if the organization requires vendor
  accountability for the secret backend itself.
- **Strict cryptographic boundary requirements.** Needed if the deployment
  requires certified HSM/KMS integrations, managed keys, or formal key-custody
  controls beyond normal OpenBao operation.

For the near-term architecture, avoid optimizing for those features first. The
better sequence is:

```text
1. OpenBao HA in one region/cluster.
2. Reliable backups, restore drills, audit logs, and alerting.
3. Gateway runtime cache and cache invalidation.
4. Clear manual recovery runbook.
5. Re-evaluate enterprise-class DR/replication only when RTO/RPO or scale
   requirements force it.
```

The first product milestone should not depend on Vault Enterprise-equivalent
features. It should depend on a correct ownership split:

```text
OpenBao = credential material, policy, audit, leases, revocation.
Application DB = integration metadata and CredentialRefs.
Gateway = runtime projection, cache, validation, fail-closed behavior.
Operations = backup, restore, monitoring, unseal, incident runbooks.
```

### Infisical

Use this as a product and developer-experience reference.

- Provides a user-facing platform for managing application secrets, API keys,
  database credentials, certificates, and configuration.
- Has concepts for environments, access controls, delivery across
  infrastructure, rotation, and dynamic secrets.

What to copy:

- user-facing secret-management flows;
- environment-scoped secrets;
- audit and access-control UX;
- CLI/API ergonomics for automation.

What not to copy blindly:

- becoming a general-purpose secrets product if the immediate need is only
  integration credentials for `trogon-gateway`.

### Phase

Use this as another user-facing secrets and environment-variable platform
reference.

- Focuses on creating, managing, and deploying application secrets and
  environment variables.
- Emphasizes end-to-end encryption, runtime injection, team sharing, audit logs,
  and RBAC.

What to copy:

- clear separation between secret storage and runtime injection;
- team/project/environment mental model;
- developer-friendly import and injection workflows.

### Conjur Open Source

Use this as a machine-identity and policy reference.

- Provides open-source secrets management for applications, automation, and
  non-human identities.
- Emphasizes authentication, authorization, RBAC/policy, audit, and secret
  retrieval by trusted workloads.
- Has a stronger machine-access model than a simple dashboard-backed
  environment-variable system.

What to copy:

- explicit machine identity for gateway/runtime secret reads;
- policy-as-code style separation between writers, readers, rotators, and
  auditors;
- workload authentication as a first-class concern.

What not to copy blindly:

- making the first implementation depend on a broad enterprise IAM model when
  OpenBao gives a simpler first backend for this project.

### Coolify

Use this as the closest self-hosted PaaS reference.

- Lets users manage environment variables through a UI.
- Separates build-time and runtime variables.
- Supports team, project, and environment-based shared variables.
- Treats locked secrets differently from editable variables.

What to copy:

- build-time versus runtime availability flags;
- scoped variables by team, project, and environment;
- locked secret UX for values that should not be displayed again.

For `trogon-gateway`, this maps to:

```text
operator/runtime deployment config
  -> static config or environment delivery

self-serve provider credentials
  -> OpenBao-backed credential refs
  -> gateway runtime cache
```

### Forgejo And Gitea Actions

Use these as references for scoped secrets and delivery into untrusted or
semi-trusted execution.

- Secrets can exist at user, organization, or repository scope.
- Secrets are stored encrypted and revealed to workflows only when needed.
- Secret values are not displayed after creation.
- Fork or untrusted workflow behavior requires special restrictions.

What to copy:

- owner scope hierarchy;
- "write once, do not reveal again" UX;
- explicit delivery boundary from stored secret to runtime execution;
- restrictions for untrusted execution contexts.

### Woodpecker CI

Use this as a FOSS CI reference for secret delivery into isolated jobs.

- Pipeline steps can request named secrets and receive them as environment
  values or plugin settings.
- The pipeline definition references a secret by name, not by embedding the raw
  value.
- The runtime delivery boundary is explicit: stored secret first, job/container
  environment second.

What to copy:

- declarative secret references;
- explicit step-level delivery;
- clear distinction between stored secret names and runtime variable names.

### GitLab CI/CD Variables

Use this as a mature CI/CD reference, while remembering that GitLab is
open-core.

- Variables can be scoped at project, group, or instance level.
- Variables can be masked, hidden, protected, or file-type.
- Values are delivered to jobs as environment variables or temporary files.

What to copy:

- visibility levels such as normal, masked, hidden, and protected;
- file-style delivery for credentials that tools expect as files;
- environment and branch protection concepts.

### Kubernetes Secrets

Use this as the infrastructure primitive reference, not the product UX.

- Kubernetes Secrets are the common workload-delivery object in Kubernetes.
- They need RBAC and encryption-at-rest hardening.
- By default, they are not a complete secret-management product.

What to copy:

- delivery to workloads as mounted files or environment variables;
- RBAC-aware access boundaries;
- integration point for self-hosted deployments that already use Kubernetes.

### External Secrets Operator

Use this as a sync/reference pattern for Kubernetes deployments.

- Reads from external secret managers and writes Kubernetes Secrets.
- Demonstrates the "secret manager is truth, platform primitive is delivery"
  model.

What to copy:

- controller-driven reconciliation from source-of-truth to runtime delivery;
- separation between external secret store and Kubernetes delivery object.

### SOPS And Sealed Secrets

Use these as GitOps references, not as the main self-serve secret store.

- SOPS encrypts structured files such as YAML, JSON, ENV, INI, and binary files.
- Sealed Secrets encrypts Kubernetes Secrets so they can be stored in Git and
  decrypted only by the cluster controller.

What to copy:

- safe-at-rest GitOps workflows;
- value encryption while preserving useful metadata;
- operator-managed bootstrap and disaster-recovery patterns.

What not to copy as the primary product model:

- requiring end users to manage encrypted files or Kubernetes manifests for each
  self-serve integration.

### Keywhiz

Use this as a historical architecture reference only.

- Keywhiz was built as a system for centrally managing and distributing secrets.
- Its model is useful for studying encrypted central storage, mTLS client
  access, and secret distribution to services.
- The upstream project is deprecated, so it should not be treated as a current
  implementation candidate.

What to copy:

- central secret service plus trusted client retrieval;
- short-lived local runtime access after a central authorization decision;
- operational lessons around secret distribution.

What not to copy:

- adopting deprecated software for the production backend.

## Goals

- Let end users register integrations without direct access to deployment
  infrastructure.
- Keep integration metadata separate from credential material.
- Keep secrets out of TOML, NATS subjects, NATS headers, logs, metrics, traces,
  URLs, and public API responses.
- Support both hosted and self-hosted deployments.
- Keep current operator-managed configuration as a valid adapter.
- Make credential rotation and revocation first-class.
- Avoid forcing every source adapter to understand storage backends.
- Preserve source-specific validation with existing rich value objects.

## Non-Goals

- Do not replace provider-specific webhook validation logic.
- Do not make every credential decryptable if a verifier is sufficient.
- Do not require Kubernetes Secrets for self-serve hosted use.
- Do not expose raw credentials after registration except where a provider flow
  requires a one-time display.
- Do not publish secret material through NATS.

## Current Credential Types

The gateway has multiple credential classes:

| Source | Credential | Runtime Need |
| --- | --- | --- |
| GitHub | `webhook_secret` | Raw secret for HMAC verification. |
| Discord | `bot_token` | Raw token for gateway connection. |
| Slack webhook | `signing_secret` | Raw secret for request signature verification. |
| Slack socket mode | `app_token` | Raw token for outbound socket connection. |
| Telegram | `webhook_secret` | Equality-style webhook secret validation. |
| Telegram registration | `bot_token` | Raw token for `setWebhook` and bot API calls. |
| Twitter/X | `consumer_secret` | Raw secret for CRC response and signature validation. |
| GitLab | `signing_token` | Equality-style token validation. |
| incident.io | `signing_secret` | Raw secret for signature verification. |
| Linear | `webhook_secret` | Raw secret for signature verification. |
| Microsoft Graph | `client_state` | Equality-style validation. |
| Notion | `verification_token` | Equality-style validation. |
| Sentry | `client_secret` | Raw secret for signature verification. |

The store must handle two broad shapes:

- **Decryptable secrets:** needed when HMAC signing, provider API calls, OAuth
  refresh, bot tokens, or outbound connections require the original value.
- **Verifier-only secrets:** useful when validation is equality-style. The
  platform can store a keyed digest instead of a decryptable secret if the raw
  value is never needed again.

Verifier-only storage should be supported per credential kind, not assumed for
all integrations.

## What Belongs In The Secret Store

Store credential material in OpenBao when the platform must retrieve the raw
value later:

- provider OAuth refresh tokens;
- provider OAuth access tokens if they are needed for API calls and cannot be
  cheaply reissued from the refresh token;
- bot tokens;
- webhook signing secrets;
- provider API keys;
- private keys;
- generated webhook secrets shown once to the user;
- OAuth client secrets;
- external service connection strings.

Use verifier-only storage instead of decryptable storage when the platform only
needs to compare a submitted value:

- platform-issued API keys after first display;
- webhook tokens that use equality comparison and are never needed again in raw
  form;
- personal access tokens issued by this platform if only incoming request
  authentication is needed.

Do not put every auth-related value in OpenBao by default:

- user passwords belong as password hashes in the auth database;
- normal web session tokens belong in the session/token subsystem;
- short-lived JWTs usually do not need storage at all;
- CSRF tokens, nonce values, and one-time login challenges belong in their
  purpose-built short-lived stores;
- integration metadata belongs in the application database and should reference
  credentials by `CredentialRef`.

The rule is:

```text
Need raw value later? OpenBao.
Only need to verify a submitted value? keyed hash / verifier-only.
Short-lived app session state? auth/session store, not OpenBao.
Metadata/routing/status? application database.
```

## API Key Model

API keys need two separate treatments.

See `API_KEY.md` for the standalone API-key design note.

### Provider API Keys

Provider API keys are credentials issued by another service that this platform
must use later. Examples include tokens for GitHub, Slack, Linear, MCP servers,
LLM providers, or other outbound APIs.

Store provider API keys as decryptable credential material:

```text
Provider API key
  -> Credential metadata in application DB
  -> raw value in OpenBao
  -> CredentialDeliveryPolicy controls runtime use
```

Provider API keys should carry delivery policy:

```text
CredentialDeliveryPolicy
  allowed_runtime_services
  allowed_hosts
  injection_locations
  allowed_environments
```

Typical policy:

```text
Bearer provider token
  -> allowed_hosts: provider API hosts
  -> injection_locations: request headers
  -> runtime_services: connector/session allowed to call that provider
```

The raw provider key should not be returned after creation. The UI/API should
show metadata such as display name, kind, fingerprint, last used time, status,
and allowed delivery targets.

### Platform-Issued API Keys

Platform-issued API keys are keys this platform gives to users, services, or
automation so they can call Trogonai APIs.

Do not store platform-issued API keys as decryptable OpenBao secrets by default.
The platform only needs to verify a presented key, not recover the raw value.

Use verifier-only storage:

```text
API key shown once to caller
  -> non-secret key id / prefix for lookup
  -> verifier digest stored in application DB
  -> raw key discarded after display
```

Suggested metadata:

```text
ApiKey
  id
  owner_id
  display_name
  public_prefix
  verifier_digest
  fingerprint
  scopes
  allowed_vaults
  allowed_integrations
  allowed_environments
  created_by
  created_at
  expires_at
  revoked_at
  last_used_at
  last_used_from
```

The verifier digest should be computed with a server-side verifier secret or
pepper. The verifier secret can live in OpenBao or another key-management
backend. The API key record itself stays in the application database.

Platform API key create flow:

```text
1. Generate high-entropy random secret.
2. Create non-secret lookup id / public prefix.
3. Compute verifier digest.
4. Store metadata and verifier digest in DB.
5. Return raw key once.
6. Discard raw key.
```

API key verification flow:

```text
1. Parse public prefix / key id.
2. Load API key metadata and verifier digest.
3. Check owner, scopes, status, expiry, and revocation.
4. Verify presented key against digest in constant time.
5. Record non-secret usage metadata.
6. Authorize requested operation from scopes and domain policy.
```

API keys should not directly grant raw secret access. An API key can authorize a
domain command or a runtime session. The domain layer still decides whether that
command or session may resolve a `CredentialRef`.

High-risk platform keys should also support an asymmetric signed-request mode,
inspired by the Coinbase CDP API-key pattern documented in `API_KEY.md`. In that
mode, the database stores public verification material, permissions, policy,
status, and audit metadata. The private key is generated client-side or returned
once and discarded. OpenBao may store verifier peppers, issuer keys, or
certificate authority material, but individual caller private keys should not be
stored there by default.

The enterprise-ready product direction is the combination of Unkey-style API
key management and Coinbase-style signed high-authority keys. OpenBao supports
that model by protecting provider credentials, verifier material, issuer keys,
and certificate authority material. OpenBao should not become the public API key
management surface.

This is the key distinction:

```text
Provider API key
  -> raw value needed later
  -> OpenBao

Trogonai API key
  -> only verification needed
  -> verifier-only DB record
  -> raw value shown once
```

## Data Model

Integration metadata should refer to credentials by stable references:

```text
Integration
  id
  owner_id
  source
  status
  routing
  credential_refs
  provider_registration_state

CredentialRef
  id
  version
  owner_id
  source
  kind
```

Credential metadata should be queryable without exposing credential material:

```text
CredentialMetadata
  ref
  status
  storage_backend
  created_at
  created_by
  rotated_at
  revoked_at
  last_used_at
  last_failed_at
  fingerprint
```

`fingerprint` must be non-secret. It can be a short digest over a keyed hash,
provider id, or generated credential id. It exists for support, audit, and
rotation screens, not for validation.

## Dynamic Integration Management API

When integrations become user-managed, the API should stop treating source
configuration files as the control plane. Configuration files remain useful for
local development and operator-managed bootstrap, but the normal product path
should be:

```text
management API
  -> validate source-specific request
  -> write credential material to SecretStore
  -> write integration metadata to application database
  -> emit integration changed event
  -> gateway refreshes runtime cache/projection
```

The database should store integration state, routing, owner scope, provider
metadata, and credential references. It should not store raw credential material
unless the chosen adapter is explicitly the encrypted database adapter.

### Strong Domain Layer In Front Of OpenBao

OpenBao should sit behind a strong application/domain layer. The public API
should not expose OpenBao paths, mounts, engines, tokens, policies, or raw secret
operations.

The intended layering is:

```text
HTTP/API/console
  -> command handlers
  -> domain service / aggregate
  -> integration repository
  -> SecretStore port
  -> OpenBao adapter
```

The API should expose domain commands:

```text
CreateGitHubWebhookIntegration
CreateSlackOAuthIntegration
ReplaceTelegramBotToken
DisableIntegration
AttachCredentialToEnvironment
RestrictCredentialDelivery
```

It should not expose storage commands:

```text
PutSecret(path, value)
ReadSecret(path)
WriteOpenBaoPolicy(policy)
MountSecretsEngine(name)
```

Domain enforcement should happen through these boundaries:

- **Type system.** Use rich value objects for `OwnerId`, `IntegrationId`,
  `Environment`, `SourceKind`, `CredentialKind`, `CredentialRef`,
  `CredentialVersion`, `DeliveryTarget`, `AllowedHost`, and routing targets.
- **Source-specific constructors.** A GitHub webhook secret, Slack signing
  secret, Telegram bot token, and OAuth refresh token should not all be plain
  strings by the time they enter the domain.
- **Command-specific authorization.** A caller may be allowed to rotate a Slack
  token but not read credential material, change OpenBao paths, or attach the
  credential to production.
- **State machines.** Integrations and credentials should move through allowed
  states such as `pending`, `active`, `disabled`, `previous`, and `revoked`.
- **Database constraints.** Store only valid owner/source/environment/routing
  relationships, unique active credential refs where required, and explicit
  delivery rules.
- **OpenBao path generation.** The OpenBao adapter should derive paths from
  validated domain values. No external caller should provide arbitrary OpenBao
  paths.
- **OpenBao policy as defense in depth.** OpenBao policies should restrict the
  control plane to write/update operations and restrict the gateway to read only
  the credential paths it is supposed to resolve.
- **Audit events.** Emit domain events such as `IntegrationCreated`,
  `CredentialReplaced`, `CredentialRevoked`, and `DeliveryPolicyChanged` instead
  of generic "secret written" events.

Important domain rules to enforce:

- A credential belongs to exactly one owner scope.
- A credential kind must be valid for its source.
- A verifier-only credential must not be returned as plaintext.
- Production credentials must not be delivered to development or preview unless
  an explicit delivery policy allows it.
- A disabled integration must not resolve runtime credential material.
- A revoked credential version must not be accepted by the gateway.
- A gateway service identity can read runtime material, but a browser/user API
  identity cannot.
- Rotation/replacement must update the DB ref/version and invalidate the gateway
  cache.
- Secret material must never appear in logs, NATS messages, metric labels,
  traces, URLs, or API responses.

If "domain enforcement" also means outbound network domains, model that as part
of credential delivery:

```text
CredentialDeliveryPolicy
  credential_ref
  allowed_environments
  allowed_sources
  allowed_runtime_services
  allowed_hosts
```

For example, a Slack bot token should only be usable by Slack connector code and
only for Slack API hosts. A GitHub App token should only be usable for GitHub
API hosts. This is more relevant for outbound connector calls than inbound
webhook signature verification.

The key rule is:

```text
OpenBao enforces storage access.
The application domain enforces product meaning.
Runtime delivery policies enforce where and how material can be used.
```

### Secret Touch Points And Consistency

Yes, trusted services in this architecture do touch secrets. The goal is not
"no service ever sees plaintext." The goal is "only the smallest trusted
boundary sees plaintext, for the shortest useful time, with domain rules and
audit around that access."

Expected plaintext touch points:

- **Management API / control plane.** Receives raw material when a user creates
  or replaces a credential. It validates the request, writes material to
  `SecretStore`, then discards the raw value.
- **OAuth/provider callback worker.** Receives access tokens, refresh tokens, or
  provider secrets from an upstream provider and writes them to `SecretStore`.
- **Gateway/runtime service.** Resolves `CredentialRef` values into runtime
  material needed for HMAC verification, bot connections, socket connections, or
  outbound provider API calls. It caches material in memory only.
- **Lifecycle worker.** May touch secret material for provider-led validation,
  replacement, or sync workflows when that workflow cannot be completed through
  metadata alone.

Boundaries that should not see plaintext:

- browser clients;
- public API responses after one-time display;
- application database rows, unless using the explicit encrypted database
  adapter;
- retry queues, workflow state, or saga payloads;
- NATS messages, subjects, headers, logs, traces, metrics, URLs, or durable
  workflow payloads;
- domain events and outbox messages.

The domain model should pass `SecretString`-like values only through command
handlers and ports. Domain entities should persist `CredentialRef`,
`CredentialMetadata`, status, fingerprint, and delivery policy, not raw
credential values.

There is no true distributed transaction between the application database and
OpenBao. Do not design as if one exists. Use a saga/state-machine plus outbox
and reconciliation.

The secret-bearing part of the saga should run synchronously inside the trusted
command handler while the submitted `SecretString` is still in memory. Durable
background processing should continue only non-secret work unless the secret has
already been written to OpenBao.

Preferred create flow:

```text
1. Validate and authorize the command.
2. Open DB transaction.
3. Create Integration / CredentialVersion rows with status `pending_secret_write`.
4. Commit DB transaction.
5. Write raw material to OpenBao at a path derived from the credential version id.
6. Open DB transaction.
7. Mark credential version `active` and integration `active` or `pending_provider`.
8. Write IntegrationChanged / CredentialChanged to the transactional outbox.
9. Commit DB transaction.
10. Outbox publisher notifies gateway.
11. Gateway reloads DB projection and resolves refs through cache/SecretStore.
```

Who continues the saga:

```text
Command handler
  -> owns the plaintext step from user/provider input to OpenBao write
  -> performs short in-memory retries while the request is still active

Integration lifecycle worker / reconciler
  -> owns non-secret continuation after OpenBao has the value
  -> activates DB state, publishes outbox events, invalidates caches, and
     cleans up orphaned OpenBao versions

Gateway
  -> consumes projection changes and resolves active CredentialRefs at runtime
```

Why DB intent first:

- the platform has a durable record that a credential version is being created;
- OpenBao paths can be generated from stable domain ids;
- failures can be retried or reconciled without inventing metadata afterward;
- orphan OpenBao secrets are easier to detect because they carry known
  credential metadata.

### Idempotency And Client Retry Contract

Use idempotency for every write command that can create, rotate, revoke,
delete, reconnect, or resubmit credential material.

The client provides an opaque idempotency key for one logical user action. The
server defines the scope. The raw idempotency key must never be treated as
globally unique by itself.

```text
IdempotencyRecord
  owner_id
  workspace_id
  command_namespace
  target_resource_id
  idempotency_key
  request_fingerprint
  operation_id
  resource_id
  response_snapshot
  status
  created_by_actor_id
  created_at
  expires_at
```

Recommended uniqueness:

```text
unique(owner_id, command_namespace, idempotency_key)
```

For operations that target an existing resource, include the target in the
effective scope:

```text
unique(owner_id, command_namespace, target_resource_id, idempotency_key)
```

This means two companies can use the same idempotency key without colliding:

```text
company_a + credential.create + abc123
company_b + credential.create + abc123
```

Those are different scoped records. The client supplies the retry key, but the
server supplies the tenant and command scope after authentication.

Command namespaces should be explicit:

```text
credential.create
credential.rotate
credential.resubmit_secret
credential.revoke
credential.delete
integration.connect
integration.reconnect
integration.disconnect
api_key.create
api_key.reroll
api_key.revoke
```

The backend should compute `request_fingerprint` from the normalized command.
For commands that include secret material, compute the fingerprint while the
secret is in memory and persist only a keyed, non-recoverable fingerprint. Do
not persist the raw secret or full raw request body in the idempotency table.

The contract is:

```text
same owner + same namespace + same key + same request fingerprint
  -> return the same operation/resource/status

same owner + same namespace + same key + different request fingerprint
  -> reject with idempotency conflict

different owner + same namespace + same key
  -> unrelated operation

same owner + different key
  -> new logical operation, subject to pending-operation limits
```

Replay access should still be authorized. A retry is not trusted because it has
the idempotency key. The caller must authenticate, resolve to the same owner
scope, and be allowed to see or continue the operation. By default, response
replay should require the same actor/client that created the idempotency record
or an actor with explicit permission to view the target resource.

The API should return a backend operation id:

```text
{
  "operation_id": "op_123",
  "credential_version_id": "credv_456",
  "status": "pending_secret_write"
}
```

After the first write request, clients should prefer polling the operation:

```text
GET /v1/operations/op_123
```

Retrying the original `POST` with the same idempotency key must converge on the
same `operation_id`; it must not allocate another pending credential version.

The idempotency response snapshot must be metadata-only. It may include ids,
status, validation errors, and safe display fields. It must not include raw
secrets, provider tokens, one-time API keys, private keys, or other material
that would violate the plaintext boundary.

Use additional database constraints to prevent pending-operation explosions:

```text
unique(integration_id, credential_kind, environment)
  where status in (
    'pending_secret_write',
    'rotation_pending',
    'resubmission_required'
  )

max pending credential operations per owner
max pending credential operations per target resource
max retry attempts per operation before backoff
```

If the same user action is retried ten times, the database should still contain
one credential intent and one credential version. Extra rows should appear only
when the user explicitly starts a new logical operation with a new idempotency
key and the pending-operation policy allows it.

Durable idempotency ledgers must distinguish failed in-progress attempts from
completed attempts. If the command fails before a metadata-only response can be
stored, the in-progress record should be cleared or expired quickly so the same
logical retry can run again. If the command succeeds, the completed record
should stay until TTL so the same retry can replay the original response instead
of mutating state again.

Idempotency records and pending credential versions need TTLs:

```text
idempotency record TTL
  -> long enough for client/network retries
  -> short enough to avoid unbounded ledger growth

pending_secret_write TTL
  -> short because plaintext was not persisted
  -> expires to secret_write_failed or needs_secret_resubmission

cleanup_pending TTL
  -> retried by cleanup worker with backoff
  -> escalates if cleanup cannot complete
```

OpenBao writes should include non-secret metadata:

```text
owner_id
integration_id
credential_id
credential_version
source
kind
operation_id
created_at
```

Failure handling:

- **DB intent write fails before OpenBao write:** return failure; no secret was
  stored.
- **Process crashes after DB intent but before OpenBao write:** no secret was
  stored. The reconciler expires `pending_secret_write` after a short TTL and
  marks it `secret_write_failed`; the user or provider must resubmit material.
- **OpenBao write fails after DB intent exists:** mark credential version
  `secret_write_failed`. The command handler may retry briefly while plaintext is
  still in memory. After the request/process ends, automatic retry requires the
  user or provider to resubmit material because the DB and outbox do not contain
  the secret.
- **Process crashes during OpenBao write and outcome is unknown:** reconcile by
  checking the deterministic OpenBao path and operation metadata. If the expected
  version exists, continue with DB activation. If it does not exist, mark
  `secret_write_failed` and require resubmission.
- **OpenBao write succeeds but DB activation fails:** retry activation from
  existing store metadata. The gateway lifecycle handler now has recovery
  commands for write and rotation activation so a worker can append the missing
  lifecycle event without recovering plaintext from the DB, outbox, or client.
  The gateway recovery worker scans the lifecycle event stream, groups decoded
  events by the credential id carried in the payload, replays each aggregate,
  and plans recovery commands from pending lifecycle state and the deterministic
  OpenBao credential id. The worker persists its raw stream cursor in
  `GATEWAY_CREDENTIAL_LIFECYCLE_WORKER_CHECKPOINTS` and only advances it after
  planned recoveries complete without failures. Failed recoveries keep that
  cursor pinned, save bounded retry/backoff metadata on the checkpoint, and
  emit stuck-state reporting once the failure age crosses the worker threshold.
  If activation cannot complete, a reconciler should revoke or delete the
  orphaned OpenBao version.
- **DB activation succeeds but event publish fails:** transactional outbox retry
  publishes the event later.
- **Gateway misses the event:** periodic projection polling or a version cursor
  eventually reloads the integration.

The midway-failure recovery matrix is:

| Failure Point | Secret In DB? | Secret In OpenBao? | Recovery |
| --- | --- | --- | --- |
| Before DB intent commit | No | No | Return failure. |
| After DB intent, before OpenBao write | No | No | Expire pending intent; request resubmission. |
| During OpenBao write | No | Unknown | Check deterministic path/version/operation metadata. |
| OpenBao write succeeded, DB activation failed | No | Yes | Reconciler activates DB state or revokes orphan. |
| DB activation committed, outbox publish failed | No | Yes | Transactional outbox retries. |
| Gateway missed event | No | Yes | Gateway polling/version cursor reloads projection. |

The invariant is:

```text
If OpenBao does not have the secret, no background process can recreate it.
If OpenBao does have the secret, background processes can continue using refs
and metadata only.
```

If the product later needs fully asynchronous secret submission, use an explicit
secure staging design instead of silently putting plaintext in normal workflow
state:

```text
Option A: write directly to OpenBao staging path, then promote by CredentialRef.
Option B: encrypted database adapter with envelope encryption and short TTL.
Option C: require client/provider resubmission on failure.
```

Default to Option C unless there is a strong product reason to support
asynchronous secret-bearing retries.

Preferred replacement flow:

```text
1. Create new credential version as `pending_secret_write`.
2. Write new value to OpenBao.
3. Validate with provider when possible.
4. Mark new version `active`.
5. Mark old version `previous`.
6. Emit outbox event and invalidate gateway cache.
7. Keep `previous` only for an explicit grace period when required.
8. Revoke/delete old version after confirmation or expiry.
```

Preferred disable/delete flow:

```text
1. Mark integration disabled in DB.
2. Emit outbox event and invalidate gateway cache.
3. Gateway stops resolving runtime material for the integration.
4. Revoke/delete OpenBao material asynchronously with retry.
```

This ordering favors fail-closed behavior. If OpenBao cleanup is delayed, the
application domain still says the integration is disabled, so runtime code must
not use the credential.

### Cleanup And Reconciliation

Cleanup should be explicit lifecycle work, not an accidental side effect of
deleting a row.

There are two different meanings of cleanup:

```text
Logical cleanup
  -> domain state prevents future use
  -> gateway cache is invalidated
  -> runtime projection no longer includes the credential

Physical cleanup
  -> OpenBao version/path is revoked, destroyed, or deleted
  -> DB metadata is tombstoned or retained for audit
  -> external provider credential may be revoked when supported
```

Logical cleanup must happen first. Physical cleanup can lag and retry.

Use explicit states:

```text
CredentialVersion
  active
  previous
  pending_secret_write
  secret_write_failed
  revocation_requested
  revoked
  destroy_requested
  destroyed
  cleanup_failed

Integration
  pending
  active
  disabled
  delete_requested
  deleted
  cleanup_failed
```

The cleanup worker should own asynchronous cleanup after domain state is already
fail-closed:

```text
CleanupWorker
  -> scans DB rows in revocation/delete/cleanup_failed states
  -> revokes or destroys matching OpenBao material
  -> revokes provider-side credentials when the provider supports it
  -> records result and next retry time
  -> emits non-secret audit/outbox events
```

Cleanup jobs should be idempotent. Deleting the same OpenBao version twice,
revoking an already-revoked provider token, or processing the same outbox event
twice must be safe.

Recommended cleanup ordering:

```text
Disable integration
  -> mark integration disabled in DB
  -> write outbox event
  -> gateway invalidates cache and stops resolving refs
  -> cleanup worker revokes/destroys OpenBao material if disable is permanent

Delete integration
  -> mark integration delete_requested in DB
  -> write outbox event
  -> gateway invalidates cache and removes runtime projection
  -> cleanup worker revokes provider registration when possible
  -> cleanup worker revokes/destroys OpenBao material
  -> mark integration deleted or tombstoned
```

Do not hard-delete metadata immediately. Keep enough tombstone/audit metadata to
answer support and security questions:

```text
integration_id
owner_id
source
credential_ref
credential_version
credential_kind
fingerprint
created_at
revoked_at
destroyed_at
deleted_at
operation_id
last_cleanup_error
```

The metadata must still be non-secret. Fingerprints are allowed; raw credential
material is not.

Reconciliation should run in both directions:

- **DB -> OpenBao.** For every DB credential state that requires cleanup, verify
  the expected OpenBao path/version is revoked or destroyed.
- **OpenBao -> DB.** For OpenBao secrets carrying Trogonai metadata, verify a
  matching DB credential/version still exists. If not, mark an orphan cleanup
  task and revoke/destroy according to retention policy.
- **DB -> Gateway.** Verify gateways have observed a projection version newer
  than the disable/delete/revoke event.
- **Provider -> DB.** When providers support lookup, verify webhook/bot/token
  registrations still match the active integration state.

Use retention windows instead of immediate physical destruction when useful:

```text
previous credential version -> keep until grace period expires
revoked credential version  -> keep metadata, revoke runtime use immediately
destroyed credential value  -> remove OpenBao material after retention policy
deleted integration         -> tombstone metadata for audit/support
```

Cleanup must never be required for runtime safety. Runtime safety comes from DB
state, gateway projection, cache invalidation, and OpenBao read policy. Cleanup
reduces blast radius and storage residue after the domain has already failed
closed.

### API Resource Shape

The public API should expose an `Integration` resource, not a generic secret
resource:

```text
Integration
  id
  owner_id
  source
  display_name
  status
  routing
  provider_metadata
  credential_refs
  created_at
  updated_at
  disabled_at
```

`credential_refs` are internal references. API responses may include credential
metadata such as kind, version, status, and fingerprint, but should never include
plaintext secret values.

### Example Endpoints

```text
POST   /v1/integrations
GET    /v1/integrations
GET    /v1/integrations/{integration_id}
PATCH  /v1/integrations/{integration_id}
POST   /v1/integrations/{integration_id}/rotate
POST   /v1/integrations/{integration_id}/disable
POST   /v1/integrations/{integration_id}/enable
DELETE /v1/integrations/{integration_id}
```

For source-specific setup flows, use nested actions rather than leaking raw
storage concepts:

```text
POST /v1/integrations/github/webhook
POST /v1/integrations/slack/oauth/start
GET  /v1/integrations/slack/oauth/callback
POST /v1/integrations/telegram/register-webhook
POST /v1/integrations/discord/bot
```

Those endpoints can share the same internal service boundary:

```text
IntegrationService
  create(request)
  update_metadata(integration_id, patch)
  rotate_credential(integration_id, kind, new_material)
  disable(integration_id)
  enable(integration_id)
  delete(integration_id)
```

### Create Flow

For a webhook-style integration:

```text
POST /v1/integrations/github/webhook
{
  "owner_id": "org_123",
  "display_name": "main repo",
  "routing": {
    "nats_subject": "github.acme.main"
  },
  "credentials": {
    "webhook_secret": "raw value from user or generated value"
  },
  "provider": {
    "repository": "acme/main"
  }
}
```

The handler should:

```text
1. authorize owner access;
2. validate the source-specific request into rich value objects;
3. write `webhook_secret` to SecretStore;
4. receive a `CredentialRef`;
5. create the integration row in the database;
6. emit `IntegrationChanged`;
7. return integration metadata only.
```

Response:

```text
201 Created
{
  "id": "int_123",
  "source": "github",
  "status": "active",
  "routing": {
    "nats_subject": "github.acme.main"
  },
  "credentials": [
    {
      "kind": "webhook_secret",
      "version": 1,
      "status": "active",
      "fingerprint": "whsec_7f3a"
    }
  ]
}
```

### Generated Secret Flow

Some providers require the platform to generate a value that the user copies
into the provider dashboard. In that case the API may return the generated value
once:

```text
POST /v1/integrations/github/webhook/generate-secret
```

Response:

```text
201 Created
{
  "integration_id": "int_123",
  "webhook_secret": "shown-once-raw-generated-value",
  "credential": {
    "kind": "webhook_secret",
    "version": 1,
    "fingerprint": "whsec_7f3a"
  }
}
```

After that response, the raw value should not be retrievable through the
management API.

### OAuth Flow

OAuth integrations need a pending state because the credential material arrives
from the provider callback:

```text
POST /v1/integrations/slack/oauth/start
  -> create pending integration setup record
  -> return provider authorization URL

GET /v1/integrations/slack/oauth/callback
  -> validate state
  -> exchange code for tokens
  -> write refresh/access tokens to SecretStore
  -> create or activate integration row in database
  -> emit IntegrationChanged
```

The DB stores provider identity, scopes, workspace/team ids, status, and
credential refs. OpenBao stores refresh tokens, access tokens, and client
secrets when raw values are needed later.

### Runtime Projection

The gateway should not parse config files for dynamic integrations. It should
load a runtime projection from the database:

```text
IntegrationRuntimeConfig
  integration_id
  owner_id
  source
  routing
  provider_metadata
  credential_refs
```

At startup or refresh:

```text
database runtime projection
  -> resolve needed CredentialRefs through SecretStore/cache
  -> build existing typed source config
  -> install/update handler runtime state
```

This keeps the existing source-specific validators and value objects useful.
The difference is the source of the input: database rows instead of TOML files.

### Change Propagation

The control plane should notify the gateway when integration state changes:

```text
IntegrationChanged {
  integration_id
  owner_id
  source
  change_kind
  version
}
```

The gateway can handle this by invalidating the affected cache entry and
reloading that integration from the database. If the event is missed, periodic
polling or a version cursor should eventually reconcile the state.

### Authorization Boundary

The management API needs two different authorization checks:

- user/API access to create, update, rotate, disable, or delete an integration;
- runtime service access for the gateway to read integration projections and
  resolve credential refs.

Users should be able to manage integration metadata and submit new credential
material. They should not be able to read stored raw credential material after
creation.

## OpenComputer Credentials Pattern

OpenComputer's agent credentials model is a useful reference because it treats
user-provided provider keys as hard secrets while still giving users a simple
management API.

Useful ideas to copy or adapt:

- **Credentials are first-class resources.** A credential has an id, provider,
  optional name, default flag, creation time, and display hint. The raw value is
  stored once and referenced by id from another resource.
- **Responses are metadata-only.** Create, list, and attach operations return
  metadata such as id, provider, name, default status, and a short display hint.
  They do not echo the raw value after submission.
- **Attach credentials by reference.** Agents attach to a credential id instead
  of embedding a provider key in every agent definition. For this project,
  integrations should attach to `CredentialRef` values instead of embedding
  credential material in integration rows.
- **Support defaults carefully.** OpenComputer resolves a session credential from
  the agent-specific credential first, then the organization default for that
  provider. A similar model could work for provider-level defaults, but
  integration-specific credentials should still win.
- **Keep browser tokens scoped.** Backend/org-level management credentials should
  stay on the backend. Browser clients should receive only scoped session/API
  tokens that can submit or manage allowed integration operations.
- **Inline creation is ergonomic.** Creating an agent can accept a provider key
  inline, store it as a credential, and attach it in one operation. Integration
  creation can do the same: accept the provider credential once, store it through
  `SecretStore`, then persist only metadata and refs.
- **Stable id, replaceable value.** Updating the value behind an existing
  credential lets running work pick up the safer value without changing every
  attached resource. For this project, a stable credential id with versioned
  values maps well to gateway cache invalidation and runtime refresh.
- **Missing credential is a hard failure.** If no credential can be resolved, the
  session fails to start. For this project, an integration with no resolvable
  required credential should stay `pending`, `failed`, or disabled; it should not
  run with verification silently bypassed.

The sandbox secret-store model also has useful runtime-delivery ideas:

- **Opaque runtime placeholders.** OpenComputer injects sealed placeholders into
  the sandbox instead of real values, then substitutes the real value only in a
  trusted host-side boundary. This is valuable inspiration if future gateway
  work executes untrusted or semi-trusted connector code.
- **Per-secret host restrictions.** Secrets can be limited to specific outbound
  hosts. This maps well to provider API tokens: a Slack token should only be used
  for Slack hosts, a GitHub token for GitHub hosts, and so on.
- **Egress allowlists.** Secret use can be coupled to allowed destinations. This
  is more relevant for outbound connector calls than inbound webhook
  verification.
- **Layered stores.** A base runtime can carry broad credentials while a fork or
  child runtime overlays more specific credentials. This could inspire
  owner/project/integration credential scope rules where the most specific
  credential wins.

Things not to copy blindly:

- **Do not require an egress proxy for inbound webhook verification.** The
  gateway must verify incoming signatures and equality tokens near the HTTP
  boundary. A host-side substitution proxy is a better fit for outbound API calls
  or untrusted execution sandboxes.
- **Do not store raw secrets in the application database by default.**
  OpenComputer's sandbox docs describe encrypted database storage for that
  product. This project's preferred FOSS source-of-truth backend remains
  OpenBao. An encrypted database adapter can still exist as a deployment option.
- **Do not make everything session-pinned.** OpenComputer sessions pin the
  credential selection, while value rotation can flow through. For the gateway,
  integration runtime state should be refreshable through DB projection changes,
  cache invalidation, and explicit lifecycle events.
- **Do not copy the provider model too narrowly.** OpenComputer's credential
  providers are model providers. This project needs a broader source/kind model:
  GitHub webhook secrets, Slack signing secrets, bot tokens, OAuth refresh
  tokens, provider API keys, and verifier-only values.

The most useful adaptation is:

```text
Integration
  -> references CredentialRef metadata
  -> raw value lives in OpenBao
  -> gateway receives only the runtime material it is authorized to use
  -> outbound credentials can carry allowed provider hosts
  -> API responses show metadata/fingerprints only
```

## Managed Agent Credential Vault Pattern

The credential-vault screenshots show a runtime-access model for agents and MCP
servers. This is closer to `trogon-gateway` runtime credential delivery than the
Vercel environment-variable modal because it models who can use a credential,
where it may be injected, and which outbound hosts it may reach.

The visible product model is:

```text
CredentialVault
  id
  display_name
  status
  created_at
  credentials[]

Credential
  id
  display_name
  type
  target key
  delivery policy
  lifecycle state
```

The screenshots show three credential types:

```text
MCP OAuth
  -> for MCP servers that support OAuth
  -> connection flow instead of manual token entry

Bearer token
  -> long-lived API key or personal access token for an MCP server
  -> keyed by MCP server URL

Environment variable
  -> key/value style credential for CLIs, SDKs, or direct API calls
  -> runtime receives an environment-like name, but not necessarily raw storage
```

The useful ideas to copy are:

- **Vault as a user/workspace/owner collection.** A vault groups credentials that
  can be referenced by runtime sessions or gateway projections.
- **Credential type drives validation and delivery.** OAuth, bearer-token, and
  environment-variable credentials have different required fields and different
  runtime behavior.
- **Write-only secret fields.** Token, access token, refresh token, client
  secret, and secret value fields are submitted but should never be returned in
  API responses.
- **Runtime matching key.** MCP credentials are keyed by server URL; environment
  variable credentials are keyed by variable/secret name.
- **Allowed hosts.** A credential can be restricted to specific outbound hosts.
  This is the network-domain enforcement we should copy for outbound provider
  API calls.
- **Injection location.** A credential can be allowed in request headers, request
  body, or both. For Trogonai, this becomes part of `CredentialDeliveryPolicy`.
- **Acknowledgement for shared credentials.** The UI forces the user to
  acknowledge workspace/API-key blast radius before adding the credential.
- **Runtime re-resolution.** Running sessions can re-resolve credentials after
  rotation, archive, or deletion instead of requiring a full restart.
- **Archive versus delete.** Archive can purge secret payloads while retaining a
  non-secret audit record. Delete is hard deletion.

The equivalent Trogonai model should look like:

```text
CredentialVault
  owner_id
  display_name
  status
  metadata

Credential
  credential_ref
  vault_id
  source
  kind
  display_name
  target_key
  delivery_policy
  lifecycle_state
  fingerprint

CredentialDeliveryPolicy
  allowed_environments
  allowed_runtime_services
  allowed_hosts
  injection_locations
```

Raw material still lives in OpenBao:

```text
CredentialVault / Credential metadata
  -> application database

raw credential material
  -> OpenBao

runtime access
  -> gateway/session projection resolves allowed CredentialRefs
```

Do not copy the "workspace-wide shared credential" behavior blindly. If a
credential can be used by any API key in a workspace, the blast radius is large.
For this project, prefer explicit owner, project, environment, integration, and
runtime-service scopes.

For outbound provider calls, this model is stronger than simple env vars:

```text
Slack token
  -> allowed_runtime_services: slack connector
  -> allowed_hosts: slack.com / slack-edge.com as needed
  -> injection_locations: Authorization header only

GitHub token
  -> allowed_runtime_services: github connector
  -> allowed_hosts: api.github.com / github.com as needed
  -> injection_locations: Authorization header only
```

For inbound webhook verification, the gateway should still resolve material near
the HTTP boundary and verify signatures directly. Do not force inbound webhook
verification through an outbound credential-injection proxy.

## Store Interface

The source adapters should not know whether credentials came from TOML,
environment variables, Kubernetes Secrets, NATS KV, a database, or an external
secret manager. They should receive the same typed source config they receive
today.

A future store boundary can look like this:

```rust
pub struct CredentialRef {
    pub id: CredentialId,
    pub version: CredentialVersion,
    pub owner_id: OwnerId,
    pub source: SourceKind,
    pub kind: CredentialKind,
}

pub enum SecretMaterial {
    Plaintext(SecretString),
    Verifier(SecretVerifier),
}

#[async_trait::async_trait]
pub trait SecretStore {
    async fn put(&self, scope: CredentialScope, kind: CredentialKind, value: SecretString)
        -> Result<CredentialRef, SecretStoreError>;

    async fn get(&self, credential: &CredentialRef)
        -> Result<SecretMaterial, SecretStoreError>;

    async fn rotate(&self, credential: &CredentialRef, value: SecretString)
        -> Result<CredentialRef, SecretStoreError>;

    async fn revoke(&self, credential: &CredentialRef)
        -> Result<(), SecretStoreError>;

    async fn metadata(&self, credential: &CredentialRef)
        -> Result<CredentialMetadata, SecretStoreError>;
}
```

The exact Rust API can differ, but the boundary should preserve these
properties:

- `put` accepts raw material only at the trusted write boundary.
- `get` returns raw material only to trusted runtime code.
- `metadata` is safe for UI and support workflows.
- `rotate` creates a new version rather than mutating history in place.
- `revoke` prevents future reads without deleting audit metadata.

## Adapters

### Static Config Adapter

Use this for the current TOML and environment-variable model.

- TOML literal and `{ env = "..." }` values resolve during startup.
- The adapter can synthesize `CredentialRef` values for uniform runtime code.
- No write API is required.
- This remains the default for local development and operator-managed
  deployments.

### Kubernetes Secret Adapter

Use this for self-hosted deployments that already delegate secret lifecycle to
Kubernetes.

- `CredentialRef` maps to namespace, Secret name, and key.
- Reads happen through the Kubernetes API or mounted files.
- Writes should be optional and controlled by RBAC.
- This adapter is useful for operator workflows, not the primary hosted
  self-serve path.

### OpenBao Adapter

Use this as the first production-grade self-hosted/FOSS backend.

- `CredentialRef` maps to an OpenBao path plus credential version.
- Integration metadata stays in application storage; OpenBao stores credential
  material.
- OpenBao owns secret access policy, audit behavior, durability, and backend
  encryption.
- The gateway reads from OpenBao on startup, cache miss, reconnect, explicit
  refresh, rotation, or revocation.
- The gateway should not read OpenBao for every webhook request.
- OpenBao KV storage can hold decryptable credential material. Verifier-only
  credential kinds can instead use keyed digests stored in application storage
  or OpenBao metadata, depending on the access and audit requirements.

### Managed Cloud Secret Manager Adapter

Use this when the deployment already has a managed secret system.

Potential backends:

- AWS Secrets Manager
- AWS SSM Parameter Store
- GCP Secret Manager
- Azure Key Vault
- 1Password Connect

This adapter keeps the gateway/control plane responsible for metadata and
references while delegating encryption, rotation primitives, access policy, and
audit trails to the external provider.

For this project, AWS should be treated as a managed deployment option after the
OpenBao backend, not as the default architecture.

### Encrypted Database Adapter

Use this for hosted self-serve when the platform already has a durable database.

- Store metadata and encrypted payloads in database tables.
- Encrypt secret values with envelope encryption.
- Store key id and ciphertext together.
- Keep data encryption keys out of the database.
- Prefer KMS-managed key encryption keys.

This adapter is operationally straightforward and works well when integration
metadata also lives in the database.

### Encrypted NATS KV Adapter

Use this when the platform wants NATS JetStream KV as the credential backing
store.

- Store only encrypted payloads in KV.
- Keep key material outside NATS.
- Use a stable bucket naming scheme by environment and tenant boundary.
- Include credential version in the KV key or value envelope.
- Do not publish plaintext credentials through subjects.

This fits a NATS-centered runtime, but it still needs KMS or another external
key source for envelope encryption.

### Test Adapter

Use an in-memory adapter for unit tests and local component tests.

- It may store plaintext in memory.
- It must still use the same `SecretStore` interface.
- It should make it easy to assert whether a credential was read, rotated, or
  revoked.

## Encryption

For any adapter that stores raw credentials in a database or KV store, use
envelope encryption:

```text
plaintext secret
  -> data encryption key
  -> ciphertext
  -> data encryption key encrypted by KMS key
```

The stored envelope should include:

```text
version
algorithm
kms_key_id
encrypted_data_key
nonce
ciphertext
created_at
```

Use authenticated encryption. Do not invent cryptographic formats by hand if a
well-maintained crate or provider SDK offers envelope encryption primitives.

Key rotation must support:

- rotating the wrapping key without changing the provider credential;
- rotating the provider credential and creating a new credential version;
- disabling old versions after a grace period.

## Caching

Secret reads are on the request path for webhook verification and on the
connection path for outbound integrations. Caching is useful, but it must be
bounded and revocation-aware.

This is a common secret-manager-backed architecture:

```text
secret manager = durable source of truth
application/service = local short-lived cache
request path = cache read, not remote secret-manager read
```

For this project:

```text
OpenBao = storage, policy, audit, rotation
gateway cache = webhook hot path
source adapter = receives typed credential config
```

The gateway should not do this:

```text
every webhook request -> OpenBao read -> verify signature
```

The gateway should do this:

```text
webhook request -> in-process cache -> verify signature
                 -> OpenBao only on cache miss or refresh
```

Recommended cache shape:

```text
SecretCacheKey
  credential_ref
  version

SecretCacheEntry
  material
  expires_at
  negative_cache_until
```

Cache policy:

- Cache decryptable secrets in memory only.
- Use short TTLs by default, such as 30 to 300 seconds.
- Add jitter so many gateway instances do not refresh at once.
- Never persist decrypted cache entries.
- Do not cache authorization failures for long.
- Include credential version in the cache key.
- Invalidate cache entries on rotation, revocation, and integration disable.
- Record cache hit/miss metrics without secret labels.

For webhook verification, the gateway can usually cache the credential for a
short period because providers retry quickly and request volume can be bursty.
For bot tokens and socket-mode connections, cache for the life of the connection
but re-read on reconnect.

For equality-style credentials that use verifier-only storage, cache the verifier
instead of raw material.

## Rotation Model

Rotation should not require downtime.

Credential states:

```text
pending
active
previous
revoked
expired
```

Verification can accept both `active` and `previous` versions during a grace
period when a provider cannot switch atomically. Outbound calls should use only
the active version.

Rotation flow:

1. User or system creates a new credential version.
2. Platform validates the new credential if the provider supports validation.
3. New version becomes `active`.
4. Prior version becomes `previous`.
5. Gateway invalidates cache for the credential ref.
6. Prior version expires after the grace period or explicit confirmation.

## Registration Flow

For self-serve registration:

1. User selects source and integration name.
2. Platform creates integration metadata with status `pending`.
3. Platform either generates a webhook secret or receives one from the user.
4. Secret store writes credential material and returns `CredentialRef`.
5. Integration metadata stores the reference.
6. Platform registers the webhook with the provider when possible.
7. Gateway starts accepting traffic after metadata and credentials are active.

Provider-specific notes:

- If the provider supports webhook registration through API, prefer platform-led
  registration.
- If the provider requires manual setup, show the user the callback URL and any
  generated secret once.
- If the provider returns verification challenges, store the challenge state in
  integration metadata, not in the secret store.
- If OAuth is involved, treat access tokens, refresh tokens, and client secrets
  as separate credential kinds.

## Runtime Resolution

The gateway should resolve credentials near the point where source config is
materialized.

Static operator config:

```text
TOML/env -> typed source config -> mounted source handler
```

Self-serve config:

```text
integration metadata -> credential refs -> SecretStore -> typed source config -> mounted source handler
```

This keeps existing source handlers mostly unchanged. They still receive
validated rich types such as `GitHubWebhookSecret`, `SlackSigningSecret`, or
`DiscordBotToken`. The difference is that those types are hydrated from a store
instead of directly from TOML.

## Observability

Emit metrics and logs for operations, never values:

- secret read count by backend, source, credential kind, and result;
- secret cache hit/miss count by source and credential kind;
- rotation count by source and credential kind;
- revocation count by source and credential kind;
- provider registration success/failure;
- validation failure counts.

Do not label metrics with tenant ids, user ids, integration ids, secret ids, or
provider tokens unless cardinality and privacy have been explicitly reviewed.

Logs should use credential refs, source, owner scope, and operation result. They
must not include plaintext values or signature headers.

## Failure Behavior

Secret-store failures should fail closed:

- Webhook verification returns unauthorized or service unavailable depending on
  whether the failure is credential mismatch or store unavailability.
- Outbound connector startup should refuse to start without required credentials.
- Provider registration should keep the integration in `pending` or `failed`
  state with a redacted error.
- Repeated store failures should surface in health and readiness checks if they
  prevent configured sources from functioning.

Avoid silently disabling verification because a credential cannot be loaded.

## Current Implementation State

The first implementation slice added a `secret_store` module inside
`trogon-gateway`, following ADR 0002's guidance to start inside the owning crate
until reuse is real.

Current slice includes:

- credential reference value objects;
- source and credential kind enums;
- store operation traits;
- `StaticConfigSecretStore`;
- in-memory test adapter;
- config resolution routed through the static config adapter while preserving
  existing provider-specific validators;
- OpenBao Testcontainers coverage for both adapter copy-in/copy-out behavior and
  the lifecycle-runtime path through `CredentialLifecycleRuntimeHandler`.

Verified commands from the implementation pass:

```text
mise exec -- cargo test -p trogon-gateway
mise exec -- cargo check -p trogon-gateway
mise exec -- cargo test -p trogon-gateway runtime_handler_with_openbao_testcontainer_applies_lifecycle_and_resolves_precise_value
mise exec -- git diff --check
```

The event-sourcing slice now owns protobuf contracts under
`proto/trogonai/gateway/credentials/v1`:

- `commands.proto` defines the create, rotate, revoke request payloads and the
  metadata-only command response shape.
- `events.proto` defines the credential lifecycle event payloads used by the
  event store.
- `idempotency.proto` defines the NATS KV idempotency record used for safe
  retries.
- `projection.proto` defines the NATS KV runtime projection checkpoint used to
  avoid full startup replay after the first successful refresh.
- `state.proto` defines metadata-only credential lifecycle state snapshots used
  by command execution replay.
- `types.proto` defines shared credential refs, source kinds, credential kinds,
  statuses, and storage backends.

The HTTP management API still accepts and returns the existing snake_case JSON
adapter shape. That preserves current route behavior while the protobuf package
becomes the durable contract for command, event, and KV payloads.

The current gateway runtime credential slice covers GitHub, GitLab,
incident.io, Linear, Microsoft Graph, Notion, Sentry, Slack, Telegram, and
Twitter/X webhook routes, plus the Discord gateway runner. Discord is
source-scoped rather than integration-scoped: the management API writes
`/-/credentials/discord/bot-token` into a `CredentialScope::source`, runtime
projection keys it as `discord`, and the runner resolves
`CredentialKind::BotToken` from `RuntimeCredentialRegistry` before opening the
WebSocket when `TROGON_GATEWAY_ENABLE_RUNTIME_DISCORD_CREDENTIALS` is enabled.
Gateway startup now stores the runtime projection cursor in
`GATEWAY_CREDENTIAL_RUNTIME_PROJECTION_CHECKPOINTS`. After the first refresh,
startup scans only new raw lifecycle events, reloads the full changed aggregate
state from the lifecycle event store, applies the resulting decider state to the
runtime projection, and advances the checkpoint after refresh succeeds. After
startup, a continuous checkpointed projection worker keeps using that cursor so
later lifecycle events can update gateway runtime credential resolution without
requiring a process restart.

Credential lifecycle command execution now uses the same scheduler-style
snapshot boundary as the scheduler deciders. Commands read metadata-only
`CredentialLifecycleStateSnapshot` payloads before replay and write snapshots on
a fixed 32-event frequency. Snapshot payloads may include lifecycle refs,
fingerprints, metadata, failure reasons, and revocation facts, but they must not
include plaintext credential material.

The internal credential management API also exposes
`GET /-/credentials/recovery/status` for operators. It uses the same admin token
boundary as create, rotate, and revoke commands and returns only recovery worker
checkpoint metadata: last scanned raw stream sequence, next scan sequence,
consecutive failure count, first failure time, retry-after time, retry-delay
state, and stuck-recovery state. It does not expose secret material, OpenBao
paths, lifecycle event payloads, idempotency records, or recovery command
payloads.

The recovery worker records OpenTelemetry counters under the `trogon-gateway`
meter:

```text
gateway.credential_lifecycle.recovery.passes
gateway.credential_lifecycle.recovery.errors
gateway.credential_lifecycle.recovery.scanned_events
gateway.credential_lifecycle.recovery.recoveries
gateway.credential_lifecycle.recovery.stuck_reports
```

The first operator alerts should watch for `stuck_reports` increasing, repeated
`failed_recovery` pass outcomes without checkpoint advancement, and retry delay
that outlives the stuck-after policy. The recovery status endpoint gives the
operator the matching cursor, failure count, first failure time, and retry-after
time needed to decide whether to wait, inspect OpenBao metadata, or intervene.

The local operator flow now has a concrete compose path:

```text
start NATS and OpenBao
  -> set OPENBAO_ADDR and OPENBAO_TOKEN
  -> set TROGON_GATEWAY_CREDENTIAL_MANAGEMENT_ADMIN_TOKEN
  -> enable the runtime credential flag for the source under test
  -> call /-/credentials with x-trogon-admin-token and Idempotency-Key
  -> receive metadata-only CredentialRef and lifecycle state
  -> gateway runtime resolver reads the active value from OpenBao on cache miss
  -> recovery status and recovery metrics expose pending activation problems
```

The current real OpenBao lifecycle proof writes
`copy-this-value-in-and-out`, rotates it to
`rotated-copy-this-value-in-and-out`, resolves both values through the runtime
registry, revokes the credential, and asserts that lifecycle event payloads do
not contain either plaintext value.

The compose stack was verified with the gateway management API and local
OpenBao:

```text
create GitHub webhook secret
  -> metadata-only active CredentialRef v1
  -> OpenBao CLI reads copy-this-value-in-and-out

rotate GitHub webhook secret
  -> metadata-only active CredentialRef v2
  -> OpenBao CLI reads rotated-copy-this-value-in-and-out at version 2

revoke GitHub webhook secret
  -> metadata-only revoked CredentialRef v2
  -> OpenBao version 2 returns null data with deletion_time set
```

## Documentation Required For Credential Vault Experience

To build the experience shown in the credential-vault screenshots, the project
needs documentation at several levels. This section is a checklist of the docs
or spec sections that should exist before implementation is considered complete.

### Product And UX Specs

- [ ] Credential vault list view: columns, search, filtering, empty state,
  pagination, active/archived/deleted status, and row actions.
- [ ] Create vault flow: required fields, naming rules, owner scope, default
  status, and who can create vaults.
- [ ] Add credential modal: shared fields, type-specific fields, validation
  errors, acknowledgement copy, save behavior, loading state, and failure state.
- [ ] Credential type matrix for MCP OAuth, bearer token, environment variable,
  webhook secret, bot token, OAuth refresh token, provider API key, and
  verifier-only values.
- [ ] Allowed-hosts UX: syntax, wildcard rules, validation, examples, and how
  denied requests are reported.
- [ ] Injection-location UX: headers, body, environment-like delivery, and which
  credential types support each mode.
- [ ] Archive/delete UX: copy, confirmation, retention behavior, recovery
  behavior, and audit/tombstone visibility.
- [ ] Rotation/replacement UX: when old and new versions overlap, what users see,
  and when resubmission is required.
- [ ] Warning and acknowledgement UX for shared credentials and workspace/API-key
  blast radius.

### Domain And Data Model Specs

- [ ] `CredentialVault` model: owner, name, status, created/updated metadata,
  archived/deleted metadata, and authorization scope.
- [ ] `Credential` model: vault id, credential ref, kind, source, display name,
  target key, lifecycle state, fingerprint, and metadata.
- [ ] `CredentialDeliveryPolicy` model: allowed environments, runtime services,
  allowed hosts, injection locations, and provider/source restrictions.
- [ ] `CredentialRef` path/version semantics and how refs map to OpenBao paths.
- [ ] Credential state machine: pending, active, previous,
  pending_secret_write, secret_write_failed, revocation_requested, revoked,
  destroy_requested, destroyed, and cleanup_failed.
- [ ] Integration state machine and how vault credentials attach to integrations,
  runtime sessions, or gateway projections.
- [ ] Tombstone/audit metadata model for archived/deleted credentials and vaults.

### API Contracts

- [ ] Create/list/read/archive/delete credential vault APIs.
- [ ] Add credential APIs for each credential type.
- [ ] Replace/rotate credential APIs.
- [ ] Archive/delete credential APIs.
- [ ] Runtime projection API consumed by gateway/session workers.
- [ ] Credential resolution API internal to trusted runtime services.
- [ ] Protocol Buffers contracts for first-party credential lifecycle events,
  runtime projection records, and other durable NATS or KV payloads owned by
  Trogonai.
- [ ] Scoped idempotency contract for create/replace/delete commands: owner
  scope, command namespace, target resource scope, request fingerprint,
  operation id, metadata-only response replay, conflict behavior, and TTL.
- [ ] Error contract for validation failures, denied hosts, missing credentials,
  store failures, and resubmission-required states.
- [ ] Metadata-only response contract proving plaintext is never returned after
  one-time display.

### Security And Authorization Specs

- [ ] Threat model for credential vaults, runtime resolution, provider tokens,
  outbound injection, and webhook verification.
- [ ] Plaintext touch-point inventory: management API, OAuth callback workers,
  gateway/runtime, lifecycle workers, and forbidden boundaries.
- [ ] RBAC/authorization matrix for users, workspace admins, service identities,
  gateway identities, cleanup workers, and support roles.
- [ ] OpenBao policy model for control-plane writes, gateway reads, lifecycle
  cleanup, and audit-only access.
- [ ] Redaction and logging rules for API responses, errors, traces, metrics,
  NATS messages, workflow payloads, and audit events.
- [ ] Allowed-host enforcement and bypass analysis.
- [ ] Workspace/shared credential blast-radius policy and acknowledgement rules.
- [ ] Retention policy for metadata, tombstones, archived values, destroyed
  values, and provider-side revocation evidence.

### Saga, Failure, And Cleanup Specs

- [ ] DB/OpenBao create saga with DB intent, OpenBao write, DB activation, outbox,
  and gateway projection refresh.
- [ ] Midway failure matrix: before DB intent, after DB intent, during OpenBao
  write, after OpenBao write, after DB activation, after event publication, and
  after gateway miss.
- [ ] Resubmission rules when OpenBao does not have the secret and plaintext was
  not persisted.
- [ ] Secure staging alternatives if asynchronous secret-bearing retries become
  necessary.
- [ ] Cleanup and reconciliation spec: logical cleanup, physical cleanup,
  OpenBao orphan cleanup, provider cleanup, gateway projection verification, and
  retry/backoff.
- [ ] Idempotency and deduplication rules for command handlers, cleanup workers,
  outbox publishers, and gateway refreshes, including pending-operation caps and
  safe resubmission behavior.

### Runtime Delivery Specs

- [ ] Gateway/session projection shape.
- [ ] Cache key, TTL, jitter, invalidation, and version semantics.
- [ ] Runtime resolution rules for active, previous, revoked, disabled, archived,
  and deleted credentials.
- [ ] Outbound injection rules for headers, body, and environment-like delivery.
- [ ] Inbound webhook verification rules that do not depend on outbound
  injection proxies.
- [ ] Provider-specific delivery rules for Slack, GitHub, Telegram, Discord,
  Linear, Sentry, GitLab, Notion, Microsoft Graph, and MCP servers.

### Operations And Runbooks

- [ ] OpenBao deployment profile: dev, single-region HA, production HA, and
  managed cloud fallback.
- [ ] OpenBao auth method for control plane, gateway, lifecycle worker, and
  cleanup worker.
- [ ] OpenBao path convention, mount convention, policy convention, and metadata
  convention.
- [ ] Backup, restore, unseal/key custody, audit log, monitoring, and alerting
  runbooks.
- [ ] Incident runbooks for leaked credential, stuck `pending_secret_write`,
  orphan OpenBao secret, cleanup failure, missed gateway invalidation, and
  provider revocation failure.
- [ ] Migration runbook from TOML/env config to DB/OpenBao credential refs.

### Testing And Acceptance Specs

- [ ] UI acceptance scenarios matching the screenshots.
- [ ] API contract tests for metadata-only responses and type-specific
  validation.
- [ ] Saga failure-injection tests for every midway failure point.
- [ ] Cleanup/reconciliation tests for DB -> OpenBao, OpenBao -> DB,
  DB -> Gateway, and Provider -> DB.
- [ ] Security tests for plaintext redaction, forbidden log payloads, denied
  hosts, unauthorized reads, and service identity boundaries.
- [ ] Load tests for gateway cache hits/misses and OpenBao read pressure.
- [ ] Backup/restore and disaster-recovery drills for OpenBao-backed credentials.

### Existing Coverage In This Note

This file already covers the first draft of:

- OpenBao versus Infisical direction;
- Vercel-style environment variable management;
- credential vault and managed-agent reference patterns;
- domain layer in front of OpenBao;
- plaintext touch points;
- DB/OpenBao saga and midway failure handling;
- cleanup and reconciliation;
- runtime cache guidance;
- rotation/replacement guidance;
- failure behavior and observability basics.

## Suggested Implementation Path

1. Keep current TOML/env behavior.
2. Introduce credential reference value objects and a `SecretStore` trait.
3. Implement a static config adapter that wraps current resolved secrets.
4. Move source config hydration behind a resolver that can use a `SecretStore`.
5. Add an in-memory test adapter.
6. Add integration metadata tables, repository, and runtime projection loading.
7. Add the management API for create, rotate, disable, enable, and delete flows.
8. Add an OpenBao adapter for self-serve production credentials.
9. Add gateway-side cache, rotation, and revocation semantics.
10. Add AWS adapters where managed deployment demand exists.
11. Consider encrypted database, encrypted NATS KV, or Valkey-backed helpers only
   when a separate deployment need justifies them.

Start inside the owning gateway or platform crate until reuse is real. Per ADR
0002, split into a separate crate only when multiple packages need the store
contract or adapters independently.

## Open Questions

- What OpenBao path convention should credential refs map to?
- Which OpenBao auth method should each deployment profile use?
- What is the first integration metadata backend: database, NATS KV, or another
  control-plane store?
- What is the owner boundary: workspace, tenant, user, organization, or project?
- Which sources can safely use verifier-only storage?
- Should secret rotation be synchronous with provider registration or a separate
  workflow?
- How should gateways discover integration metadata changes: watch stream, poll,
  control-plane push, or local reload?
- What is the required revocation latency?
- Which deployment profiles need a Kubernetes Secret adapter?
- Should one gateway instance serve many tenants, or should tenant isolation map
  to process, namespace, or account boundaries?
