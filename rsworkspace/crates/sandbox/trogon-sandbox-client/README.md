# Sandbox client

`trogon-sandbox-client` is the SDK for talking to the sandbox broker over NATS. `SandboxClient` sends the broker's
request and reply messages and maps every answer onto a typed result.

## Connecting

`SandboxClient::connect` opens a NATS connection, `SandboxClient::from_client` wraps one you already have, and
`SandboxClient::from_bus` accepts any `Bus` implementation, which is how tests substitute an in-memory broker.
`with_api_key` sends an `Authorization: Bearer` header on every request, and `with_default_timeout` sets the reply
deadline for calls that take no explicit timeout (10 seconds by default).

## Errors

Every call returns `ClientError`:

| Variant | Meaning |
| --- | --- |
| `Denied` | The broker refused the lease operation with a typed reason. |
| `DriveDenied` | The broker refused a drive operation. |
| `Timeout` | No reply arrived before the deadline. |
| `Transport` | The bus or the reply encoding failed. |
| `FileOpUnsupported`, `FileOpFailed`, `FileNotFound`, `EditRejected` | The file operation did not succeed. |

`ClientError::class` reports whether a retry may succeed and whether the request may already have been applied.
`Timeout` and `Transport` both leave the outcome unknown, so retry with the same idempotency key or check state first.

## Files

Files move in chunks of `FILE_TRANSFER_CHUNK_BYTES`, and file and directory transfers verify content with SHA-256.
Requests on the file subjects use the chunked transfer protocol, so payloads larger than the NATS maximum payload
cross the wire.

## Logs

`SandboxClient::follow_logs` streams a command's output as `LogFollowEvent` values. It resumes from the last cursor
after a failed read, reports gaps as `ResyncRequired`, and gives up after `FOLLOW_LOGS_MAX_RECONNECTS` consecutive
failures.
