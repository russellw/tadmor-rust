# Stack

**Status:** adopted 2026-10-05.

## Decision

- **Back end:** Axum 0.8 on Tokio, serving both the JSON API required by
  tadmor's `spec/api.md` and the user interface.
- **Data access:** SQLx 0.8 (Postgres driver only, no TLS), with its
  compile-time checked `query!` macros and its migration runner.
- **User interface:** server-rendered Askama templates. No SPA, no npm, no
  WebAssembly, no JavaScript build step. If a small amount of client-side
  behaviour is needed, it is handwritten, or at most one vendored
  JavaScript file (such as htmx), which needs a conversation first.
- **Email:** lettre, sending over SMTP with STARTTLS through `native-tls`,
  which links the operating system's OpenSSL.
- **Passwords:** PBKDF2-HMAC-SHA256 at 600,000 iterations, as tadmor does,
  through RustCrypto's `pbkdf2` and `sha2`.
- **JSON:** serde and serde_json.
- **Tests:** `cargo test`, the test harness built into the toolchain.
- **Database:** Postgres 17 with the shared schema from tadmor's
  `db/migrations/`.
- **Toolchain:** Rust and Cargo from the operating system's packages
  (Ubuntu 26.04 ships `rustc` and `cargo` 1.93.1; the OS is out of scope
  for the metrics). Building `openssl-sys` needs `pkg-config` and
  `libssl-dev`.

## How Rust differs from the other counterparts

In Go, .NET and Python, the mainstream choice turned out to cost almost
nothing, because the standard library or the framework already covered
nearly everything. Rust's standard library has no HTTP server, TLS,
cryptography, JSON, async runtime or SMTP client. Every credible Rust web
stack is therefore a tree of crates.io packages, and the choices below
decide which trees to accept.

The project's owner decided that, as for PHP, Java and Ruby, this
counterpart measures **mainstream Rust**. Where the mainstream crate and
the lean one differ, mainstream wins, and the cost is recorded here.

Mainstream was judged by crates.io downloads over the 90 days to
2026-10-05:

| Slot | Chosen | Downloads | Alternatives |
| ---- | ------ | --------: | ------------ |
| Web framework | axum | 126.7 M | actix-web 11.3 M, rocket 2.2 M, poem 0.8 M |
| Postgres | sqlx | 42.5 M | tokio-postgres 19.2 M, diesel 8.4 M, sea-orm 4.5 M |
| Templates | askama | 13.3 M | minijinja 11.9 M, tera 6.3 M, maud 2.4 M |
| Email | lettre | 7.0 M | none comparable |

tokio-postgres's figure includes everything that uses it underneath,
deadpool-postgres among them, so sqlx's lead in direct use is wider than
the table shows. Askama and minijinja are close. Askama was taken because
its templates are checked at compile time, which is how Rust code usually
renders HTML alongside Axum.

## Measured trees

Resolved on 2026-10-05 with Cargo 1.93.1 on linux/x64, by `cargo tree -e
normal,build --target x86_64-unknown-linux-gnu`. Identities are the owners
crates.io lists for each crate (`/api/v1/crates/<name>/owners`): either
individual accounts or GitHub teams (`github:tokio-rs:core`). A team hides
its members, as a PyPI organization does, so a team counts once, and the
figures are low in that respect.

The comparison below was measured by hand while choosing, counting crates
by name. The committed manifest counts each version separately (`syn` 2
and 3, for example), as tadmor's metrics doc defines it. Through
`tools/measure.py` it gives, for the chosen stack as locked:

| | runtime | build | runtime+build |
| --- | ---: | ---: | ---: |
| Crates | 149 | 34 | **183** |
| Identities | 113 | 33 | **126** |

Build here means build scripts' dependencies, proc macros, and everything
only they use. The vendored source is 60.3 MB (1.33 M lines) for runtime
and 13.7 MB (0.34 M lines) for build.

| Option | Crates | Identities |
| ------ | -----: | ---------: |
| **Chosen** (axum, sqlx, askama, lettre, pbkdf2, serde_json) | **173** | **126** |
| The same, with our own SMTP client over `native-tls` | 165 | 118 |
| The same, with tokio-postgres and deadpool-postgres instead of sqlx | 116 | 88 |
| tadmor, runtime+build (for reference) | 179 | 179 |

