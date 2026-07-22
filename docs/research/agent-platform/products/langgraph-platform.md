# LangGraph / LangGraph Platform: what "agent" means

Part of Agent Definition Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence from docs.langchain.com (OSS + LangSmith/Agent Server) and the
`langchain-ai/langgraph` source (prebuilt agent, SDK schema, CLI manifest).

## The `agent` noun (primary-source quotes)

LangGraph splits the noun three ways: agent (behavior), graph (deployable
code), assistant (configured instance):

- **Agent, defined behaviorally:** "Agents are dynamic and define their own
  processes and tool usage... Agents have more autonomy than workflows, and
  can make decisions about the tools they use... Agents operate in
  continuous feedback loops, and are used in situations where problems and
  solutions are unpredictable. Workflows have predetermined code paths."
  (workflows-agents) LangGraph itself is "a low-level orchestration
  framework and runtime for building, managing, and deploying long-running,
  stateful agents."
- **The deployable unit is a graph**, code, named in `langgraph.json`
  (`"graphs": {"my_agent": "./graphs/agent.py:graph"}`), compiled at server
  start. The prebuilt `create_react_agent(model, tools, prompt, ...)`
  returns a `CompiledStateGraph`, **the agent literally is a graph** (and
  the helper is being renamed to `create_agent` in `langchain.agents`).
- **Assistant, the persistent resource:** "In practice, an assistant is just
  an *instance* of a graph with a specific configuration." "Multiple
  assistants can reference identical graphs while maintaining distinct
  configurations such as prompts, models, or available tools." SDK:
  "assistants... are versioned configurations of your graph." Schema:
  `assistant_id` (UUID), `graph_id`, `config`, `context`, `version: int`,
  `name`, timestamps.
- **Conceptual model: assistant-as-config-over-graph.** Directly comparable
  to OpenComputer's agent/revision split, but with the loop's *structure*
  (nodes/edges) as customer code rather than a platform-owned runtime.

## Subagents

- Five documented multi-agent patterns (multi-agent docs): **Subagents** ("A
  main agent coordinates subagents as tools. All routing passes through the
  main agent"), **Handoffs** ("Tool calls update a state variable that
  triggers routing... switching agents"), **Skills** ("A single agent stays
  in control while loading context from skills as needed"), **Router**, and
  **Custom Workflow**.
- The structural primitive is the **subgraph**: "a graph that is used as a
  node in another graph." Two composition modes: shared state keys (add the
  compiled subgraph as a node; communicate over shared channels) vs
  different schemas (call inside a node with explicit state transforms,
  "common for multi-agent systems maintaining separate message histories per
  agent").
- **Nesting:** "Subgraphs support multiple nesting levels (parent → child →
  grandchild)" provided the structure is statically discoverable.
