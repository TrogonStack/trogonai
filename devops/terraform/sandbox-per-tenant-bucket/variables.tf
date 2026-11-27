variable "tenant_id" {
  description = "Tenant identifier (used in bucket name, KMS alias, and IAM resource names)"
  type        = string

  validation {
    condition     = can(regex("^[a-z0-9-]+$", var.tenant_id))
    error_message = "tenant_id must be lowercase alphanumeric with hyphens only."
  }
}

variable "region" {
  description = "AWS region for the tenant bucket and KMS CMK"
  type        = string
}

variable "kms_key_alias" {
  description = "Optional KMS alias name without the alias/ prefix (defaults to sandbox-<tenant_id>)"
  type        = string
  default     = ""
}

variable "retention_days" {
  description = "S3 object expiration in days (0 disables expiration rule)"
  type        = number
  default     = 0
}
