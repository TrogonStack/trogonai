# Scheduler and execution separation TODO

This checklist tracks the work required to make the scheduler a transport-neutral clock for a shared, multi-tenant worker. Customers register runnable configuration and destinations such as webhooks without knowing about NATS subjects or Accounts.

Status: design direction agreed, implementation not started.

## Settled direction

- [x] The scheduler owns timing, pause, resume, removal, completion, and occurrence identity.
- [x] A schedule references one stable runnable registration or binding. `ExecutionTargetRef` is a placeholder until the public domain name is decided.
- [x] A schedule does not own destination configuration, webhook URLs, NATS subjects, credentials, headers, payload configuration, or transport selection.
- [x] The shared worker emits an occurrence and does not choose a company, destination, NATS Account, subject, URL, credential, or adapter.
- [x] Execution resolves the runnable target and its current dependencies when an occurrence begins.
- [x] Occurrence admission records an immutable plan containing the resolved runnable and destination revisions.
- [x] Later occurrences see configuration updates. An already admitted occurrence keeps its resolved plan.
- [x] Stable runnable and destination IDs are never reassigned to another logical resource or company. Removal leaves a tombstone.
- [x] NATS remains internal scheduling and work-queue infrastructure.
- [x] Webhook delivery is at-least-once. Exactly-once effects require receiver-side deduplication.

## Blocking decisions

These decisions must be recorded before changing scheduler schemas.

- [ ] Define the public name and owning bounded context for the stable runnable registration that a schedule references.
  - [ ] Decide whether the reference lives directly on Schedule or in an execution-owned binding keyed by ScheduleId.
  - [ ] Treat `ExecutionTargetRef` only as a conceptual placeholder until this decision is accepted.
  - [ ] Do not reuse `SessionExecutionPlan`, `WorkContract`, `ReconcileAction`, or A2A `PushNotificationConfigId`; none represents a reusable runnable registration.
- [ ] Define the internal occurrence handoff contract.
  - [ ] Include stable ScheduleId and deterministic OccurrenceId.
  - [ ] Include only the opaque runnable reference and scheduling facts required by execution.
  - [ ] Decide how At, Every, and Cron obtain stable occurrence identity. RRULE is currently the only kind that records planned and fired occurrences.
- [ ] Define occurrence admission and immutable-plan semantics.
  - [ ] Specify which runnable, destination, payload, deadline, and non-secret configuration facts are snapshotted.
  - [ ] Store SecretRef or CredentialBinding references, never resolved secret material.
  - [ ] Define whether retries reuse the admitted plan. Recommended: reuse the plan while rechecking live disable and revocation state before every retry.
- [ ] Define update ordering.
  - [ ] Update first means the new occurrence resolves the new revision.
  - [ ] Admission first means the admitted occurrence retains its recorded revision.
  - [ ] An ordinary update does not silently mutate an admitted plan.
- [ ] Define runnable and destination disable semantics.
  - [ ] Use an explicit lifecycle such as `Active -> Disabling -> Disabled`.
  - [ ] Disable first rejects new occurrence or delivery admission.
  - [ ] Admission first may finish the already admitted attempt.
  - [ ] Disable does not report effective while an admitted but unsent attempt can still cross the send boundary.
  - [ ] After disable reports effective, no new send or retry may begin.
  - [ ] Document that an HTTP request already sent cannot be recalled.
  - [ ] Define lease expiry and recovery when a worker crashes during disabling.
- [ ] Define webhook registration trust.
  - [ ] Decide whether authenticated registration is sufficient product ownership.
  - [ ] If control of the external endpoint must be proven, define a challenge or provider-authentication flow.
- [ ] Define retry and terminal-failure policy.
  - [ ] Classify retryable network failures and HTTP statuses with typed outcomes.
  - [ ] Define attempt limit or deadline, exponential backoff with jitter, `Retry-After`, cancellation, and dead-letter behavior.
