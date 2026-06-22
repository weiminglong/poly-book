resource "aws_s3_bucket" "data" {
  bucket_prefix = "${var.project_name}-data-"
  # Never auto-delete a populated market-data bucket on `terraform destroy`
  # — this is the system's durable history.
  force_destroy = false
}

# Versioning protects against accidental overwrite/delete of captured data and is
# a prerequisite for recovery from the deterministic-filename overwrite class of
# bug.
resource "aws_s3_bucket_versioning" "data" {
  bucket = aws_s3_bucket.data.id
  versioning_configuration {
    status = "Enabled"
  }
}

resource "aws_s3_bucket_lifecycle_configuration" "data" {
  bucket = aws_s3_bucket.data.id

  rule {
    id     = "transition-to-ia"
    status = "Enabled"
    filter {}

    transition {
      days          = 30
      storage_class = "STANDARD_IA"
    }
  }

  # Bound storage growth from versioning: keep noncurrent versions for a recovery
  # window, then expire them.
  rule {
    id     = "expire-noncurrent-versions"
    status = "Enabled"
    filter {}

    noncurrent_version_expiration {
      noncurrent_days = 30
    }
  }
}

# Customer-managed KMS key for the data bucket. Unlike SSE-S3 (AES256), a CMK
# gives key-level access control and a CloudTrail audit of every decrypt, and lets
# access be revoked via the key policy.
resource "aws_kms_key" "s3" {
  description             = "${var.project_name} S3 market-data bucket encryption"
  deletion_window_in_days = 30
  enable_key_rotation     = true
}

resource "aws_kms_alias" "s3" {
  name          = "alias/${var.project_name}-s3"
  target_key_id = aws_kms_key.s3.key_id
}

resource "aws_s3_bucket_server_side_encryption_configuration" "data" {
  bucket = aws_s3_bucket.data.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm     = "aws:kms"
      kms_master_key_id = aws_kms_key.s3.arn
    }
    # S3 Bucket Keys cut KMS request cost/throttling for high object counts.
    bucket_key_enabled = true
  }
}

# Server access logging to a dedicated, hardened log bucket so reads/writes/
# deletes of captured market data are auditable.
#
# This bucket holds short-lived (90-day), append-only S3 access logs delivered by
# the AWS log-delivery service. The following tfsec checks are intentionally
# waived for it (not the data bucket, which has all of them):
#   - versioning: access logs are immutable single-writer deliveries; versioning
#     adds cost/noise with nothing to protect against.
#   - self server-access-logging: a log bucket logging itself is a recursive loop.
#   - customer-managed KMS: SSE-S3 (AES256, below) is the right tier for logs;
#     the log-delivery service writes plainly and a CMK adds key-policy/cost
#     overhead disproportionate to 90-day operational logs.
#tfsec:ignore:AVD-AWS-0090
#tfsec:ignore:AVD-AWS-0089
resource "aws_s3_bucket" "logs" {
  bucket_prefix = "${var.project_name}-logs-"
  force_destroy = false
}

# Encrypt the access-log bucket at rest with SSE-S3 (AES256). The S3 log-delivery
# service writes objects here, so SSE-S3 (not a CMK) is the compatible default —
# a CMK is the right tier for the data bucket, not 90-day operational logs.
#tfsec:ignore:AVD-AWS-0132
resource "aws_s3_bucket_server_side_encryption_configuration" "logs" {
  bucket = aws_s3_bucket.logs.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_public_access_block" "logs" {
  bucket = aws_s3_bucket.logs.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_lifecycle_configuration" "logs" {
  bucket = aws_s3_bucket.logs.id

  rule {
    id     = "expire-access-logs"
    status = "Enabled"
    filter {}

    expiration {
      days = 90
    }
  }
}

# Allow the S3 log-delivery service to write access logs, scoped to this account
# and the data bucket as source (modern bucket-policy grant, not ACLs).
resource "aws_s3_bucket_policy" "logs" {
  bucket = aws_s3_bucket.logs.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Sid       = "S3ServerAccessLogsPolicy"
      Effect    = "Allow"
      Principal = { Service = "logging.s3.amazonaws.com" }
      Action    = "s3:PutObject"
      Resource  = "${aws_s3_bucket.logs.arn}/*"
      Condition = {
        ArnLike      = { "aws:SourceArn" = aws_s3_bucket.data.arn }
        StringEquals = { "aws:SourceAccount" = data.aws_caller_identity.current.account_id }
      }
    }]
  })
}

resource "aws_s3_bucket_logging" "data" {
  bucket        = aws_s3_bucket.data.id
  target_bucket = aws_s3_bucket.logs.id
  target_prefix = "s3-access/"

  # S3 validates that the target bucket grants the log-delivery service write
  # access, so the bucket policy must be applied first.
  depends_on = [aws_s3_bucket_policy.logs]
}

resource "aws_s3_bucket_public_access_block" "data" {
  bucket = aws_s3_bucket.data.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}
