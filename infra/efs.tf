# EFS for the durable write-ahead log (audit P1-INFRA-1 / P2-INFRA-1).
#
# AUTHORED BUT NOT YET APPLIED against a live AWS account: this passes
# `terraform validate`/`fmt` but has not been `terraform apply`ed or load-tested.
# Treat the apply + a restart/failover drill as the remaining verification.
#
# The WAL is the durability backbone and must survive task restarts and host
# loss. On ephemeral Fargate storage it is lost on every restart; mounting it on
# EFS makes it durable and shareable between the active `ingest` writer and a
# standby (the flock single-writer lease still guarantees one writer at a time).

resource "aws_security_group" "efs" {
  name_prefix = "${var.project_name}-efs-"
  description = "EFS mount target security group (NFS from ECS tasks only)"
  vpc_id      = aws_vpc.main.id

  ingress {
    from_port       = 2049
    to_port         = 2049
    protocol        = "tcp"
    security_groups = [aws_security_group.ecs.id]
    description     = "NFS from ECS tasks"
  }

  # EFS mount targets only answer NFS from in-VPC tasks; they never initiate
  # outbound traffic to the internet. Scope egress to the VPC CIDR instead of
  # 0.0.0.0/0 so a compromised mount-target ENI cannot exfiltrate.
  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = [aws_vpc.main.cidr_block]
    description = "In-VPC only (mount targets do not egress to the internet)"
  }

  lifecycle {
    create_before_destroy = true
  }

  tags = { Name = "${var.project_name}-efs" }
}

resource "aws_efs_file_system" "wal" {
  creation_token = "${var.project_name}-wal"
  encrypted      = true

  # The WAL is a short-lived tail; keep it on the cheaper IA tier after a week.
  lifecycle_policy {
    transition_to_ia = "AFTER_7_DAYS"
  }

  tags = { Name = "${var.project_name}-wal" }
}

# One mount target per AZ so the WAL is reachable from a task in either subnet
# (dual-AZ failover).
resource "aws_efs_mount_target" "wal" {
  count           = length(aws_subnet.public)
  file_system_id  = aws_efs_file_system.wal.id
  subnet_id       = aws_subnet.public[count.index].id
  security_groups = [aws_security_group.efs.id]
}

# Access point pins ownership/permissions so the non-root container user (uid
# 10001, see Dockerfile) can read/write the WAL directory.
resource "aws_efs_access_point" "wal" {
  file_system_id = aws_efs_file_system.wal.id

  posix_user {
    uid = 10001
    gid = 10001
  }

  root_directory {
    path = "/wal"
    creation_info {
      owner_uid   = 10001
      owner_gid   = 10001
      permissions = "0755"
    }
  }

  tags = { Name = "${var.project_name}-wal" }
}
