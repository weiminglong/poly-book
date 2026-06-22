# Application stdout/stderr logs are retention-bounded operational telemetry, not
# sensitive data; the CloudWatch default encryption is sufficient and a CMK adds
# key-policy/cost overhead disproportionate to the risk. (Secrets are never
# logged — they are read from mounted files, see operations.md.)
#tfsec:ignore:AVD-AWS-0017
resource "aws_cloudwatch_log_group" "app" {
  name              = "/ecs/${var.project_name}"
  retention_in_days = var.log_retention_days
}
