# Alert Definitions

**State: partial.** Six of the nine required alerts have a signal to fire
on. Three do not, and are recorded below with what is missing rather than
written against metrics that do not exist. No alerts are deployed: this file
defines them, and wiring them into a monitoring backend is separate work.

Two of those three, suspicious API-key verification failures and
signed-request replay attempts, now have a specified source rather than an
open question: `api_key.denied` and `api_key.signed_request_replayed` in
[ADR#0061](../../../docs/adr/0061-api-key-rotation-grace-and-audit-set.md)
section 5. They remain unfirable because the API key platform is unbuilt,
which is a different problem from not knowing what to fire on. The third,
orphan cleanup backlog, still has neither.

Every alert below names the instrument it reads, the condition, the severity,
and the runbook the responder opens. An alert without a runbook link is an
alert that wakes someone with nowhere to go.

## Instruments available

```text
gateway.credential.revocation.latency      histogram, seconds
gateway.credential.cache.hits              counter, label: source
gateway.credential.cache.misses            counter, label: source
gateway.credential.delivery.denied         counter, labels: source, reason
gateway.credential.resolve.failures        counter, label: source
gateway.credential.store.write.failures    counter, labels: source, operation
gateway.credential.recovery.passes         counter, label: outcome
gateway.credential.recovery.errors         counter, label: reason
gateway.credential.recovery.scanned_events counter
gateway.credential.recovery.recoveries     counter, labels: status, kind
gateway.credential.recovery.stuck_reports  counter
```

`outcome` is one of `retry_delayed`, `failed_recovery`, `advanced`,
`recovered`, `stuck`, `idle`, `error`. `reason` on
`delivery.denied` is one of `runtime_service`, `host`, `injection_location`.
`status` on `recoveries` is `planned`, `recovered`, or `failed`. `operation`
on `store.write.failures` is one of `put`, `rotate`, `revoke`, `destroy`.

Labels are deliberately low-cardinality. The rejected host is **not** a
label on `delivery.denied`: it is attacker-controlled, so labelling it hands
an attacker a cardinality bomb. Alerts below therefore fire on denial *rate*,
and the responder gets the host from the logs.

---

## Stuck pending credentials

**Signal:** `gateway.credential.recovery.stuck_reports`
**Condition:** any increase over a 5 minute window.
**Severity:** page.
**Runbook:** [stuck-pending-secret-write.md](stuck-pending-secret-write.md)

The worker only reports stuck after 30 minutes of continuous failure, so this
counter is already debounced by the worker itself. Any increase means a
credential has been unresolvable for at least half an hour and the automatic
repair has given up. Do not add a further threshold on top; that delays the
page by the debounce twice.

**Companion condition, same runbook, ticket severity:**
`recovery.passes{outcome="failed_recovery"}` increasing while
`recovery.scanned_events` stays flat. Each pass is failing at the same
sequence. This precedes the stuck report and is the earlier, cheaper catch.

## OpenBao read failure rate

**Signal:** `gateway.credential.resolve.failures` against
`gateway.credential.cache.misses`
**Condition:** failures exceed 5% of cache misses over 10 minutes, or any
failures at all across more than one `source`.
**Severity:** page.
**Runbook:** [unseal-and-key-custody.md](unseal-and-key-custody.md), then
[gateway-projection-miss.md](gateway-projection-miss.md)

Rate against misses, not against total resolves: a cache hit never touches
OpenBao, so a failure ratio computed over all resolves is diluted by cache
hit rate and moves when the cache moves rather than when OpenBao does.

Failures spread across sources point at OpenBao itself (sealed, unreachable,
policy). Failures confined to one source point at that source's credentials.

## Gateway projection lag

**Signal:** `gateway.credential.recovery.passes{outcome="retry_delayed"}`
plus `GET /-/credentials/recovery/status`
**Condition:** `retry_delayed` outcomes continuing for longer than the
15 minute maximum backoff, meaning the worker is not recovering between
retries.
**Severity:** ticket, escalating to page if it precedes a stuck report.
**Runbook:** [gateway-projection-miss.md](gateway-projection-miss.md)

The honest limitation: this measures the *recovery* worker's progress, not
the projection refresh worker's checkpoint lag directly. There is no
instrument for the projection checkpoint's distance behind the stream head,
and that is the metric this alert actually wants. Until it exists, the
recovery worker's health is a correlated proxy, and a projection refresh
falling behind while recovery stays healthy will not fire this.

## Cache miss spike

**Signal:** `gateway.credential.cache.misses` against
`gateway.credential.cache.hits`, by `source`
**Condition:** miss ratio for a source exceeds 50% over 15 minutes while
resolve volume is non-trivial.
**Severity:** ticket.
**Runbook:** [gateway-projection-miss.md](gateway-projection-miss.md)

Expected steady-state miss rate is low: entries live 300 seconds plus
per-credential jitter, so misses should be roughly the number of distinct
credentials divided by the TTL, independent of request volume. A ratio that
tracks request volume means entries are not being retained.

Two benign causes to rule out before investigating: a deploy (empty cache,
resolves once per credential, settles within a TTL) and a genuinely new
integration. Both are self-limiting within 15 minutes, which is why the
window is that long.

A miss spike is also a load amplifier: every miss is an OpenBao read, so a
cache that stops retaining converts request traffic directly into vault
traffic.

## Denied host spike

**Signal:** `gateway.credential.delivery.denied{reason="host"}`
**Condition:** any sustained non-zero rate. A single denial is a
misconfiguration; a sustained rate is either a broken deployment or probing.
**Severity:** ticket. Page if the rate is high enough to suggest enumeration.
**Runbook:** [gateway-projection-miss.md](gateway-projection-miss.md)

The `reason` label separates three different stories that share one counter:

- `host`: a credential was requested for a host its policy does not allow.
  Exfiltration attempt, or a misconfigured integration pointing at the wrong
  endpoint.
- `runtime_service`: an unauthorized runtime service asked for a
  credential. Closer to a lateral-movement signal than a configuration one.
- `injection_location`: a credential was about to be placed somewhere its
  policy forbids, typically a query parameter instead of a header. This one
  is nearly always a code defect, and it is worth a ticket even at a rate of
  one.

Denials happen before the store read, so a denied caller cannot warm the
cache and cannot distinguish a present credential from an absent one. That
property means a denial spike is safe to investigate at ticket pace: nothing
is leaking while you look.

---

## Repeated OpenBao write failures

**Signal:** `gateway.credential.store.write.failures`
**Condition:** more than 3 in a 5 minute window for any one `operation`.
**Severity:** page for `put` and `rotate`; ticket for `revoke` and `destroy`.
**Runbook:** [stuck-pending-secret-write.md](stuck-pending-secret-write.md)

A failed `put` or `rotate` leaves the credential in `pending_write`, which is
the state the recovery worker eventually reports as stuck. This alert exists
so that failure is visible in minutes rather than after the worker's 30
minute debounce, so it fires well before
[stuck pending credentials](#stuck-pending-credentials) does. If both fire,
this one is the earlier symptom of the same incident, not a second one.

The severity split is deliberate. A failed write blocks a customer action
right now. A failed revoke or destroy does not: logical revocation already
took the credential out of runtime use, and physical cleanup is designed to
be retried. Paging on cleanup teaches responders to ignore the page.

Break the metric down by `source` before opening the runbook. One source
failing points at that integration's path or metadata; every source failing
points at OpenBao itself, and
[the read failure alert](#openbao-read-failure-rate) should be firing too.

---

## Alerts with no signal

These three cannot be written yet. Each names what would have to exist
first.

### Orphan cleanup backlog

**Missing:** the cleanup worker, and any measure of backlog. See
[cleanup-worker-failure.md](cleanup-worker-failure.md) and
[orphan-openbao-secret-cleanup.md](orphan-openbao-secret-cleanup.md).

**Interim:** manual reconciliation, unscheduled.

**To close:** the worker plus a gauge of unreconciled paths. The gauge is the
alert, not the worker's liveness: a worker reporting healthy passes while its
backlog grows is exactly the failure a liveness alert cannot see.

### Suspicious API-key verification failures

**Missing:** API-key verification. The `SecretVerifier` path exists for
comparing supplied material against stored material, but there is no
issued-API-key surface with its own verification failures to count.

**To close:** the API key issuance and verification work, with a failure
counter labelled by outcome and never by key or caller identity. The alert
condition then wants failures clustered on one key (credential stuffing
against a known key) distinguished from failures spread across many keys
(enumeration), which requires the counter to carry enough shape to tell those
apart without carrying the key itself.

**Signal source, now specified:** `api_key.denied`, one of the seven required
first-release audit facts in
[ADR#0061](../../../docs/adr/0061-api-key-rotation-grace-and-audit-set.md)
section 5. It lands with build item 6i in CREDENTIAL_PLATFORM_SPEC.md.

The label restriction and the clustering question resolve across two
surfaces rather than one. The metric carries keyspace and outcome, never a
key id, and answers whether the failure rate is abnormal. The audit fact
carries the key id and answers which key, because that is what an audit
fact is for. The responder pivots from the first to the second, so the alert
never has to fire on an identifier to stay actionable.

### Signed-request replay attempts

**Missing:** signed-request verification entirely. Fully bound request
signing is specified in
[ADR#0051](../../../docs/adr/0051-fully-bound-request-signing.md) and
[ADR#0050](../../../docs/adr/0050-signed-first-caller-authentication.md), and
is not implemented. There is no nonce cache and therefore no replay to
detect.

**To close:** the signing implementation, with a counter for rejected
signatures labelled by rejection reason. Replay specifically (a nonce or
timestamp already seen) must be distinguishable from a malformed or
mismatched signature: the first is an attack, the second is usually a client
bug, and collapsing them makes the alert unactionable.

**Signal source, now specified:** `api_key.signed_request_replayed`, a
distinct audit fact from `api_key.denied` in
[ADR#0061](../../../docs/adr/0061-api-key-rotation-grace-and-audit-set.md)
section 5, which is exactly the separation this alert asks for: a spent
`jti` produces the replay fact, every other rejection produces the denial
fact. It lands with build item 6i, and the replay store it reads from is
specified in
[ADR#0051](../../../docs/adr/0051-fully-bound-request-signing.md) section 4.

One caveat for whoever wires this. The verifier fails closed when the replay
store is unavailable, so a store outage produces denials, not replays. Alert
on the replay fact for attacks and on replay-store availability separately,
or an outage reads as a quiet period on the security signal.

---

## Notes on wiring

**Route by owner, not by severity alone.** Every alert here lands on whoever
owns OpenBao and credential lifecycle. If that is one person, say so in the
routing rather than fanning out to a general channel where it will be
ignored.

**No alert fires on a credential identifier.** Not in labels, not in the
alert body. The runbooks take the responder from a rate to the specific
credential through logs, which are access-controlled; alert payloads
generally are not.

**Alerts that cannot fire are worse than absent alerts.** The four above are
listed as missing rather than defined against imaginary metrics, so nobody
configures a monitor that stays green because nothing is emitting.
