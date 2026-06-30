resource "aws_ecs_cluster" "main" {
  name = var.project_name

  # Container Insights: cluster-level CPU/mem/network/storage metrics for the
  # capture tasks, so resource pressure is observable alongside the app metrics.
  setting {
    name  = "containerInsights"
    value = "enabled"
  }
}

resource "aws_ecs_cluster_capacity_providers" "main" {
  cluster_name = aws_ecs_cluster.main.name

  capacity_providers = ["FARGATE", "FARGATE_SPOT"]

  # A Spot reclaim must not drop capture. Keep a base of on-demand (FARGATE)
  # tasks always running, and only use FARGATE_SPOT for additional capacity above
  # the base (a single FARGATE_SPOT task would make every routine Spot reclaim a
  # capture gap).
  default_capacity_provider_strategy {
    capacity_provider = "FARGATE"
    base              = var.ingest_on_demand_base
    weight            = 1
  }

  default_capacity_provider_strategy {
    capacity_provider = "FARGATE_SPOT"
    weight            = 4
  }
}

resource "aws_ecs_task_definition" "app" {
  family                   = var.project_name
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = var.task_cpu
  memory                   = var.task_memory
  execution_role_arn       = aws_iam_role.ecs_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  container_definitions = jsonencode([
    {
      name = var.project_name
      # Bootstrap placeholder only. Every deploy registers a new task-definition
      # revision pinned to an immutable image digest (repo@sha256:...) via
      # .github/workflows/deploy.yml, and the service below ignores
      # task_definition changes, so the running task tracks the deploy-pinned
      # digest and never resolves the mutable :latest tag at runtime.
      image     = "${aws_ecr_repository.app.repository_url}:latest"
      essential = true

      command = ["--config", "/etc/poly-book/default.toml", "ingest"]

      portMappings = [
        {
          containerPort = 9090
          protocol      = "tcp"
        }
      ]

      # Durable WAL on EFS so it survives task restarts and host loss.
      mountPoints = [
        {
          sourceVolume  = "wal"
          containerPath = "/data/wal"
          readOnly      = false
        }
      ]

      environment = concat(
        [
          {
            name  = "PB__STORAGE__PARQUET_BASE_PATH"
            value = "s3://${aws_s3_bucket.data.id}/orderbook"
          },
          {
            name  = "PB__WAL__BASE_PATH"
            value = "/data/wal"
          },
          {
            name  = "PB__METRICS__LISTEN_ADDR"
            value = "0.0.0.0:9090"
          },
          {
            name  = "AWS_DEFAULT_REGION"
            value = var.aws_region
          }
        ],
        [for k, v in var.app_env_vars : { name = k, value = v }]
      )

      secrets = var.enable_clickhouse_service ? [
        {
          name      = "PB__STORAGE__CLICKHOUSE_URL"
          valueFrom = var.clickhouse_app_url_secret_arn
        }
      ] : []

      logConfiguration = {
        logDriver = "awslogs"
        options = {
          "awslogs-group"         = aws_cloudwatch_log_group.app.name
          "awslogs-region"        = var.aws_region
          "awslogs-stream-prefix" = "ecs"
        }
      }
    }
  ])

  volume {
    name = "wal"
    efs_volume_configuration {
      file_system_id     = aws_efs_file_system.wal.id
      transit_encryption = "ENABLED"
      authorization_config {
        access_point_id = aws_efs_access_point.wal.id
        iam             = "ENABLED"
      }
    }
  }

  lifecycle {
    precondition {
      condition     = !var.enable_clickhouse_service || length(trimspace(var.clickhouse_app_url_secret_arn)) > 0
      error_message = "clickhouse_app_url_secret_arn is required when enable_clickhouse_service is true so app tasks receive an authenticated ClickHouse URL."
    }
  }
}

resource "aws_ecs_service" "app" {
  name            = var.project_name
  cluster         = aws_ecs_cluster.main.id
  task_definition = aws_ecs_task_definition.app.arn
  desired_count   = var.desired_count

  # On-demand base so a Spot reclaim cannot drop all ingest capacity; Spot for
  # any capacity above the base.
  capacity_provider_strategy {
    capacity_provider = "FARGATE"
    base              = var.ingest_on_demand_base
    weight            = 1
  }

  capacity_provider_strategy {
    capacity_provider = "FARGATE_SPOT"
    weight            = 4
  }

  network_configuration {
    subnets          = aws_subnet.public[*].id
    security_groups  = [aws_security_group.ecs.id]
    assign_public_ip = true
  }

  # Auto-roll-back a deployment whose new tasks fail to start/stabilize, instead
  # of leaving the service stuck on a broken task definition with no operator
  # signal (no deployment circuit breaker).
  deployment_circuit_breaker {
    enable   = true
    rollback = true
  }

  lifecycle {
    ignore_changes = [task_definition]
  }
}
