# AES256 (the ECR default) is sufficient for container images: they carry no
# secrets and the deploy pulls by immutable digest, so layer confidentiality is
# not the threat model. A CMK would add key-policy/cost overhead with no
# corresponding risk reduction.
#tfsec:ignore:AVD-AWS-0033
resource "aws_ecr_repository" "app" {
  name = var.project_name
  # Immutable tags so a pushed tag can never be silently overwritten with a
  # different image (supply-chain integrity). The deploy already pulls by digest
  #, so immutability does not constrain rollouts.
  image_tag_mutability = "IMMUTABLE"
  force_delete         = true

  image_scanning_configuration {
    scan_on_push = true
  }
}

resource "aws_ecr_lifecycle_policy" "app" {
  repository = aws_ecr_repository.app.name

  policy = jsonencode({
    rules = [
      {
        rulePriority = 1
        description  = "Keep only 5 images"
        selection = {
          tagStatus   = "any"
          countType   = "imageCountMoreThan"
          countNumber = 5
        }
        action = {
          type = "expire"
        }
      }
    ]
  })
}
