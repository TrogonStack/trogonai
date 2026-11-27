# Sandbox credential broker

`trogon-sandbox-credential-broker` issues short-lived, scoped credential tokens to
sandbox workers over NATS request and reply. The broker keeps the long-lived provider
credentials sealed at rest and hands out only a token that is narrow in scope and short
in life.

The crate ships a library and a `trogon-sandbox-credential-broker` binary.

## Subjects

The broker answers three request subjects. The provider and tenant come from the subject,
so a caller cannot ask for another tenant's credential through the payload.

| Subject | Request | Reply |
| --- | --- | --- |
| `sandbox.credentials.issue.<provider>.<tenant>` | `CredentialIssueRequest` | `CredentialIssueOutcome` |
| `sandbox.credentials.revoke.<provider>` | `CredentialRevokeRequest` | `CredentialAckOutcome` |
| `sandbox.credentials.rotate.<provider>` | `CredentialRotateRequest` | `CredentialAckOutcome` |

A refusal by the broker, such as an unknown credential or a denied scope, is part of the
reply. A request that cannot be parsed gets no reply and is logged.

## Storage

`CredentialStore`, `TokenStore`, `AuditLogger`, `CommittedPolicyStore`, and
`CommittedPolicyWriter` are the storage seams. Each has an in-memory implementation for
tests and a NATS implementation for the binary.

| Seam | NATS implementation | Backing |
| --- | --- | --- |
| `CredentialStore` | `NatsKvCredentialStore` | one KV bucket per provider |
| `TokenStore` | `NatsKvTokenStore` | one KV bucket, keyed by token id |
| `AuditLogger` | `JetStreamAuditLogger` | a JetStream stream kept for a year |
| `CommittedPolicyStore`, `CommittedPolicyWriter` | `NatsKvCommittedPolicies` | one KV bucket, one key per tenant and lease |

## Sealing

Every credential record is sealed with AES-256-GCM. Each record gets its own data key,
wrapped under the root key with the tenant as associated data. The payload's associated
data binds the provider, tenant, credential id, and version, so a record copied to another
tenant or version does not open.

`InProcessKey` is the root key, parsed from the standard base64 encoding of 32 bytes. Its
`Debug` output never shows the key. `EnvelopeKms` wraps it for callers that want the
envelope operations. Records written before the envelope format have no associated data;
`CredentialBroker::migrate_legacy` rewrites them, and `--allow-legacy-credentials false`
turns legacy reads off once none remain.

## Policy fence

An issue request may carry a `PolicyFence`: the lease and the policy revision the caller
believes is committed. The broker checks that the committed policy for the lease has that
revision and names the requested pull secret. `FenceMode::Required` also refuses a request
with no fence; `FenceMode::Optional` accepts it. `CommittedFence` lets a worker stamp the
committed revision onto the specs it dispatches.

## Running

| Flag | Environment | Default | Meaning |
| --- | --- | --- | --- |
| `--nats-url` | `NATS_URL` | `nats://127.0.0.1:4222` | NATS server to connect to |
| `--kms-key` | `SANDBOX_KMS_KEY` | required | root key, base64 of 32 bytes |
| `--allow-legacy-credentials` | `SANDBOX_CREDENTIAL_ALLOW_LEGACY` | `true` | keep reading legacy records |
| `--fence-mode` | `SANDBOX_CREDENTIAL_FENCE_MODE` | `optional` | `optional` or `required` |

The observability flags come from `trogon-sandbox-observability`. The broker stops
accepting requests on Ctrl-C and finishes the requests it is already handling.

## Tests

The NATS tests start a server through `trogon_nats::test_support::JetStreamTestServer`,
which needs Docker. Run them with `cargo test -p trogon-sandbox-credential-broker`.
