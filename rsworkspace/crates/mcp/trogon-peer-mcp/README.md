# Peer native-hook adapter

This library implements the `peer_prompt_hook` MCP tool for native
`UserPromptSubmit` delivery. It uses the repository's locked rmcp SDK and accepts
an injected backend implementing `EnrollPeer` and `PreparePeerBatch` with the
generated peer request/reply contracts. It provides no network backend, executable,
credential provisioning, heartbeat loop, or installed host configuration.

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

Prompt hooks have one total two-second budget covering lock acquisition,
enrollment, preparation and rendering. A deadline does not confirm whether a
remote operation completed. Retry with the same native session and prompt
identifiers; the reserved identity and any existing preparation remain intact.

Tool failures become MCP application error results with bounded static messages
and a stable per-variant `structuredContent.code` from `PeerMcpError::code()`,
so callers can distinguish failures without matching diagnostic text. Typed
backend sources remain available through the direct `invoke` API; peer bodies
are excluded from adapter `Debug` output. The backend remains responsible for
durability, current authorization, generation fencing, content retrieval and
digest verification. Retry content unavailability preserves the backend's
existing preparation record.

The current library advertises only the prompt hook. Model-facing roster, send,
status and acknowledgment operations, best-effort detach, and stdio executable
wiring belong to subsequent layers. Those operations fail closed here; the
adapter never fabricates a successful send or acknowledgment.

Native IDs are observations from an owner-controlled client connection. This
adapter does not attest that a call originated in a particular host hook or
isolate hostile programs running under the same local user. A parsed ID, successful
MCP response, or prepared batch does not prove that the host inserted context or
that a model understood or acted on it.
