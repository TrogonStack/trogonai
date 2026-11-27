# Add a registry pull secret

This guide explains how to store a registry credential in the credential broker, reference it from a lease through `pull_secret`, rotate or revoke it, and migrate stored credentials to the envelope format. Follow it when you configure private base image pulls.

## Why pull secrets exist

A lease can pull its base image from a private registry. Credentials must never appear in NATS wire messages or lease payloads. The lease carries only a `PullSecretId`, and the credential broker, `trogon-sandbox-credential-broker`, holds the credential and issues short-lived scoped tokens over NATS request and reply on the `sandbox.credentials.issue.>` subjects. Workers do not hold long-lived provider credentials.

## Prerequisites

- NATS admin access, which is the same trust level `trogon-sandbox-admin` needs for its other commands.
- Registry read credentials for the target, such as a robot account, an IAM role or a personal access token.
- A running `trogon-sandbox-credential-broker`. Its root key is the standard base64 encoding of 32 bytes, supplied as `SANDBOX_KMS_KEY`.

## Register a pull secret

The credential material is read from standard input, so it never appears in the command line or shell history. Set `SANDBOX_KMS_KEY` to the same root key the broker uses.

```sh
trogon-sandbox-admin credential set --tenant acme registry acme-ghcr-read < ghcr.robot.json
```

The arguments are the provider (`registry` for a pull secret), then the credential id. `--tenant` names the tenant that owns the credential. The record is stored encrypted in the broker's KV store, never in a lease payload. The other providers the command accepts are `s3`, `daytona`, `modal`, `azure`, `docker` and `ssh`.

List the stored ids for a provider:

```sh
trogon-sandbox-admin credential list registry
```

## Reference it from a lease

Pass the id on the registry image variant:

```json
{
  "spec": {
    "image": {
      "kind": "registry",
      "reference": {
        "registry": "ghcr.io",
        "repository": "acme/private-agent",
        "tag": "v1.2.0"
      },
      "pull_secret": "acme-ghcr-read"
    }
  }
}
```

In the TypeScript client the field is `pull_secret` on the `registry` image. See [Sandbox foundation](../../architecture/sandbox-foundation.md) for the client.

Pull secrets apply to base image pulls. Snapshot restores use the platform snapshot registry and its own credentials. See [Registry RBAC](../../architecture/sandbox/registry-rbac.md).

## Rotate credentials

```sh
trogon-sandbox-admin credential rotate --tenant acme registry acme-ghcr-read < ghcr.robot.json
```

Schedule rotation before the upstream registry token expires.

## Revoke a token

Revoke an issued token by its id:

```sh
trogon-sandbox-admin credential revoke-token --tenant acme registry <token-id>
```

Tokens a worker already holds can remain valid until their TTL expires unless you revoke them.

## Provision a per-tenant bucket and key

Volume buckets are provisioned outside the cluster. Copy the starter module at `devops/terraform/sandbox-per-tenant-bucket/` into your infrastructure repository:

```sh
cd devops/terraform/sandbox-per-tenant-bucket
terraform init
terraform apply \
  -var tenant_id=acme \
  -var region=us-east-1 \
  -var retention_days=90
```

The module outputs `bucket_name`, `bucket_arn`, `kms_key_arn`, `kms_key_alias`, `iam_role_arn` and `iam_policy_arn`. Key rotation is enabled on the key.

## Credential encryption and the envelope migration

Stored credentials use a versioned record format.

- Format 1 (legacy): AES-256-GCM under the process key (`SANDBOX_KMS_KEY`) with no associated data. A record copied between tenants or credential ids still decrypts.
- Format 2 (envelope): each record has a random data key. The data key is wrapped under the root key with the tenant as associated data and stored with the record. The payload is encrypted with associated data covering the provider, tenant, credential id and record version. Swapping a record between tenants or credential ids, or editing its version or recorded tenant, makes it fail to decrypt.

New writes (`credential set` and `credential rotate`) always use format 2 and need `--tenant`. A credential is opened only for the tenant on the request subject, so a credential stored for one tenant is reported as not found to another. A broker build that predates format 2 cannot read those records.

### Roll out the migration

1. Deploy the current `trogon-sandbox-credential-broker` to every replica. Legacy records stay readable because `SANDBOX_CREDENTIAL_ALLOW_LEGACY` defaults to true, but a legacy record is opened without tenant binding until you migrate it.
2. Preview, then migrate. Legacy records carry no tenant, so you name the owner, and a run without ids gives every legacy record of the provider to that one tenant. With a single tenant, `trogon-sandbox-admin credential migrate --dry-run --tenant <tenant> <provider|all>` lists what would change. Rerun it without `--dry-run` to rewrite. With several tenants, list the legacy ids with the dry run, map each to its owner, and run the command once per tenant with `--credential-id <id>` repeated for that tenant's ids. The command rewrites only records still in format 1, is idempotent and resumable, and prints counts of migrated, would migrate, failed and legacy remaining records. It exits non-zero if any record failed or, on a run without `--credential-id`, if any legacy record remains.
3. Verify zero legacy records with a real run that exits zero and reports `legacy remaining 0` (`all` covers every provider). Only then set `SANDBOX_CREDENTIAL_ALLOW_LEGACY=false` on every broker replica so a downgraded record cannot be planted. The order matters: migrate, verify zero, then disable.

Rolling back to a build that predates the envelope format is not possible once records are rewritten. Restore the KV bucket from a backup instead.

## Related

- [Registry RBAC](../../architecture/sandbox/registry-rbac.md)
- [Alerts](../../reference/sandbox/alerts.md)
- [Build an agent image](./build-an-agent-image.md)
