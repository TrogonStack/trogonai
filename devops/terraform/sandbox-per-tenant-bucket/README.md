# Per-tenant sandbox volume bucket

A starter Terraform module that provisions one isolated S3 bucket per tenant
for sandbox volumes, encrypted with a tenant-scoped KMS key, plus an IAM role
and policy that grant a worker read and write access to that bucket and key
only.

The module is not wired into CI. Copy it into your infrastructure repository
and call it with your own remote state backend.

## Resources

| Resource | Purpose |
|----------|---------|
| `aws_kms_key` | Tenant key with `enable_key_rotation = true` |
| `aws_kms_alias` | `alias/sandbox-<tenant_id>`, or `alias/<kms_key_alias>` |
| `aws_s3_bucket` | Dedicated volume bucket |
| `aws_s3_bucket_server_side_encryption_configuration` | SSE-KMS with the tenant key and a bucket key |
| `aws_s3_bucket_versioning` | Versioning enabled |
| `aws_s3_bucket_lifecycle_configuration` | Abort incomplete multipart uploads after 7 days, optional expiration |
| `aws_s3_bucket_public_access_block` | All public access blocked |
| `aws_iam_role` and `aws_iam_policy` | Worker read and write scoped to the bucket and key |

## Usage

```bash
cd devops/terraform/sandbox-per-tenant-bucket

terraform init
terraform validate

terraform apply \
  -var tenant_id=acme \
  -var region=us-east-1 \
  -var retention_days=90
```

The module configures the `aws` provider with `var.region`. Credentials come
from the usual AWS provider sources, such as the environment or a profile.

## Caveats

- The bucket name ends with the AWS account id to avoid global name
  collisions.
- The IAM role trusts `ec2.amazonaws.com`. For EKS IRSA or another identity,
  replace the assume-role policy in your copy.
- `retention_days = 0`, the default, disables object expiration.
- Registry access control for snapshots is configured in the Helm chart, not
  here: see `accessControl` in `devops/helm/sandbox/charts/zot/values.yaml`.
