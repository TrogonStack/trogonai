output "bucket_name" {
  description = "Per-tenant S3 bucket name"
  value       = aws_s3_bucket.tenant.id
}

output "bucket_arn" {
  description = "Per-tenant S3 bucket ARN"
  value       = aws_s3_bucket.tenant.arn
}

output "kms_key_arn" {
  description = "Tenant-scoped KMS CMK ARN (annual rotation enabled)"
  value       = aws_kms_key.tenant.arn
}

output "kms_key_alias" {
  description = "KMS alias for the tenant CMK"
  value       = aws_kms_alias.tenant.name
}

output "iam_role_arn" {
  description = "IAM role ARN for tenant worker volume access"
  value       = aws_iam_role.tenant_worker.arn
}

output "iam_policy_arn" {
  description = "IAM policy ARN scoped to the tenant bucket and CMK"
  value       = aws_iam_policy.tenant_worker.arn
}
