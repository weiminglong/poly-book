resource "aws_ecs_cluster" "main" {
  name = var.project_name

  setting {
    name  = "containerInsights"
    value = "disabled"
  }
}

resource "aws_ecs_cluster_capacity_providers" "main" {
  cluster_name = aws_ecs_cluster.main.name

  capacity_providers = ["FARGATE_SPOT"]

  default_capacity_provider_strategy {
    capacity_provider = "FARGATE_SPOT"
    weight            = 1
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
      # digest and never resolves the mutable :latest tag at runtime (audit A.51).
      image     = "${aws_ecr_repository.app.repository_url}:latest"
      essential = true

      command = ["--config", "/etc/poly-book/default.toml", "ingest"]

      portMappings = [
        {
          containerPort = 9090
          protocol      = "tcp"
        }
      ]

      environment = concat(
        [
          {
            name  = "PB__STORAGE__PARQUET_BASE_PATH"
            value = "s3://${aws_s3_bucket.data.id}/orderbook"
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
}

resource "aws_ecs_service" "app" {
  name            = var.project_name
  cluster         = aws_ecs_cluster.main.id
  task_definition = aws_ecs_task_definition.app.arn
  desired_count   = var.desired_count

  capacity_provider_strategy {
    capacity_provider = "FARGATE_SPOT"
    weight            = 1
  }

  network_configuration {
    subnets          = aws_subnet.public[*].id
    security_groups  = [aws_security_group.ecs.id]
    assign_public_ip = true
  }

  # Auto-roll-back a deployment whose new tasks fail to start/stabilize, instead
  # of leaving the service stuck on a broken task definition with no operator
  # signal (audit finding P2-INFRA-1: no deployment circuit breaker).
  deployment_circuit_breaker {
    enable   = true
    rollback = true
  }

  lifecycle {
    ignore_changes = [task_definition]
  }
}
