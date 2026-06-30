# --- ECS Execution Role (pull images, push logs) ---

resource "aws_iam_role" "ecs_execution" {
  name = "${var.project_name}-ecs-execution"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Action    = "sts:AssumeRole"
      Effect    = "Allow"
      Principal = { Service = "ecs-tasks.amazonaws.com" }
    }]
  })
}

resource "aws_iam_role_policy_attachment" "ecs_execution" {
  role       = aws_iam_role.ecs_execution.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}

locals {
  ecs_runtime_secret_arns = compact([
    var.serve_api_auth_token_secret_arn,
    var.clickhouse_password_secret_arn,
    var.clickhouse_app_url_secret_arn
  ])
}

resource "aws_iam_role_policy" "ecs_execution_runtime_secrets" {
  count = length(local.ecs_runtime_secret_arns) > 0 ? 1 : 0
  name  = "${var.project_name}-runtime-secrets"
  role  = aws_iam_role.ecs_execution.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "secretsmanager:GetSecretValue",
          "ssm:GetParameter",
          "ssm:GetParameters"
        ]
        Resource = local.ecs_runtime_secret_arns
      }
    ]
  })
}

# --- ECS Task Roles ---

resource "aws_iam_role" "ecs_task" {
  name = "${var.project_name}-ecs-task-writer"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Action    = "sts:AssumeRole"
      Effect    = "Allow"
      Principal = { Service = "ecs-tasks.amazonaws.com" }
    }]
  })
}

resource "aws_iam_role" "ecs_task_readonly" {
  name = "${var.project_name}-ecs-task-readonly"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Action    = "sts:AssumeRole"
      Effect    = "Allow"
      Principal = { Service = "ecs-tasks.amazonaws.com" }
    }]
  })
}

resource "aws_iam_role" "ecs_task_reconcile" {
  name = "${var.project_name}-ecs-task-reconcile"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Action    = "sts:AssumeRole"
      Effect    = "Allow"
      Principal = { Service = "ecs-tasks.amazonaws.com" }
    }]
  })
}

resource "aws_iam_role" "ecs_task_clickhouse" {
  name = "${var.project_name}-ecs-task-clickhouse"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Action    = "sts:AssumeRole"
      Effect    = "Allow"
      Principal = { Service = "ecs-tasks.amazonaws.com" }
    }]
  })
}

resource "aws_iam_role_policy" "ecs_task_s3" {
  name = "${var.project_name}-s3-access"
  role = aws_iam_role.ecs_task.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "s3:GetObject",
          "s3:PutObject",
          "s3:ListBucket"
        ]
        Resource = [
          aws_s3_bucket.data.arn,
          "${aws_s3_bucket.data.arn}/*"
        ]
      },
      {
        # The data bucket is SSE-KMS encrypted, so the task must use the CMK to
        # read (Decrypt) and write (GenerateDataKey) objects. Scoped to this one
        # key, not "*".
        Effect = "Allow"
        Action = [
          "kms:Decrypt",
          "kms:GenerateDataKey"
        ]
        Resource = aws_kms_key.s3.arn
      }
    ]
  })
}

resource "aws_iam_role_policy" "ecs_task_reconcile_s3" {
  name = "${var.project_name}-s3-reconcile"
  role = aws_iam_role.ecs_task_reconcile.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        # Required by the offline `reconcile` command, which replaces stale
        # Parquet partitions (delete-then-write) when rebuilding from the WAL.
        # Keep this destructive permission off the always-on ingest/serve role.
        Effect = "Allow"
        Action = [
          "s3:GetObject",
          "s3:PutObject",
          "s3:ListBucket",
          "s3:DeleteObject"
        ]
        Resource = [
          aws_s3_bucket.data.arn,
          "${aws_s3_bucket.data.arn}/*"
        ]
      },
      {
        Effect = "Allow"
        Action = [
          "kms:Decrypt",
          "kms:GenerateDataKey"
        ]
        Resource = aws_kms_key.s3.arn
      }
    ]
  })
}

resource "aws_iam_role_policy" "ecs_task_readonly_s3" {
  name = "${var.project_name}-s3-readonly"
  role = aws_iam_role.ecs_task_readonly.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "s3:GetObject",
          "s3:ListBucket"
        ]
        Resource = [
          aws_s3_bucket.data.arn,
          "${aws_s3_bucket.data.arn}/*"
        ]
      },
      {
        Effect   = "Allow"
        Action   = ["kms:Decrypt"]
        Resource = aws_kms_key.s3.arn
      }
    ]
  })
}

