# Registered Session plan contract fixtures

This suite exercises `trogonai.session.plans.v1alpha1.SessionExecutionPlan` and
its typed dependency, model, compaction and working-directory values. The
content schema is separate from the Session store's opaque
`StoredSessionExecutionPlan` bytes and digest.

## Run the suite

```sh
mise run --skip-tools proto:validate -- --suite session_plan
```

The shared runner requires known fields and successful parsing before it checks
Protovalidate. Invalid cases name the expected diagnostic. Existing nested
legacy-required fields retain their parsing behavior; missing new fields are
rejected by explicit validation requiredness.

## Descriptor guarantees

| Contract | Local validation |
| --- | --- |
| Plan identity | Version 1, exact plan message name, required Session/project/revision/configuration/implementation and native settings references. |
| Native settings | Exact existing `NativeConfiguration` values, with finite payload and contract-name bounds. Empty native bytes remain valid when the registered native contract admits them. |
| Candidate | Unchanged execution retains the revision configuration. A candidate can carry a different effective configuration; actual candidate delta and edit authorization require source verification. |
| Dependencies | Complete original typed declarations, declarer identity and binding name; required matching resolution kind, exact-pin equality and explicit optional unavailability. Binding uniqueness is per declaring resource. |
| Memory | Multiple exact entries, unique entry identities, positive source revisions and declared-kind agreement. A required memory cannot resolve to an empty set. |
| Connection use | The complete existing `ResolvedConnectionUse` is retained, including its connection/release/custody, authorization generation, ceiling, permit reference, mode and proxy binding. The declaration and resolution names must agree. |
| Model access | Explicit absence of platform-managed access or a complete primary/auxiliary route set. Model selections and catalog pins agree, and auxiliary roles are unique. Driver/protocol fields describe required support without proving it. |
| Compaction | Explicit absence of platform compaction or an exact policy with finite token, pass and encoded-field budgets. Enabled compaction joins its managed producer model. |
| Platform harness | Managed model access and a registered platform compaction policy are required, including a disabled policy when supported by its exact release. |
| Variables | Explicit bindings, bounded nonempty root keys, no null root values and bounded root strings. Native-only variables remain in native settings. |
| Filesystem | Bounded checkout reference and normalized root-relative working path. Missing or empty relative path means the checkout root. |

The collection-limit fixture retains its complete 257-entry payload in compact
JSON formatting. It requires the specific 256-item-limit diagnostic; duplicate
binding rejection alone cannot make that case pass.

## Finite profile

| Value | Descriptor limit |
| --- | --- |
| Session/project/resource ids, binding names and versions | 128 UTF-8 bytes where the new plan owns that bound |
| Contract message names | 512 UTF-8 bytes |
| Individual native configuration payload | 32,768 bytes |
| Root native settings plus effective projection | 49,152 bytes |
| Dependency resolutions | 256 |
| Resolved memory entries per binding | 128 |
| Auxiliary model roles | 32 |
| Work input references | 128 |
| Root variable entries / key / string | 128 entries / 128 UTF-8 bytes / 4,096 UTF-8 bytes |
| Checkout id / URI / revision | 128 / 2,048 / 512 UTF-8 bytes |
| Relative working path | 4,096 UTF-8 bytes |
| Compaction token budget / passes | 16,777,216 tokens / 16 passes |
| Compaction text or identifier field budget | At most 65,536 bytes |
| Compaction accounting, header, envelope or complete record budget | At most 131,072 bytes |

These are contract ceilings, not operational allocation promises. The exact
registered runtime and resource owners can require narrower bounds. Maximum
encoded plan size, aggregate nested configuration size, recursive variable
depth/node/byte limits, source artifact limits and actual encoder accounting
must be checked before allocation or dispatch by the executable verifier.

## Evidence boundary

Fixtures establish descriptor shape, local bounds and equality relationships.
Their configuration bytes, locators, resource identities and digests are
placeholders. A passing fixture does not establish an admitted runtime release,
valid configuration bytes, verified digest, authoritative declaration, valid
dependency graph, current lifecycle, policy permission or working-directory
containment.

The verifier must select the exact descriptor and semantic interpreter from a
protected registered contract before decoding original plan bytes. It rejects
unknown fields/oneofs and unsupported contracts, verifies original byte digests
and all source joins, and preserves those bytes instead of hashing a decoded
re-encoding. It validates variable values recursively against the actual pinned
Agent schema, and verifies the complete dependency closure by real immutable
resource identity rather than caller-supplied graph ids.

Model-access grants, credential material, secret locators and copied live
policy are outside the immutable plan. Reused connection-use ceilings and
permit-history references record admission evidence; they do not freeze or
replace live authorization. Authenticated original sources and post-await
withdrawal checks remain mandatory at execution and recovery boundaries.

The plan can represent capabilities whose real owners are not yet implemented.
Every such path must fail with an explicit unsupported result until its
registered adapter and actual authoritative owners provide the required
semantics. No fixture identity or advertised protocol flag supplies support.

The owning draft is [ADR#0031](../../../docs/adr/0031-agent-implementation-and-session-plan.md).
