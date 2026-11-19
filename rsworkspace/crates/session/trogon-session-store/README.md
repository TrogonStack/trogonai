# Session creation and delegation persistence

This crate owns a bounded Session creation and child-delegation persistence
profile using the canonical Session protobuf facts and the existing decider
stream read and append traits. It records creation and dispatch intent. It does
not start an execution attempt or make a child runnable.

`SessionRuntime` requires a `SessionAdmission` owner and a `SessionFactCodec`.
The application must supply authenticated caller and current resource policy,
registered immutable plan admission, original source authority, and canonical
sealed-content authentication bound to the Session key, concrete event type
and event identity. The crate supplies no permissive admission adapter or
plaintext production codec. The public test fixtures provide synthetic policy
and randomized envelopes, so those tests do not prove production authority or
cryptography.

## Creation and recovery

`create_session` consumes original `CreateSession` bytes and a stable
`CommandId`. It writes the concrete `SessionStarted` fact under `NoStream`.
`dispatch_delegation` consumes original `DispatchDelegation` bytes. It writes
`DelegationDispatched` under the exact parent head, retaining the original
child plan, checkout and optional candidate. Coordinator dispatch requires a
nonterminal parent and current owner authorization. It has no implicit Ready
prerequisite.

`recover_child` reads the original acknowledged parent dispatch and rechecks
current target admission. It copies the retained original nested encodings into
an atomic `[SessionStarted, ParentLinked]` batch under `NoStream`. The parent
fact's stable identity derives the child command identity. Duplicate success
requires the original creation prefix's event identities, concrete types and
exact original plaintext, including candidate presence and lineage, to match.
A failed precondition or duplicate identity receipt alone is insufficient.

After any append error, complete authenticated original-content evidence can
prove that the intended creation or dispatch already exists, even when another
reconciler sealed the same originals with a different nonce. Returning an
observed result does not attribute the write to the completing call. A completed
bounded read with no acknowledged occurrence returns `AppendUnknown`.
Authentication, source, authority and storage failures retain their typed
errors and also leave any preceding append unresolved. None causes an
automatic append retry. This rule applies to these stored intent facts and
makes no claim about external side effects.

Original source and admission leases remain live across sealing, persistence
and reconciliation. Their original deadlines and withdrawal futures remain in
force. Reauthorization cannot extend an already-held lease or the operation's
15-second deadline.

## Source and lifecycle limits

The supported history contains Session genesis, optional creation-time parent
lineage, delegation dispatches, child-delegation reservations, ordinary attempt
start, Ready and end facts, and Session terminal facts. Attempt fold checks
preserve the monotonic attempt number, exact predecessor, immutable plan and
Ready-after-start relationship. Original attempt and Ready bytes are passed to
the admission owner. A reference claiming Ready is not attestation verification.
Checkpoint restoration and other event families are refused by this profile.

The native resolver validates the current protected stream configuration on
each access. The lifecycle reader accepts at most 1,024 facts, a 64 MiB aggregate
sealed history, 262,144 bytes per sealed fact and 131,072 bytes per original
plaintext fact. Plans remain bounded to 65,536 bytes. Unknown fields, repeated
singular fields, unsupported history and invalid digest material are refused.

Parent and child writes occur on separate subjects. Recovery checks the exact
parent source before and after child creation, but those checks are not a
cross-stream transaction. `SourceChanged`, authority loss or another
post-append error may leave a durable child creation. Such a child has no
execution attempt or Ready authority. Reconcile the original operation and
identity before any further action. Do not manufacture a new command identity
to bypass an uncertain result.

Execution launch, Ready production and attestation verification, cascade and
rewind reconciliation, operation settlement, privacy overlays, snapshots and
full Session-store conformance remain outside this profile.

## Validation

Run the crate tests from the Rust workspace:

```sh
mise exec -- cargo test -p trogon-session-store -- --test-threads=1
```

The tests use isolated NATS containers. When the Docker default bridge lacks
capacity, `TROGON_SESSION_TEST_NETWORK` may name an existing dedicated Docker
network for these fixtures. Hosted CI uses the default fixture network.