Each direct dependency's marginal cost, meaning crates and identities that
no other direct dependency also brings in:

| Dependency | Its tree | Marginal |
| ---------- | -------- | -------- |
| sqlx | 121 crates, 89 identities | 73 crates, 50 identities |
| lettre (with native-tls) | 77 crates, 62 identities | 20 crates, 19 identities |
| axum | 54 crates, 39 identities | 19 crates, 10 identities |
| askama | 18 crates, 12 identities | 8 crates, 4 identities |
| tokio, serde, serde_json, sha2, pbkdf2 | | 1 crate, 0 identities |

- **Tokio is unavoidable.** Every Postgres driver in the table except
  diesel (which links libpq and is synchronous) runs on Tokio, and the
  synchronous `postgres` crate is a wrapper around `tokio-postgres`. Axum
  comes from the same organization and sits on hyper and tower, so it is
  cheap once Tokio is present.
- **SQLx is the expensive choice.** Through `url` and `idna`, it brings in
  ICU4X's Unicode crates (18 crates under the `icu4x-release` team), and it
  has its own copies of things tokio-postgres would share. tokio-postgres
  with deadpool would save 49 crates and 30 identities. That was the
  lean alternative, and it was passed over in favour of mainstream.
- **lettre costs 19 identities for one feature,** a PDF attached to a
  short note. tadmor-ruby wrote this itself in about 40 lines rather than
  take Action Mailer. Here lettre was taken under the mainstream rule. It
  is the first thing to revisit if the comparison ever favours lean.
- **Passwords add nothing.** SQLx already depends on RustCrypto's `sha2`
  and `hmac` for SCRAM authentication, so `pbkdf2` adds one crate from the
  same team.
- **No decimal crate.** As in tadmor, all money arithmetic happens in SQL.
  Amounts cross the boundary as text, cast with `::numeric` on the way in
  and `::text` on the way out, so they never touch binary floating point.

## Code that runs at build time

Rust runs third-party code at build time, and unlike npm's
`ignore-scripts` it cannot be turned off: **build scripts** (`build.rs`)
and **procedural macros** are compiled and executed, unsandboxed, on every
developer and CI machine. The chosen tree has 18 crates with build scripts
(including `openssl-sys`, which probes the system for OpenSSL) and 13
proc-macro crates (`serde_derive`, `tokio-macros`, `askama_macros`,
`sqlx-macros`, `tracing-attributes`, `thiserror-impl`, ICU4X's
`*-derive` crates, and others). In tadmor's terms this is install-time
code execution that is **required**. Vendoring makes it reviewable, but
it still runs.

`sqlx-macros` checks every `query!` against a live database at compile
time, or, with `SQLX_OFFLINE=true`, against query metadata committed under
`.sqlx/`. `.cargo/config.toml` sets `SQLX_OFFLINE=true`, so the build
always uses the committed metadata and needs no database and no network.
`tools/sqlx-prepare.sh` (`make sqlx-prepare`) regenerates it. It rebuilds
a scratch database from `db/migrations/` with `psql`, then runs `cargo
check` with `SQLX_OFFLINE_DIR` set, which makes the macros write one file
per query. That is all `sqlx-cli`'s `cargo sqlx prepare` does, so
`sqlx-cli` is not adopted: it would add a large build-time tree for
nothing.

## Permitted packages

The crates above, with exactly the features listed in `Cargo.toml` and
the transitive tree they require as locked in `Cargo.lock`, and nothing
else without a conversation first. In particular:

- **No `axum-extra`, `tower-http`, or `tower-sessions`.** Cookie parsing,
  sessions over the shared `sessions` table, and the one stylesheet are
  our own code.
- **No `tracing-subscriber`, `anyhow`, `dotenvy`, `chrono`, or `uuid`.**
  Logging goes to stderr, configuration comes from the environment through
  `std::env`, and dates cross the boundary as ISO text.
- **No SQLx default features.** In particular no `tls-*`: the database is
  local, and the only TLS is lettre's, to the mail relay.
