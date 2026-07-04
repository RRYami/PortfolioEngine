.PHONY: up down logs rebuild db-up db-down db-reset migrate prepare psql test

# Full stack: prices + api + frontend + postgres (dashboard on :3000).
up:
	docker compose up -d --build --wait

down:
	docker compose down

logs:
	docker compose logs -f

rebuild:
	docker compose build --no-cache

db-up:
	docker compose up -d --wait postgres

db-down:
	docker compose down

db-reset:
	docker compose down -v
	docker compose up -d --wait postgres

migrate:
	cargo sqlx migrate run

prepare:
	cargo sqlx prepare --workspace

psql:
	psql $$(cat .env | grep DATABASE_URL | cut -d '=' -f2-)

test:
	cargo test --workspace
