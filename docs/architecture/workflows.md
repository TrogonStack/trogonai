# Workflows

Inspired by Kiro.

This is a proposal for durable agent workflow contracts. The protobuf packages
remain `v1alpha1` and are excluded from Rust code generation while their design
is reviewed. They describe definitions, commands, events, replay state, faults,
and read requests. There is no workflow executor, coordinator, watcher service,
or query handler in this change.

## Why workflow state needs its own owner

A main Session can decide that a change needs implementation, independent
reviews, a revision if the review rejects it, and a wait for an external check.
The expensive work happens in separate Sessions. Keeping the whole process in
the main conversation makes continuation depend on that conversation retaining
every instruction, result, and unresolved decision.

A workflow definition instead makes the control structure and data dependencies
explicit. A run records which work was dispatched, what evidence came back, and
why the next step became eligible. A process restart can reconstruct that
decision without asking a model to remember it or paying to repeat completed
work. External waits and user decisions are durable pending work rather than
repeated model turns.

The ownership boundaries are:

| Owner | Responsibility |
| --- | --- |
| Workflow definition | Immutable control tree, declared data interfaces, pinned Agent revisions, completion conditions, and limits |
| Workflow run | Exact admitted plan and inputs, occurrence identities, launches, verified results, control decisions, interactions, and plan revisions |
| Session | Agent execution plan, transcript, operations, tools, artifacts, attempts, and execution termination |
| Admission and registered adapters | Live authorization, byte and digest verification, typed capture, supported contracts, and external observation authenticity |
| Query projection | Rebuildable summaries and bounded reads with explicit freshness |

