# Registered Session plan verification

This crate decodes immutable Session plan artifacts and implements the admission
port used by `trogon-session-store`. Storage continues to retain an opaque
`StoredSessionExecutionPlan`; interpreting its bytes requires the exact
registered contract selected by the host.

`RegisteredPlanDecoder::compiled_contract()` identifies the compiled plan and
Agent configuration descriptor closure together with the versioned semantic
profile. `new` accepts that exact tuple. Admission hashes original artifacts,
preflights their protobuf encoding and retains their original bytes. Unknown
fields, repeated singular fields, conflicting oneofs and duplicate map keys are
refused before a decoder can discard them. Alternate legal encodings retain
their distinct original artifact identity.

## Supported admission profile

The executable profile admits native runtime settings interpreted by an exact
registered `RuntimePlanInterpreter`, immutable checkout projections, bounded
closed variable bindings and original root delegate declarations. Delegation
joins the original parent configuration, selected binding, parent plan digest,
exact resolved child revision and the child's original configuration labels.
Every recovery performs current target admission again.

Tool, skill, memory, connection, transitive and unavailable dependency
resolutions remain represented by the wire contract and are refused by this
admission profile. Managed model routes, platform compaction, work contracts,
platform harness execution and candidate execution are also refused. Adding a
capability requires its actual resource and runtime adapters with their complete
source and interpretation checks.

## Host integration obligations

`PlanOwners` has no production default. Its adapters must read authenticated
original Agent revision and configuration artifacts, enforce current lifecycle,
project, caller and target policy, and retain all original source and policy
leases. Checkout admission must establish the actual immutable checkout root,
filesystem contract and symlink containment. A revision, digest, relative path
or copied plan declaration cannot establish those facts.

`PlanSourceLease` exposes the original configuration and the exact runtime
registration under that retained authority. `RuntimePlanInterpreter` must
validate the complete native settings against its registered descriptor and
semantics, validate its supported integrations, and derive the exact effective
configuration. Matching hashes alone cannot implement an interpreter. A future
candidate adapter must additionally prove the original delta and eligibility;
matching a candidate reference does not supply that proof.

The admission result retains the original leases across subsequent Session
sealing, native append and reconciliation. Original deadlines never renew, and
withdrawal narrows authority. The store also requires an application-owned
`SessionFactCodec` implementing authenticated canonical sealing. Admission does
not start an execution attempt or produce Ready evidence.

## Verification scope

Public tests exercise original artifact and declaration joins, typed refusal,
variable validation, lease withdrawal and the complete admitted-plan to native
JetStream child-recovery path. Resource owners, the native runtime interpreter
and the fact codec in those tests are explicit fixtures. These component tests
do not establish a deployed registry or policy adapter, production encryption,
a runnable registered runtime, host measurement, Ready, or complete platform
conformance.
