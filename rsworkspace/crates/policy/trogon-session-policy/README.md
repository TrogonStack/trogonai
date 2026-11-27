# Session confinement

This package adds a trusted host boundary for private model calls, structured WASM tools, and raw shell commands.
The host loads one policy before it starts the agent and retains that policy outside the guest.
`SessionHost` owns the effect adapters, resource budgets, reviewer, and denial sink.

## Admission and privacy

`SessionPolicy` composes with `trogon-policy-eval` through `RequestRuleEngine`.
Its privacy ceiling can only narrow existing platform permits, ceilings, and holds.
It never grants a missing platform entitlement.

Every destination has an exact host, port, and administrative location class.
Local-only policies refuse subscription destinations, including incidental model routes.
Worker, reviewer, title, summary, compaction, embedding, and search calls each require an explicit route.

The execution network is the intersection of the privacy destinations and execution allowlist.
A broad execution allowlist cannot override a narrow privacy policy.
Undeclared routes fail closed, and provider errors cause no fallback or automatic retry.

The generic egress ceiling applies privacy limits to platform connection admission.
Tool dispatch additionally applies execution limits, so a denied guest network does not prevent the trusted host from calling its admitted local model.
Unsupported delegation is refused because child policy and context transfer need their own admission.

## Tools and review

Each structured tool names a digest-pinned WASM component and its own file and network capabilities.
File grants require an explicit access mode, and network grants must fit the privacy and execution intersection.
Unknown tools and unknown capability fields are refused.

Raw shell grants require `MicroVm` or `FullVm` isolation and mandatory egress enforcement.
`ShellGrant::constrain_lease` projects the grant onto the existing broker request and refuses host mounts, credentials, mutable images, and native escape options.
It caps CPU, memory, and lease lifetime, disables snapshots, and requests output-history disposal.

Deterministic admission runs before any model review.
The reviewer returns `NoVeto` or `Veto`, with no permission-granting result.
An unavailable reviewer refuses the action and records a denial.

## Ownership and audit

`SessionHost` exposes no policy setter and no adapter getter.
A changed policy fingerprint requires a new session, and `close` permanently refuses subsequent requests.
Denial receipts contain the host-selected session ID, policy fingerprint, operation class, and typed reason.

Prompts, commands, paths, tool arguments, and destination names are absent from denial receipts.
The host supplies a protected local audit sink and awaits its acknowledgement.
An audit failure never admits the refused effect.

## Native integration and acceptance

The asynchronous adapter traits fit native hosts without selecting an async runtime.
Provider adapters must serialize the exact admitted route and pin destination resolution without redirects or hidden calls.
WASM adapters must verify component digests, open confined directory handles, and instantiate fresh guests with only their admitted capabilities.

VM adapters must use `constrain_lease` on the real broker request and enforce every remaining command limit, including the process ceiling.
An adapter must refuse execution when its worker cannot enforce the complete grant.
Broker credentials, policy documents, audit records, and private storage stay outside the agent's filesystem and address space.

This package supplies the host admission and dispatch library, with recording adapters in its tests.
Provider gateway inference, production effect adapters, protected storage, and deployment isolation still need their native acceptance evidence.
The existing managed-model Session plan profile remains unchanged and continues to refuse unsupported capabilities.

## Configuration and checks

Version 1 JSON is an administrative input for this library.
It is not a new AgentConfiguration or SessionExecutionPlan wire contract.
`tests/support/mod.rs` contains a complete synthetic document that the dispatcher tests parse through the public API.
`examples/private-session.json` is a parser-tested configuration with all seven model purposes and a network-denied shell.
Its address and component digest are placeholders that require administrative replacement before use.

Run these commands from `rsworkspace`:

```sh
cargo test --locked -p trogon-session-policy
cargo clippy --locked -p trogon-session-policy --all-targets -- -D warnings
cargo fmt -p trogon-session-policy --check
```

The crate also declares the native Bazel target through `workspace_crate()`.
Tests exercise real policy evaluation and dispatch with recording effect adapters.
They establish admission behavior, not native operating-system isolation or a deployed provider service.
