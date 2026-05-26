COMPOSE_SMOKE := devops/docker/compose/compose.a2a.smoke.yml
COMPOSE_FULL := devops/docker/compose/compose.a2a.full.yml

.PHONY: smoke smoke-down smoke-build smoke-full smoke-full-down

smoke-build:
	docker compose -f $(COMPOSE_SMOKE) build

smoke:
	docker compose -f $(COMPOSE_SMOKE) up -d --build
	docker compose -f $(COMPOSE_SMOKE) --profile smoke run --rm a2a-smoke-test; \
	rc=$$?; docker compose -f $(COMPOSE_SMOKE) down; exit $$rc

smoke-down:
	docker compose -f $(COMPOSE_SMOKE) --profile smoke down -v

smoke-full:
	docker compose -f $(COMPOSE_FULL) down -v 2>/dev/null || true
	docker compose -f $(COMPOSE_FULL) up -d --build --force-recreate
	docker compose -f $(COMPOSE_FULL) --profile smoke-full run --rm a2a-smoke-test; \
	rc=$$?; docker compose -f $(COMPOSE_FULL) down -v; exit $$rc

smoke-full-down:
	docker compose -f $(COMPOSE_FULL) --profile smoke-full down -v
