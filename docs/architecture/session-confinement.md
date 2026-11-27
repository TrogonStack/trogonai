# Session privacy and execution confinement

Session confinement has two independent policies: where session data can go and which effects its agent can request.
`trogon-session-policy` implements a host-owned snapshot, deterministic admission, optional reviewer vetoes, and asynchronous effect dispatch.
Its [package reference](../../rsworkspace/crates/policy/trogon-session-policy/README.md) states the adapter and acceptance requirements.

## Privacy bounds execution

The privacy policy supplies the maximum destination set for the session.
The execution network is its intersection with the execution allowlist, so execution configuration cannot authorize an additional destination.
Per-tool WASM network grants must fit that intersection.

Model calls also use admitted destinations, including titles, compaction, summaries, embeddings, and search.
Each purpose requires an explicit route, and a missing route refuses the call.
Local-only sessions cannot fall back to a subscription provider.

Private and subscription sessions retain separate host bindings and storage ownership.
A new session does not authorize copying another session's private context.
The native owner must admit any selected context transfer before it supplies that data to the new session.

## Deterministic authority precedes review

The host resolves the exact tool, resource limits, component digest, and network grants before it calls a reviewer.
Structured tools use separate WASM instances with individual file and network grants.
Raw shell commands require a VM worker with mandatory egress enforcement.

The local reviewer can return a veto, but it cannot add a destination, tool, file grant, or execution privilege.
An unavailable reviewer refuses the action.
Prompt injection against the reviewer therefore cannot bypass deterministic admission.

`SessionPolicy` also implements the existing `RequestRuleEngine` narrowing interface.
Platform permits, live ceilings, and holds remain authoritative before this additional ceiling is considered.
A passing confinement rule cannot replace a missing platform permit.

## Host ownership and native integration

The trusted host retains the policy and effect adapters outside the agent's address space and filesystem grants.
It binds the session to an authenticated caller, rather than selecting authority from an agent-supplied session identifier.
Policy changes require a new host session, while revocation closes the current host permanently.

Denials carry bounded metadata without prompts, commands, paths, or destination names.
The host awaits its protected local denial sink, and an audit failure cannot admit an effect.
This library creates no telemetry exporter or provider connection.

Native integration must enforce the actual grants, including directory-handle confinement, pinned network resolution, component identity, VM placement, and command limits.
The broker projection refuses ambient host mounts and native escape options, but a declared capability alone does not prove enforcement.
Provider and worker acceptance must exercise the intended runtime before deployment.

The provider gateway's inference path and managed-model Session plan admission remain separate unfinished integrations.
This package does not enable those unsupported profiles or replace deployment acceptance with recording-adapter tests.
Existing operating-system account separation remains necessary wherever it protects private storage from other local processes.
