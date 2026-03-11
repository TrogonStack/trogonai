#!/usr/bin/env bash
set -e
BASE="$(dirname "$0")/target/debug"

# proxy — redirect everything to mock server on 9999
NATS_URL="nats://127.0.0.1:4222" PROXY_PREFIX="trogon" PROXY_PORT="8080" \
  PROXY_BASE_URL_OVERRIDE="http://localhost:9999" RUST_LOG="info" \
  "$BASE/proxy" > /tmp/proxy.log 2>&1 &

# worker — MemoryVault with seeded tokens (no VAULT_ADDR so it stays in-memory)
NATS_URL="nats://127.0.0.1:4222" PROXY_PREFIX="trogon" \
  WORKER_CONSUMER_NAME="proxy-workers" \
  VAULT_TOKEN_tok_mock_prod_test01="real-secret-key-abc" \
  VAULT_TOKEN_tok_anthropic_prod_test01="sk-ant-test-key" \
  VAULT_TOKEN_tok_github_prod_test01="ghp-test-key" \
  VAULT_TOKEN_tok_linear_prod_test01="lin-test-key" \
  RUST_LOG="info" \
  "$BASE/worker" > /tmp/worker.log 2>&1 &

# agent — use tok_ tokens so the proxy can detokenize them
NATS_URL="nats://127.0.0.1:4222" PROXY_URL="http://localhost:8080" \
  ANTHROPIC_TOKEN="tok_anthropic_prod_test01" \
  GITHUB_TOKEN="tok_github_prod_test01" \
  LINEAR_TOKEN="tok_linear_prod_test01" \
  SLACK_TOKEN="tok_linear_prod_test01" \
  TENANT_ID="default" AUTOMATIONS_API_PORT="8090" RUST_LOG="info" \
  "$BASE/agent" > /tmp/agent.log 2>&1 &

# webhook receivers
NATS_URL="nats://127.0.0.1:4222" GITHUB_WEBHOOK_PORT="8081" RUST_LOG="info" \
  "$BASE/trogon-github" > /tmp/github.log 2>&1 &

NATS_URL="nats://127.0.0.1:4222" LINEAR_WEBHOOK_PORT="8082" RUST_LOG="info" \
  "$BASE/trogon-linear" > /tmp/linear.log 2>&1 &

NATS_URL="nats://127.0.0.1:4222" DATADOG_WEBHOOK_PORT="8083" RUST_LOG="info" \
  "$BASE/trogon-datadog" > /tmp/datadog.log 2>&1 &

echo "All services started"
