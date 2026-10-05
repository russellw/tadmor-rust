# tadmor-rust developer tasks.
#
# Cargo builds only from the committed vendor/ (.cargo/config.toml), with
# the network off, and checks queries against the committed .sqlx/ rather
# than a database. No target touches the network except vendor-sync.

# Connection strings. Override on the command line, e.g.
#   make run DATABASE_URL=postgres://user:pass@host:5432/db
PG ?= postgres://tadmor:tadmor@127.0.0.1:5432
DATABASE_URL ?= $(PG)/tadmor_rust?sslmode=disable
TEST_DATABASE_URL ?= $(PG)/tadmor_rust_test?sslmode=disable
PREPARE_DATABASE_URL ?= $(PG)/tadmor_rust_prepare?sslmode=disable
HTTP_ADDR ?= 127.0.0.1:8080

.DEFAULT_GOAL := help
.PHONY: help build release run test check sqlx-prepare vendor-check vendor-sync db clean

help: ## List available targets
	@grep -E '^[a-zA-Z_-]+:.*## ' $(MAKEFILE_LIST) | \
		awk 'BEGIN{FS=":.*## "}{printf "  make %-13s %s\n", $$1, $$2}'

build: ## Build the server (debug)
	cargo build --locked

release: ## Build the server (release) into target/release/tadmor
	cargo build --locked --release

run: build ## Build and run the server (migrates on start)
	DATABASE_URL='$(DATABASE_URL)' HTTP_ADDR=$(HTTP_ADDR) target/debug/tadmor

test: ## Run the tests (integration tests wipe TEST_DATABASE_URL)
	TEST_DATABASE_URL='$(TEST_DATABASE_URL)' cargo test --locked

check: vendor-check ## Verify vendor/ and that .sqlx/ covers every query (offline)
	cargo check --locked --all-targets

sqlx-prepare: ## Regenerate .sqlx/ against a scratch database (wipes PREPARE_DATABASE_URL)
	tools/sqlx-prepare.sh '$(PREPARE_DATABASE_URL)'

vendor-check: ## Verify vendor/, Cargo.lock, and dependencies.json (offline)
	tools/vendor.py check

vendor-sync: ## Re-resolve from crates.io, apply the cooldown, re-vendor (network)
	tools/vendor.py sync

db: ## Create the dev, test, and prepare databases on the Postgres at PG
	for db in tadmor_rust tadmor_rust_test tadmor_rust_prepare; do \
		psql '$(PG)/postgres?sslmode=disable' -qXc "CREATE DATABASE $$db" || true; \
	done

clean: ## Remove build output
	cargo clean
