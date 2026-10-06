# tadmor-rust

The Rust counterpart of [tadmor](https://github.com/russellw/tadmor):
the same business management product, specified by tadmor's `spec/` and
checked by its `conformance/` suite, built on Axum, SQLx and server-rendered
Askama templates so the stacks can be compared. See
[`docs/stack.md`](docs/stack.md) for why this stack, and what it costs.

Status: complete against the spec at `spec/UPSTREAM`. The server
implements the whole JSON API, and all 35 conformance cases pass. The
user interface, server-rendered at the root path, covers every item of
the checklist in `spec/domain.md` §13; `docs/ui-coverage.md` records
where each is met and how it was checked.

## Layout

```
templates/         the UI's Askama templates
static/            app.js (the line editor, the only script) and app.css
src/               the server: services (auth, banking, users, master, calendar,
                   currency, documents, inventory, orders, payments,
                   posting, printing, settlement, reporting, yearend)
                   holding the business
                   rules; pdf, a small PDF writer, and mailer (lettre); http/
                   (the JSON API's routes, extractors, session middleware);
                   ui/ (the screens);
                   error (the API's error type), db, config
tests/             integration tests, driving the router in-process
db/migrations/     tadmor's shared schema (a copy; see spec/UPSTREAM)
spec/              tadmor's stack-neutral specification (a copy)
conformance/       tadmor's black-box conformance suite (a copy)
.sqlx/             query metadata that sqlx::query! checks against at build time
vendor/            every third-party crate, committed (tools/vendor.py)
tools/             vendor.py (vendoring and dependencies.json), sqlx-prepare.sh,
                   conformance.sh
docs/              the stack decision, and the UI's coverage of the checklist
```

## Prerequisites

- **Rust 1.93 and Cargo** from Ubuntu's `rustc` and `cargo` packages, plus
  `pkg-config` and `libssl-dev` for `openssl-sys`.
- **Postgres 17** reachable at `PG` (default
  `postgres://tadmor:tadmor@127.0.0.1:5432`), with a role that can create
  the `citext` extension. `make db` creates the databases.
- `psql`, for `make sqlx-prepare`, `make conformance` and `make db`.
- Go, for `make conformance` only (the suite is a stdlib-only Go program).

## Configuration

| Env var | Required | Default | Purpose |
| ------- | -------- | ------- | ------- |
| `DATABASE_URL` | yes | | Postgres connection string |
| `HTTP_ADDR` | no | `:8080` | Listen address (`:port` means every interface) |
| `PORT` | no | | Listen port; overrides `HTTP_ADDR` when set |
| `SMTP_ADDR` | no | | SMTP server `host:port`; unset turns email off (the email endpoints answer 501) |
| `SMTP_USER`, `SMTP_PASS` | no | | SMTP credentials; unset sends without authentication |
| `MAIL_FROM` | with `SMTP_ADDR` | | The From address of outgoing mail |
| `TEST_DATABASE_URL` | for tests | | Database the integration tests reset and use |

## Build, run, test

Run `make` to list the targets. Every build is offline: Cargo reads crates
only from `vendor/` and checks queries only against `.sqlx/`.

```sh
make db             # create tadmor_rust, tadmor_rust_test, tadmor_rust_prepare
make run            # build and run on 127.0.0.1:8080 (migrates on start);
                    # the UI is at http://127.0.0.1:8080/
make test           # unit and integration tests
make check          # vendor/ and .sqlx/ are complete and consistent
make conformance    # tadmor's suite against a fresh release server
make conformance ARGS="-v -run '^(auth|users)/'"
echo 'the-password' | make adduser EMAIL=you@example.com NAME='Your Name'
```

The first administrator is created out of band with `tadmor adduser`
(`--admin=false` makes an ordinary login), which reads the password from
the first line of stdin and migrates the database first. The conformance
suite needs Go (`go run`), as it does for every implementation.

> The integration tests and `make sqlx-prepare` **drop and recreate the
> `public` schema** of their databases. Point them only at throwaway ones.

After adding or changing a `sqlx::query!`, run `make sqlx-prepare` and
commit `.sqlx/` with the code. To change dependencies, edit `Cargo.toml`,
run `make vendor-sync` (online), and commit `Cargo.toml`, `Cargo.lock`,
`vendor/`, and `dependencies.json` together.

## Specification

`spec/`, `conformance/` and `db/migrations/` are copies from tadmor at the
commit named in `spec/UPSTREAM`. They are never edited here. To catch up,
run `spec/export.sh ../tadmor-rust` from tadmor.