- [ ] Choose the historical event and cutover strategy before editing `ScheduleCreated`.
  - [ ] Verify whether any persisted schedule streams exist outside tests.
  - [ ] If persisted history exists, choose a versioned event, upcaster or dual decoder, or an explicit migration mapping.
  - [ ] Define how an old raw NATS delivery and message map to a runnable registration. Do not invent a target ID during decode.
  - [ ] Define mixed-version worker behavior and rollback.

## Architecture decision record

- [ ] Add an ADR covering:
  - [ ] Scheduler, execution, routing, destination, policy, and secret-custody ownership.
  - [ ] The stable runnable registration and occurrence handoff.
  - [ ] Configuration resolution and immutable occurrence plans.
  - [ ] ID permanence and tombstone behavior.
  - [ ] Tenant ownership and foreign-ID information hiding.
  - [ ] Update, disable, retry, and crash-recovery ordering.
  - [ ] Webhook ownership or control proof.
  - [ ] At-least-once delivery and idempotency expectations.
  - [ ] Historical event compatibility and rollout strategy.

## Build the execution prerequisites

The scheduler contract cannot cut over until execution can consume occurrences.

- [ ] Implement the accepted runnable-registration resource.
  - [ ] Add rich identifier and revision value objects.
  - [ ] Derive company ownership from authenticated control-plane context, never a payload field.
  - [ ] Add create, update, disable, query, optimistic-concurrency, and tombstone behavior.
  - [ ] Prevent ownership changes and ID reuse.
- [ ] Implement a generic `DeliveryDestination` resource owned by routing or delivery.
  - [ ] Add DeliveryDestinationId, DestinationRevision, and typed lifecycle state.
  - [ ] Support a transport-neutral destination kind with Webhook as the first adapter.
  - [ ] Store endpoint metadata and SecretRef or CredentialBinding references separately from schedules.
  - [ ] Add tenant-scoped create, update, disable, query, and tombstone behavior.
  - [ ] Hide foreign destination existence by returning the same not-found result used for absent IDs.
- [ ] Implement occurrence admission in the execution authority.
  - [ ] Resolve the current runnable registration and destination under one trusted company context.
  - [ ] Atomically persist one immutable occurrence execution plan before dispatch.
  - [ ] Record the resolved runnable and destination revisions.
  - [ ] Persist a durable work item or outbox so a crash after admission cannot lose the occurrence.
  - [ ] Deduplicate repeated wakeups by deterministic OccurrenceId.
- [ ] Implement delivery-attempt admission.
  - [ ] Serialize begin, update, disable, retry admission, lease expiry, and recovery per destination.
  - [ ] Re-enter the live active-status gate before every retry.
  - [ ] Reuse the occurrence plan and idempotency key across retries.
  - [ ] Persist attempt state and outcome without sensitive configuration or payload material.

## Replace the scheduler wire contract

- [ ] Replace embedded Delivery and Message with the accepted runnable reference in:
  - [ ] `proto/trogonai/scheduler/schedules/v1/create_schedule.proto`
  - [ ] `proto/trogonai/scheduler/schedules/v1/schedule_created.proto`
- [ ] Apply the compatibility decision to:
  - [ ] `proto/trogonai/scheduler/schedules/v1/delivery.proto`
  - [ ] `proto/trogonai/scheduler/schedules/v1/message.proto`
  - [ ] Reserve removed protobuf field numbers and names, or introduce a versioned package or event.
- [ ] Review and update occurrence contracts:
  - [ ] `proto/trogonai/scheduler/schedules/v1/schedule_occurrence_scheduled.proto`
  - [ ] `proto/trogonai/scheduler/schedules/v1/schedule_occurrence_recorded.proto`
  - [ ] `proto/trogonai/scheduler/schedules/v1/events.proto`
  - [ ] `proto/trogonai/scheduler/schedules/state/v1/state.proto`
- [ ] Regenerate bindings under `rsworkspace/crates/platform/trogonai-proto/src/gen/`.
- [ ] Update scheduler protobuf codecs and fixtures under `rsworkspace/crates/platform/trogonai-proto/src/scheduler/schedules/`.

## Simplify the scheduler command domain

- [ ] Replace delivery and message fields with the accepted runnable reference in:
  - [ ] `rsworkspace/crates/scheduler/trogon-scheduler-domain/src/commands/create_schedule.rs`
  - [ ] `rsworkspace/crates/scheduler/trogon-scheduler-domain/src/commands/proto_wire.rs`
