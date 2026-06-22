data "aws_availability_zones" "available" {
  state = "available"
}

resource "aws_vpc" "main" {
  cidr_block           = "10.0.0.0/16"
  enable_dns_hostnames = true
  enable_dns_support   = true

  tags = { Name = var.project_name }
}

resource "aws_internet_gateway" "main" {
  vpc_id = aws_vpc.main.id

  tags = { Name = var.project_name }
}

resource "aws_subnet" "public" {
  count             = 2
  vpc_id            = aws_vpc.main.id
  cidr_block        = cidrsubnet(aws_vpc.main.cidr_block, 8, count.index)
  availability_zone = data.aws_availability_zones.available.names[count.index]

  # Public subnets with auto-assigned public IPs: the capture tasks need
  # outbound internet (Polymarket WS/REST, S3, CloudWatch) and this MVP topology
  # deliberately omits a NAT gateway to avoid its hourly + per-GB cost for a
  # single capture task. Direct inbound exposure is contained by the task
  # security group (only the metrics port, and only from inside the VPC) and the
  # app's loopback binding + optional bearer auth (P2-SEC-2). Revisit with private
  # subnets + NAT if the task count or inbound surface grows.
  #tfsec:ignore:aws-ec2-no-public-ip-subnet
  map_public_ip_on_launch = true

  tags = { Name = "${var.project_name}-public-${count.index}" }
}

resource "aws_route_table" "public" {
  vpc_id = aws_vpc.main.id

  route {
    cidr_block = "0.0.0.0/0"
    gateway_id = aws_internet_gateway.main.id
  }

  tags = { Name = "${var.project_name}-public" }
}

resource "aws_route_table_association" "public" {
  count          = 2
  subnet_id      = aws_subnet.public[count.index].id
  route_table_id = aws_route_table.public.id
}

resource "aws_security_group" "ecs" {
  name_prefix = "${var.project_name}-ecs-"
  description = "ECS task security group"
  vpc_id      = aws_vpc.main.id

  # Metrics endpoint — reachable only from inside the VPC (an in-VPC Prometheus /
  # managed scraper), never the public internet. Exposing /metrics to 0.0.0.0/0
  # leaks operational detail and adds attack surface; scrape externally through a
  # private path (VPC peering, PrivateLink, or a VPN) rather than widening this.
  ingress {
    from_port   = 9090
    to_port     = 9090
    protocol    = "tcp"
    cidr_blocks = [aws_vpc.main.cidr_block]
    description = "Prometheus metrics (in-VPC scrape only)"
  }

  # Outbound to the internet is REQUIRED: the capture task connects to the
  # Polymarket WS/REST endpoints, S3, and CloudWatch, all public AWS/venue
  # endpoints, and there is no NAT gateway (see the public-subnet note above).
  # The narrower risk (data exfiltration on a non-443 port) is accepted for a
  # read-only capture task; tighten to 443/53 here if the egress surface matters.
  #tfsec:ignore:aws-ec2-no-public-egress-sgr
  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
    description = "Outbound to Polymarket WS/REST, S3, and CloudWatch"
  }

  lifecycle {
    create_before_destroy = true
  }

  tags = { Name = "${var.project_name}-ecs" }
}

# VPC Flow Logs → CloudWatch, so rejected/accepted network flows are auditable
# for incident forensics (who talked to the capture task, exfil attempts).
# Flow logs are retention-bounded operational telemetry, not sensitive data; the
# CloudWatch default encryption is sufficient and a CMK adds overhead.
#tfsec:ignore:AVD-AWS-0017
resource "aws_cloudwatch_log_group" "flow_logs" {
  name              = "/vpc/${var.project_name}/flow-logs"
  retention_in_days = var.log_retention_days
}

resource "aws_iam_role" "flow_logs" {
  name_prefix = "${var.project_name}-flowlogs-"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "vpc-flow-logs.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })
}

# The `:*` is the log-STREAM ARN pattern within this one flow-log group — streams
# are created per-ENI at delivery time and cannot be enumerated in advance, so
# this is the minimal grant (scoped to a single group), not a global wildcard.
#tfsec:ignore:AVD-AWS-0057
resource "aws_iam_role_policy" "flow_logs" {
  name_prefix = "${var.project_name}-flowlogs-"
  role        = aws_iam_role.flow_logs.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Action = [
        "logs:CreateLogStream",
        "logs:PutLogEvents",
        "logs:DescribeLogStreams",
      ]
      Resource = "${aws_cloudwatch_log_group.flow_logs.arn}:*"
    }]
  })
}

resource "aws_flow_log" "main" {
  vpc_id          = aws_vpc.main.id
  traffic_type    = "ALL"
  iam_role_arn    = aws_iam_role.flow_logs.arn
  log_destination = aws_cloudwatch_log_group.flow_logs.arn

  tags = { Name = "${var.project_name}-flow-logs" }
}
