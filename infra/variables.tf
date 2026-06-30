variable "aws_region" {
  description = "AWS region"
  type        = string
  default     = "us-east-1"
}

variable "project_name" {
  description = "Prefix for all resources"
  type        = string
  default     = "poly-book"
}

variable "github_org" {
  description = "GitHub username or org for OIDC trust"
  type        = string
}

variable "github_repo" {
  description = "GitHub repo name for OIDC trust"
  type        = string
  default     = "poly-book"
}

variable "task_cpu" {
  description = "Fargate task CPU units (256 = 0.25 vCPU)"
  type        = number
  default     = 256
}

variable "task_memory" {
  description = "Fargate task memory in MiB"
  type        = number
  default     = 512
}

variable "desired_count" {
  description = "Number of ECS tasks (set to 0 to stop everything)"
  type        = number
  default     = 1
}

variable "log_retention_days" {
  description = "CloudWatch log retention in days"
  type        = number
  default     = 7
}

variable "app_env_vars" {
  description = "Extra environment variables for the app container (e.g. PB__FEED__WS_URL)"
  type        = map(string)
  default     = {}
}

variable "ingest_on_demand_base" {
  description = "Number of ingest tasks pinned to on-demand FARGATE (immune to Spot reclaim); Spot is used only above this base"
  type        = number
  default     = 1
}

variable "serve_desired_count" {
  description = "Number of read-only `serve` API tasks (0 disables the serve service)"
  type        = number
  default     = 1
}

variable "serve_tokens" {
  description = "Comma-separated token IDs (or slugs) the serve API follows. Must be set for serve to start."
  type        = string
  default     = ""
}

variable "serve_api_auth_token_secret_arn" {
  description = "Secrets Manager or SSM parameter ARN injected as PB__API__AUTH_TOKEN for the externally bound serve API."
  type        = string
  default     = ""
}

variable "enable_clickhouse_service" {
  description = "Provision a single-node ClickHouse service on ECS+EFS. For production prefer managed ClickHouse and point PB__STORAGE__CLICKHOUSE_URL at it."
  type        = bool
  default     = false
}

variable "clickhouse_user" {
  description = "Application ClickHouse username for the optional ECS ClickHouse service."
  type        = string
  default     = "poly_book"
}

variable "clickhouse_password_secret_arn" {
  description = "Secrets Manager or SSM parameter ARN injected as CLICKHOUSE_PASSWORD for the optional ECS ClickHouse service."
  type        = string
  default     = ""
}

variable "clickhouse_app_url_secret_arn" {
  description = "Secrets Manager or SSM parameter ARN injected as PB__STORAGE__CLICKHOUSE_URL for app/serve tasks when using the optional ECS ClickHouse service. The secret value should include credentials, e.g. http://poly_book:<password>@clickhouse.poly-book.internal:8123."
  type        = string
  default     = ""
}

variable "clickhouse_cpu" {
  description = "ClickHouse task CPU units"
  type        = number
  default     = 1024
}

variable "clickhouse_memory" {
  description = "ClickHouse task memory in MiB"
  type        = number
  default     = 4096
}