- [ ] Add a rich runnable-reference value object instead of a primitive string.
- [ ] Remove or relocate transport and payload configuration from the scheduler domain:
  - [ ] Delivery and DeliveryRoute.
  - [ ] SamplingSource and SamplingSubject.
  - [ ] TtlDuration when it belongs to delivery rather than scheduling.
  - [ ] ScheduleMessage, customer payload headers, and reserved transport-header validation.
  - [ ] `rsworkspace/crates/scheduler/trogon-scheduler-domain/src/commands/domain/schedule_event_delivery.rs`
  - [ ] `rsworkspace/crates/scheduler/trogon-scheduler-domain/src/commands/domain/schedule_event_sampling_source.rs`
  - [ ] `rsworkspace/crates/scheduler/trogon-scheduler-domain/src/commands/domain/message.rs`
  - [ ] Related exports in `rsworkspace/crates/scheduler/trogon-scheduler-domain/src/commands/domain/mod.rs`, `rsworkspace/crates/scheduler/trogon-scheduler-domain/src/constants.rs`, and `rsworkspace/crates/scheduler/trogon-scheduler-domain/src/subject.rs`.
- [ ] Update command, state, pause, resume, remove, conversion, unit, and property tests.

## Replace scheduler checkpoints

- [ ] Replace stored delivery and message configuration with the runnable reference in:
  - [ ] `proto/trogonai/scheduler/schedules/checkpoints/v1/checkpoint.proto`
  - [ ] `proto/trogonai/scheduler/schedules/checkpoints/v1/delivery.proto`
  - [ ] `proto/trogonai/scheduler/schedules/checkpoints/v1/message.proto`
  - [ ] `rsworkspace/crates/scheduler/trogon-scheduler/src/processor/execution/checkpoints/record.rs`
  - [ ] `rsworkspace/crates/scheduler/trogon-scheduler/src/processor/execution/checkpoints/codec.rs`
  - [ ] `rsworkspace/crates/scheduler/trogon-scheduler/src/processor/execution/checkpoints/codec/twin.rs`
- [ ] Bump the checkpoint key prefix or bucket so old rebuildable bytes are not decoded as the new shape.
- [ ] Update checkpoint codec, twin, store, and recovery tests.

## Make every schedule an internal clock

- [ ] Make At, Every, Cron, and RRULE produce the same transport-neutral occurrence handoff.
- [ ] Ensure every native NATS schedule targets only a scheduler-owned internal subject.
- [ ] Remove business targets from `Nats-Schedule-Target`.
- [ ] Remove customer sampling sources from `Nats-Schedule-Source`.
- [ ] Remove `DispatchRequest` and all direct scheduler publication to business subjects.
- [ ] Generalize RRULE-only wakeup names, payloads, and consumers for every schedule kind.
- [ ] Preserve RRULE continuation and durable occurrence sequencing.
- [ ] Define pause and remove behavior for wakeups that arrive after lifecycle state changes.
- [ ] Hand admitted occurrences to the execution authority.
- [ ] Update:
  - [ ] `rsworkspace/crates/scheduler/trogon-scheduler/src/processor/execution/reconciliation/request.rs`
  - [ ] `rsworkspace/crates/scheduler/trogon-scheduler/src/processor/execution/reconciliation/reconcile.rs`
  - [ ] `rsworkspace/crates/scheduler/trogon-scheduler/src/processor/execution/reconciliation/recorded_events.rs`
  - [ ] `rsworkspace/crates/scheduler/trogon-scheduler/src/processor/execution/reconciliation/schedule_subject.rs`
  - [ ] `rsworkspace/crates/scheduler/trogon-scheduler/src/processor/execution/reconciliation/rrule_wakeup_payload.rs`
  - [ ] `rsworkspace/crates/scheduler/trogon-scheduler/src/processor/execution/wakeup.rs`
  - [ ] `rsworkspace/crates/scheduler/trogon-scheduler/src/processor/execution/execution_schedules/mod.rs`
  - [ ] `rsworkspace/crates/scheduler/trogon-scheduler/src/processor/execution/worker/processor.rs`
  - [ ] `rsworkspace/crates/scheduler/trogon-scheduler/src/processor/execution/worker/consumer.rs`
  - [ ] Related module exports, constants, errors, mocks, and tests.