- **No `sqlx-cli`** (above).
- **No PDF, CSV, or compression crates.** Printed documents come from a
  small PDF writer of our own (src/pdf.rs, as tadmor's Go one), using the
  standard-14 Helvetica fonts, so nothing is embedded; its content
  streams are uncompressed, since the standard library has no deflate.
  Bank statement CSV is read by a small RFC 4180 reader in
  src/banking.rs.
- **No `rand`, `getrandom`, `base64`, or `hex` as direct dependencies.**
  Salts and session tokens are read from `/dev/urandom`, and stored and
  sent in hex, with a few lines of our own code.
- **No test frameworks or HTTP test clients.** `cargo test` drives the
  router in-process through tower's `ServiceExt::oneshot`, from a crate
  already in the tree.

## Supply-chain posture

- **Vendored and committed.** `cargo vendor` puts every crate's source into
  `vendor/`, unmodified, and `.cargo/config.toml` replaces crates.io with
  it, with the network off. A clean clone builds with the OS toolchain
  alone. **Level 4 of tadmor's ladder, measured on 2026-10-05:** a fresh
  clone, built in an `ubuntu:26.04` container with `--network=none`, the
  host's `/usr` and `/etc/alternatives` mounted read-only, and an empty
  `CARGO_HOME`, built `--release --locked` twice into separate target
  directories, producing byte-identical 1.7 MB binaries.
- **Other platforms' crates are stubs.** `Cargo.lock` covers every
  platform, and Cargo reads every locked crate's manifest even when it
  will not build it. The 51 crates that only other platforms use
  (`windows-sys` and the like) are vendored as their `Cargo.toml` and an
  empty library file, so their code is neither committed nor trusted.
- **Pinned.** `Cargo.lock` records each crate's exact version and sha256,
  and Cargo checks every vendored file against the checksums `cargo vendor`
  recorded beside it.
- **Cooldown.** No crate version published less than 7 days ago is locked.
  On 2026-10-05 the resolver chose five that were (`tokio` 1.53.2, `mio`
  1.2.4, `cc` 1.6.0, `libc` 0.2.190, `yoke-derive` 0.8.4), so these are held
  back with `cargo update --precise` until they age.
- **Install-time code execution:** required (build scripts and proc
  macros, above). There is no switch to block it.
- **Tooling.** `tools/vendor.py` (standard library only) is the whole
  toolchain beyond Cargo. `sync` resolves online in a scratch copy, applies
  the cooldown, vendors, and writes `dependencies.json` from `cargo tree`
  and the crates.io owners API, in the format of tadmor's
  `docs/counterpart-metrics.md`. `check` verifies `vendor/` offline against
  `Cargo.lock` and the manifest, and fails if an ignore rule would hide a
  vendored file from git.

| Crate | Version | Published | Role |
| ----- | ------- | --------- | ---- |
| axum | 0.8.9 | 2026-04-14 | HTTP routing, extractors, form and JSON bodies |
| tokio | 1.53.1 | 2026-07-20 | async runtime |
| sqlx | 0.8.6 | 2025-05-19 | Postgres driver, pool, migrations, checked queries |
| askama | 0.16.1 | 2026-09-04 | compile-time HTML templates |
| serde, serde_json | 1.x | | JSON |
| lettre | 0.11.23 | 2026-08-03 | SMTP with STARTTLS |
| pbkdf2, sha2 | 0.13.0, 0.11.0 | | password hashing |

## Migrations

`sqlx::migrate!` embeds `db/migrations/` at compile time and applies it on
startup. The shared files follow golang-migrate's
`<version>_<name>.up.sql`/`.down.sql` naming, which SQLx reads as
reversible migrations. SQLx records them in its own `_sqlx_migrations`
table, which `spec/README.md` allows as a framework's bookkeeping.
Migrations of our own, if we ever need any, go in a separate directory and
are applied after the shared ones.

## Revisit when

- **Ubuntu ships Rust 1.94 or later.** SQLx 0.9.0 (2026-05-21) requires
  it. On 0.9 the tree is 4 crates and 4 identities smaller.
- **The comparison asks for lean Rust as well.** That would be a second
  counterpart, `tadmor-rust-lean`, with tokio-postgres, deadpool, and our
  own SMTP client. On today's figures it would be 116 crates and 88
  identities.
- **SQLx drops `url`/`idna`, or `idna` drops ICU4X.** Either would remove
  about 20 crates from the tree.
