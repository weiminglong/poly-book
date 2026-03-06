# Operations Guide

This document collects configuration, deployment, and infrastructure details that
are useful for operators but too heavy for the main README.

## Configuration

Runtime config is layered in this order:

1. `config/default.toml`
2. Environment variables with the `PB__` prefix
3. CLI flags

Current defaults:

```toml
[feed]
ws_url = "wss://ws-subscriptions-clob.polymarket.com/ws/market"
rest_url = "https://clob.polymarket.com"
gamma_url = "https://gamma-api.polymarket.com"
ping_interval_secs = 10
reconnect_base_delay_ms = 100
reconnect_max_delay_ms = 30000
rate_limit_requests = 1500
rate_limit_window_secs = 10

[storage]
parquet_base_path = "./data"
parquet_flush_interval_secs = 300
parquet_row_group_size = 65536
checkpoints_enabled = true
checkpoint_interval_secs = 60
clickhouse_url = "http://localhost:8123"
clickhouse_database = "poly_book"
clickhouse_batch_interval_secs = 1
clickhouse_batch_size = 10000

[metrics]
listen_addr = "0.0.0.0:9090"
endpoint = "/metrics"

[logging]
level = "info"
format = "pretty"
```

Example overrides:

```bash
PB__STORAGE__PARQUET_BASE_PATH=/tmp/poly-book-data \
PB__LOGGING__LEVEL=debug \
cargo run -- auto-ingest
```

## Data Layout

Parquet data is partitioned by dataset and time:

```text
data/<dataset>/<year>/<month>/<day>/<hour>/*.parquet
```

Primary datasets:

- `book_events`
- `trade_events`
- `ingest_events`
- `book_checkpoints`
- `replay_validations`
- `execution_events`

## CI

GitHub Actions runs the following checks on pushes and pull requests to `main`:

- `cargo check --all-targets`
- `cargo test`
- `cargo clippy --all-targets -- -D warnings`
- `cargo fmt --all -- --check`

## Deployment

Merges to `main` trigger the deploy workflow after CI passes.

Deployment flow:

1. Build the Docker image
2. Push the image to Amazon ECR
3. Register a new ECS task definition
4. Update the ECS service
5. Wait for service stability

The workflow uses GitHub OIDC and an AWS IAM role stored in the
`AWS_DEPLOY_ROLE_ARN` repository secret.

## Infrastructure

Terraform in `infra/` provisions the AWS resources used by the current
deployment target:

- ECR for image storage
- ECS Fargate Spot for compute
- S3 for Parquet storage
- VPC, subnets, IAM, and CloudWatch resources

Bootstrap:

```bash
cd infra
cp terraform.tfvars.example terraform.tfvars
terraform init
terraform apply
```

Then:

1. Set `github_org` in `terraform.tfvars`
2. Copy the `github_actions_role_arn` output into the GitHub secret
   `AWS_DEPLOY_ROLE_ARN`

## Cost Control

Set `desired_count = 0` in `infra/terraform.tfvars` and re-apply Terraform to
stop running tasks while preserving the deployed resources.

## Local Inspection

Useful helper commands:

```bash
just parquet-ls
just parquet-count
just parquet-peek
just parquet-schema
just parquet-stats
```