## Implement webhook delivery safely

- [ ] Resolve SecretRef or CredentialBinding immediately before an attempt through the tenant-scoped secret boundary.
- [ ] Never place secret values in schedules, events, occurrence plans, NATS messages, projections, logs, traces, retry records, or dead-letter records.
- [ ] Require HTTPS outside explicitly isolated development fixtures.
- [ ] Canonicalize URLs and reject embedded user information.
- [ ] Resolve and validate DNS and every destination IP for each connection attempt.
- [ ] Block loopback, private, reserved, link-local, multicast, carrier-grade NAT, and cloud-metadata addresses for IPv4 and IPv6.
- [ ] Prevent DNS rebinding and IPv4-mapped IPv6 bypasses.
- [ ] Disable redirects or revalidate every redirect target without forwarding credentials across origins.
- [ ] Keep TLS verification enabled and control proxy behavior.
- [ ] Apply request, response, connection, and total-attempt timeouts and size limits.
- [ ] Enforce outbound concurrency and egress limits.
- [ ] Prevent header injection and restrict configurable outbound headers.
- [ ] Add typed webhook authentication or request signing through credential bindings.
- [ ] Derive one deterministic idempotency key per logical occurrence and keep it stable across redelivery and retries.
- [ ] Redact URLs, headers, bodies, tokens, and resolved credentials from diagnostics.

## Replace projections and queries

- [ ] Make schedule read models expose only timing, lifecycle, occurrence progress, and the runnable reference.
- [ ] Replace delivery and message fields in the KV projection:
  - [ ] `proto/trogonai/scheduler/schedules/projections/v1/schedule_projection.proto`
  - [ ] `proto/trogonai/scheduler/schedules/projections/v1/delivery.proto`
  - [ ] `proto/trogonai/scheduler/schedules/projections/v1/message.proto`
  - [ ] `rsworkspace/crates/scheduler/trogon-scheduler/src/projections/schedules/`
  - [ ] `rsworkspace/crates/scheduler/trogon-scheduler/src/queries/`
- [ ] Add a new Postgres migration after `rsworkspace/crates/scheduler/trogon-scheduler/migrations/postgres/0002_create_schedules_projection.sql`.
  - [ ] Do not rewrite migration `0002` if it has shipped.
  - [ ] Replace delivery and message columns with the runnable reference.
  - [ ] Update projection selects, writes, and row decoding.
  - [ ] Reset or advance projection checkpoints so history rebuilds deterministically.
- [ ] Add destination, runnable-registration, occurrence-plan, and delivery-attempt read models only in their owning components.

## Historical migration and rollout

- [ ] Implement the selected decoder, upcaster, versioned event, or migration path for historical ScheduleCreated events.
- [ ] Add dual-read support for historical checkpoints and projections only where required during cutover.
- [ ] Map existing raw NATS schedules to explicitly registered runnable targets and destinations, or reject them with an operator-visible migration report.
- [ ] Purge or fence installed native schedules that still target business subjects before enabling internal-wakeup replacements.
- [ ] Preserve occurrence continuity during cutover without missing or duplicating delivery windows.
- [ ] Define partial-rollout behavior for old and new workers running simultaneously.
- [ ] Define rollback without reactivating unfenced native business-target schedules.

## Downstream fixtures and generated contracts

- [ ] Update scheduler tests in both scheduler crates.
- [ ] Update directly affected decider fixtures and suites:
  - [ ] `rsworkspace/cli/trogon-decider-test/src/codec/tests.rs`
  - [ ] `rsworkspace/cli/trogon-decider-test/schedules.yaml`
  - [ ] `rsworkspace/crates/decider/trogon-decider-sim/tests/schedules.rs`
  - [ ] `rsworkspace/crates/decider/trogon-decider-wasm-runtime/src/registry/tests.rs`
  - [ ] `rsworkspace/crates/decider/trogon-decider-wasm-runtime/tests/nats_execution.rs`
  - [ ] `rsworkspace/crates/decider/trogon-decider-wasm-runtime/tests/schedules_execution.rs`
