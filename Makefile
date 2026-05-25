COMPOSE_FILE := devops/docker/compose/compose.a2a.smoke.yml

.PHONY: smoke smoke-down smoke-build

smoke-build:
	docker compose -f $(COMPOSE_FILE) build

smoke:
	docker compose -f $(COMPOSE_FILE) up -d --build
	docker compose -f $(COMPOSE_FILE) --profile smoke run --rm a2a-smoke-test
	docker compose -f $(COMPOSE_FILE) down

smoke-down:
	docker compose -f $(COMPOSE_FILE) --profile smoke down -v
