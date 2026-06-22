# Read-only `serve` API service.
#
# AUTHORED BUT NOT YET APPLIED against a live AWS account: passes
# `terraform validate`/`fmt`, not `apply`ed. The documented topology is
# `ingest` (writes the WAL) + `serve` (hydrates from S3 checkpoints, tails the
# shared WAL, serves HTTP/WS). serve mounts the same EFS WAL as ingest; it does
# not take the writer flock (only writers do), it just reads the tail and commits
# its consumer position files.

resource "aws_ecs_task_definition" "serve" {
  family                   = "${var.project_name}-serve"
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = var.task_cpu
  memory                   = var.task_memory
  execution_role_arn       = aws_iam_role.ecs_execution.arn
  task_role_arn            = aws_iam_role.ecs_task.arn

  container_definitions = jsonencode([
    {
      name = "${var.project_name}-serve"
      # Bootstrap placeholder; the deploy workflow pins an immutable digest and
      # the service ignores task_definition changes.
      image     = "${aws_ecr_repository.app.repository_url}:latest"
      essential = true

      command = ["--config", "/etc/poly-book/default.toml", "serve", "--tokens", var.serve_tokens]

      portMappings = [
        { containerPort = 3000, protocol = "tcp" },
        { containerPort = 9090, protocol = "tcp" }
      ]

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
          { name = "PB__WAL__BASE_PATH", value = "/data/wal" },
          { name = "PB__API__LISTEN_ADDR", value = "0.0.0.0:3000" },
          { name = "PB__METRICS__LISTEN_ADDR", value = "0.0.0.0:9090" },
          { name = "AWS_DEFAULT_REGION", value = var.aws_region }
        ],
        var.enable_clickhouse_service ? [
          {
            name  = "PB__STORAGE__CLICKHOUSE_URL"
            value = "http://${aws_service_discovery_service.clickhouse[0].name}.${aws_service_discovery_private_dns_namespace.internal[0].name}:8123"
          }
        ] : [],
        [for k, v in var.app_env_vars : { name = k, value = v }]
      )

      logConfiguration = {
        logDriver = "awslogs"
        options = {
          "awslogs-group"         = aws_cloudwatch_log_group.app.name
          "awslogs-region"        = var.aws_region
          "awslogs-stream-prefix" = "serve"
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
}

resource "aws_ecs_service" "serve" {
  name            = "${var.project_name}-serve"
  cluster         = aws_ecs_cluster.main.id
  task_definition = aws_ecs_task_definition.serve.arn
  desired_count   = var.serve_desired_count

  # serve is the latency-sensitive read path; keep it entirely on-demand.
  capacity_provider_strategy {
    capacity_provider = "FARGATE"
    weight            = 1
  }

  network_configuration {
    subnets          = aws_subnet.public[*].id
    security_groups  = [aws_security_group.ecs.id]
    assign_public_ip = true
  }

  deployment_circuit_breaker {
    enable   = true
    rollback = true
  }

  lifecycle {
    ignore_changes = [task_definition]
  }
}

# The API port is reachable only from inside the VPC; front it with an ALB or an
# authenticating proxy before any public exposure (and set api.auth_token, see
# docs/serve-api.md).
resource "aws_security_group_rule" "serve_api_ingress" {
  type              = "ingress"
  from_port         = 3000
  to_port           = 3000
  protocol          = "tcp"
  cidr_blocks       = [aws_vpc.main.cidr_block]
  security_group_id = aws_security_group.ecs.id
  description       = "serve HTTP/WS API (VPC-internal only)"
}
