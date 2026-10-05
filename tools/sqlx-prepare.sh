#!/usr/bin/env bash
# Regenerate .sqlx/, the query metadata the build checks every sqlx::query!
# against (docs/stack.md, "Code that runs at build time").
#
#   tools/sqlx-prepare.sh [database-url]
#
# The database (default tadmor_rust_prepare) is a scratch one: its public
# schema is dropped and rebuilt from db/migrations with psql, so the
# metadata describes exactly the shared schema. The macros then write one
# file per query while compiling against it, because SQLX_OFFLINE_DIR is
# set; this is all that sqlx-cli's `cargo sqlx prepare` does.
set -euo pipefail

export PGOPTIONS="-c client_min_messages=warning"
url="${1:-postgres://tadmor:tadmor@127.0.0.1:5432/tadmor_rust_prepare?sslmode=disable}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

psql "$url" -qX -v ON_ERROR_STOP=1 -c 'DROP SCHEMA public CASCADE; CREATE SCHEMA public' >/dev/null
for f in db/migrations/*.up.sql; do
	psql "$url" -qX -v ON_ERROR_STOP=1 --single-transaction -f "$f" >/dev/null
done

rm -rf .sqlx
mkdir .sqlx
cargo clean -q -p tadmor
SQLX_OFFLINE=false SQLX_OFFLINE_DIR="$root/.sqlx" DATABASE_URL="$url" cargo check -q --all-targets
echo "wrote $(ls .sqlx | wc -l) query files into .sqlx/"
