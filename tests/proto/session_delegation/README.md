# Session delegation contract fixtures

These fixtures exercise the draft Session delegation descriptors through the
shared [protobuf fixture runner](../validate.py). They cover selected-declaration
identity, retained child creation inputs, parent/child lineage and local
lifecycle shape.

## Run the fixtures

```sh
mise run --skip-tools proto:validate -- --suite session_delegation
```

The runner checks fixture field names against the descriptors, requires every
payload to parse, and matches each expected validation rejection against the
pinned Buf diagnostics. A parser error is not an accepted semantic rejection.

## Covered contracts

| Contract | Observable validation |
| --- | --- |
| Declaration identity | Child and external dispatch, child creation, lineage and fold require a plan-scoped binding reference. Binding names and digest material cannot be empty. |
| Repair inputs | Child dispatch and its fold retain an execution plan and checkout, plus an optional candidate. Plan bytes and supplied digest material cannot be empty. |
| Parent/child ids | The tested dispatch, creation, linkage and parent-terminal messages reject self-links. |
| Lifecycle values | Cascade policy, delegation kind, attempt outcome and parent-terminal cause reject zero and unknown values. Existing non-success attempt outcomes and both cascade modes remain valid. |
| Delegation kind | Child folds require child-specific fields and reject external references. External folds reject child-specific fields, including explicitly present empty child ids. |

New required fields use explicit presence with Protovalidate requiredness so
omission can parse and be rejected at the validation boundary. Existing shared
types retain their wire semantics: omitting a legacy-required inner member is
a parser refusal, not a semantic fixture result. The absence cases cover the
new nested binding digest, execution plan and checkout fields; empty material
is exercised inside the reused shared types.

## Evidence boundary

Passing fixtures establish descriptor parsing and message-local constraints.
The plan bytes and digest material are placeholders. They do not establish
actual digest agreement, authenticated source events, current Agent lifecycle,
policy admission, original-source equality, fresh child identity, runtime
dispatch, side-effect idempotency or snapshot recovery.

In particular, an already-existing child is not a successful retry merely
because `NoStream` rejected another append. The runtime must compare the
verified original creation evidence with the exact retained request and refuse
conflicting or unverifiable material, as specified by draft
[ADR#0035](../../../docs/adr/0035-session-store-decider-aggregate.md).

Draft records missing the new evidence are unsupported until an explicitly
reviewed migration can establish their original sources. A target reference
or current Agent configuration cannot reconstruct an omitted declaration
identity. The fixtures make no claim that the draft contracts are implemented
or promoted into generated runtime bindings.