The definition and run each have their own aggregate stream. This follows
accepted [ADR#0045](../adr/0045-event-sourced-service-module-layout.md).
Session integration builds on the contracts discussed in draft
[ADR#0035](../adr/0035-session-store-decider-aggregate.md); this proposal does
not promote that draft or implement its coordinator.

## A definition describes both control and data

The definition is a flat collection of named nodes with a root reference.
Admission proves that the references form a reachable, acyclic control tree
with one owner per node. Repetition creates new occurrences of a body; it does
not create a cycle in the definition. Flat references give edits and recorded
decisions stable node identities without recursively embedding mutable plans.

An agent step pins an immutable Agent revision, a prompt template with explicit
bindings, and a registered output capture contract when it produces outputs.
Every step occurrence receives a fresh Session. Model and effort settings stay
in the selected Agent's native configuration, following the boundary in draft
[ADR#0062](../adr/0062-runtime-owned-settings-and-platform-declarations.md).

Declared values, artifacts, and completion evidence carry data between steps.
Bindings distinguish workflow inputs, node inputs, completed producer outputs,
repeat variables, and external observations. Admission checks their types and
lexical scope. A parallel branch cannot consume an unfinished sibling's output;
the join makes that evidence available to subsequent work.

| Control node | Reason for the boundary |
| --- | --- |
| Sequence | A later step starts only after the previous step satisfies its completion contract |
| Parallel | Independent work can proceed concurrently with a declared join and sibling disposition |
| Repeat | A review or refinement cycle has explicit feedback values, an exit condition, and an iteration limit |
| Wait | External observations, input responses, and delays suspend eligibility without occupying an agent turn |

Parallel joins distinguish every branch succeeding, every branch settling, and
the first successful branch. The run records the evidence and any winning
occurrence. A decision to cancel remaining siblings still requires recording
what those siblings eventually did.

A first-success join retains the chosen branch occurrence and its successful
completion, even when other branches have already completed. This selection
also stays visible if the join's acceptance condition rejects the chosen
outputs. Admission matches the selection to the pinned definition and verifies
its output contract before downstream work becomes eligible.

A Session that successfully runs a review may return a negative review verdict.
That is a valid result for a repeat condition to evaluate. Treating it as an
execution failure would trigger the wrong retry behavior and discard the reason
the implementation needs another iteration.

Saved recipes and plans authored for one request use the same
`WorkflowDefinition` payload. Provisioning assigns a saved identity and
revision; starting an ad hoc run does not require creating that registry entry.
Exact serialized definition bytes and their digest are the source of each run
plan. A decoded copy is derived by verification and validation rather than
stored as another independently writable truth.

## A run preserves paid work and unresolved effects

An occurrence identifies one entry into a node, including its nested iteration
path and plan revision. An invocation identifies one execution attempt within
that occurrence. Retrying preserves the occurrence and increments the attempt;
another repeat iteration creates different occurrences. These identities keep
results from different iterations, retries, and plan edits from colliding.

A dispatch event retains the complete admitted Session creation command,
command identity, execution plan, deadline, and authorization evidence. Repair
reuses that command and Session identity. It must not compile a different plan
because configuration changed while the coordinator was unavailable.

For a parent-linked Session, the parent dispatch fact is appended before child
creation. The workflow records a verified reference to that fact. The run
stream and Session streams are separate consistency boundaries, so recovery
must reconcile them rather than assume a cross-stream atomic append.

Result receipts bind an invocation to its exact execution plan and a verified
Session event. Admission failures have separate evidence because a rejected
launch may never create a Session. Captured outputs and artifact references
remain available after another branch fails or the run is cancelled.
Indeterminate effects block automatic retry until reconciliation establishes
what happened. A late verified receipt can settle that uncertainty after a
terminal run marker without restarting downstream work.

A late verified activation can identify a subscription after an indeterminate
registration cleanup. Replay retains that cleanup receipt and binds it to the
activation. Subsequent settlement targets the known activation. A termination
snapshot cannot settle the same wait through both its registration intent and
an activation.

Recovery from a failed run is an explicit new generation. It identifies the
prior failure, replaces failed occurrences and their failed controlling
ancestors, and binds retained successful completions into those replacement
parents. The old receipts and completions stay immutable. Admission rejects
recovery of successful, cancelled, or business-rejected runs and refuses to
repeat indeterminate effects. Invocation budgets remain global; refreshing a
deadline requires live admission. Recovery can tighten invocation, concurrency,
and depth bounds while preserving every earlier tighter bound. Recovery
therefore preserves successful reviews or other siblings rather than
dispatching them again.

Pause and cancellation record intent separately from application. Pause stops
new scheduling at a durable boundary; already-dispatched work remains tracked.
Cancellation requests propagate to active work and watchers, then settle with
evidence of completed cleanup or explicit unresolved effects. Neither operation
undoes an external effect or converts missing evidence into success.

## Interactions survive an idle coordinator

Input requests retain their question, response contract, intended audience,
request identity, deadline, and resolution. A response is correlated with the
request and its actor before it can unblock work. A main Session can route the
question, but routing does not confer permission to answer for a user.

Guidance is a message to an active step or the coordinating Session. It has
dispatch and delivery evidence, so accepting guidance does not imply the
destination received it. Guidance does not rewrite a previously dispatched
Session plan. A structured input response and an informational message have
different effects on eligibility and are recorded separately.

External waits pin a registered watcher and observation contract. The run
retains the resolved subscription, cursor, activity identity, deadline,
captured values, and condition decision. Activity, cursor progress, and a
satisfied resolution require retained activation evidence, including after
subscription release. Polling schedules carry an explicit
interval; event subscriptions can wait without polling. A stable source and
source event identity deduplicate redelivery across repeated waits. Each wait
occurrence has its own identity and continuation cursor, so a new iteration
does not mistake an already-consumed delivery for new progress. Delays use
durable timer evidence.

## A plan edit changes only unfinished work

A revision at an applied pause boundary records new exact definition bytes,
its predecessor, the reason for the edit, and the affected pending work. The
previous plan remains available
for interpreting completed results and active invocations. Admission must prove
that the edit preserves completed and dispatched work, its data dependencies,
and its identities. A continuation binds replacement work to an already-entered
sequence or repeat container. Its prior steps and results remain pinned; only
its unentered continuation changes. The revised plan explicitly identifies
preserved occurrences so an edit cannot erase inconvenient history.

Run limits bound invocation count, concurrency, deadlines, and control depth.
Retries consume invocation capacity; repeat iterations consume iteration
capacity. Hitting a repeat limit follows its declared failure, accept-last, or
input-request policy, with a durable decision. Accepting the last result at a
limit is an explicit policy
result, not evidence that the repeat condition became true.

Provider usage and cost settlement stay in the Session and
[usage settlement ledger](./usage-settlement-ledger.md) contracts. A workflow
invocation limit bounds coordination work; it does not replace cost admission
or promise that an invocation costs a fixed amount. Dispatch must recheck live
policy as well as these recorded limits.

## History invariants require a decider

Protovalidate checks one message at a time. The future decider and adapters must
enforce relationships with history and external evidence before appending facts.

| Transition | Required evidence |
| --- | --- |
| Start a run | Exact definition bytes match their digest; the decoded definition, declared inputs, live authority, and selected contracts pass admission |
| Enter a node | Its controlling occurrence and current plan make it eligible; inputs resolve from admitted sources with compatible types |
| Dispatch an invocation | The occurrence is eligible, capacity is available, admission is current, and the recorded Session command matches the pinned Agent and resolved inputs |
| Accept a result | The receipt belongs to that invocation and Session plan; the referenced event and captured outputs are verified; duplicate receipt identities agree |
| Complete a node or join | Accepted results and child outcomes satisfy the declared policy; unresolved siblings and effects remain recorded |
| Advance a repeat | Its completed body and one variable snapshot justify the condition and feedback; the next iteration is within the limit or has an explicit limit disposition |
| Resolve input or a watch | The request is outstanding, the response or activity is authorized and correctly correlated, and the deadline and observation contract are satisfied |
| Apply pause or cancellation | No new work escapes the recorded boundary; launched work and cleanup remain accounted for |
| Retry | The predecessor has a determinate retryable outcome, the retry policy allows it, and successful siblings keep their results |
| Recover failed work | The prior generation failed; the recorded failed frontier and retained successful siblings match history; new identities and live admission justify resumed work |
| Revise the plan | The predecessor is current; only pending work changes; active and completed occurrences remain interpretable against their original revision |
| Finish a run | The root has its declared outcome and outstanding effects are settled or explicitly identified; subsequent receipts only reconcile existing work |

The schemas deliberately do not embed executable predicates, arbitrary watcher
URLs, credentials, or a universal model-control object. Registered contracts
keep those extension points typed and admitted without making the workflow a
second Agent runtime.

## Command and event surface

Commands use the existing Decider envelope for command identity and expected
stream revision. Starting or provisioning requires an absent stream; later
commands require its expected revision. Faults use the error channel and never
become ordinary domain events.

| Intent | Commands | Recorded facts |
| --- | --- | --- |
| Manage saved recipes | `ProvisionWorkflowDefinition`, `ReviseWorkflowDefinition`, `ArchiveWorkflowDefinition`, `UnarchiveWorkflowDefinition` | Definition provisioning, immutable revision, archive, and unarchive |
| Start and enter work | `StartRun`, `EnterNode`, `BeginRepeatIteration` | Run configuration, resolved occurrence inputs, repeat variable snapshots |
| Launch and collect agent work | `DispatchNode`, `RecordInvocationResult`, `CompleteNode` | Complete Session launch, verified receipt, accepted or rejected node completion |
| Advance control | `RecordControlDecision` | Sequence, join, and repeat decisions with their evidence |
| Retry and recover | `AuthorizeRetry`, `RetryFailedWorkflowWork` | Retry authorization or a failed-generation recovery frontier with retained completions |
| Request and answer input | `RequestInput`, `ProvideInput` | Outstanding request and correlated answer, decline, or expiry |
| Exchange guidance | `SendGuidance`, `RecordGuidanceDelivery` | Directional guidance and delivery or undeliverable evidence |
| Attach and maintain watchers | `RegisterExternalWait`, `RecordExternalWaitActivation`, `RenewExternalSubscription` | Registration intent, activation result, and lease renewal |
| Observe external progress | `RecordExternalActivity`, `AdvanceExternalCursor`, `ResolveExternalWait` | Typed activity, exact cursor advance, and wait resolution |
| Release subscriptions | `RecordExternalSubscriptionCleanup` | Cleanup outcome retained independently of the wait result |
| Wait for time | `RegisterTimerWait`, `ResolveTimerWait` | Due time and fired, cancelled, or timed-out timer outcome |
| Pause and continue | `RequestPause`, `ApplyPause`, `ResumeRun` | Pause intent, applied boundary, and resumption |
| Stop work | `RequestCancellation`, `RecordCancellationAcknowledgement` | Targeted cancellation intent and its acknowledged outcome |
| Change future work | `ReviseRemainingPlan` | Exact replacement plan and continuation bindings at the applied pause |
| Finish a generation | `TerminateRun` | Accepted, rejected, failed, or cancelled outcome with effect accounting |

Watcher activation is separate from registration intent because recording a
desired subscription cannot prove an external service created it. Cleanup
receipts are separate from a satisfied condition because consuming an event
cannot prove the subscription was released. Lease and timer transitions carry
their own identities for retry and replay.

Cancellation can race an attach whose reply was lost. Without confirmed
activation, termination accounts for the registration intent with an explicit
unresolved effect or verified never-activated receipt. With confirmed
activation, it accounts for each subscription and its cleanup. Accepted and
business-rejected completion require determinate Session effects and verified
subscription release or absence. Missing activation evidence alone proves
neither.

The selected accepted or business-rejected termination accounts for every
retained invocation and known activation, or for the registration intent of a
wait with no activation. Its settlement references must resolve to retained
determinate results, released subscriptions, or verified never-activated
registrations. Historical failure markers remain snapshots of their generation;
recovery and late reconciliation can add work and evidence after those markers.

## Reads are bounded and disclose freshness

The run query package defines detail, list, node, interaction, and history body
types independently of replay state. This follows the existing Session query
pattern and draft
[ADR#0060](../adr/0060-session-query-contract-separate-from-projection.md).
Requests declare the contract version the caller can decode. Responses clamp
to that version and report contract elisions and withheld content rather than
silently dropping history.

Run summaries preserve the absence of a display name. Interaction views target
the run or an affected occurrence; an invocation-targeted input request names
its owning occurrence in this read model. Guidance and watches remain scoped
to an occurrence. Run-level input questions remain visible in interaction
pages and history.

`GetWorkflowRunPlan` retrieves exact admitted plan bytes, their digest, and their
definition contract version for the current or a named plan revision. This is
also available for ad hoc plans and pending-work revisions, whose artifacts
cannot be retrieved from the saved-definition registry. Inspecting an old
occurrence uses its pinned revision rather than the run's current plan.

Definition queries retrieve an exact saved revision or the latest revision and
list recipes by placement and archive status. A retrieved historical artifact
can have an older content revision than its accompanying current metadata.
Definition freshness names the event ordinal, including archive transitions,
rather than using content revision as a substitute for projection progress.

Run-scoped consistency names a run ordinal. List freshness uses a projection
checkpoint because ordinals on independent streams cannot be compared as a
global position. A read that cannot satisfy its requested position fails on the
transport error channel rather than returning a stale success.

An unsatisfied definition or run consistency wait can report reached projection
progress on the error channel without asserting success. History also
distinguishes verified Session results from rejected launches that never created
a Session.

Page tokens are opaque authenticated continuations. A server binds them to the
caller, filters, ordering, contract version, projection generation, and pinned
checkpoint or run ordinal. Pagination ends when the continuation is absent,
including when an intermediate page is empty. A changed authorization or
projection invalidates the relevant continuation. These token behaviors still
need implementation and are not established by a nonempty byte field.

## Contract validation and remaining implementation

The schema fixture task is `mise run proto:validate-workflows`. It builds a
descriptor image, checks that each fixture parses, then runs the pinned Buf
Protovalidate implementation. Invalid fixtures must fail for their expected
constraints. Buf formatting, lint, build, and breaking checks also cover the
packages despite their code generation exclusion.

This establishes local contract constraints, not replay correctness or working
orchestration. [ADR#0064](../adr/0064-schema-constraints-as-documentation.md)
keeps runtime enforcement a separate implementation decision. Promotion needs
deciders, registered capture and watcher adapters, Session reconciliation,
durable timers, bounded scheduling, admission enforcement, and query
projections. Their acceptance checks must include process interruption,
duplicate and late deliveries, concurrent edits, cancellation cleanup, and
preservation of completed results.

The contracts live under `proto/trogonai/workflows/definitions/` and
`proto/trogonai/workflows/runs/`; validation fixtures live under
`tests/proto/workflows/`. The [Agent Platform](./agent-platform.md) explains the
surrounding Agent and Session ownership boundaries.
