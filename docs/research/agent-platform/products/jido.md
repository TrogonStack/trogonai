# Jido: what "agent" means

Part of Agent Definition Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence from the `agentjido/jido` source (v2.3.2, Apache-2.0, maintained by
Mike Hostetler) and the `jido_ai` companion. Elixir/OTP framework; README:
"Jido helps you build agent systems as ordinary Elixir and OTP software.
Agents hold state and implement `cmd/2`. Actions do work and transform that
state. Signals route events into the system. Directives describe effects for
the runtime to execute... AI is optional."

## The `agent` noun (primary-source quotes)

- The agent is **not the process**. From `guides/core-loop.md`: Agent =
  "Immutable struct + module: defines schema, handles cmd/2, pure decision
  logic"; AgentServer = "GenServer process: holds agent state, executes
  directives, routes signals."
- Moduledoc (`lib/jido/agent.ex`): "An Agent is an immutable data structure
  that holds state and can be updated via commands... The fundamental
  operation is `cmd/2`: `{agent, directives} = MyAgent.cmd(agent, MyAction)`.
  Key invariants: the returned agent is **always complete**...; directives
  are **runtime-owned external effects only**, they never modify agent
  state... The Agent struct is immutable. All operations return new agent
  structs. Server/OTP integration is handled separately by
  `Jido.AgentServer`."
- Struct fields (Zoi schema): `id`, `agent_module`, `name`, `description`,
  `category`, `tags`, `vsn`, `schema` (state-validation schema), `state`
  (map).
- README framing: "Functional agent state model inspired by Elm/Redux:
  `cmd/2` as the core operation: actions in, updated agent + directives out."
- **Conceptual model: a split.** The *definition* is a compile-time module
  (`use Jido.Agent`, agent-as-config); the *value* is an immutable struct
  (agent-as-state); the *running instance* is a supervised GenServer
  (`AgentServer`, agent-as-process). Jido is the only product in the study
  that makes the state/process distinction formally explicit and keeps the
  decision logic pure.

## Subagents

- First-class and **dynamic**: the `%SpawnAgent{}` directive
  (`lib/jido/agent/directive.ex`): "Spawn a child agent with parent-child
  hierarchy tracking... Child's parent reference points to the spawning
  agent; parent monitors the child process; parent tracks child in its
  children map by tag; child exit signals are delivered to parent as
  `jido.agent.child.exit`; child can use `emit_to_parent/3` while attached."
- **Logical hierarchy is not the same as OTP supervision:** "The logical
  relationship is independent from OTP supervisory ancestry. If the child
  later becomes orphaned, the current parent ref is cleared and the child
  must be explicitly reattached with `AdoptChild`."
- Fields on spawn: agent module or pre-built struct, `tag`, `opts`, `meta`,
  `restart` policy (default `:transient`).
- **Orphan policy is a config knob:** `on_parent_death` on the child server:
  `:stop` (default), `:continue`, `:emit_orphan`.
- **No depth or fan-out limits** found in code.
- **Communication is signals** (CloudEvents-compliant envelopes):
  `emit_to_parent/3` upward; lifecycle signals (`child.started`,
  `child.exit`) downward to the parent's children map. No shared state;
  isolation is BEAM process isolation.
