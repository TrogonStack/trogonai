# AWS Bedrock AgentCore: what "agent" means

Part of Agent Definition Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence snapshot retrieved 2026-07-13, refreshed and extended 2026-07-15
against the GA-era product family. AWS documentation is mutable, so the
sections below name the page and section used. Version-sensitive SDK claims
were checked against these authoritative anchors:

- AWS Developer Guide,
  [How it works: AgentCore Runtime, Versions, Endpoints, and Sessions](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/runtime-how-it-works.html),
  [Runtime session lifecycle](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/runtime-sessions.html),
  [lifecycle settings](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/runtime-lifecycle-settings.html),
  [versioning](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/agent-runtime-versioning.html),
  [troubleshooting](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/runtime-troubleshooting.html),
  [Harness vs. Runtime: Conceptual difference](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/harness-vs-runtime.html),
  and [AgentCore harness](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/harness.html);
  the harness has its own follow-up dossier,
  [AWS AgentCore Harness](./aws-agentcore-harness.md).
- AWS Developer Guide,
  [Runtime service contract: Supported protocols and HTTP protocol contract](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/runtime-service-contract.html),
  [Runtime A2A support](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/runtime-a2a.html),
  and [AG-UI: deploy guide](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/runtime-agui.html)
  plus [protocol contract](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/runtime-agui-protocol-contract.html).
