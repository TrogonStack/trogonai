# CrewAI: what "agent" means

Part of Agent Definition Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence from docs.crewai.com and the `crewAIInc/crewAI` source (pydantic
models, prompt templates, delegation tools).

## Source anchors

Sources retrieved 2026-07-13. Source-level claims and defaults are pinned to
the official [CrewAI 1.15.2 release](https://github.com/crewAIInc/crewAI/releases/tag/1.15.2)
at commit
[`289686a`](https://github.com/crewAIInc/crewAI/commit/289686ab4960708f556ca38687e0dee2112e3e30).
Where the live documentation and release source differ, such as the documented
`max_iter` default of 20 versus the 1.15.2 source default of 25, this dossier
uses the pinned release source.

- Product concepts: [Agents, sections "Overview of an Agent", "Agent
  Attributes", and "Creating Agents"](https://docs.crewai.com/en/concepts/agents),
  [Crews](https://docs.crewai.com/en/concepts/crews),
  [Processes](https://docs.crewai.com/en/concepts/processes), and
  [Flows](https://docs.crewai.com/en/concepts/flows).
- Persistence concepts: [Memory](https://docs.crewai.com/en/concepts/memory)
  and [Knowledge](https://docs.crewai.com/en/concepts/knowledge).
- Stable agent and prompt symbols: [`BaseAgent` identity, persona,
  delegation, and iteration fields](https://github.com/crewAIInc/crewAI/blob/289686ab4960708f556ca38687e0dee2112e3e30/lib/crewai/src/crewai/agents/agent_builder/base_agent.py#L251-L288),
  [`Agent` implementation](https://github.com/crewAIInc/crewAI/blob/289686ab4960708f556ca38687e0dee2112e3e30/lib/crewai/src/crewai/agent/core.py#L170-L195),
  [prompt substitution](https://github.com/crewAIInc/crewAI/blob/289686ab4960708f556ca38687e0dee2112e3e30/lib/crewai/src/crewai/utilities/prompts.py#L135-L180),
  and [persona, ReAct, manager, and delegation prompt text](https://github.com/crewAIInc/crewAI/blob/289686ab4960708f556ca38687e0dee2112e3e30/lib/crewai/src/crewai/translations/en.json#L2-L59).
- Stable orchestration symbols: [delegation execution](https://github.com/crewAIInc/crewAI/blob/289686ab4960708f556ca38687e0dee2112e3e30/lib/crewai/src/crewai/tools/agent_tools/base_agent_tools.py#L46-L120),
  [`Crew` fields](https://github.com/crewAIInc/crewAI/blob/289686ab4960708f556ca38687e0dee2112e3e30/lib/crewai/src/crewai/crew.py#L218-L300),
  [hierarchical manager construction](https://github.com/crewAIInc/crewAI/blob/289686ab4960708f556ca38687e0dee2112e3e30/lib/crewai/src/crewai/crew.py#L1480-L1505),
  and [`Process` values](https://github.com/crewAIInc/crewAI/blob/289686ab4960708f556ca38687e0dee2112e3e30/lib/crewai/src/crewai/process.py#L1-L11).

## The `agent` noun (primary-source quotes)

- [Agents, "Overview of an Agent"](https://docs.crewai.com/en/concepts/agents):
  "An autonomous unit that can perform specific tasks, make decisions
  based on its role and goal, use tools to accomplish objectives, communicate
  and collaborate with other agents, maintain memory of interactions, and
  delegate tasks when allowed." And: "Think of an agent as a specialized team
  member with specific skills, expertise, and responsibilities."
- In code, `Agent` is a **pydantic model** (extends `BaseAgent`), an
  in-memory object with a fresh `id: UUID4` per construction, no persistent
  resource in OSS. Its own [`__repr__`](https://github.com/crewAIInc/crewAI/blob/289686ab4960708f556ca38687e0dee2112e3e30/lib/crewai/src/crewai/agent/core.py#L1274-L1275)
  states the identity model: `Agent(role=..., goal=..., backstory=...)`.
- The persona *is* the prompt. The core
  [prompt slice](https://github.com/crewAIInc/crewAI/blob/289686ab4960708f556ca38687e0dee2112e3e30/lib/crewai/src/crewai/translations/en.json#L7-L12):
  `"You are {role}. {backstory}\nYour personal goal
  is: {goal}"`, role/goal/backstory are string-substituted into the system
  message verbatim
  ([`Prompts._build_prompt`](https://github.com/crewAIInc/crewAI/blob/289686ab4960708f556ca38687e0dee2112e3e30/lib/crewai/src/crewai/utilities/prompts.py#L135-L180)).
- The loop is prompt-encoded ReAct (when the model lacks native tool
  calling):
  ["Thought / Action / Action Input / Observation... Final Answer"](https://github.com/crewAIInc/crewAI/blob/289686ab4960708f556ca38687e0dee2112e3e30/lib/crewai/src/crewai/translations/en.json#L7-L20),
  ending with the pressure line: "Begin! This is VERY important to you, use
  the tools available and give your best Final Answer, your job depends on
  it!"
- **Conceptual model: agent-as-role (persona).** The required triad is
  role/goal/backstory; everything else (llm, tools, memory, max_iter=25,
  max_rpm, cache, guardrails) hangs off the persona.

## Subagents

- **Delegation is a peer mechanism inside a crew roster**, not parent/child
  spawning. `allow_delegation: bool = False`, when enabled, two tools are
  injected: DelegateWorkTool and AskQuestionTool, addressed to "coworkers"
  by role name.
- The tool description encodes their context-isolation stance: "The input to
  this tool should be the coworker, the task you want them to do, and ALL
  necessary context to execute the task, **they know nothing about the task,
  so share absolutely everything you know**, don't reference things but
  instead explain them." Delegation creates a fresh `Task` on the fly and
  calls `selected_agent.execute_task(task, context)`
  ([prompt](https://github.com/crewAIInc/crewAI/blob/289686ab4960708f556ca38687e0dee2112e3e30/lib/crewai/src/crewai/translations/en.json#L57-L59);
  [execution](https://github.com/crewAIInc/crewAI/blob/289686ab4960708f556ca38687e0dee2112e3e30/lib/crewai/src/crewai/tools/agent_tools/base_agent_tools.py#L99-L120)).
- **Hierarchical process** adds a manager: "Tasks are not pre-assigned; the
  manager allocates tasks to agents based on their capabilities, reviews
  outputs, and assesses task completion." Default manager persona shipped in
  translations ("Crew Manager... seasoned manager with a knack for getting
  the best out of your team"), or a custom `manager_agent`/`manager_llm`.
- **Roster is declared** (`Crew(agents=[...])`, fixed at kickoff); the
  manager's task allocation is the dynamic part.
- Shared across the crew: crew memory, crew knowledge, cache. Isolated per
  agent: own `tools`, `llm`, agent-level `knowledge_sources` ("stored in
  collections named after the agent's role") and agent-level memory.
- **Nesting** happens via Flows calling crews (`crew().kickoff(...)` inside
  a `@listen` step); no crews-within-crews in OSS. Remote delegation exists
  via an `a2a` field (A2AConfig/Server/Client) on the agent.
- Communication between tasks is output-as-context: `Task.context =
  [other_tasks]` injects their outputs as "This is the context you're
  working with."

## Configuration surface (what, where, why)

- **Agent fields** (BaseAgent + Agent): required `role`, `goal`, `backstory`;
  then `llm`, `tools`, `max_iter` (25), `max_rpm`, `max_execution_time`,
  `max_retry_limit` (2), `cache`, `verbose`, `allow_delegation` (False),
  `memory` (bool or Memory/scope/slice), `knowledge_sources`, `embedder`,
  `system_template`/`prompt_template`/`response_template` (override the
  built-in persona assembly), `use_system_prompt`, `respect_context_window`,
  `inject_date`, `planning`/`planning_config`, `guardrail` +
  `guardrail_max_retries`, `callbacks`, `apps` (AMP platform apps), `mcps`
  (including `crewai-amp:` slugs), `skills`, `checkpoint`, `security_config`,
  `from_repository` (loads a stored agent config from the CrewAI AMP
  platform), `a2a`, `executor_class`.
- **Crew fields:** `agents`, `tasks`, `process` (sequential | hierarchical;
  a `consensual` enum member is a TODO), `memory`, `knowledge_sources`,
  `manager_llm`/`manager_agent`, `planning` + `planning_llm`, callbacks
  (`before/after_kickoff`, `step_callback`, `task_callback`), `max_rpm`,
  `stream`, `output_log_file`, `security_config`, `tracing`.
- **Task fields:** required `description` and `expected_output`; `agent`,
  `context`, `async_execution`, `output_pydantic`/`output_json`/
  `response_model`, `output_file`, `human_input`, `markdown`, `guardrail(s)`,
  `tools`.
- **Where config lives:** Python code or the YAML convention
  (`config/agents.yaml` with role/goal/backstory blocks containing `{topic}`
  placeholders; `config/tasks.yaml` mapping tasks to agent names).
- **Stated rationale for the triad** is definitional rather than empirical:
  role "defines the agent's function and expertise within the crew"; goal is
  "the individual objective that guides the agent's decision-making";
  backstory "provides context and personality to the agent, enriching
  interactions." No measured claim for why persona helps is documented.

## Binding time

- **Kickoff is the binding moment:** `crew.kickoff(inputs={"topic": ...})`
  interpolates `{placeholders}` into every agent's role/goal/backstory and
  every task's description/expected_output (`interpolate_inputs`, with
  originals preserved in `_original_*`).
- **No mid-run mutation mechanism** documented; persona is fixed for the
  run.
- **No versioning in OSS.** The `from_repository` field implies the AMP
  platform stores named agent configs server-side, but no public versioning
  API is documented.

## Relationships between nouns

- **Crew 1:N agents, 1:N tasks; task → 0..1 agent** (or manager-assigned in
  hierarchical); agent → 0..1 crew backref.
- **Run = kickoff.** No session noun in OSS; `kickoff()` returns a
  `CrewOutput` (raw, structured, `tasks_output`, token usage). Variants:
  `kickoff_async`, `akickoff`, `kickoff_for_each`.
- **Flow ⊃ crews:** "Flows allow you to create structured, event-driven
  workflows... connect multiple tasks, manage state, and control the flow of
  execution." Flow state gets a UUID; snapshots support Resume (same
  `flow_uuid`) vs Fork (fresh `state.id`). Crews = autonomy; flows =
  developer control.
- **Memory**, unified Memory (LanceDB at `./.crewai/memory`), crew- or
  agent-scoped, persists across kickoffs. **Knowledge**, ChromaDB-backed
  reference material, crew- or agent-level. **Tool**, BaseTool attached to
  agents or tasks.

## Lifecycle

- **Agents have no lifecycle**: constructed, used during a kickoff,
  garbage-collected. No daemon, no persisted record.
- **Executor loop:** `AgentExecutor` (native tool calling) or deprecated
  `CrewAgentExecutor` ReAct loop, `while not AgentFinish`, bounded by
  `max_iter` (25) and `max_execution_time`; retries per `max_retry_limit`.
- **The library runs the loop in-process, synchronously**, the customer's
  Python process owns everything.
- **Persists across kickoffs:** long-term memory (LanceDB), knowledge
  stores, flow state snapshots. Nothing else.

## What makes it "an agent" here (our inference)

Our inference: CrewAI defines an agent **socially**, an agent is a persona
(role/goal/backstory) occupying a seat on a team, and agency emerges from
the org chart: task assignment, delegation to "coworkers," a manager
hierarchy, shared memory. Technically it is just a pydantic config object
consumed by an in-process ReAct loop; conceptually it is the
anthropomorphic extreme of the study, and the interesting datum is that its
delegation design assumes zero shared context ("they know nothing about the
task"), the same conclusion Cognition reached from the opposite direction.

## Open questions

- CrewAI AMP/Enterprise deployment model (does a deployed crew get an
  ID/versions/endpoints?) is undocumented publicly; `from_repository` and
  `crewai-amp:` slugs are the only OSS traces.
- The `a2a` field's maturity and semantics (server vs client configs) need
  a deeper read.
- `Process.consensual` has been a TODO for years, signals the roadmap
  beyond sequential/hierarchical.
- No session noun: multi-turn user interaction is out of scope for crews
  (flows + chat_llm hint at it).
