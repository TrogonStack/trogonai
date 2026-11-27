terraform {
  required_version = ">= 1.5.0"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = ">= 5.0"
    }
  }
}

data "aws_caller_identity" "current" {}

resource "aws_kms_key" "tenant" {
  description             = "Sandbox per-tenant volume encryption key for ${var.tenant_id}"
  enable_key_rotation     = true
  deletion_window_in_days = 30
  tags = {
    Name     = "sandbox-${var.tenant_id}"
    TenantId = var.tenant_id
    Managed  = "trogon-sandbox"
  }
}

resource "aws_kms_alias" "tenant" {
  name          = var.kms_key_alias != "" ? "alias/${var.kms_key_alias}" : "alias/sandbox-${var.tenant_id}"
  target_key_id = aws_kms_key.tenant.key_id
}

resource "aws_s3_bucket" "tenant" {
  bucket = "sandbox-volumes-${var.tenant_id}-${data.aws_caller_identity.current.account_id}"

  tags = {
    Name     = "sandbox-volumes-${var.tenant_id}"
    TenantId = var.tenant_id
    Managed  = "trogon-sandbox"
  }
}

resource "aws_s3_bucket_versioning" "tenant" {
  bucket = aws_s3_bucket.tenant.id

  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_server_side_encryption_configuration" "tenant" {
  bucket = aws_s3_bucket.tenant.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm     = "aws:kms"
      kms_master_key_id = aws_kms_key.tenant.arn
    }
    bucket_key_enabled = true
  }
}

resource "aws_s3_bucket_lifecycle_configuration" "tenant" {
  bucket = aws_s3_bucket.tenant.id

  rule {
    id     = "abort-incomplete-multipart"
    status = "Enabled"

    abort_incomplete_multipart_upload {
      days_after_initiation = 7
    }
  }

  dynamic "rule" {
    for_each = var.retention_days > 0 ? [1] : []
    content {
      id     = "expire-old-objects"
      status = "Enabled"

      expiration {
        days = var.retention_days
      }
    }
  }
}

resource "aws_s3_bucket_public_access_block" "tenant" {
  bucket = aws_s3_bucket.tenant.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

data "aws_iam_policy_document" "tenant_worker" {
  statement {
    sid    = "ListBucket"
    effect = "Allow"
    actions = [
      "s3:ListBucket",
      "s3:GetBucketLocation",
    ]
    resources = [aws_s3_bucket.tenant.arn]
  }

  statement {
    sid    = "ObjectRW"
    effect = "Allow"
    actions = [
      "s3:GetObject",
      "s3:PutObject",
      "s3:DeleteObject",
    ]
    resources = ["${aws_s3_bucket.tenant.arn}/*"]
  }

  statement {
    sid    = "KmsUse"
    effect = "Allow"
    actions = [
      "kms:Encrypt",
      "kms:Decrypt",
      "kms:GenerateDataKey",
      "kms:DescribeKey",
    ]
    resources = [aws_kms_key.tenant.arn]
  }
}

resource "aws_iam_policy" "tenant_worker" {
  name        = "sandbox-volumes-${var.tenant_id}-worker"
  description = "Read/write access to tenant ${var.tenant_id} sandbox volume bucket"
  policy      = data.aws_iam_policy_document.tenant_worker.json
}

resource "aws_iam_role" "tenant_worker" {
  name = "sandbox-volumes-${var.tenant_id}-worker"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Principal = {
        Service = "ec2.amazonaws.com"
      }
      Action = "sts:AssumeRole"
    }]
  })

  tags = {
    TenantId = var.tenant_id
    Managed  = "trogon-sandbox"
  }
}

resource "aws_iam_role_policy_attachment" "tenant_worker" {
  role       = aws_iam_role.tenant_worker.name
  policy_arn = aws_iam_policy.tenant_worker.arn
}
