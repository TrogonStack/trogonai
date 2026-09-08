# DeepSeek Harness

Post-synthesis product case study. Evidence was retrieved 2026-08-20 from
DeepSeek Harness release `dsh-v0.1.0-rc.8`, commit
`141eb6fef83422698aef7a981029e843e8161534`. Every upstream source link below
is pinned to that commit. The TrogonAI callability check used repository commit
`c8e05872b9a3b156b974d9773b9723f07493cb1d`.

## ACP status and version

DeepSeek Harness has a native ACP agent implementation. The
`@deepseek-ai/dsh-acp` package describes itself as an automation-only JSON-RPC
stdio server and is version `0.1.0-rc.8`; it pins
`@agentclientprotocol/sdk` `0.25.1`
([packages/acp/acp/package.json:2-4](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/acp/acp/package.json#L2-L4),
[packages/acp/acp/package.json:34-36](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/acp/acp/package.json#L34-L36)).
That SDK defines `PROTOCOL_VERSION` as wire version 1
([typescript-sdk/src/schema/index.ts:310](https://github.com/agentclientprotocol/typescript-sdk/blob/cd8dc79b94a9d131687a2cdd02298820c32f5880/src/schema/index.ts#L310)),
and the server returns that constant during `initialize`. The wire-reported
agent identity is independently hard-coded as `deepseek-harness-acp` version
`0.0.1`, so it must not be mistaken for the package release version
([packages/acp/acp/src/index.ts:290-301](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/acp/acp/src/index.ts#L290-L301)).

This is deliberately not an editor integration. Its supported surface is for
programmatic clients, while navigation, transcript replay, commands, modes,
elicitation, reasoning, plans, titles, and tool presentation remain outside
the ACP package
([packages/acp/acp/README.md:5-7](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/acp/acp/README.md#L5-L7)).

## Capabilities and session lifecycle

| Surface | Behavior at the pinned release |
|---|---|
| Transport | Newline-delimited JSON-RPC over stdin/stdout. Stdout is reserved for protocol frames. |
| Prompts | Text and resource-link text are supported. Inline raster images are advertised only when a durable attachment store and the exact provider/model route support them. Audio and embedded context are false. |
| Agent capabilities | No load-session, editor, terminal, filesystem, or MCP capability is advertised. |
| Authentication | `authMethods` is empty and `authenticate` is a no-op. Provider credentials are a process-launch concern, not an ACP authentication exchange. |
| Session creation | `session/new` creates a fresh harness agent and requires an absolute `cwd`. Non-empty `additionalDirectories` or `mcpServers` are rejected. |
| Prompt concurrency | One prompt may be in flight per session. The response waits for admission, whole-agent idle, and ordered committed output delivery. |
| Output | Only committed assistant text and images become `agent_message_chunk` updates. Reasoning, tool activity, plans, and live deltas remain in the harness session log. |
| Cancellation | `session/cancel` aborts prompt admission or cancels the addressed agent after admission. Unknown session ids are no-ops. |

The capability and lifecycle rows follow the package's method-by-method
contract
([packages/acp/acp/README.md:20-34](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/acp/acp/README.md#L20-L34)).
One connection may own several independent sessions, but the implementation is
fresh-session only: load, list, resume, delete, fork, and per-session close are
not implemented. Disconnect or plugin disposal cancels and drains all agents
owned by that connection before releasing them
([packages/acp/acp/README.md:36-40](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/acp/acp/README.md#L36-L40),
[packages/acp/acp/README.md:76-81](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/acp/acp/README.md#L76-L81)).

## Permissions and trust boundary

DeepSeek Harness self-serves its tools inside the child process instead of
requesting client filesystem or terminal services. When a bridge-owned tool
approval has a tool-call id, the ACP agent sends one `allow_once` and one
`reject_once` option. A cancelled response fails closed, and no durable grant
is inferred
([packages/acp/acp/src/index.ts:268-285](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/acp/acp/src/index.ts#L268-L285)).
The runnable example selects `workspace-write` or `danger-full-access` through
`DSH_PERMISSION_MODE`; under `workspace-write`, the client decides each wider
retry and the server does not expose a picker or persist client policy
([examples/acp-agent/README.md:20-24](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/examples/acp-agent/README.md#L20-L24)).

This boundary is narrower than the common editor-host pattern. A compatible
client may advertise no optional capabilities because the child owns its
filesystem, terminal, model route, and tools. The host still owns the final
permission answer and the OS process lifetime.

## Process ownership, invocation, and provider authentication

The ACP package attaches an `AgentSideConnection` directly to the harness
process's stdin and stdout
([packages/acp/acp/src/index.ts:443-448](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/acp/acp/src/index.ts#L443-L448)).
Therefore the ACP client host, not the agent package, must spawn and supervise
that process. The pinned release documents this exact source-checkout
invocation:

```sh
DEEPSEEK_API_KEY=... pnpm --dir /path/to/deepseek-harness run demo:acp
```

The script boots the repository's composed ACP example, and its stdout carries
only protocol frames
([package.json:141-145](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/package.json#L141-L145),
[examples/acp-agent/README.md:5-16](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/examples/acp-agent/README.md#L5-L16)).
`DEEPSEEK_API_KEY` authenticates the harness to the model provider. It does not
authenticate the ACP peer. The ACP package exports library entry points and
declares no executable, while the tagged documentation gives the composed
repository demo as its runnable surface
([packages/acp/acp/package.json:13-31](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/acp/acp/package.json#L13-L31),
[packages/acp/acp/README.md:42-44](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/acp/acp/README.md#L42-L44)).
A production host would therefore need a packaged profile or an equivalent
composition rather than treating the package export itself as a binary.

## Relationship to the ACP subagent provider

The same repository also implements the client side in
`@deepseek-ai/dsh-subagent-acp`. This is not another server mode. It is an
out-of-process subagent provider that spawns one fresh ACP child for each run,
then performs `initialize`, `session/new`, and `session/prompt`. It derives the
child cwd from the parent session unless explicitly overridden and owns the
child's cancellation, stdin closure, termination escalation, and whole-tree
exit proof
([packages/subagent/subagent-acp/README.md:5-17](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent-acp/README.md#L5-L17)).

The provider advertises no optional client capabilities, auto-answers
permission requests according to its `allow` or `reject` policy, collects only
committed `agent_message_chunk` text, and gives the remote child a fresh context
with no parent conversation
([packages/subagent/subagent-acp/README.md:19-34](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent-acp/README.md#L19-L34),
[packages/subagent/subagent-acp/README.md:64-84](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent-acp/README.md#L64-L84)).
It is the first-party reference client for the automation server, and it proves
the intended process boundary. It does not make the TrogonAI repository capable
of hosting DeepSeek Harness because that TypeScript provider is not wired into
TrogonAI's Rust/NATS runtime.

## Channel mapping

No messaging or editor channel is mapped through this ACP surface. The package
explicitly leaves interactive presentation and human questions to separate Web
host and client modules. Its in-repository consumer is machine-to-machine
subagent delegation, so this is evidence for ACP as an internal execution
boundary rather than ACP as a channel gateway
([packages/acp/acp/README.md:5-7](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/acp/acp/README.md#L5-L7)).

## Callability from TrogonAI today

**Verdict: wire-compatible target, not callable end to end at the checked
commit.** Both sides use ACP wire v1, and DeepSeek Harness's required core
method subset is representable by TrogonAI's pinned Rust SDK. No static
protocol-version blocker is visible. The blocker is process hosting:

- `acp-nats-stdio` accepts ACP from a client on its own stdin/stdout and
  forwards it to a NATS-backed agent. It occupies the agent-facing direction,
  not the client host direction (`rsworkspace/crates/acp/acp-nats-stdio/README.md:1-18`,
  `rsworkspace/crates/acp/acp-nats-stdio/src/main.rs:38-41,83-89`).
- The ACP crate family contains no child-process spawn call at the checked
  commit. The current decision record likewise identifies the missing
  component as a client host that spawns an ACP CLI, speaks the client role,
  and bridges the session onto NATS
  (`docs/research/acp/decision-record.md:33-49`).
- Provider credentials must be supplied at process launch, and the exact
  upstream runnable is the repository demo command above. An eventual host
  must also accept a self-served agent that advertises no client filesystem or
  terminal callbacks and must send empty `mcpServers` and
  `additionalDirectories`.

The minimum integration is therefore the planned `acp-host` process boundary,
a deployable DeepSeek Harness ACP composition, an injected
`DEEPSEEK_API_KEY`, an absolute session cwd, and an explicit permission policy.
Until that host exists, documenting the command does not make the product
callable from TrogonAI.

## Design lessons

- Copy the explicit automation profile: a small negotiated surface, committed
  output only, one in-flight prompt per session, and connection-scoped teardown
  make lifecycle ownership auditable.
- Support agents that self-serve filesystem and terminal operations. A host
  must not require every ACP agent to call client-owned fs or terminal methods.
- Keep ACP peer authentication distinct from provider credentials. Empty ACP
  `authMethods` does not remove the need for controlled credential injection at
  process launch.
- Preserve the policy decision in the host. The child can request a one-shot
  permission, but a headless client still needs a fail-closed rule for choosing
  or rejecting it.

## Source manifest

- DeepSeek Harness release `dsh-v0.1.0-rc.8`, commit
  [`141eb6fef83422698aef7a981029e843e8161534`](https://github.com/deepseek-ai/deepseek-harness/tree/141eb6fef83422698aef7a981029e843e8161534),
  retrieved 2026-08-20.
- ACP TypeScript SDK `0.25.1`, package git commit
  [`cd8dc79b94a9d131687a2cdd02298820c32f5880`](https://github.com/agentclientprotocol/typescript-sdk/tree/cd8dc79b94a9d131687a2cdd02298820c32f5880),
  retrieved 2026-08-20.
- TrogonAI repository commit
  `c8e05872b9a3b156b974d9773b9723f07493cb1d`, inspected locally 2026-08-20.
