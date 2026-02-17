.PHONY: help build test run-bot run-agent-echo run-agent-llm docker-build docker-up docker-down clean

help: ## Show this help message
	@echo 'Usage: make [target]'
	@echo ''
	@echo 'Available targets:'
	@awk 'BEGIN {FS = ":.*?## "} /^[a-zA-Z_-]+:.*?## / {printf "  %-20s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

# ==============================================================================
# Build & Test
# ==============================================================================

build: ## Build all packages in release mode
	cd rsworkspace && cargo build --release

build-dev: ## Build all packages in dev mode
	cd rsworkspace && cargo build

test: ## Run all tests
	cd rsworkspace && cargo test

test-bot: ## Run telegram-bot tests
	cd rsworkspace && cargo test --package telegram-bot

test-nats: ## Run telegram-nats tests
	cd rsworkspace && cargo test --package telegram-nats

# ==============================================================================
# Run Locally
# ==============================================================================

run-bot: ## Run telegram-bot locally
	@echo "Starting Telegram Bot..."
	@cd rsworkspace && cargo run --release --package telegram-bot

run-agent-echo: ## Run telegram-agent in echo mode
	@echo "Starting Agent (Echo Mode)..."
	@cd rsworkspace && ENABLE_LLM=false cargo run --release --package telegram-agent

run-agent-llm: ## Run telegram-agent in LLM mode
	@echo "Starting Agent (LLM Mode)..."
	@cd rsworkspace && ENABLE_LLM=true cargo run --release --package telegram-agent

run-nats: ## Run NATS server locally
	@echo "Starting NATS Server..."
	@nats-server -js -m 8222

# ==============================================================================
# Docker
# ==============================================================================

docker-build: ## Build Docker images
	@echo "Building Docker images..."
	docker-compose build

docker-up-echo: ## Start services with echo agent
	@echo "Starting services (Echo Mode)..."
	docker-compose --profile echo up -d

docker-up-llm: ## Start services with LLM agent
	@echo "Starting services (LLM Mode)..."
	docker-compose --profile llm up -d

docker-up-monitoring: ## Start services with monitoring
	@echo "Starting services with monitoring..."
	docker-compose --profile llm --profile monitoring up -d

docker-down: ## Stop all services
	@echo "Stopping services..."
	docker-compose down

docker-logs: ## Show logs from all services
	docker-compose logs -f

docker-logs-bot: ## Show logs from telegram-bot
	docker-compose logs -f telegram-bot

docker-logs-agent: ## Show logs from telegram-agent
	docker-compose logs -f telegram-agent-llm

# ==============================================================================
# Development
# ==============================================================================

dev-setup: ## Setup development environment
	@echo "Setting up development environment..."
	@cp .env.example .env || true
	@echo "✅ .env file created (edit with your tokens)"
	@cp config/telegram-bot.example.toml config/telegram-bot.toml || true
	@echo "✅ config/telegram-bot.toml created"
	@echo ""
	@echo "Next steps:"
	@echo "  1. Edit .env and add your TELEGRAM_BOT_TOKEN"
	@echo "  2. Edit .env and add your ANTHROPIC_API_KEY (for LLM mode)"
	@echo "  3. Run 'make run-nats' in one terminal"
	@echo "  4. Run 'make run-bot' in another terminal"
	@echo "  5. Run 'make run-agent-echo' or 'make run-agent-llm' in another terminal"

format: ## Format code with rustfmt
	cd rsworkspace && cargo fmt

clippy: ## Run clippy linter
	cd rsworkspace && cargo clippy --all-targets --all-features

check: ## Check if code compiles
	cd rsworkspace && cargo check --all-targets

# ==============================================================================
# Examples & Demos
# ==============================================================================

demo-streaming: ## Run streaming demo
	cd rsworkspace && cargo run --package telegram-bot --example demo_streaming

test-streaming: ## Run streaming integration test
	cd rsworkspace && cargo run --package telegram-bot --example test_streaming

# ==============================================================================
# Cleanup
# ==============================================================================

clean: ## Clean build artifacts
	@echo "Cleaning build artifacts..."
	cd rsworkspace && cargo clean
	@echo "✅ Clean complete"

clean-docker: ## Remove Docker volumes
	@echo "Removing Docker volumes..."
	docker-compose down -v
	@echo "✅ Docker cleanup complete"

clean-all: clean clean-docker ## Clean everything

# ==============================================================================
# Git
# ==============================================================================

git-status: ## Show git status
	git status

commit: ## Create a commit (usage: make commit MSG="your message")
	@if [ -z "$(MSG)" ]; then \
		echo "Error: MSG is required. Usage: make commit MSG=\"your message\""; \
		exit 1; \
	fi
	git add -A
	git commit -m "$(MSG)\n\nCo-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"

push: ## Push to remote
	git push origin telegram

# ==============================================================================
# Info
# ==============================================================================

info: ## Show project information
	@echo "TrogonAI - Telegram Bot Platform"
	@echo "================================="
	@echo ""
	@echo "Components:"
	@echo "  - telegram-types: Shared data structures"
	@echo "  - telegram-nats:  NATS communication library"
	@echo "  - telegram-bot:   Telegram bot implementation"
	@echo "  - telegram-agent: AI agent with LLM integration"
	@echo ""
	@echo "Documentation:"
	@echo "  - README.md:              Project overview"
	@echo "  - TELEGRAM_SYSTEM.md:     Complete system documentation"
	@echo "  - NATS_ARCHITECTURE.md:   NATS architecture reference"
	@echo "  - STREAMING_GUIDE.md:     Streaming system guide"
	@echo "  - DEPLOYMENT.md:          Deployment guide"
	@echo ""
	@echo "Quick start:"
	@echo "  1. make dev-setup"
	@echo "  2. Edit .env with your tokens"
	@echo "  3. make run-nats"
	@echo "  4. make run-bot (in another terminal)"
	@echo "  5. make run-agent-echo (in another terminal)"
