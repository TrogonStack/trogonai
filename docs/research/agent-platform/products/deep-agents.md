---
title: "LangChain Deep Agents: what 'agent' means"
source_urls:
  - https://docs.langchain.com/oss/javascript/deepagents/overview
  - https://docs.langchain.com/oss/javascript/deepagents/subagents
  - https://docs.langchain.com/oss/javascript/deepagents/async-subagents
  - https://docs.langchain.com/oss/javascript/deepagents/dynamic-subagents
  - https://docs.langchain.com/oss/javascript/deepagents/context-engineering
  - https://docs.langchain.com/oss/javascript/deepagents/backends
  - https://docs.langchain.com/oss/javascript/deepagents/event-streaming
retrieved: 2026-08-16
status: done
---

# LangChain Deep Agents: what "agent" means

Part of [Agent platform research corpus](../index.md).
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).

The versioned code anchor is the published `deepagents` 1.12.4 package,
released 2026-08-14 from
[`deepagents@1.12.4`](https://github.com/langchain-ai/deepagentsjs/tree/2a01d04faff7962b2fef008972ac4709cc6035d1).
The npm tarball SHA-256 is
`cea1b79d542a9ee695d103d4371b5265b83eac157cbbef8101c431f7062654ec`.
Documentation source was checked at
[`langchain-ai/docs@372b262`](https://github.com/langchain-ai/docs/tree/372b2623135291f8158db398f1585aac4ad30e12).
The retrieved Markdown SHA-256 values are:

- overview: `c0cf7966769cecafee1091a1b11ed2c1f0d008a13824c1aaa867fd90471c9274`;
- subagents: `63d288b7aed8a823bde3685b104a1b8276d630cbbc24e10b2aaa11683f87cbf2`;
- async subagents: `04a7eeab64c03da487a25bd69484fbaf84c6401003043bf0b2fd6f521c5d1f25`;
- dynamic subagents: `576be1b1df5f03c891e93e54677ca2a55844f1c1d4d2664932ee24cf1ea43424`;
- context engineering: `c1157869e3e675921d62369cce80ee1d62606cf8df2b9bc4da0102d380cc3ede`;
- backends: `434da3357b2cd1b627fae496263f0f66059faaf07b33cd8bae110f70c9aa20f2`; and
- event streaming: `4541b949a8711591bffcc0b19ddf4925a6812f6d75629ddb2eb1129965bb628c`.

## The `agent` noun (primary-source quotes)

Deep Agents calls itself an "agent harness." The overview says it uses the
same tool-calling loop as other agent frameworks, then adds filesystem,
context management, delegation, and steering. The package README makes the
runtime shape concrete: "`createDeepAgent` returns a compiled LangGraph
graph."

The published 1.12.4 declaration agrees. `createDeepAgent(params)` returns a
typed `DeepAgent`, which is the compiled runnable graph with the built-in
middleware stack. The construction input includes model, tools, system
prompt, state and context schemas, middleware, subagents, response format,
checkpointer, store, backend, interrupts, name, memory, skills, permissions,
and stream transformers.

There is no OSS Agent resource, Agent ID, CRUD API, or definition version.
The optional `name` labels a constructed graph; it does not create durable
identity. Persistence enters only when the caller supplies a checkpointer or
store and invokes the graph with LangGraph thread configuration.

Conceptual model: **agent-as-compiled-harness-graph**. Configuration creates
the executable object, and invocation runs it. Identity, deployment, and
version history remain the caller's responsibility.

## Subagents

Deep Agents has two materially different child models.

**Synchronous subagents** are declared in the parent configuration and
selected through the model-visible `task` tool. A default `general-purpose`
subagent is added unless disabled. Each call gets isolated context, blocks the
supervisor until completion, and returns one final result. The docs summarize
its state semantics as "Stateless" with "no persistent state between
invocations."

For a declared synchronous subagent:

- `name`, `description`, and `systemPrompt` are required;
- model and tools inherit unless overridden;
- filesystem permissions inherit unless replaced;
- custom middleware and skills do not inherit;
- the general-purpose child is the exception that inherits parent skills; and
- the child does not receive the parent's message history, only the delegated
  task and its own isolated context.

The beta "dynamic subagents" feature does not create definitions dynamically.
Interpreter code uses loops, branches, and parallel batches to dispatch the
already configured roster. It changes orchestration timing, not Agent
identity.

**Async subagents** point to another graph or assistant on an Agent
Protocol-compatible server. Launch creates a new server thread, starts a run,
and immediately returns the thread ID as the task ID. The supervisor can
check, update, cancel, and list the task. Updating starts another run on the
same child thread with the full child conversation history. Task metadata in
the supervisor records the child name, thread ID, run ID, status, and
timestamps in a dedicated state channel.

This is the important lifecycle split: a synchronous child is a nested,
stateless invocation; an async child is a stateful graph execution with its
own durable thread. Event streaming can expose nested subagent streams, but
the sources do not state one universal nesting limit.

## Configuration surface (what, where, why)

The OSS surface is code-first:

| Concern | Configuration | Reason stated by the product |
| --- | --- | --- |
| Loop behavior | model, tools, system prompt, middleware, response format | Start with an opinionated working loop while retaining extension points |
| Durable execution state | state schema, checkpointer | Preserve messages and custom state across invocations on one thread |
| Cross-thread state | store, `StoreBackend`, memory files | Retain knowledge across conversations |
| Work environment | filesystem backend, sandbox backend, permissions | Offload context, manipulate files, and constrain access |
| Delegation | synchronous, compiled, and async subagent specs | Isolate large work and parallelize long tasks |
| Runtime input | context schema | Pass per-run data that must not persist |
| Steering | tool interrupts | Pause sensitive actions for human review |

The default `StateBackend` stores files in LangGraph state for one thread.
`StoreBackend` writes through a LangGraph store and can span threads.
`FilesystemBackend` uses real local files. `CompositeBackend` can keep normal
working files thread-scoped while routing `/memories/` to durable storage.

Filesystem permission rules are first-match-wins and permissive when no rule
matches. They do not mediate arbitrary shell access through `execute`; the
docs require sandbox or backend policy boundaries for that capability.

## Binding time

- **Construction:** `createDeepAgent` assembles the model, tools, prompts,
  middleware, backend, and subagent roster into one compiled graph.
- **Invocation:** input messages and runtime context bind per run. Context
  schema values are explicitly non-persistent.
- **Thread:** state schema values and default-backend files persist across
  invocations only when a checkpointer and stable thread identity are used.
- **Cross-thread:** a store, external filesystem, or other durable backend is
  required.
- **Definition changes:** creating a graph from changed code creates a
  different in-process object, but the OSS library has no Agent revision or
  rule for existing threads. Deployment tooling must pin code and
  configuration if replay must be reproducible.

## Relationships between nouns

| Relationship | Operational meaning |
| --- | --- |
| Agent to run | One compiled graph can be invoked many times. A run is one invocation. |
| Agent to thread | One graph can serve many caller-named LangGraph threads when checkpointing is enabled. |
| Thread to run | A thread accumulates state across many runs. |
| Agent to synchronous subagent | The child is a configured nested runnable invoked by `task`; it has no independent durable thread by default. |
| Agent to async subagent | The child is another registered graph or assistant; every launched task gets its own server thread and run. |
| Thread to backend | `StateBackend` is thread-scoped; stores and external filesystems may outlive and span threads. |

The OSS API does not define `Session` as a resource. Documentation sometimes
uses "session" conversationally, but the durable execution noun is LangGraph
`thread`, and `run` is the invocation inside it. Treating a Deep Agents run as
the session would lose the state that spans several runs; treating the thread
as the session preserves the documented continuity boundary.

## Lifecycle

The application constructs a graph, invokes or streams it, and owns the
process. Without checkpointing, lifecycle ends with the invocation. With a
checkpointer, a stable thread can resume from its LangGraph state. With a
store or external backend, selected data can survive across threads.

Synchronous child lifecycle is call, isolated loop, final return. Async child
lifecycle is create thread, start run, observe or update, then success, error,
or cancellation. The Agent Protocol server, not the parent process, owns the
child thread and run persistence.

The library does not define Agent creation, archival, deletion, or upgrade
semantics. Those appear only when the graph is put behind deployment
infrastructure such as LangSmith.

## What makes it "an agent" here (our inference)

Our inference: Deep Agents uses "agent" for an executable, opinionated
LangGraph loop, not for identity. Filesystem, context compression,
delegation, and approval middleware make the loop a harness, while thread and
store infrastructure remain separate persistence concerns.

Its strongest Agent and Session lesson is the explicit child split. Small
delegations can be nested stateless calls, while independently steerable work
requires a separate durable thread. The product therefore demonstrates both
ways other systems use "subagent" without pretending they have the same
lifecycle.

## Open questions

- The OSS library does not say which graph or configuration version an
  existing thread resumes against after application code changes.
- No durable relationship is documented between a synchronous subagent
  invocation and a parent thread beyond messages, tool calls, and streaming
  metadata.
- Async task ID is the child thread ID. The consequences of coupling
  operation identity to execution identity are not discussed.
- The docs expose nested subagent streams but do not state a general depth or
  fan-out limit.
