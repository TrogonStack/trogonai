# Runbook: Audit Log Export and Review

**State: no audit device is provisioned.** The dev compose stack does not
enable one, and there is no production cluster to enable one on. The
`audit_read` policy in [../policies](../policies) exists and is verified,
but it grants read access to credential *metadata*, which is not the same
thing as the audit log. This file describes what must be enabled and how to
review it; the enabling has not happened.

**Customer-visible impact:** none directly. The impact is on incident
response: without an audit log, "who read this credential" is
unanswerable, and [secret-leak-response.md](secret-leak-response.md) cannot
scope a leak.

## Enable an audit device

Do this before the first real secret is written to a cluster. OpenBao
refuses to serve requests when every enabled audit device fails to write,
which is the correct trade and worth understanding before you enable one:
an audit device with a full disk takes the cluster down. Enable two devices
on independent paths so one failing does not.

```bash
bao audit enable file file_path=/openbao/audit/audit.log
```

Confirm:

```bash
bao audit list -detailed
```

## What is and is not in it

Audit entries record the request and response for every API call: the
path, the operation, the authenticated identity, the token accessor, the
client address, and the timestamp.

**Secret values are HMAC'd, not written in plaintext.** The audit log tells
you a credential was read and by whom; it does not tell you what the value
was. This is the property that makes shipping the log to a general-purpose
log store acceptable. It is also why the log cannot answer "was the leaked
value this one" directly. You compare HMACs, not values.

The HMAC key is per-cluster. Entries from a restored cluster do not compare
against entries from the original.

## Export

Ship the audit log off the node. A log that only exists on the cluster it
audits is not evidence: an attacker with cluster access can edit it, and a
node loss takes it with them.

Requirements for the destination:

- Append-only or write-once retention. The reviewer must not be able to
  delete entries, and neither must the platform operators.
- Retention at least as long as your credential rotation period, so a
  review can cover a full credential lifetime.
- Access separate from OpenBao access. Someone who can read every
  credential should not also be the one who can rewrite the record of it.

## Review loop

Weekly, or after any incident. The questions worth asking of the log:

**Reads by an identity that is not the gateway.** The `gateway_read` role
should be the overwhelming majority of `secret/data/trogonai/...` reads. A
different identity reading credential data is either break-glass (which
should have a corresponding record in
[break-glass-access.md](break-glass-access.md)) or is the finding.

**Any use of the break-glass role.** Every one should map to a recorded
incident. An unmatched break-glass use is the highest-severity finding this
review produces.

**Writes to `secret/data` outside the control-plane identity.** The
`control_plane_write` role is the only one that should create or update
credential material.

**Undelete operations.** `lifecycle_worker_cleanup` is explicitly denied
undelete. An undelete in the log came from break-glass or root. A revoked
credential coming back to life is a customer-visible security event.

**Root token use.** Root tokens should be generated for a specific
operation and revoked immediately after. Standing root token use in the log
means the auth-method binding work is still outstanding, which today it is.

**Read volume per credential.** A credential read far more often than its
integration's traffic explains is worth a question. The gateway caches for
300 seconds, so read volume should be roughly integration count divided by
TTL, not proportional to request volume.

## Record the review

Each review records: the window covered, who reviewed, what was found, and
what was done. A review that finds nothing still records that it happened.
The value of the loop is in the reviews where nothing was found being
credible.
