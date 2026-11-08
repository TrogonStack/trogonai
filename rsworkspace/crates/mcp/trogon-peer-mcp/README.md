# Peer native connector reference

This library implements the `peer_prompt_hook` MCP tool for native
`UserPromptSubmit` delivery. It uses the repository's locked rmcp SDK and accepts
an injected backend implementing the typed enrollment, preparation, roster,
send, status, heartbeat, detach and application-acknowledgment ports with generated
peer request/reply contracts. `RpcPeerBackend` delegates to `PeerRpcClient`.
`trogon-peer-mcp` is the Unix stdio executable. `serve_connector` provides reusable
transport/lifecycle composition. Credentials are issued outside this crate.

`PeerMcp::new` validates installation, connector, collaboration and display-name
configuration with the core peer value objects. The first valid prompt hook
reserves the connector for one native session. Each prompt re-enrolls or
revalidates that session before preparing a batch. Another native session is
rejected before accessing the backend. Backend enrollment must return a valid
attached lease for the configured connector and the same stable peer identity.

Hook inputs contain only `session_id`, optional `prompt_id`, and the exact
`UserPromptSubmit` event name. Unknown fields are rejected. Human prompts,
working directories, transcript paths, and caller-selected sender identities
are not forwarded.

Prepared message recipient and collaboration must match the bound enrollment
and connector configuration. The shared `trogon-peer-context` renderer checks
assignment consistency, required provenance, escaping, and rendered byte bounds.
The result is one MCP text item containing native hook JSON with
`hookSpecificOutput.additionalContext`. The adapter returns the backend's same
unacknowledged assignment on retry and records no acknowledgment of its own.

Tool failures become MCP application error results with bounded static messages,
so a native hook can report failure without blocking an ordinary prompt. Typed
backend sources remain available through the direct `invoke` API; peer bodies
are excluded from adapter `Debug` output. The backend remains responsible for
durability, current authorization, generation fencing, content retrieval and
digest verification. Retry content unavailability preserves the backend's
existing preparation record.

Model-facing `peer_roster`, `peer_send`, `peer_status`, and
`peer_acknowledge_batch` require a native hook binding. Their arguments cannot
replace its attachment, sender, principal, native session, or collaboration.
Core value objects validate peer identities, idempotency keys, reply references,
and batch identifiers; message text is bounded to 2 KiB of UTF-8 bytes and has
redacted `Debug` output. Pagination fields remain opaque service query inputs;
the backend validates cursor ownership and current visibility.

These tools return metadata only. Message bodies, internal content references,
digests, and hook context are excluded from their results. Send/status receipts
must name the bound sender and configured collaboration, and send receipts must
match the requested recipient and byte count. Application acknowledgment succeeds
only after the backend returns the requested batch with an acknowledgment fact.
An unconfirmed send or acknowledgment is reported explicitly; retries retain
the original key or batch. No local success or stronger delivery fact is invented.

A local attachment reference is routing state, not proof of current authority.
The backend checks policy and live attachment generation for every operation.
The authoritative service retains generation, lease and durable-delivery decisions.
There is no separate body-pulling tool in this adapter.

Native IDs are observations from an owner-controlled client connection. This
adapter does not attest that a call originated in a particular host hook or
isolate hostile programs running under the same local user. A parsed ID, successful
MCP response, or prepared batch does not prove that the host inserted context or
that a model understood or acted on it.

## Runtime configuration

Set every peer setting explicitly. `RuntimeConfig::from_env` uses the existing
peer value objects and shared NATS configuration factory. It rejects missing or
invalid identity, routing, availability and anonymous authentication settings.

| Setting | Required value |
| --- | --- |
| `PEER_TENANT_ID` | Tenant token, including in tenant-account mode |
| `PEER_ACCOUNT_MODE` | `tenant` or explicit test-only `shared_fixture` |
| `PEER_CALLER_ID` | Broker-granted caller token |
| `PEER_CONNECTION_ID` | Broker-granted connection token |
| `PEER_INSTALLATION_ID` | Stable persistent installation identity |
| `PEER_COLLABORATION_REF` | Authorized collaboration reference |
| `PEER_DISPLAY_NAME` | Bounded peer display name |
| `PEER_AVAILABILITY` | `ready`, `busy`, `blocked`, or `draining` observation |

`NATS_URL`, `NATS_CREDS`, `NATS_NKEY`, `NATS_USER`, `NATS_PASSWORD`, `NATS_TOKEN`,
`NATS_JWT`, `NATS_SEED`, `NATS_CA`, `NATS_CERT`, `NATS_KEY` and
`NATS_TLS_DISABLED` follow the shared
[NATS configuration reference](../../platform/trogon-nats/README.md).
Secret values and opaque secret references remain on that factory's path.
TLS remains the default; disabling it is only for explicit local debugging or
tests. The executable installs no tracing subscriber and prints only static
failure diagnostics to stderr. Stdout carries MCP protocol only.

The issuer must actually restrict the configured credential to its caller and
connection grants. Configuration does not establish credential authority.
The generated connector ID belongs to this runtime and differs from the
issuer-chosen connection token and persistent enrollment identity.

## Lifecycle and deadlines

No enrollment happens at launch. A valid native prompt reserves one session
before enrollment, so a cancelled or uncertain enrollment cannot switch the
process to another native session. The complete prompt operation, local lock
wait, enrollment, preparation and rendering result included, has a two-second
deadline. Retry the same prompt identity. A timeout proves no remote absence
and never acknowledges a batch.

After valid enrollment, the runtime sends heartbeat observations every two
seconds. The service owns the ten-second lease. Heartbeats carry the configured
availability and no activity timestamp, condition or model progress assertion.
Lifecycle operations serialize with prompt work. An older enrollment generation
cannot replace a newer locally observed generation.

`peer_detach_hook` accepts only `session_id` and the exact `SessionEnd` event.
Malformed and wrong-session requests fail before backend access. Unbound
`SessionEnd` is idempotent and does not enroll. A bound detach terminally stops
new prompts and heartbeats before waiting for remote work. Its total budget is
one second, including synchronization. Repeats after confirmation are local
no-ops. Unconfirmed detach retries retain the same attachment; lease expiry is
the fallback. Durable enrollment, inbox and acknowledgment facts stay owned by
the service.

EOF, SIGINT and SIGTERM cancel native work and attempt bounded detach. Unix
stdio uses readiness-based nonblocking pipes, avoiding a blocking stdin thread
that could survive a signal while the host keeps the pipe open. Other platforms
currently return an explicit unsupported transport error. Connection startup is
bounded to two seconds; service cleanup and connection drain each have a finite
one-second budget. These transport bounds do not create delivery facts.

## Evidence boundaries

The executable tests run the repository rmcp client over real child-process
stdin/stdout, authenticated TLS/nkey broker grants, `PeerRpcApplication` and
JetStream. Disposable content, policy, catalog and presentation adapters use
explicit test-support ports. They are not production implementations. No test
establishes process-crash presentation-cache durability.

The [native plugin package](../../../../plugins/claude/peer/README.md) forwards
only native identity fields. Package validation and MCP success do not establish
actual host callback execution, context insertion, model spend or vendor
attestation. Production content/key/cache/catalog/policy infrastructure, trusted
reconciliation scheduling, independent verification and final host acceptance
remain separately owned work.