- AWS control-plane API,
  [`CreateAgentRuntime`](https://docs.aws.amazon.com/bedrock-agentcore-control/latest/APIReference/API_CreateAgentRuntime.html)
  and its [`AgentRuntimeArtifact`](https://docs.aws.amazon.com/bedrock-agentcore-control/latest/APIReference/API_AgentRuntimeArtifact.html)
  union; data-plane API,
  [`InvokeAgentRuntime`](https://docs.aws.amazon.com/bedrock-agentcore/latest/APIReference/API_InvokeAgentRuntime.html)
  and [`InvokeAgentRuntimeCommand`](https://docs.aws.amazon.com/bedrock-agentcore/latest/APIReference/API_InvokeAgentRuntimeCommand.html).
  The full operation indexes for both planes were enumerated on 2026-07-15:
  147 control-plane and 66 data-plane operations.
- AgentCore SDK for Python 1.18.0 at commit
  [`8df87bb`](https://github.com/aws/bedrock-agentcore-sdk-python/tree/8df87bb0dd67a6046413541adfeee59d69e57f49),
  specifically [`BedrockAgentCoreApp`](https://github.com/aws/bedrock-agentcore-sdk-python/blob/8df87bb0dd67a6046413541adfeee59d69e57f49/src/bedrock_agentcore/runtime/app.py#L157-L216).
  Re-verified 2026-07-15: v1.18.0 (tag published 2026-07-10) is still
  current; `main` differs only in dependency-lock updates.
- AWS Developer Guide pages for
  [Gateway](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/gateway.html)
  and its [key concepts](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/gateway-core-concepts.html),
  [workload identities](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/understanding-agent-identities.html),
  [memory strategies](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/memory-strategies.html),
  the [`Memory`](https://docs.aws.amazon.com/bedrock-agentcore-control/latest/APIReference/API_Memory.html)
  data type, and [direct code deployment](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/runtime-get-started-code-deploy.html).
- Timeline and successor facts:
  [release notes](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/release-notes.html)
  and [Amazon Bedrock Agents Classic maintenance mode](https://docs.aws.amazon.com/bedrock/latest/userguide/agents-classic-maintenance-mode.html).

## The `agent` noun (primary-source quotes)

- "An AgentCore Runtime is the foundational component that hosts your AI
  agent or tool code. It represents a containerized application that
  processes user inputs, maintains context, and executes actions using AI
  capabilities." (runtime-how-it-works)
- The decisive split (harness-vs-runtime): for the container deployment path,
  "**AgentCore Runtime** is a *serverless hosting environment*. You bring
  agent code, written in any framework or no framework, wrap it with the
  AgentCore SDK's `BedrockAgentCoreApp` entrypoint, package it into an ARM64
  container, push it to Amazon ECR, and deploy. *The orchestration loop is
  yours.*" Runtime also supports ZIP-based direct code deployment through
  `codeConfiguration`; loop ownership remains with customer code. The
  `CreateAgentRuntime` schema confirms this by omission: it carries no
  model, system prompt, or tool fields at all; it is pure compute, network,
  and protocol configuration.
- The agent is a persistent ARN'd resource:
  `arn:...:bedrock-agentcore:region:account:agent/UUID:version`, note the
  resource segment is literally `agent/` (pattern confirmed in the
  `CreateAgentRuntime` API reference).
- AgentCore Identity gives AWS's behavioral definition: "An AI-powered
  application or automated workload that performs tasks on behalf of users...
  Unlike traditional applications that run with static credentials, agents
  require dynamic identity management to securely access resources across
  multiple trust domains." (identity-terminology) "Agent identities are
  managed as specialized workload identities with agent-specific attributes
  and capabilities," a subtype, not a separate noun.
- Since June 2026 the family carries a **second, coexisting agent noun**:
  the harness, an ARN'd resource under `harness/` where "the orchestration
  loop itself is provided, powered by Strands Agents. You declare what the
  agent is (model, system prompt, tools, memory, limits) as configuration,
  and AgentCore runs the loop." (harness-vs-runtime) "Who owns the loop" is
  therefore no longer one platform-wide answer but a per-surface choice.
  Full study in the [AWS AgentCore Harness](./aws-agentcore-harness.md)
  dossier.
- **Conceptual model: agent-as-deployed-service.** On the Runtime path the
  agent's logic is an opaque customer artifact, either a container image or
  source package; the platform noun (AgentRuntime) is identity + versioning
  + addressing + isolation around it. Framework-agnostic by design: "Works
  seamlessly with popular frameworks like LangGraph, Strands, and CrewAI."

## Subagents

- **AgentCore itself has no subagent concept**, delegation is left entirely
  to the framework inside the container ("The orchestration loop is yours").
  The SDK source contains no occurrence of "subagent" in any spelling, and
  the harness tool-type enum (`remote_mcp | agentcore_browser |
  agentcore_gateway | inline_function | agentcore_code_interpreter`) has no
  agent-valued member.
- Multi-agent shows up as **infrastructure between peer agents**, not
  parent/child records:
  - A2A protocol support: "AgentCore's A2A protocol support enables seamless
    integration with A2A servers by acting as a transparent proxy layer"
    (port 9000, JSON-RPC 2.0, "built-in agent discovery through Agent Cards
    at `/.well-known/agent-card.json`"). (runtime-a2a)
  - Agent-as-tool became a config-level pattern in June 2026: "You can add
    an Amazon Bedrock AgentCore Runtime agent as a target on your gateway.
    Your gateway sends traffic directly to the runtime agent without
    aggregation or protocol translation." (release-notes) HTTP passthrough
    targets are "ideal for fronting agent URLs, external APIs,
    application-to-application (A2A) agents."
  - The Agents Classic migration guide states the stance directly: "You can
    build multi-agent patterns on AgentCore runtime using any supported
    framework. The managed harness supports an agent-as-tool pattern for
    simpler multi-agent use cases." (agents-classic-maintenance-mode)
  - Shared Memory resources: "A team of AI agents managing a supply chain
    shares memory to synchronize inventory levels." (memory) Memory's actor
    noun covers peers: an actor "can be a human user, another agent, or a
    system." (memory-terminology)
- Nothing is inherited between collaborating agents: each AgentRuntime has
  its own sessions (isolated microVMs), its own workload identity, and only
  explicitly wired shared memory.

## Configuration surface (what, where, why)

`CreateAgentRuntime` fields: `agentRuntimeArtifact` (required union of
`containerConfiguration` or `codeConfiguration`), `agentRuntimeName`,
`roleArn` (IAM, required),
`networkConfiguration` (required; VPC/subnets/security groups),
`protocolConfiguration` (`HTTP` | `MCP` | `A2A` | `AG-UI`),
`authorizerConfiguration` (inbound OAuth JWT / IAM SigV4),
`environmentVariables` (max 50), `filesystemConfigurations` (max 5;
service-managed session storage is in Preview, plus BYO EFS and S3 Files),
`lifecycleConfiguration` (`idleRuntimeSessionTimeout`, `maxLifetime`; both
documented as tunable, "Valid range: 60 to 28800 seconds"), tags. The two
deployment paths differ operationally: "Direct code deployment limits the
package size to 250MB whereas container-based packages can be up to 2 GB in
size", and direct code allows 25 new sessions/second vs 1.6 for containers.
Config lives in the control-plane API (`bedrock-agentcore-control`), the
AgentCore CLI ("a Node.js command-line tool... Under the hood, the
AgentCore CLI uses AWS CDK constructs"), or the console.

The distinctive design is that everything else an agent needs is a
**separate ARN'd primitive**. On the Runtime path these are wired in
customer code rather than by foreign key; the harness path instead wires
the same primitives by declarative reference (see Relationships):

- **Memory**, own resource (`CreateMemory`), with strategies at three
  ownership tiers: built-in ("AgentCore handles all memory extraction and
  consolidation automatically"), built-in overrides (custom prompts, managed
  pipeline), self-managed ("Complete ownership of memory processing
  pipeline"). An episodic strategy with cross-episode "reflection" joined
  the built-ins in December 2025.
- **Gateway**, "a single, secure entry point for agentic traffic:
  connecting agents to tools, to other agents, and to large language
  models"; now explicitly a three-headed router: MCP targets (aggregation
  into "a unified virtual MCP server", tools named
  `${target_name}___${tool_name}`), HTTP targets (passthrough, including
  Runtime agents and A2A), and inference targets (June 2026), which route
  "LLM requests to one or more model providers through a unified,
  model-based routing endpoint" with built-in connectors `bedrock-mantle`,
  `openai`, and `anthropic`. Bedrock Mantle is Bedrock's OpenAI-compatible
  inference endpoint ("powered by Mantle, a distributed inference engine")
  and is how third-party models such as OpenAI GPT-5.5 are served on
  Bedrock.
- **Identity**, workload identity auto-created per AgentRuntime and, per
  the same page, per Gateway ("AgentCore Gateway also creates workload
  identities automatically for agents deployed through the gateway
  service"); inbound auth (who can invoke) vs outbound auth (OAuth 2LO/3LO,
  API keys in a token vault keyed by agent + user). Payment credential
  providers (Coinbase CDP, Stripe Privy) joined the vault in Preview, May
  2026.
- **Policy** (GA March 2026), Cedar policy engines attached to Gateways:
  "the policy engine intercepts all agent requests and determines whether
  to allow or deny each action based on the defined policies", and since
  June 2026 also enforces Bedrock Guardrails "at the gateway layer, outside
  the agent's code, where the agent cannot reason around them."
- **Built-in tools** (Browser, Code Interpreter), own resources with system
  defaults `aws.browser.v1` and `aws.codeinterpreter.v1` and their own
  session APIs.
- **The evaluation family** (Evaluations GA March 2026; Optimization GA
  June 2026): ARN'd Evaluators scoring at tool-call, trace, or session
  level; online/batch/on-demand evaluation configs; Datasets (Preview); and
  Configuration bundles, "Versioned, immutable snapshots of agent
  configuration (system prompts, model IDs, tool descriptions) that
  decouple agent behavior from code."
- **Registry** (Preview, April 2026), "a centralized catalog for
  organizing, curating, and discovering resources" covering "MCP servers,
  tools, agents, agent skills, and custom resources", with an approval
  workflow on records.

Stated rationale: "The AgentCore Runtime handles scaling, session management,
security isolation, and infrastructure management, allowing you to focus on
building intelligent agent experiences rather than operational complexity";
"Gateway eliminates weeks of custom code development"; the services "work
together or independently with any open-source framework", modularity is the
explicit design goal.

The protocol contract for the container is fixed for custom HTTP containers:
`POST /invocations`, `GET /ping` (returns `Healthy | HealthyBusy`,
HealthyBusy keeps a session alive during async work), plus a first-class
WebSocket `/ws` route; the SDK's `BedrockAgentCoreApp` is a Starlette app
auto-wiring exactly these three routes, and `@app.entrypoint` registers the
customer's handler. AG-UI, formerly undocumented beyond a table row, now has
a dedicated contract and deploy guide: "AgentCore's AG-UI protocol support
enables integration with agent user interface servers by acting as a proxy
layer" on the same port 8080 paths, selected at deploy time via
`--protocol AGUI`.

## Binding time

- **At CreateAgentRuntime:** deployment artifact, protocol, network, IAM role,
  authorizer, env vars, filesystem, lifecycle, snapshotted as Version 1.
- **Versioning is automatic and immutable:** "Each update to the AgentCore
  Runtime creates a new version with a complete, self-contained
  configuration"; "Versions are immutable once created."
  (agent-runtime-versioning)
- **Endpoints are the routing layer:** "The `DEFAULT` endpoint automatically
  points to the latest version"; named endpoints (e.g. `production-endpoint`)
  pin specific versions and "must be explicitly updated to point to new
  versions." Endpoint states: CREATING, READY, UPDATING, and failure states.
- **At InvokeAgentRuntime:** `runtimeSessionId` (session identity; new id =
  new microVM), `qualifier` (which endpoint/version), `payload` (up to
  100 MB).
- **In-flight sessions are pinned, now documented:** "Each microVM session
  uses the code assets (agentRuntimeArtifact) that were deployed at the time
  of microVM creation. If you update your agent runtime with new code,
  existing sessions will continue using the previous version until they
  terminate and new sessions are created." (runtime-lifecycle-settings, with
  the same behavior in runtime-troubleshooting) This resolves the prior
  snapshot's inferred answer: updates never move a live session.
- **A request-scoped binding axis exists in preview:** Configuration
  bundles use git-style versioning (`branchName` defaulting to `mainline`,
  UUID `versionId`) and are resolved per invocation from OTEL baggage
  (`aws.agentcore.configbundle_arn` + version) inside
  `BedrockAgentCoreApp`, cached once per microVM process. Gateway can stamp
  a routing-experiment variant into the same baggage, steering which
  configuration a given request binds to without any stored FK.

## Relationships between nouns

- **AgentRuntime 1:N AgentRuntimeVersion** (immutable snapshots) and
  **1:N AgentRuntimeEndpoint** (named pointers to versions; DEFAULT
  auto-tracks latest).
- **AgentRuntime 1:N Session; Session 1:1 microVM at a time.** A session is
  identified by `runtimeSessionId` and receives a dedicated execution
  environment (microVM). The application can supply the ID, or Runtime can
  generate it on the first invocation when omitted. Sessions are a
  data-plane concept, not a control-plane resource. "AgentCore does not
  enforce session-to-user mappings, your client backend should maintain the
  relationship between users and their session IDs."
- **"Session" is now three distinct nouns.** The runtime session above; the
  Memory conversational session (`ListSessions` is keyed by memory + actor,
  "Empty sessions are automatically deleted after one day."); and Gateway
  MCP Sessions (May 2026), "a unique Mcp-Session-Id on initialize...
  scoped per authenticated user with configurable timeouts (default 1 hour,
  up to 8 hours)." None of the three are FK-linked to each other.
- **AgentRuntime 1:1 WorkloadIdentity** (auto-created); Gateway responses
  carry the same `workloadIdentityDetails`, so the 1:1 auto-creation
  extends to Gateway. Manual creation can mint "multiple identities for the
  same agent in different environments", so 1:1 is the automatic-path
  default, not a hard invariant.
- **Memory to Agent: decoupled on the Runtime path, with one new
  exception.** Memory is its own resource; binding happens in code.
  Memory:Session is many-to-many via `sessionId`/`actorId` tags; Memory 1:N
  strategies 1:N extracted records in namespaces. The `Memory` object now
  carries `managedByResourceArn`: "ARN of the resource managing this memory
  (e.g. a harness). When set, strategy modifications and deletion are only
  allowed through the managing resource", the first control-plane
  back-pointer in the family.
- **Gateway to Agent: decoupled**, wired by target configuration, though a
  Runtime can now be locked to accept traffic only from its fronting
  Gateway (`aws:SourceArn` policy for SigV4, `allowedWorkloadConfiguration`
  for JWT).
- **The harness wires the same primitives by declarative FK**, inverting
  the code-wiring rule: its schema requires an underlying AgentRuntime
  (`agentRuntimeArn`, required), references Memory by ARN or auto-creates a
  managed one, and points tools at Gateway/Browser/Code Interpreter ARNs
  (built-in singletons used when omitted). See the
  [harness dossier](./aws-agentcore-harness.md).
- **Isolation guarantee, platform-wide:** "Each user session receives its
  own dedicated microVM with isolated CPU, memory, and filesystem
  resources... After session completion, the entire microVM is terminated
  and memory is sanitized... eliminating cross-session contamination
  risks." The Browser and Code Interpreter session pages repeat the same
  microVM-per-session, sanitize-on-termination wording verbatim, so this is
  an AgentCore Tools primitive, not a Runtime-only property.

## Lifecycle

- **AgentRuntime:** CREATING → READY (or CREATE_FAILED); UPDATING → READY;
  DELETING. Updates mint versions.
- **Session states:** Active ("processing a sync request, executing a
  command, or doing background tasks"), Idle, and a third state the docs
  name inconsistently: runtime-how-it-works still says Terminated, while
  the dedicated sessions page says Stopped and reframes it as the same
  session resuming later: "The session transitions back to Active on the
  next invocation and a new compute is provisioned... The session itself
  remains valid until the AgentCore Runtime ARN is deleted." Termination
  triggers: inactivity (default 15 minutes), max compute lifetime (default
  8 hours), an explicit StopRuntimeSession, or unhealthy compute.
- **Who owns the loop: the customer, on this path.** The platform calls
  `POST /invocations` (or the configured entry point for direct code
  deployment). This is the inverse of Managed Agents/OpenComputer, and
  since June 2026 also the inverse of AWS's own harness.
- **Two side doors into the session's environment** bypass the agent loop:
  `InvokeAgentRuntimeCommand` "Executes a command in a runtime session
  container and streams the output back to the caller", sharing the
  session's container and filesystem with agent invocations; and
  interactive shells (June 2026) give "persistent terminal access to their
  sandboxed environment", with "up to 10 concurrent shell sessions" per
  runtime session, addressed by session id + shell id over WebSocket.
- **Persistence tiers:** in-microVM state is ephemeral ("Session state is
  ephemeral and should not be used for long-term durability (use AgentCore
  Memory for context durability)"); managed session storage (Preview)
  survives stop/resume but "Session data is deleted (reset to a clean
  state)" after 14 idle days or on any runtime version update; BYO EFS and
  S3 Files mounts and Memory resources persist across sessions.
- **Pricing confirms the session as the unit of execution:** "you only pay
  for actual resource consumption during your session, which spans from
  microVM boot... until session termination," per-second CPU + peak memory.
  Default quotas were raised in June 2026: "Active session workloads per
  account are now 5,000 in US East (N. Virginia) and US West (Oregon), and
  2,500 in other AWS Regions... The InvokeAgentRuntime API rate has
  increased from 25 TPS to 200 TPS per agent, per account."
- **Predecessor contrast, sharpened:** "Amazon Bedrock Agents (launched
  November 2023) is now Amazon Bedrock Agents Classic and will no longer be
  open to new customers starting on July 30, 2026." The mechanism is
  narrow: "Only CreateAgent and InvokeInlineAgent are restricted for
  accounts without prior usage", every other API stays available, and
  "There is no migration deadline." Agents Classic was the fully
  declarative opposite, AWS owned the loop, agents were console-configured
  resources with action groups and prompt overrides. AgentCore inverted the
  ownership; the harness now re-adds a managed-loop option on top, and AWS
  names AgentCore the recommended migration path (its docs avoid the word
  successor).

## What makes it "an agent" here (our inference)

Our inference: in AgentCore, "agent" is **whatever the customer's deployed
artifact does**, with the platform defining the *service shell* an agent
needs: a versioned, ARN-addressed deployment with per-session microVM
isolation, plus identity, memory, tool-gateway, policy, and evaluation as
detachable primitives. The platform's operational definition of agent-ness
is infrastructural: an agent is a workload that needs dynamic multi-domain
credentials, session isolation, and long-lived context; on the Runtime path
the loop itself is out of scope. It remains the strongest
agent-as-infrastructure model in the study, and the family now demonstrates
both poles on one substrate: the same microVM, version, and endpoint
machinery serves customer-loop agents (Runtime) and AWS-loop agents (the
harness), which is the clearest statement yet that the industry can
standardize the shell without standardizing the brain, and then sell the
brain's loop as an option.

## Open questions

- The docs disagree on the third session state's name (Terminated on
  runtime-how-it-works vs Stopped on runtime-sessions) and framing (new
  execution environment vs same session resuming); unresolved by any
  errata.
- The Harness (declarative config layer backed by Strands) blurs the
  loop-ownership story; answered in the follow-up dossier
  [AWS AgentCore Harness](./aws-agentcore-harness.md).
- Whether a harness's underlying managed AgentRuntime receives an
  auto-created workload identity is not surfaced: harness create/get
  responses carry no `workloadIdentityDetails` field, unlike Runtime and
  Gateway.
- How `managedByResourceArn` gets set on a Memory resource is undocumented:
  it appears in responses but is not a settable request field on
  CreateMemory/UpdateMemory.
- Governance beyond the harness exception is still IAM, not the data
  model: Runtime-path Memory/Gateway/Identity have no control-plane FK to
  AgentRuntime, and resource policies attach independently per ARN
  ("Resource-based policies control which principals can invoke and manage
  Agent Runtime, Endpoint, and Gateway resources.").
- Registry, Policy, and Payments were confirmed as nouns with their own
  CRUD surfaces, but their field-level schemas and their relationships to
  the agent nouns (approval workflow targets, policy-to-tool binding,
  payment session ownership) were not studied to the same depth; candidates
  for a follow-up pass.
