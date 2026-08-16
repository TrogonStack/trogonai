---
title: "LangSmith Managed Deep Agents: what 'agent' means"
source_urls:
  - https://docs.langchain.com/langsmith/javascript/managed-deep-agents-overview
  - https://docs.langchain.com/langsmith/javascript/managed-deep-agents-agent-definition
  - https://docs.langchain.com/langsmith/javascript/managed-deep-agents-project-structure
  - https://docs.langchain.com/langsmith/javascript/managed-deep-agents-deploy
  - https://docs.langchain.com/langsmith/javascript/managed-deep-agents-instructions
  - https://docs.langchain.com/langsmith/javascript/managed-deep-agents-skills
  - https://docs.langchain.com/langsmith/javascript/managed-deep-agents-identity
  - https://docs.langchain.com/langsmith/javascript/managed-deep-agents-memory
  - https://docs.langchain.com/langsmith/javascript/managed-deep-agents-sandboxes
  - https://docs.langchain.com/langsmith/javascript/managed-deep-agents-schedules
  - https://docs.langchain.com/langsmith/javascript/managed-deep-agents-local-development
retrieved: 2026-08-16
status: done
---

# LangSmith Managed Deep Agents: what "agent" means

Part of [Agent platform research corpus](../index.md).
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).

Managed Deep Agents was in public beta, limited to LangSmith Cloud's US
region, when retrieved. The versioned code anchor is the published
`managed-deepagents` 0.5.2 npm package. Its tarball SHA-256 is
`1d7edd9e2aa18da2b436a361545bf4f3b49dc43436d6aede07d7e398efb07aa2`.
The package's declared GitHub repository was not publicly readable on the
retrieval date, so the published declarations and implementation in that
tarball are the auditable code snapshot. Documentation source was checked at
[`langchain-ai/docs@372b262`](https://github.com/langchain-ai/docs/tree/372b2623135291f8158db398f1585aac4ad30e12).

The core retrieved Markdown SHA-256 values are:

- overview: `fa0f1c6bcbfa39e0be72b03217aab8a908bb1be663ee791f434cf7d7eea7b50d`;
- agent definition: `0f3ce9d09ed570b595993d6cbb1a744000252599805269e0b5d04e7c1cae1cc7`;
- project structure: `8f7d6406d4ace1871857f987072d53c90d9be45b7f7ddeecfad44bfa7b908164`;
- deploy: `1c965c9bdca945c1b9621b3994ed3634445b73c2fd27861e63c816d0e979c28d`;
- instructions: `2455eb2e157215f21d781e0bab2855e237d881a6db0b22d9a0b9701da729c38d`;
- skills: `1afd92236aaeef9c3e35903fc9d5404eec288ea17348941ee03ce07ef11f2d3d`;
- identity: `a8aa70158f5db665863f418013a49f7e0b5b83b07c8541843ecd61853b1dc786`;
- memory: `ae84c25d47587dcaa16f49e325230c965bf6e6c67387e9af00b7a1f2c62e09b8`;
- sandboxes: `0e133a2e54ae4eaecff7eaa1fefc31da1eac4c4457033549a49838515c57731f`;
- schedules: `ebe9f9fd253a4894440089b12f37a5d58d69072aaec109121ac2046bc3ac57ec`; and
- local development: `d637069a67c90ba179f27b6fc1e1d7073d1e87a425ea53a8d1d52b52e8ea068b`.

## The `agent` noun (primary-source quotes)

The overview gives the ownership split directly: "Build your agent as a
directory of files while LangSmith runs the harness and runtime." The
project may contain only one agent entry, a named `agent` export from
`defineDeepAgent`.

The published 0.5.2 type declaration sharpens that statement.
`defineDeepAgent` returns a "pre-runtime spec," not a compiled graph. At
deploy time MDA injects its backend, store, checkpointer, memory, skills, and
system prompt, then compiles the definition into a Deep Agent.

The required author-supplied `name` is used as three related identifiers:

1. the LangGraph graph ID;
2. the LangGraph assistant ID; and
3. the default LangSmith deployment name.

`mda deploy --name` can override only the deployment name. The public MDA
docs do not expose a separate Agent registry resource with its own versioned
identity. Operational persistence is supplied by the compiled assistant and
hosted deployment.

Conceptual model: **agent-as-code-first-definition compiled into a managed
assistant and deployment**. The file tree is the authoring unit; the hosted
assistant is the invocation target; the deployment owns the runtime.

## Subagents

MDA uses the Deep Agents harness, so declared synchronous subagents retain the
OSS behavior: a model-visible task dispatch, fresh isolated context, a
blocking child loop, and one final return. A subagent may set its own prompt,
model, tools, middleware, skills, response format, and permissions according
to the underlying `deepagents` contract.

`DefineDeepAgentConfig` accepts the full `createDeepAgent` surface except the
keys MDA reserves, so it can also carry async subagent specs. Those specs point
to a graph or assistant on an Agent Protocol server and create a separate
thread per task. The MDA project structure, however, permits only one local
agent entry. The current sources do not explain how several co-deployed async
graphs should be packaged inside that one-entry constraint.

No MDA-specific subagent record, version, grant, or child lifecycle API is
documented. MDA delegates those semantics to the underlying harness and Agent
Server.

## Configuration surface (what, where, why)

MDA deliberately splits configuration by owner:

| Owner and binding surface | Configuration |
| --- | --- |
| `agent.ts` | name, model, tools, middleware, subagents, permissions, interrupts, response format, state schema, context schema, and other non-managed Deep Agent options |
| Context Hub | `instructions.md` and `skills/**` |
| Managed runtime | backend, store, checkpointer, system prompt injection, skill loading, and memory mounting |
| Project declarations | identity, durable memory, sandbox, connectors, channels, schedules, and evals |
| Deployment | source archive, dependency manifest, non-reserved secrets, deployment type, and build revision |

The managed package rejects `backend`, `store`, `checkpointer`, `memory`,
`skills`, and `systemPrompt` in `defineDeepAgent`, even for untyped JavaScript
callers. This is more than convention: the runtime owns those capabilities.

The stated rationale is operational simplicity. Authors provide business
logic while MDA provides the loop, managed runtime, sandboxes, schedules, and
session continuity across restarts. The one-agent-per-project layout also
makes the folder the deployment input.

Identity can use a LangSmith API key or Supabase. The API-key default protects
the deployment but does not isolate end-user threads. Supabase mode adds
private, user-owned threads. Adding it later does not retroactively add owner
metadata to existing threads.

## Binding time

- **Authoring:** `agent.ts`, ordinary source modules, and project declarations
  define the build input.
- **Build:** `mda build` compiles the project into a managed LangGraph app.
- **Deploy:** `mda deploy` archives source, syncs context, creates or finds a
  deployment by name, and triggers a hosted deployment revision.
- **Run:** MDA inserts instructions into the system prompt on every run.
  Instructions edited in Context Hub automatically propagate without another
  source deployment. Skills are also editable in Context Hub and available to
  later executions.
- **Thread:** messages, custom state, and the default thread-scoped filesystem
  persist across runs through the managed checkpointer.
- **Cross-thread:** optional durable memory is one deployment-shared Context
  Hub tree at `/memories/agent/`.

The run-time instruction behavior is a direct exception to "sessions pin the
definition." A single durable thread can observe different instructions or
skills on later runs. The docs do not identify an immutable Context Hub
version copied into the thread, run, or trace.

Model, tools, middleware, and ordinary code require a compiled deployment
change. Existing-thread behavior after a new deployment revision is not
documented. Local changes require stopping and rerunning `mda dev` to
recompile.

## Relationships between nouns

| Relationship | Operational meaning |
| --- | --- |
| Project to Agent | Exactly one agent entry per project. |
| Agent to assistant | The agent `name` becomes the graph and assistant ID. |
| Agent to deployment | The same name is the default deployment name; an override may separate only this name. |
| Assistant to thread | One assistant serves many durable threads. The thread carries conversation and state. |
| Thread to run | One thread has many invocations; instructions are injected on each run. |
| Thread to sandbox | Default sandbox scope creates one sandbox per durable thread and reuses it across runs. |
| Agent process to sandbox | Optional `agent` scope shares one sandbox across threads handled by that process. |
| Deployment to memory | One optional read/write memory tree is shared across all threads and callers. |
| Schedule to thread | Default mode creates and deletes a fresh thread per firing; persistent mode reuses a caller-chosen thread ID. |

MDA documentation uses `session` as a synonym for conversational continuity,
but the Agent Server resource is `thread`. Its memory guide says normal
conversational memory is scoped to a "thread or session," while its sandbox
and schedule contracts consistently key persistence by thread. The closest
mapping is therefore **MDA session equals Agent Server thread**, with a run as
one invocation inside that session.

That mapping matters because the managed overview promises to keep sessions
running across restarts. The checkpointer restores the thread; it does not
turn an individual run into the durable session.

## Lifecycle

The Agent lifecycle is project scaffold, local compile, hosted deploy, build
revision, run, later redeploy, and deployment deletion. `mda delete` removes
the deployment and managed sandboxes associated with it. The sources do not
state whether thread records and checkpoints are deleted, retained, or
detached when the deployment is deleted.

Thread lifecycle depends on its origin:

- interactive threads are durable and contain many runs;
- scheduled ephemeral threads are deleted after their run;
- scheduled persistent threads reuse a stable configured ID;
- thread-scoped sandboxes are reclaimed after idle periods but can be reused
  or recovered; and
- agent-scoped sandboxes intentionally share files across threads.

Who owns the loop: LangSmith. The author supplies behavior and selected
capabilities; MDA compiles them into Deep Agents and Agent Server operates the
runtime, checkpointer, store, sandbox lifecycle, and scheduled invocation.

## What makes it "an agent" here (our inference)

Our inference: a Managed Deep Agent is a code-first behavior definition that
MDA turns into a named LangGraph assistant inside a hosted deployment. It is
not a separate identity resource in the documented authoring API. The durable
execution unit is the thread, while a run is one application of the current
assistant and current managed context to that thread.

The most consequential design choice is mixed binding time. Code, model, and
tools are deployment-bound, but instructions and skills are control-plane
content read on later runs. This favors live product tuning over strict
session reproducibility.

## Open questions

- Which exact deployment revision serves a pre-existing thread after a
  redeploy is not documented.
- No immutable instruction or skill version is shown on a thread or run, even
  though Context Hub edits can change model-visible behavior without a
  deployment.
- Deletion semantics for threads and checkpoints after `mda delete` are not
  stated.
- One project permits one agent entry, while async subagents can require
  several graph IDs. The supported co-deployment packaging is unclear.
- The public package metadata points to a repository that was not publicly
  readable on the retrieval date, limiting source-level audit beyond the npm
  artifact.
