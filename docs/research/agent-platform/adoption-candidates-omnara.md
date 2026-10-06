# Adoption candidates: Omnara

Part of Agent Definition Research. Written 2026-10-05 against
[`omnara-ai/omnara`](https://github.com/omnara-ai/omnara) at commit
`3e9edc16129c251dc1325380e6c60047c3a2f62e` (cluster-v0.1.39,
omnarad-v0.1.33). Paths below are relative to that repository.

This document is **analysis, not a decision**. It names patterns from Omnara
worth carrying into the platform, and the ones to avoid. Every candidate that
touches `proto/` or a security boundary is gated on an ADR. Where a
conclusion here differs from an accepted record in the
[ADR index](../../adr/index.md), the ADR is authoritative.

## Why Omnara is worth reading

Omnara is an Apache-2.0 managed-agents control plane that positions itself as
"the open source alternative to Claude Managed Agents". It is the closest
open-source implementation of the same product shape: the agent is a durable
log in the control plane, machines are attachable execution environments
reached through an outbound-dialing daemon, and humans interact through
inputs, interactions and an event stream.

It is a Go service on Postgres 18 (system of record, work queue and leases
via `FOR UPDATE SKIP LOCKED`), Valkey 9 (lossy pub/sub and rate limits only)
and S3-compatible blob storage, shipped as Docker Compose. There is no message
broker. Earlier third-party descriptions of Omnara as "Claude Code from your
phone" describe a product that was retired; the PyPI package is a deprecated
redirect.

## Adopt: cheap, high leverage

### Refuse to start when an operation has no authorization policy

`internal/httpapi/openapi_policies.go` maps every OpenAPI operation to a
policy, and the server fails startup if any operation lacks one. An endpoint
cannot ship unprotected by omission.

For us the equivalent is a check that every command subject has an
authorization rule, enforced at service startup or in CI. It complements
[ADR#0026](../../adr/0026-command-authorization-principal.md), which decides
*who* the principal is but not that every command has a rule at all.

### Bind an approval to the input it approved

When a gated tool call is approved, Omnara re-checks at execution time that the
approved input equals the input actually being executed, and fails with
`tool_authorization_invalidated` on mismatch.

`ApproveToolCall` in `proto/trogonai/session/sessions/v1alpha1` carries
`session_id`, `tool_call_id`, `tool_execution_id`, `approved_by` and
`turn_id`, but no digest of the input being approved. Adding one lets the
executor refuse a call whose input drifted after the human said yes. Per the
`events.proto` preamble, a new `LEGACY_REQUIRED` field is nearly free only
while no deployed producer has written these events, so this one has a
deadline.

### Bind ciphertext to its identity with AAD

Omnara's `aes-256-gcm-envelope-v1` scheme uses a fresh 32-byte data key per
secret version, wrapped by a key-encryption key, and puts `org_id`,
`secret_id`, `version_id`, `version_number`, `kind` and `payload_keys` into
the AES-GCM additional authenticated data. A ciphertext copied onto another
row, version or tenant fails authentication.

This costs nothing and should be part of whichever backend
[ADR#0023](../../adr/0023-secret-management-and-key-custody-direction.md),
[ADR#0030](../../adr/0030-customer-controlled-key-backend-routing.md) and
[ADR#0033](../../adr/0033-two-tier-key-custody-product-model.md) settle on.

### One dial-time SSRF guard for every outbound client

`internal/ssrf` validates the resolved address at dial time, which also
defeats DNS rebinding, and blocks private, link-local, cloud metadata, CGNAT
and IANA special-purpose ranges, unwrapping IPv4-embedded IPv6. Redirects are
off by default. Every outbound client goes through it: webhooks, MCP, MCP
OAuth, web fetch, model providers, sandbox provider APIs, Slack, email and
auth connectors.

Today an SSRF check exists only around the JWKS fetch in
`trogon-aauth-verify`. A shared outbound client would cover gateway,
connector and webhook egress the same way. Unlike Omnara, include an explicit
allowlist so self-hosters can reach their own internal services.

### Typed, checksummed token format

Omnara tokens look like `omnara_{pat|org|daemon|oauth}_v1_<base62>_<crc32>`.
The prefix makes leaked tokens detectable by secret scanners and tells an
operator what kind of credential leaked; the checksum rejects malformed
tokens before any lookup. Storage is a SHA-256 of the token, which is
adequate because the token is high-entropy.

## Adopt as design input

### Durable record versus preview, stated per event kind

Every agent has one timeline with four durable event kinds (`agent_input`,
`model_output`, `tool_result`, `context_checkpoint`). The stream also emits
`model_output_delta` ("deltas are a preview, not the record") and
`tool_call_update`, which may arrive out of order and are not replayed on
reconnect. `Last-Event-ID` replays only saved events. Naming the lossy
frames as lossy keeps the replay contract easy to reason about.

### An input inbox with steering

Inputs are stored first and admitted to the timeline later. Delivery modes
are `queued` (FIFO, reorderable before admission), `steering` ("joins the
current turn at the next model call, ahead of queued messages") and
`immediate` (system-only). Steering is mid-turn redirection without waiting
for the turn to end. The session contracts have no equivalent yet.

### Interactions as tool-call states

Permission requests and questions share one form schema and ride the tool-call
state machine. Resolution is first-writer-wins: an identical replay is an
idempotent success, a different answer is a conflict. Interactions are
cancelled when the guarded work disappears, never deleted, so the request
survives for audit.

### Mark model calls whose outcome is unknown

A model call interrupted mid-flight is retried as a new attempt and marked
`outcome_ambiguous: true` (up to 8 attempts), so usage and behaviour admit
the call may have happened twice. `ProvisioningFailureKind.OUTCOME_UNKNOWN`
already expresses this for provisioning; the same honesty belongs on model
calls even where reconciliation does better than a blind retry.

## Consider, but go further than Omnara

- **Write-only secrets API.** Omnara never returns a secret value, not even
  once at creation. That is stricter than
  [ADR#0048](../../adr/0048-one-time-plaintext-exposure.md); worth a
  deliberate check that the one-time exposure window is needed.
- **Outbound-dialing machine daemon.** The shape is right, but Omnara's
  daemon self-update verifies a SHA-256 from the same manifest it downloads,
  with no signature, and its machine token sits in a file readable by the
  workloads it runs. Sign updates and give workloads their own credentials.

## Do not adopt

- **Postgres row locks and leases as the single-writer mechanism.** Omnara
  binds one worker per agent with `agent_runtime_locks` (90 s lease) and a
  maintenance process that reaps dead leases; recovery re-pays for the model
  call. [ADR#0035](../../adr/0035-session-store-decider-aggregate.md) already
  answers the same invariant with a write precondition on the log.
- **Plaintext secret delivery with no masking.** `secret_env` values are
  decrypted by the API and sent in the `ProcessOffer.Env` frame; nothing
  masks them in tool output, only structured logs are regex-scrubbed.
- **API keys that never expire and carry no scopes.** Compare
  [ADR#0049](../../adr/0049-revocation-latency-target.md),
  [ADR#0050](../../adr/0050-signed-first-caller-authentication.md) and
  [ADR#0051](../../adr/0051-fully-bound-request-signing.md).
- **Permissive machine defaults.** Built-in tools, `run_command` included,
  default to `always_allow`; BYO machines run without a sandbox; managed pools
  run Ubuntu 24.04 as root (`docs/machines/pools.mdx`).
- **Agent acts as its project.** The launching user is not recorded as the
  principal and no user identity reaches MCP servers, which loses the
  attribution [ADR#0026](../../adr/0026-command-authorization-principal.md)
  requires.
- **Tenancy by query filter only, with no audit log.** Isolation rests on
  composite foreign keys and query filters, with no row-level security and no
  audit table in the 46 migrations at this commit.
