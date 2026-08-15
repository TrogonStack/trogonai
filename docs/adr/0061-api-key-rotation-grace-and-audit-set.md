---
number: "0061"
slug: api-key-rotation-grace-and-audit-set
status: accepted
date: 2026-08-15
---

# ADR#0061: API Key Rotation Grace and the First-Release Audit Set

## Context

API_KEY.md leaves the default reroll grace period and the required
first-release audit event set open. They look like two unrelated
housekeeping questions and are not: a grace period is only defensible if
there is a separate path with no window at all for the compromise case, and
the grace period creates exactly the moments (two live keys, one of them
scheduled to die) that an audit trail has to record for anyone to reason
about a rotation after the fact.

The grace period is a direct trade. Too short and a rotation breaks
deployments the platform cannot see, which teaches operators to avoid
rotating. Too long and a key the operator believes they replaced stays live
across the window.

The audit list in API_KEY.md has twelve entries, and one of them,
`api_key.verified`, has a volume equal to the platform's authenticated
request rate. Shipping it as an audit fact would make the audit log the
highest-volume stream on the platform and put its retention and cost
profile directly on the request path. Three others,
`api_key.permission_added`, `api_key.permission_removed`, and
`api_key.rate_limit_changed`, describe a single policy-patch operation from
three angles.

## Decision

### 1. Reroll is planned rotation; revoke is compromise

Reroll issues a new key that inherits policy, identity, roles, permissions,
scopes, and limits, and leaves the old key valid for a grace window. Revoke
takes effect at the next verification with no window.

Keeping the two separate is what makes a non-zero default grace safe.
Nobody has to choose between a rotation that will not break their fleet and
a containment action that takes effect now.

### 2. Default grace: 24 hours, 1 hour for management keyspaces

```text
default keyspaces      24 hours
management keyspaces    1 hour
configurable range      0 to 7 days, per keyspace
```

24 hours covers a deploy cycle for consumers the platform has no visibility
into. Management keys have few consumers and privileged reach, so they get
the shorter default. A grace of 0 is legal and means the old key stops
verifying the moment the new one is issued.

### 3. During grace, both keys verify and both are visible

The old key's audit facts carry `superseded_by` and the new key's carry
`rerolled_from`, the fields API_KEY.md already names. The list view shows
the old key as `rotating` with its expiry rather than as `active`, so a
rotation somebody started and forgot to finish is visible in the product
instead of only in the data.

### 4. Signed keys rotate by public-key swap, not by reroll

Add the new public key, move traffic, revoke the old one. The window is
bounded by `max active public keys per signed key`, fixed at 2, which makes
it a rotation window rather than an accumulating set. The reroll grace
machinery in sections 2 and 3 is bearer-only, and saying so keeps it from
being implemented twice.

The rotation flow never requires the caller to upload a private key, per
[ADR#0050](./0050-signed-first-caller-authentication.md) section 2.

### 5. Seven required audit facts for the first release

```text
api_key.created
api_key.rerolled
api_key.revoked
api_key.denied
api_key.public_key_added
api_key.public_key_revoked
api_key.signed_request_replayed
```

Each is either a change of authority or an attack signal. Two of them,
`api_key.denied` and `api_key.signed_request_replayed`, are the signals the
two unfirable alerts in `devops/openbao/runbooks/alerts.md` are waiting on
(suspicious API-key verification failures, and signed-request replay
attempts), so shipping this set closes those alerts rather than leaving
them specified with no source.

### 6. `api_key.verified` is a metric, not an audit fact

Successful verification is recorded as a metric, counted by keyspace and
result, plus the provenance already attached to the resulting events per
[ADR#0039](./0039-self-authenticating-event-provenance.md). Provenance
attributes the action to the key that authorized it without a separate
per-request audit record, which is the property the audit event was
reaching for and the only part of it that was load-bearing.

### 7. Merged and deferred

- `api_key.permission_added`, `api_key.permission_removed`, and
  `api_key.rate_limit_changed` collapse into `api_key.policy_changed`
  carrying a before/after diff. There is one policy-patch endpoint, so
  three event types for one operation would fragment a single change across
  events that always co-occur.
- `api_key.expired` is emitted lazily on the first denied use, not by a
  sweeper. Expiry is a property of the record, and a sweeper would emit
  facts about keys nobody was using.

### 8. Audit fact content

Every fact carries key id, keyspace id, project id, actor, result,
timestamp, and source address. None carries the presented key, the request
token, the verifier digest, or the pepper. Redaction is structural, through
`SecretString` and its peers, rather than a formatting rule, matching the
existing no-plaintext-in-logs and no-plaintext-in-traces tests, which pass
because the redaction lives in the `Debug` implementations and cannot be
bypassed by a different exporter.

Audit facts remain a distinct concept from domain events, per the
persistence work that names them separately.

The one legitimate exception to actor attribution is the bootstrap
`api_key.created` in
[ADR#0059](./0059-api-keyspaces-and-root-key-bootstrap.md) section 4, which
has a null actor because no authenticated actor exists at that moment.

## Consequences

- Operators get a rotation that does not break their fleet and a revocation
  that takes effect immediately, without either compromising the other.
- Management keys rotate on a tighter default than product keys, which is
  the correct asymmetry and one operators can widen per keyspace if they
  have a reason.
- A half-finished rotation is a visible product state rather than a silent
  second live key.
- The audit set goes from twelve entries to eight (seven required plus the
  merged `api_key.policy_changed`), and the removed entry is the one whose
  volume would have made audit a request-path cost.
- Two of the three alerts blocked on the API key platform gain their
  signal as a direct consequence of this set shipping.
- Consumers of audit facts must tolerate one unattributed create.

## References

- [ADR#0039: Self-Authenticating Event Provenance](./0039-self-authenticating-event-provenance.md)
- [ADR#0048: One-Time Plaintext Exposure Contract](./0048-one-time-plaintext-exposure.md)
- [ADR#0050: Signed Proof-of-Possession as the Strongly Recommended Caller Authentication](./0050-signed-first-caller-authentication.md)
- [ADR#0051: Fully Bound Per-Request Signing Contract](./0051-fully-bound-request-signing.md)
- [ADR#0059: API Keyspaces and Root Key Bootstrap](./0059-api-keyspaces-and-root-key-bootstrap.md)
- [ADR#0060: API Key Authorization Model and Rate Limits](./0060-api-key-authorization-model-and-rate-limits.md)
