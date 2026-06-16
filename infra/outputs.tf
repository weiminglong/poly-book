output "ecr_repository_url" {
  description = "ECR repository URL for Docker images"
  value       = aws_ecr_repository.app.repository_url
}

output "ecs_cluster_name" {
  description = "ECS cluster name"
  value       = aws_ecs_cluster.main.name
}

output "ecs_service_name" {
  description = "ECS ingest service name"
  value       = aws_ecs_service.app.name
}

output "serve_service_name" {
  description = "ECS serve (read-only API) service name"
  value       = aws_ecs_service.serve.name
}

output "wal_efs_id" {
  description = "EFS file system id backing the durable WAL"
  value       = aws_efs_file_system.wal.id
}

output "s3_bucket_name" {
  description = "S3 bucket for Parquet storage"
  value       = aws_s3_bucket.data.id
}

output "github_actions_role_arn" {
  description = "IAM role ARN for GitHub Actions OIDC (add as GitHub secret AWS_DEPLOY_ROLE_ARN)"
  value       = aws_iam_role.github_actions.arn
}