- Related machinery: `Jido.Pod` ("agent with a canonical named topology of
  collaborators" persisted in state and reconciled after thaw) and
  Poolboy-based pre-warmed agent worker pools.

## Configuration surface (what, where, why)

- **Compile-time config** (`use Jido.Agent, ...`): `name`, `description`,
  `category`, `tags`, `vsn`, `schema` (NimbleOptions or Zoi state schema),
  `strategy` ("Execution strategy module or {module, opts}. Default:
  Jido.Agent.Strategy.Direct"), `plugins`, `signal_routes` ("Compile-time
  signal route table. Each route maps signal type/pattern to an action
  target"), `default_plugins` (override/disable), `schedules` ("Declarative
  cron schedules as {cron_expr, signal_type}"), `jido` (instance module).
- **Server start options** (`Jido.AgentServer.start_link`): `agent`
  (required), `id` (auto-generated if absent), `initial_state`, `registry`,
  `default_dispatch`, `error_policy`, `max_queue_size` (default 10,000),
  `parent`, `on_parent_death`, `spawn_fun`, `debug`.
- **Where config lives:** macro options in code (compile-time), keyword opts
  at server start, application config for Jido instances (`use Jido,
  otp_app: :my_app`, multiple isolated instances per app, "no global
  singletons").
- **Stated rationale** is the purity boundary (README): "Jido keeps agent
  decisions and state transitions explicit, actions may be pure or effectful,
  and directives are for effects you want the runtime to own." Directives vs
  StateOps encode this: StateOps (SetState, ReplaceState, ...) are handled by
  the strategy and "never reach the runtime"; directives are the inverse.
- **jido_ai** layers LLM behavior on top: `use Jido.AI.Agent` wires a ReAct
  strategy, `model:` resolved at runtime, tools are ordinary Jido actions,
  and each instance gets its own Task.Supervisor via a default skill plugin.

## Binding time

- **Compile time:** routes, plugins, merged state schema, plugin actions and
  schedules are validated and baked into module attributes; plugin
  requirement violations fail the build. "Cannot add new routes or plugins
  at runtime."
- **Struct creation (`new/1`):** initial state assembled from schema
  defaults + plugin defaults + opts; `strategy.init/2` runs.
- **Runtime-mutable via directives:** cron jobs (`%Cron{}`/`%CronCancel{}`),
  sensors (`%StartSensor{}`/`%StopSensor{}`), children
  (`%SpawnAgent{}`/`%StopChild{}`/`%AdoptChild{}`), and the state map itself
  through `cmd/2` (functionally, new struct each time).
- **Versioning:** `vsn` is a user-supplied string, not auto-incremented. The
  `Jido.Agent.Identity` default plugin separately keeps a monotonic `rev`
  integer for identity-profile mutations ("Identity state is immutable:
  updates produce a new struct with a bumped revision") plus
  created/updated timestamps.
- Definition changes are code deploys (BEAM releases/hot upgrades); nothing
  product-specific governs in-flight instances.

## Relationships between nouns

- **Agent (struct+module) 1:1-per-instance AgentServer (process);** many
  instances of one agent module may run, each with its own id.
- **Strategy 1:1 agent module** (compile-time): owns how `cmd/2` executes
  (Direct default; ReAct in jido_ai driven by `tick/2` + `%Schedule{}`
  directives).
- **Plugin N:1 agent module** (compile-time): contributes actions, routes,
  schedules, state defaults.
- **Sensor N:1 server** (runtime): transforms external events into signals.
- **Signal**, a CloudEvents envelope, the universal message. **Instruction**,
  normalized action + params + context (jido_action package). **Directive**,
  runtime-owned effect emitted by `cmd/2`. **StateOp**, internal state
  change, never reaches the runtime.
- **Parent:child**, logical refs (`ParentRef` with pid/id/tag/meta;
  `children` map of tag → ChildInfo) layered over a flat DynamicSupervisor
  (`Jido.AgentSupervisor`); death coupling is per-child policy, not
  supervision-tree structure.
- No session noun. The closest is the server instance's lifetime plus
  hibernate/thaw persistence.

## Lifecycle

- **Create:** `MyAgent.new(id:, state:)`, pure. **Start:**
  `Jido.AgentServer.start_link(agent: MyAgent)` under a DynamicSupervisor;
  `post_init` builds the signal router, starts plugin children and sensors,
  restores from storage if the lifecycle module says so, registers
  schedules, notifies the parent, and lands in `:idle`.
- **Status machine:** `:initializing → :idle → :processing → :idle` (or
  `:stopping`).
- **The loop is event-driven, not autonomous polling:** signals arrive via
  `call/2` (sync) or `cast/2` (async); processing runs in a Task; a drain
  loop executes queued directives; multi-step reasoning (ReAct) is driven by
  the strategy's `tick/2` scheduled via directives. **The customer's BEAM
  application owns the loop**; Jido is a library/framework, not a hosted
  service.
- **Stop:** `%Directive.Stop{}` from inside `cmd/2`, or supervisor shutdown.
  `terminate/2` hibernates via the lifecycle module, cancels crons, stops
  sensors.
- **Persistence:** `Jido.Persist` hibernate/thaw (`checkpoint/2` /
  `restore/2` callbacks to pluggable storage); `Jido.Agent.InstanceManager`
  gives keyed singletons with idle-timeout auto-hibernate. Pods persist and
  reconcile their collaborator topology after thaw.

## What makes it "an agent" here (our inference)

Our inference: Jido defines an agent as a **pure state machine with an
identity**, an immutable struct whose only operation is
`cmd(agent, action) → {new_agent, directives}`, and treats everything the
industry usually bundles into "agent" (process, loop, effects, scheduling,
children) as runtime concerns owned by a separate GenServer. LLM reasoning is
literally optional ("AI is optional"). It is the study's cleanest separation
of agent-as-definition / agent-as-state / agent-as-process, and its
parent-child model (logical refs with explicit orphan policy, decoupled from
supervision) is the most nuanced subagent lifetime-coupling design observed.

## Open questions

- No session/run noun: how multi-turn conversations map onto instances is
  left to the application (jido_ai adds request tracking in state).
- `vsn` is free-form and unenforced; no migration story for state schema
  changes across hibernate/thaw was found.
- No nesting/fan-out limits: supervision defaults are the only backpressure
  besides `max_queue_size`.
- The 49-repo org split (jido, jido_action, jido_signal, jido_ai) spreads
  the definition across packages; API stability across them is unverified.