# EFS access for the durable WAL volume. Required because the WAL EFS access
# point is mounted with IAM authorization (`iam = "ENABLED"`); without this the
# ingest/serve tasks cannot mount it.
resource "aws_iam_role_policy" "ecs_task_efs" {
  name = "${var.project_name}-efs-access"
  role = aws_iam_role.ecs_task.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "elasticfilesystem:ClientMount",
          "elasticfilesystem:ClientWrite"
        ]
        Resource = aws_efs_file_system.wal.arn
        Condition = {
          StringEquals = {
            "elasticfilesystem:AccessPointArn" = aws_efs_access_point.wal.arn
          }
        }
      }
    ]
  })
}

resource "aws_iam_role_policy" "ecs_task_reconcile_efs" {
  name = "${var.project_name}-efs-reconcile-access"
  role = aws_iam_role.ecs_task_reconcile.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "elasticfilesystem:ClientMount",
          "elasticfilesystem:ClientWrite"
        ]
        Resource = aws_efs_file_system.wal.arn
        Condition = {
          StringEquals = {
            "elasticfilesystem:AccessPointArn" = aws_efs_access_point.wal.arn
          }
        }
      }
    ]
  })
}

resource "aws_iam_role_policy" "ecs_task_readonly_efs" {
  name = "${var.project_name}-efs-readonly-task-access"
  role = aws_iam_role.ecs_task_readonly.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "elasticfilesystem:ClientMount",
          # serve writes only its consumer_*.pos files on the WAL access point.
          "elasticfilesystem:ClientWrite"
        ]
        Resource = aws_efs_file_system.wal.arn
        Condition = {
          StringEquals = {
            "elasticfilesystem:AccessPointArn" = aws_efs_access_point.wal.arn
          }
        }
      }
    ]
  })
}

# --- GitHub Actions OIDC Role ---

resource "aws_iam_openid_connect_provider" "github" {
  url             = "https://token.actions.githubusercontent.com"
  client_id_list  = ["sts.amazonaws.com"]
  thumbprint_list = ["ffffffffffffffffffffffffffffffffffffffff"]
}

resource "aws_iam_role" "github_actions" {
  name = "${var.project_name}-github-actions"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Action = "sts:AssumeRoleWithWebIdentity"
      Principal = {
        Federated = aws_iam_openid_connect_provider.github.arn
      }
      Condition = {
        StringEquals = {
          "token.actions.githubusercontent.com:aud" = "sts.amazonaws.com"
        }
        StringLike = {
          "token.actions.githubusercontent.com:sub" = "repo:${var.github_org}/${var.github_repo}:ref:refs/heads/main"
        }
      }
    }]
  })
}

resource "aws_iam_role_policy" "github_actions" {
  name = "${var.project_name}-deploy"
  role = aws_iam_role.github_actions.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "ecr:GetAuthorizationToken"
        ]
        Resource = "*"
      },
      {
        Effect = "Allow"
        Action = [
          "ecr:BatchCheckLayerAvailability",
          "ecr:GetDownloadUrlForLayer",
          "ecr:BatchGetImage",
          "ecr:PutImage",
          "ecr:InitiateLayerUpload",
          "ecr:UploadLayerPart",
          "ecr:CompleteLayerUpload"
        ]
        Resource = aws_ecr_repository.app.arn
      },
      {
        # RegisterTaskDefinition / DescribeTaskDefinition do not support
        # resource-level permissions in IAM, so they must use "*" (this is an AWS
        # limitation, not over-permissioning).
        Effect = "Allow"
        Action = [
          "ecs:RegisterTaskDefinition",
          "ecs:DescribeTaskDefinition"
        ]
        Resource = "*"
      },
      {
        # UpdateService / DescribeServices DO support resource-level permissions,
        # so scope them to just the poly-book services — a compromised pipeline can
        # no longer repoint or redeploy arbitrary ECS services in the account
        #. Both the ingest (`app`) and `serve` services are deployable.
        Effect   = "Allow"
        Action   = ["ecs:UpdateService", "ecs:DescribeServices"]
        Resource = [aws_ecs_service.app.id, aws_ecs_service.serve.id]
      },
      {
        Effect = "Allow"
        Action = "iam:PassRole"
        Resource = [
          aws_iam_role.ecs_execution.arn,
          aws_iam_role.ecs_task.arn,
          aws_iam_role.ecs_task_readonly.arn,
          aws_iam_role.ecs_task_clickhouse.arn
        ]
      }
    ]
  })
}
