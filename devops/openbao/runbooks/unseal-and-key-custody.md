# Runbook: Unseal and Key Custody

**Customer-visible impact when OpenBao is sealed:** every credential
resolution that misses the gateway cache fails. Integrations keep working
until their cached material expires (300s TTL, 30s jitter), then fail
closed. Nothing is lost; nothing resolves.

## The rule

Production OpenBao auto-unseals against a cloud KMS key. The Shamir quorum
seal is prohibited as the routine production seal
([ADR#0052](../../../docs/adr/0052-cloud-kms-production-seal.md)). The
single exception is a deployment whose network cannot reach any cloud KMS,
recorded per deployment with its custody ceremony documented.

This means the normal answer to "OpenBao restarted, who has the shares" is
**nobody, and nobody needs them.** A restart unseals itself by asking KMS to
unwrap the root key. If you find yourself typing unseal shares in
production, something is wrong with the deployment, not with the procedure.

## Check the seal state

```bash
bao status
```

`Sealed false` is healthy. `Sealed true` on a running production node means
the auto-unseal failed, which is a KMS or IAM problem, not an OpenBao one.

## KMS outage

The failure mode has a specific shape, and knowing it prevents the wrong
response:

- **Nodes that are already unsealed stay unsealed and keep serving.** The
  KMS is only consulted at unseal time. An outage is invisible to a healthy
  cluster.
- **A node that restarts during the outage blocks unsealed.** It cannot
  join, cannot serve, and will unseal on its own when KMS returns.
- **Do not restart nodes to "fix" a KMS outage.** A running node is the
  asset. Restarting it converts a non-event into an outage.

Response:

1. Confirm the KMS key is reachable and the IAM binding intact from a node's
   network position. The failure is usually a revoked or scoped-down IAM
   permission, not the provider being down.
2. If a single node is stuck, leave it out. Quorum on the remaining nodes
   keeps the cluster serving.
3. If the whole cluster is down and KMS will not return, you are in the
   recovery path below.

## Recovery keys are not an unseal path

Auto-unseal still generates a recovery-key quorum at initialization. Those
shares:

- **can** authorize administrative operations: generating a root token,
  approving a seal migration;
- **cannot** unseal the cluster or decrypt the root key.

So they are not a fallback when KMS is unreachable. Anyone reaching for
recovery keys during a KMS outage has misunderstood what they do.

## Custody ceremony

Recovery shares are break-glass only
([ADR#0023](../../../docs/adr/0023-secret-management-and-key-custody-direction.md)):

- Shares are generated at initialization and distributed to distinct
  holders. One holder with two shares defeats the quorum.
- Shares are held out of band. Not in the credential vault they protect,
  not in the same cloud account as the KMS key, not in a shared password
  manager that platform operators can all read.
- Any use is a two-person operation, recorded, with the root token revoked
  immediately after.
- Holders are re-attested on a schedule. A share held by someone who left
  is not a share.

See [break-glass-access.md](break-glass-access.md) for what an actual
break-glass operation looks like.

## Seal migration

Migrating away from a KMS key requires the **current seal to still be
reachable**. Migration decrypts with the old seal and re-encrypts with the
new one. This has a hard consequence:

> You can migrate away from a failing KMS key only while that key still
> unwraps. Permanent loss of the seal key with no surviving migration path
> loses the cluster, storage backups included.

Storage backups do not help: they are encrypted by the barrier keyring,
which is encrypted by the root key, which is wrapped by the seal key. This
is why [ADR#0052](../../../docs/adr/0052-cloud-kms-production-seal.md)
requires deletion protection, retained prior key versions, and a documented
multi-region or recovery path on the KMS key. Those requirements are the
whole defense.

Migration takes a brief full-cluster restart. Plan it as a maintenance
window, not as an emergency response.

## Dev and local

Dev mode uses a single-share seal held by the server itself. No custody, no
ceremony, nothing to protect. See
[openbao-dev-setup.md](openbao-dev-setup.md).
