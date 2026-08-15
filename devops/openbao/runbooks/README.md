# Credential Platform Runbooks

Operator procedures for the credential vault: the OpenBao deployment that
holds credential material, and the gateway components that write, project,
and resolve it.

These are operator documents, not product documentation. Customer-facing
key-custody guides live in `docs/how-to/`.

## Index

| Runbook | Covers | State |
| --- | --- | --- |
| [openbao-dev-setup.md](openbao-dev-setup.md) | Local and CI OpenBao | Complete |
| [openbao-production-ha.md](openbao-production-ha.md) | Production cluster shape | Deployment not built |
| [unseal-and-key-custody.md](unseal-and-key-custody.md) | Seal, KMS outage, recovery keys | Complete |
| [backup-and-restore.md](backup-and-restore.md) | Snapshots and the restore drill | Complete |
| [audit-log-export-and-review.md](audit-log-export-and-review.md) | Audit device and review loop | Audit device not provisioned |
| [secret-leak-response.md](secret-leak-response.md) | A credential is exposed | Complete |
| [stuck-pending-secret-write.md](stuck-pending-secret-write.md) | Activation never lands | Complete |
| [orphan-openbao-secret-cleanup.md](orphan-openbao-secret-cleanup.md) | Material with no metadata | Manual only, worker not built |
| [cleanup-worker-failure.md](cleanup-worker-failure.md) | Cleanup worker down | Worker not built |
| [gateway-projection-miss.md](gateway-projection-miss.md) | Gateway cannot resolve | Complete |
| [provider-revocation-failure.md](provider-revocation-failure.md) | Provider-side revoke fails | Manual only, not built |
| [break-glass-access.md](break-glass-access.md) | Emergency direct access | Complete |
| [alerts.md](alerts.md) | Alert definitions | Partial, see the file |

"Not built" means the system the runbook would operate does not exist yet.
Those files state what is missing and give the manual interim procedure
where one is possible. They are not placeholders for a procedure that
exists and is undocumented.

## Shared context

**Path convention.** Credential material lives at
`secret/data/trogonai/{owner_id}/credentials/{credential_id}` and its
metadata at `secret/metadata/trogonai/{owner_id}/credentials/{credential_id}`,
where `owner_id` is the project id (ADR#0046) and `credential_id` is
`openbao:{owner}:{scope_key}:{kind}`. The credential id contains `:` and
`/`, so it is not a single path segment. See
[../policies/README.md](../policies/README.md) for why that matters to
policy authoring.

**Roles.** Five policies exist in [../policies](../policies): `control_plane_write`,
`gateway_read`, `lifecycle_worker_cleanup`, `audit_read`, and
`break_glass_admin`. Binding them to an auth method is an open decision;
today every component authenticates with a root or dev token, which means
the least-privilege split these policies describe is not yet enforced in
any running deployment.

**Admin API.** The gateway exposes credential management under
`/-/credentials`, guarded by a bearer admin token. Write operations require
an `Idempotency-Key` header. Recovery state is at
`GET /-/credentials/recovery/status`.

**Metrics.** All credential metrics are OTel instruments emitted by the
gateway:

```text
gateway.credential.revocation.latency     histogram, seconds
gateway.credential.cache.hits             counter, label: source
gateway.credential.cache.misses           counter, label: source
gateway.credential.delivery.denied        counter, labels: source, reason
gateway.credential.resolve.failures       counter, label: source
gateway.credential.recovery.passes        counter
gateway.credential.recovery.errors        counter
gateway.credential.recovery.scanned_events counter
gateway.credential.recovery.recoveries    counter
gateway.credential.recovery.stuck_reports counter
```
