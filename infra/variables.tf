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
