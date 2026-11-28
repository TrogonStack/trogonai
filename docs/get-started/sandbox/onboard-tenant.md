# Onboard a new tenant end-to-end

This tutorial takes you from a tenant id `acme` to a provisioned tenant with its own S3 bucket, KMS key, registry pull policy and quota caps. At the end you can mint API keys for the tenant, run leases, publish snapshots and tag them.

It assumes you finished [your first sandbox lease](./first-sandbox-lease.md) and [publishing a signed snapshot](./publish-signed-snapshot.md). You also need `terraform` on your path, an AWS account with KMS and S3 permissions, and the sandbox stack deployed to a cluster with the chart in `devops/helm/charts/trogon-sandbox/`. The admin commands below run `trogon-sandbox-admin` from any host that can reach the stack's NATS server.

## What you will build

```text
Terraform module     -> AWS: bucket + KMS key + IAM role
Helm chart           -> registry access control with a tenant pull-only policy
trogon-sandbox-admin -> API key + image allowlist + quota + snapshot tag for the tenant
```

## Steps

### 1. Provision the per-tenant AWS infrastructure

From the repository root:

```bash
cd devops/terraform/sandbox-per-tenant-bucket
terraform init
terraform apply \
  -var tenant_id=acme \
  -var region=us-east-1 \
  -var retention_days=90
```

The module creates:

- An S3 bucket named `sandbox-volumes-acme-<account id>` for the tenant's volumes.
- A KMS key with the alias `alias/sandbox-acme` and key rotation enabled.
- An IAM role, `sandbox-volumes-acme-worker`, that workers assume to reach the bucket.

`retention_days` expires objects after that many days. The default of `0` turns the expiration rule off.

### 2. Add the tenant to the registry access control

Edit `devops/helm/charts/trogon-sandbox/values.yaml`. The registry settings sit under the `zot` key:

```yaml
zot:
  accessControl:
    platformIdentity:
      serviceAccount: sandbox-snapshot-builder
      pushRepositories: ["snapshots/**"]
    tenants:
      - tenantId: acme
        serviceAccount: sandbox-acme-puller
```

Then upgrade the release:

```bash
helm upgrade <release> devops/helm/charts/trogon-sandbox/ --reuse-values
```

This grants `sandbox-acme-puller` read-only access to the `snapshots/acme` repository. The snapshot builder identity keeps push access to `snapshots/**`.

### 3. Mint an API key for the tenant

```bash
export NATS_URL=nats://<nats host>:4222
trogon-sandbox-admin api-key mint \
  --tenant acme \
  --service-account acme-primary \
  --name "acme primary key"
```

Copy the printed token. It is the bearer credential the tenant's clients present. Store it safely.

### 4. Configure the tenant's image allowlist

```bash
trogon-sandbox-admin image-policy set \
  --tenant acme \
  --pattern 'registry.example.com/agents/*'
```

A pattern is an exact image reference or a prefix ending in a single `*`.

### 5. Set per-tenant quotas

```bash
trogon-sandbox-admin tenant-limit set \
  --tenant acme \
  --max-cpu-seconds 3600 \
  --max-memory-mib 4096
```

Check the monthly usage later with `trogon-sandbox-admin quota show --tenant acme`.

### 6. Optional: configure the prewarm policy

If acme wants warm sandboxes ready for fast leases:

```bash
trogon-sandbox-admin prewarm set acme sha256:<image digest> 5 docker
```

The positional arguments are the tenant, the digest, the pool size and one or more provider kinds.

### 7. Optional: set a friendly snapshot tag

```bash
trogon-sandbox-admin tag set acme prod sha256:<snapshot digest> --artifact-type sandbox_rootfs_v1
```

The tenant can now lease from the tag by sending this image in its spec, and the broker resolves it to the digest:

```json
{
  "kind": "snapshot_tag",
  "tag": { "tenant": "acme", "name": "prod" },
  "artifact_type": "sandbox_rootfs_v1"
}
```

### 8. Verify

The tenant's first lease and snapshot publish should now succeed. Watch the audit subjects:

```bash
nats sub 'sandbox.audit.snapshot-tag.>'
nats sub 'sandbox.audit.credentials'
```

## Roll back

To off-board the tenant:

```bash
trogon-sandbox-admin api-key revoke --token <token>
trogon-sandbox-admin tenant-limit clear --tenant acme
trogon-sandbox-admin image-policy clear --tenant acme
helm upgrade <release> devops/helm/charts/trogon-sandbox/ --reuse-values   # after removing acme from values.yaml
terraform destroy -var tenant_id=acme -var region=us-east-1
```

The KMS key is scheduled for deletion after a 30-day window, so you can recover it until then.

## See also

- [Registry RBAC](../../architecture/sandbox/registry-rbac.md)
- [Volumes](../../architecture/sandbox/volumes.md)
- [Add a pull secret](../../how-to/sandbox/add-a-pull-secret.md)
- [Deploy the sandbox stack](../../how-to/sandbox/deploy.md)
- [Alerts](../../reference/sandbox/alerts.md)
