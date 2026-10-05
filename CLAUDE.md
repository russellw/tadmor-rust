The goal of this project is to develop comprehensive business management software.
It is the Rust counterpart of tadmor (~/tadmor): the same product, specified by
tadmor's spec/ and checked by its conformance/ suite, built on a different stack so
the two can be compared (see ~/tadmor/docs/counterpart-metrics.md).

Technology stack:
Postgres for the database, using the shared schema from tadmor's db/migrations.
Rust (Ubuntu's rustc and cargo packages) with Axum on Tokio for the back end, and SQLx
for Postgres. Server-rendered Askama templates for the user interface; no npm, no
WebAssembly, no JavaScript build. lettre with native-tls for email. cargo test for tests.
Python as necessary for ancillary scripts (standard library only).
See docs/stack.md for the decision and its rationale.

Schema design:
The schema is shared with tadmor and is not ours to redesign. sqlx::migrate! applies
db/migrations; never alter a shared table, view, trigger, or function. Objects of our
own, if ever needed, go in migrations kept outside db/migrations/, applied afterwards.

Dependencies:
Supply-chain conscious throughout; keep the third-party footprint small, pinned, and
reviewable in-repo. The only permitted crates are those in Cargo.toml, with the features
listed there, and the tree they lock, as described in docs/stack.md. New crates or
features need a conversation first. vendor/ holds every locked crate, committed;
.cargo/config.toml builds from it with the network off, and nothing is fetched from
crates.io at build or run time. Change dependencies only through tools/vendor.py sync
(which enforces the 7-day cooldown and writes dependencies.json), never by building
against crates.io; commit Cargo.toml, Cargo.lock, vendor/, and dependencies.json
together, after tools/vendor.py check passes.

Working on it:
Money arithmetic happens in SQL, where numeric is exact. Amounts cross into and out of
Rust as text (::numeric in, ::text out); never let an f32 or f64 near an amount.
Run every database session in UTC.
spec/, conformance/, and db/migrations/ are copies from tadmor (spec/UPSTREAM);
never edit them here. Re-export from tadmor with spec/export.sh.
Queries go through sqlx::query! and friends, checked at build time against .sqlx/.
After adding or changing one, run `make sqlx-prepare` and commit .sqlx/ with the code.
The exceptions are src/documents.rs, src/payments.rs and src/settlement.rs, where the
document kinds differ only in names: their SQL is built at run time from each kind's
description (`Kind`, `PaymentKind`, `Settler`). Table and column names there come only
from those descriptions, and every value is a bind parameter.
Postgres builds those read shapes with json_build_object, every decimal cast to text.
Before committing, run `make check` and `make test`; both must pass.

Version control:
Commit directly to the default branch. Do not create feature branches.
