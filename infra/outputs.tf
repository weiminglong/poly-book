output "ecr_repository_url" {
  description = "ECR repository URL for Docker images"
  value       = aws_ecr_repository.app.repository_url
}

output "ecs_cluster_name" {
  description = "ECS cluster name"
  value       = aws_ecs_cluster.main.name
}

output "ecs_service_name" {
  description = "ECS service name"
  value       = aws_ecs_service.app.name
}

output "s3_bucket_name" {
  description = "S3 bucket for Parquet storage"
  value       = aws_s3_bucket.data.id
}

output "github_actions_role_arn" {
  description = "IAM role ARN for GitHub Actions OIDC (add as GitHub secret AWS_DEPLOY_ROLE_ARN)"
  value       = aws_iam_role.github_actions.arn
}
