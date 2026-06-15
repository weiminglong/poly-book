resource "aws_s3_bucket" "data" {
  bucket_prefix = "${var.project_name}-data-"
  # Never auto-delete a populated market-data bucket on `terraform destroy`
  # (audit finding A.134) — this is the system's durable history.
  force_destroy = false
}

# Versioning protects against accidental overwrite/delete of captured data and is
# a prerequisite for recovery from the deterministic-filename overwrite class of
# bug (audit finding A.134).
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

resource "aws_s3_bucket_server_side_encryption_configuration" "data" {
  bucket = aws_s3_bucket.data.id

  rule {
    apply_server_side_encryption_by_default {
      sse_algorithm = "AES256"
    }
  }
}

resource "aws_s3_bucket_public_access_block" "data" {
  bucket = aws_s3_bucket.data.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}
