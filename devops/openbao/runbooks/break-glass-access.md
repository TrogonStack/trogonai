# Runbook: Break-Glass Access

Direct access to OpenBao, bypassing the platform secrets service. Used when
the control plane cannot perform an operation the incident requires.

**Customer-visible impact:** none directly. The impact is on trust: every
break-glass use is an operator reading or mutating customer credential
material outside the audited application path. It is justified by an
incident or it is a finding.

## When it is warranted

Break-glass is for when the normal path is unavailable, not when it is
inconvenient:

- The gateway cannot start, and a credential must be revoked now.
- A revocation must be verified directly because the event stream and
  OpenBao disagree, typically after a restore.
- Reconciliation after a restore requires enumerating managed paths, which
  no API exposes.
- An undelete is genuinely required. This is the only role that can do it;
  `lifecycle_worker_cleanup` is explicitly denied undelete precisely so that
  reversing a revocation is a deliberate, attended act.

It is **not** warranted for reading a credential to "check" it, for
debugging an integration, or for anything a `PUT` through the management API
would accomplish. Reading credential material never fixes anything, and the
read itself is the risk.

## What the role can and cannot do

`break_glass_admin` ([../policies/break-glass-admin.hcl](../policies/break-glass-admin.hcl))
grants full control of `trogonai/` credential paths: create, read, update,
delete, undelete, destroy, and list.

It grants nothing on `sys/`, nothing on `auth/`, and nothing on any other
mount. Holding it cannot rewrite policy, create auth roles, or mint further
access. That boundary is the point: escalating past the credential subtree
is a separate and louder step, not a side effect of holding this token.

It is issued as a short-TTL token behind an approval and is never attached to
a running service identity. A break-glass policy bound to a long-lived
service token is not break-glass; it is the default posture with an alarming
name.

## Procedure

**1. Record the reason before you start.** The incident, what the normal path
cannot do, and what you intend to do. Written first, because a record written
afterward is written by someone who already knows how it turned out.

**2. Get a second person.** Break-glass is a two-person operation
([ADR#0023](../../../docs/adr/0023-secret-management-and-key-custody-direction.md)).
The second person observes and co-signs the record. They are not a rubber
stamp: their job is to ask whether the normal path was actually tried.

**3. Mint the token.** Today this means generating a root token using the
recovery-key quorum, because binding the policies to an auth method is an
open decision and no break-glass auth role exists in any running deployment.
That gap is real and worth stating plainly: the current procedure grants more
than `break_glass_admin` does, because root grants everything. Until the auth
binding exists, the scoping in the policy file is a design, not a control.

**4. Do the single operation you recorded.** Not the adjacent one that looks
useful while you are in there.

**5. Revoke the token immediately.** Not at the end of the incident, at the
end of the operation.

```bash
bao token revoke -self
```

**6. Close the record.** What was done, what the result was, and whether
anything unexpected was observed.

## Verify afterward

- The audit log shows the operation and shows the token revoked. If no audit
  device is enabled, this cannot be verified, and the record must say so
  rather than implying it was checked. See
  [audit-log-export-and-review.md](audit-log-export-and-review.md).
- The event stream is reconciled with whatever you changed in OpenBao. A
  direct OpenBao mutation leaves the application's view stale by definition:
  see [gateway-projection-miss.md](gateway-projection-miss.md).
- The next audit review matches this use to this record. An unmatched
  break-glass use is the highest-severity finding that review produces.

## The undelete case specifically

Undelete resurrects a revoked credential. If a customer was told a credential
was revoked, undeleting it makes that statement false. Do not undelete to
"restore service" after an accidental revocation: supply a new credential
instead. Undelete is for reconciling a restore that lost a revocation, and
for nothing else.