- **Checkpointer inheritance is a per-subgraph knob:** per-invocation
  (default, inherits parent's checkpointer), per-thread
  (`checkpointer=True`, state accumulates), stateless
  (`checkpointer=False`).
- **Handoffs are a state-machine move, not a spawn:** "agent nodes can
  return a `Command` object that combines control flow and state updates"
  (`goto`, `update`, `resume`); cross-subgraph handoffs use
  `graph=Command.PARENT` and require manual message hygiene ("you must
  include both the AIMessage containing the tool call [and] a ToolMessage
  acknowledging the handoff"). "Unlike single-agent middleware... you must
  explicitly decide what messages pass between agents."
- Everything is **declared in graph code at compile time**; the dynamic part
  is routing, not roster.

## Configuration surface (what, where, why)

- **Graph (compile time, code):** state schema, nodes, edges, checkpointer,
  store, interrupts, cache, recursion limits; `create_react_agent` fields:
  model (static or per-state dynamic callable), tools, prompt,
  response_format, pre/post model hooks, context_schema, name ("used when
  adding [the] agent graph to another graph as a subgraph node").
- **Assistant (API resource):** `config` (tags, recursion_limit,
  `configurable` dict), `context` (static input-shaped context, SDK
  v0.6.0+), `name`, `description`, `metadata`.
- **Manifest (`langgraph.json`):** dependencies, graphs, checkpointer/store
  (with TTL and semantic index), auth, http (feature switches:
  `disable_assistants`, `disable_mcp`, `disable_a2a`; the platform exposes
  MCP and **A2A endpoints by default**), encryption, webhooks.
- **Stated rationale for assistants:** "This architecture allows
  non-engineers to manage assistant configurations through the LangSmith UI
  without requiring code changes or redeployment of the underlying graph";
  "rapid iteration without modifying or redeploying your graph code."

## Binding time

- **Deploy time:** graph structure (from `langgraph.json`).
- **Assistant create/update:** every update creates a new integer version:
  "Each update automatically creates a new version. Users can promote any
  version to active status or rollback." Constraint: "you must provide the
  entire configuration payload. The update endpoint creates new versions
  from scratch and does not merge."
- **Run creation:** per-run `config`, `context`, `interrupt_before/after`,
  `checkpoint` (resume/time-travel from any checkpoint),
  `multitask_strategy`, `durability` (`sync`/`async`/`exit`).
- **Mid-run:** `interrupt()` in graph code pauses; `Command(resume=...)`
  continues; `update_state` forks a new checkpoint at any point in history.
- **In-flight runs on assistant update:** not explicitly documented (runs
  reference the assistant and the config they started with, implied);
  flagged in Open questions.

## Relationships between nouns

- **Deployment 1:N graphs; graph 1:N assistants** (one default assistant
  auto-created per graph at deploy); **assistant 1:N versions, 1:N crons.**
- **Thread** = the state container: "A thread maintains the state of a graph
  across multiple interactions/invocations (aka runs). It accumulates and
  persists the graph's state." Threads are **not owned by an assistant**:
  any assistant can run on a thread. Thread 1:N checkpoints (a linked list
  via `parent_config`, namespaced per subgraph), 1:N runs. Statuses: idle,
  busy, interrupted, error.
- **Run** = "an invocation of an assistant"; belongs to exactly one thread
  (or none, stateless runs) and references one assistant. Statuses:
  pending, running, error, success, timeout, interrupted. Double-texting
  handled by `multitask_strategy`: reject, interrupt, rollback, enqueue.
- **Store** = cross-thread long-term memory (namespaced key-value with
  optional semantic index); **checkpoint** = thread-scoped short-term
  memory. "Persistence layer gives agents short-term memory through
  checkpointers and long-term memory through stores."
- **Cron** references one assistant (optionally one thread) on a schedule.

## Lifecycle

- **Assistant:** full CRUD + `get_versions` + `set_latest` (version
  promotion/rollback); delete can cascade to threads.
- **Thread:** create (with TTL, or `supersteps` for cross-deployment
  copying), get_state at any checkpoint (time travel), update_state (fork),
  history, copy, prune.
- **Run:** create/stream on a thread; `stream_resumable` for reconnecting;
  `DisconnectMode` cancel/continue; durability modes control checkpoint
  timing.
- **Who runs the loop:** shared: the **platform executes** the graph
  ("LangSmith Deployment is a workflow orchestration runtime purpose-built
  for agent workloads"), but the **loop's shape is customer code** (the
  ReAct prebuilt: "calls tools in a loop until a stopping condition is met...
  The process repeats until no more tool_calls are present").
- **Persists:** checkpoints (thread), store items (global), threads
  themselves; assistant versions.

## What makes it "an agent" here (our inference)

Our inference: LangGraph refuses to make "agent" a noun at the platform
layer: an agent is any graph whose routing decisions are made by an LLM
("workflows have predetermined code paths; agents define their own"). The
durable resources are the **assistant** (versioned config over a graph) and
the **thread** (accumulated state), cleanly separating the three things
other products blur: code (graph, deploy-time), configuration (assistant,
versioned), and state (thread + checkpoints, time-travelable). For a service
design, its checkpoint/fork/time-travel model and the
full-payload-versioned assistant are the most complete binding-time answer
in the study.

## Open questions

- In-flight run behavior across assistant version updates is undocumented.
- The Agent Server product naming ("agent" in marketing, "assistant" in the
  API) is in active flux; `create_react_agent` → `create_agent` migration
  suggests further renaming.
- A2A and MCP endpoints are on by default at the platform edge; how the
  assistant maps to an A2A Agent Card was not covered in the pages read.
- `context` vs `config.configurable` overlap (v0.6.0 addition) is still
  settling.
