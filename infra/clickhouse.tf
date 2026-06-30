# Optional single-node ClickHouse on ECS + EFS.
#
# AUTHORED BUT NOT YET APPLIED against a live AWS account: passes
# `terraform validate`/`fmt`, not `apply`ed or load-tested. Gated on
# `enable_clickhouse_service` (default false).
#
# NOTE: a single ClickHouse node on Fargate+EFS is a basic, non-HA starting
# point. For production prefer managed ClickHouse (ClickHouse Cloud) or a
# dedicated, EBS-backed instance and just set PB__STORAGE__CLICKHOUSE_URL — EFS is
# NFS and is not ideal for ClickHouse's IO pattern. serve discovers this node via
# Cloud Map private DNS (clickhouse.<project>.internal:8123).

resource "aws_service_discovery_private_dns_namespace" "internal" {
  count       = var.enable_clickhouse_service ? 1 : 0
  name        = "${var.project_name}.internal"
  description = "Internal service discovery for ${var.project_name}"
  vpc         = aws_vpc.main.id
}

resource "aws_service_discovery_service" "clickhouse" {
  count = var.enable_clickhouse_service ? 1 : 0
  name  = "clickhouse"

  dns_config {
    namespace_id = aws_service_discovery_private_dns_namespace.internal[0].id
    dns_records {
      ttl  = 10
      type = "A"
    }
    routing_policy = "MULTIVALUE"
  }

  health_check_custom_config {
    failure_threshold = 1
  }
}

resource "aws_security_group" "clickhouse" {
  count       = var.enable_clickhouse_service ? 1 : 0
  name_prefix = "${var.project_name}-ch-"
  description = "ClickHouse SG (HTTP + native from ECS tasks only)"
  vpc_id      = aws_vpc.main.id

  ingress {
    from_port       = 8123
    to_port         = 8123
    protocol        = "tcp"
    security_groups = [aws_security_group.ecs.id]
    description     = "ClickHouse HTTP from ECS tasks"
  }

  ingress {
    from_port       = 9000
    to_port         = 9000
    protocol        = "tcp"
    security_groups = [aws_security_group.ecs.id]
    description     = "ClickHouse native from ECS tasks"
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  lifecycle {
    create_before_destroy = true
  }

  tags = { Name = "${var.project_name}-ch" }
}

# Durable EFS volume for ClickHouse data.
resource "aws_efs_file_system" "clickhouse" {
  count          = var.enable_clickhouse_service ? 1 : 0
  creation_token = "${var.project_name}-ch"
  encrypted      = true
  tags           = { Name = "${var.project_name}-ch" }
}

resource "aws_efs_mount_target" "clickhouse" {
  count           = var.enable_clickhouse_service ? length(aws_subnet.public) : 0
  file_system_id  = aws_efs_file_system.clickhouse[0].id
  subnet_id       = aws_subnet.public[count.index].id
  security_groups = [aws_security_group.efs.id]
}

resource "aws_ecs_task_definition" "clickhouse" {
  count                    = var.enable_clickhouse_service ? 1 : 0
  family                   = "${var.project_name}-clickhouse"
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = var.clickhouse_cpu
  memory                   = var.clickhouse_memory
  execution_role_arn       = aws_iam_role.ecs_execution.arn
  task_role_arn            = aws_iam_role.ecs_task_clickhouse.arn

  container_definitions = jsonencode([
    {
      name      = "clickhouse"
      image     = "clickhouse/clickhouse-server:24.8"
      essential = true

      portMappings = [
        { containerPort = 8123, protocol = "tcp" },
        { containerPort = 9000, protocol = "tcp" }
      ]

      ulimits = [
        { name = "nofile", softLimit = 262144, hardLimit = 262144 }
      ]

      environment = [
        { name = "CLICKHOUSE_DB", value = "poly_book" },
        { name = "CLICKHOUSE_USER", value = var.clickhouse_user },
        { name = "CLICKHOUSE_DEFAULT_ACCESS_MANAGEMENT", value = "1" }
      ]

      secrets = [
        {
          name      = "CLICKHOUSE_PASSWORD"
          valueFrom = var.clickhouse_password_secret_arn
        }
      ]

      mountPoints = [
        {
          sourceVolume  = "ch-data"
          containerPath = "/var/lib/clickhouse"
          readOnly      = false
        }
      ]

      logConfiguration = {
        logDriver = "awslogs"
        options = {
          "awslogs-group"         = aws_cloudwatch_log_group.app.name
          "awslogs-region"        = var.aws_region
          "awslogs-stream-prefix" = "clickhouse"
        }
      }
    }
  ])

  volume {
    name = "ch-data"
    efs_volume_configuration {
      file_system_id     = aws_efs_file_system.clickhouse[0].id
      transit_encryption = "ENABLED"
    }
  }

  lifecycle {
    precondition {
      condition     = length(trimspace(var.clickhouse_password_secret_arn)) > 0
      error_message = "clickhouse_password_secret_arn is required when enable_clickhouse_service is true."
    }
  }
}

resource "aws_ecs_service" "clickhouse" {
  count           = var.enable_clickhouse_service ? 1 : 0
  name            = "${var.project_name}-clickhouse"
  cluster         = aws_ecs_cluster.main.id
  task_definition = aws_ecs_task_definition.clickhouse[0].arn
  desired_count   = 1

  # Stateful single node: pin to on-demand and never run two at once.
  capacity_provider_strategy {
    capacity_provider = "FARGATE"
    weight            = 1
  }

  deployment_minimum_healthy_percent = 0
  deployment_maximum_percent         = 100

  network_configuration {
    subnets          = aws_subnet.public[*].id
    security_groups  = [aws_security_group.clickhouse[0].id]
    assign_public_ip = true
  }

  service_registries {
    registry_arn = aws_service_discovery_service.clickhouse[0].arn
  }
}
