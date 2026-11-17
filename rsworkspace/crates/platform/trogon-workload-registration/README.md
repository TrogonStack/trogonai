# Governed workload registration

## Administrative runner

The Unix-only administrative binary accepts independently approved registration
snapshots from a trusted controller. It holds no approval signing key and does
not create approvals, watch files, renew authority automatically or restart a
producer claim.

Each command requires an exact tenant, project, trust domain, independently
provisioned nonzero recovery fence and pinned Ed25519 approval public key. The
public key is exactly 32 bytes encoded as 64 hexadecimal characters. The
producer validates its order and verifies domain-separated canonical protobuf
approvals against that pin.

Configure an explicit `NATS_URL` and native authenticated TLS credentials before
invocation. The runner uses the shared native `NatsConfig` environment contract:
`NATS_CA`, `NATS_CERT`, `NATS_KEY`, `NATS_CREDS`, `NATS_NKEY`, `NATS_USER`,
`NATS_PASSWORD`, `NATS_TOKEN`, `NATS_JWT` and `NATS_SEED`. Mutual TLS can supply
authentication; otherwise a native credential is required. JWT authentication
requires its signing seed. Disabled or malformed TLS and implicit server
defaults are refused.

Use a separately constrained provisioning identity to create or verify the
protected registration streams. This command does not read controller input:

```sh
trogon-workload-registration provision \
  --tenant "$TENANT" --project "$PROJECT" --trust-domain "$TRUST_DOMAIN" \
  --recovery-fence "$RECOVERY_FENCE" \
  --approval-public-key "$APPROVAL_PUBLIC_KEY_HEX"
```

Use the scoped publication identity and the trusted controller's pipe for the
long-running producer. Provisioning is a separate operation:

The controller commands below stand for your independently secured approval
service. This package does not supply an approval signer.

```sh
trusted-registration-controller | trogon-workload-registration serve \
  --tenant "$TENANT" --project "$PROJECT" --trust-domain "$TRUST_DOMAIN" \
  --recovery-fence "$RECOVERY_FENCE" \
  --approval-public-key "$APPROVAL_PUBLIC_KEY_HEX"
```

The controller writes complete frames in this order:

1. A four-byte big-endian unsigned payload length, from 1 through 262144 bytes.
1. The raw 64-byte Ed25519 signature.
1. Exactly that many bytes of canonical binary `RegistrationSnapshot` protobuf.

Sign the exported `approval_signing_bytes(config, payload)` result. It includes
`SIGNING_CONTEXT`, the canonical trust-domain byte length as an unsigned
big-endian 16-bit integer, the exact trust-domain bytes and the exact payload.
JSON, PEM, encoded signatures and alternate protobuf encodings are not accepted.
The approver must issue fresh finite approvals with the
required monotonic registration and trust generations. A new frame is an
independently signed approval, not permission to replay or renew an old one.

The runner checks that stdin is an actual readable Unix pipe or FIFO using
Tokio's checked owned-descriptor API. Redirected regular files, terminals and
non-Unix hosts are refused. There is no blocking stdin worker.

The initial complete frame has a five-second input budget. After activation,
waiting for the next frame's first byte is bounded by the current approval's
original monotonic deadline. Once a frame starts, its entire remaining header,
signature and payload have five seconds, further bounded by the current lease.
Broker effects have five seconds and also retain the original incoming and
current approval deadlines. A fresh approval never restarts an older deadline
from a later wall-clock sample.

The producer uses the shared native `AuditWriter` and `JetStreamSink` through
the production wrapper. The platform enforces the durable exclusive claim,
audited activation, live-first denial and broker conflict fencing. The runner
keeps one producer handle and applies signed frames sequentially.

Publisher and revoker NATS credentials remain trusted control-plane
capabilities. Broker ACLs restrict subjects; they do not execute this approval
verifier. Protect those credentials and the controller pipe independently.

Use the independently constrained emergency identity for a signed whole-scope
denial. A snapshot with no bindings or no approved roots is denial-only:

```sh
trusted-denial-controller | trogon-workload-registration withdraw \
  --tenant "$TENANT" --project "$PROJECT" --trust-domain "$TRUST_DOMAIN" \
  --recovery-fence "$RECOVERY_FENCE" \
  --approval-public-key "$APPROVAL_PUBLIC_KEY_HEX"
```

`withdraw` consumes one verified denial frame and exits after the production
operation completes. A grant cannot be submitted through that command.

EOF, malformed input, expired authority, interruption, termination, timeout or
an uncertain operation ends `serve`. The runner does not reclaim the same
fence. Follow the protected producer recovery procedure: independently advance
the fence, provision the new streams and revoke the old publication credential
before a replacement producer is enabled. Do not configure automatic same-fence
restarts as a recovery mechanism.

The runner emits no input data or receipt bodies. Normal success is exit zero
for `provision` and `withdraw`; `serve` remains active until a terminal outcome.
Use the broker's protected state and platform audit to verify accepted effects.
Failure categories are available without echoing arguments, snapshots,
signatures, credentials or private values:

| Exit | Category |
| --- | --- |
| 2 | Configuration, arguments or startup environment |
| 3 | Controller input or frame deadline |
| 4 | Invalid approval or a non-denial withdrawal |
| 5 | Publication failure, poisoned authority or operation deadline |
| 6 | Cancellation or approval expiry |

Use `serve --help`, `withdraw --help` or `provision --help` for static command
reference. Successful local protocol and broker tests do not establish deployed
SPIRE attestation, protected controller custody, issuer availability or recovery
acceptance.