- [ ] Update generated protobuf and WIT snapshots affected by the scheduler contract.

## Verification and acceptance

### Boundary tests

- [ ] Prove a schedule cannot contain a raw NATS subject, webhook URL, credential, customer header, payload configuration, or caller-supplied company identity.
- [ ] Prove the shared worker cannot select or override ownership, destination, transport, or credentials.
- [ ] Prove one company cannot create, inspect, update, disable, or execute another company's runnable registration or destination, including forged opaque IDs.
- [ ] Prove resource ownership cannot change and deleted IDs cannot be reused.
- [ ] Prove every schedule kind emits only internal occurrences.

### Ordering and recovery tests

- [ ] Race update against occurrence admission in both orders.
- [ ] Race disable against occurrence admission and begin-send in both orders.
- [ ] Prove an admitted plan remains immutable after later configuration updates.
- [ ] Prove disable blocks new attempts and retries after its effective acknowledgement.
- [ ] Test lease expiry and worker crash before plan persistence, after plan persistence, before send, during send, after remote success but before local acknowledgement, and while disabling.
- [ ] Prove duplicate wakeups and worker redelivery create one logical occurrence and reuse one idempotency key.
- [ ] Prove different occurrences receive different idempotency keys.

### Webhook security tests

- [ ] Test HTTP rejection, localhost, private and reserved IPv4, IPv6, IPv4-mapped IPv6, metadata endpoints, DNS rebinding, redirect-to-private, user information, invalid TLS, oversized payloads and responses, and timeouts.
- [ ] Test that credentials are not forwarded across redirects or origins.
- [ ] Test secret tenant isolation, just-in-time resolution, rotation, error redaction, and absence from persisted and emitted artifacts.
- [ ] Test typed retry classification for connection errors, timeouts, 408, 429, retryable 5xx, permanent 4xx, malformed responses, `Retry-After`, deadline exhaustion, and dead-letter behavior.

### Compatibility and integration tests

- [ ] Prove historical events, state, checkpoints, and projections decode or migrate deterministically.
- [ ] Test cutover from installed native schedules for missed and duplicate occurrence windows.
- [ ] Test rollback during partial deployment.
- [ ] Integration-test concurrent occurrences for multiple companies through the same shared worker.
- [ ] Audit logs, traces, metrics, events, database rows, retry records, and dead-letter records for URLs, credentials, payloads, and secret material.

### Required repository checks

- [ ] `mise run proto:generate`
- [ ] `mise run github-actions:assert-proto-generated`
- [ ] `mise run github-actions:decider-test-suites`
- [ ] Targeted scheduler and execution package tests.
- [ ] Real-NATS scheduler integration tests.
- [ ] Postgres migration and projection-rebuild tests.
- [ ] `mise run rust-lint`
- [ ] `mise run rust-pr-check`

## Definition of done

- [ ] The scheduler persists only timing, lifecycle, occurrence progress, and the stable runnable reference.
- [ ] No scheduler path can publish directly to a customer business subject or external destination.
- [ ] Execution resolves current configuration and records an immutable occurrence plan before dispatch.
- [ ] Ownership is derived from authenticated context and revalidated by the execution and destination authorities.
- [ ] Disable and retry behavior has a tested linearization point and documented external-side-effect limit.
- [ ] Existing schedule history and installed native schedules have a verified migration and rollback path.
- [ ] The shared worker passes cross-company isolation, race, crash, idempotency, webhook-security, and secret-leakage tests.
- [ ] All required repository checks pass.

## Explicit non-goals for the first implementation

- Additional destination adapters beyond Webhook, except for interfaces and fixtures needed to prove transport neutrality.
- Exactly-once webhook side effects without cooperation from the receiver.
- Exposing NATS Accounts or subjects in customer-facing scheduler APIs.
- Reusing the session-specific SessionExecutionPlan as the reusable runnable registration.
