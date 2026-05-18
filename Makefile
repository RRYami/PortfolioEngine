.PHONY: db-up db-down db-reset migrate prepare psql test

db-up:
	docker compose up -d --wait

db-down:
	docker compose down

db-reset:
	docker compose down -v
	docker compose up -d --wait

migrate:
	cargo sqlx migrate run

prepare:
	cargo sqlx prepare --workspace

psql:
	psql $$(cat .env | grep DATABASE_URL | cut -d '=' -f2-)

test:
	cargo test --workspace
