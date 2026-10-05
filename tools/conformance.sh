#!/usr/bin/env bash
# Run tadmor's conformance suite (conformance/, exported from tadmor) against
# this implementation, from scratch: wipe a dedicated database, bootstrap one
# administrator with a random password, start the server with email
# disabled, run the suite, and always stop the server again. Extra arguments
# go to the suite (e.g. -v, or -run 'auth').
#
#   DATABASE_URL  must name a database ending in _conformance (default:
#                 postgres://tadmor:tadmor@127.0.0.1:5432/tadmor_rust_conformance)
#   HTTP_ADDR     where the server listens (default 127.0.0.1:8095)
#   SERVER        the server executable (default: the release build)
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

export DATABASE_URL="${DATABASE_URL:-postgres://tadmor:tadmor@127.0.0.1:5432/tadmor_rust_conformance?sslmode=disable}"
HTTP_ADDR="${HTTP_ADDR:-127.0.0.1:8095}"
SERVER="${SERVER:-$repo_root/target/release/tadmor}"
mkdir -p target

db_name="${DATABASE_URL##*/}"
db_name="${db_name%%\?*}"
if [[ "$db_name" != *_conformance ]]; then
	echo "refusing to wipe $db_name: the conformance database's name must end in _conformance" >&2
	exit 1
fi

echo "==> Wiping $db_name"
PGOPTIONS="-c client_min_messages=warning" psql "$DATABASE_URL" -qX -v ON_ERROR_STOP=1 \
	-c 'DROP SCHEMA public CASCADE; CREATE SCHEMA public' >/dev/null

email="admin@conformance.test"
password="$(head -c 18 /dev/urandom | base64 | tr -d '/+=')"
echo "==> Bootstrapping administrator (migrates the empty database)"
printf '%s\n' "$password" | "$SERVER" adduser --email="$email" --name='Conformance Admin' >/dev/null

echo "==> Starting server on $HTTP_ADDR (email disabled)"
env -u SMTP_ADDR -u SMTP_USER -u SMTP_PASS -u MAIL_FROM HTTP_ADDR="$HTTP_ADDR" \
	"$SERVER" >"$repo_root/target/conformance-server.log" 2>&1 &
server_pid=$!
trap 'kill "$server_pid" 2>/dev/null || true; wait "$server_pid" 2>/dev/null || true' EXIT

for _ in $(seq 1 60); do
	if ! kill -0 "$server_pid" 2>/dev/null; then
		echo "server exited before becoming ready; see target/conformance-server.log" >&2
		exit 1
	fi
	if curl -sf "http://$HTTP_ADDR/readyz" >/dev/null; then
		break
	fi
	sleep 0.5
done

echo "==> Running the conformance suite"
(cd conformance && go run . -base-url "http://$HTTP_ADDR" -email "$email" -password "$password" "$@")
