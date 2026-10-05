# tadmor specification

This directory specifies tadmor's observable behavior independently of
its technology stack. It exists so that counterpart implementations,
the same product built on different stacks to compare supply-chain
footprint, ergonomics, and performance, can be built from a shared
definition of "done" and checked against it mechanically.

tadmor itself (Go + Postgres + React) is the **reference implementation**.
Where this spec and tadmor disagree, that is a bug in one of them; decide
which and fix it.

## Documents

| File | What it pins down |
| ---- | ----------------- |
| [`api.md`](api.md) | The HTTP/JSON contract: conventions, authentication, every endpoint, request and response shapes, status codes. |
| [`domain.md`](domain.md) | The business rules behind the API: entities, lifecycles, money arithmetic, posting rules, multi-currency, orders, inventory, banking, year-end, reports. |
| [`../conformance/`](../conformance/) | A black-box test suite that drives any implementation over HTTP and checks it against the two documents above. |
| [`../db/migrations/`](../db/migrations/) | The shared Postgres schema and seed data, which every implementation uses (below). |

## What is contract and what is free

**Contract** (an implementation must match):

- The HTTP API in `api.md`: paths, methods, JSON field names and types,
  status codes, and the initial data a fresh instance starts with.
- The business rules in `domain.md`, to the extent they are observable
  through the API: amounts, statuses, journal entries, report figures,
  and which operations are refused.
- Exact decimal arithmetic. Money never passes through binary floating
  point anywhere in the pipeline.
- The shared schema (next section).

**Free** (whatever suits the stack):

- Code structure, package layout, error *message* wording, logging, the
  migration *runner*, the build system, and the deployment shape.

**Required, but not checked by the suite:**

- **A user interface of the counterpart's own.** The comparison is
  between whole products, and in tadmor the front end carries nearly all
  of the supply-chain surface, so a counterpart without one, or one that
  reuses tadmor's `web/`, would not be comparable. The counterpart builds
  its UI in its own stack and its own code: an SPA, server-rendered pages,
  or anything else. It must cover the screens and actions listed in
  `domain.md` §13. The JSON API stays mandatory alongside it, whatever the
  UI is, because it is the integration surface and the only thing the
  conformance suite can test across stacks.

**Out of scope for conformance** (described here only so a counterpart can
offer an equivalent product):

- PDF layout. The PDF endpoints must return a valid PDF with the specified
  headers. What goes on the page is described, but not checked byte for
  byte.
- Actual email delivery. The suite runs with email sending disabled (see
  `api.md` §5.11).
- Operational concerns: the deployment model, TLS termination, backups,
  and the out-of-band bootstrap of the first administrator.

## The shared schema

Every implementation runs on **Postgres 17 or later** with the schema in
`db/migrations/`, taken at the same commit as the spec. Much of the
domain lives there (generated line amounts, constraint triggers that
guard posting and settlement, the reporting views), so a counterpart
inherits those rules rather than reimplementing them, and the comparison
is between stacks rather than between database designs.

- **Apply every `*.up.sql` in lexical order**, each in its own
  transaction, recording which have run so none runs twice. Any runner
  will do: the files follow the golang-migrate naming, and tadmor's own
  runner records versions in a `schema_migrations` table. The `.down.sql`
  files are for rolling back by hand and are never needed in normal
  running. The seed data of `api.md` §4 is part of the migrations, so a
  fresh database with them applied, plus one administrator, is a fresh
  instance.
- **The role must be able to create the `citext` extension.** It is a
  trusted extension, so any role with `CREATE` on the database can.
- **Run every database session in UTC** (`SET TIME ZONE 'UTC'`, or the
  `timezone` connection parameter). The aging views and the default
  movement date use `current_date`, which follows the session's timezone,
  and "today" is the UTC date (`api.md` §1.2).
- **Business data lives in the shared tables**, read and written through
  them. A counterpart should use the views and generated columns rather
  than recompute what they provide.
- **Never alter the shared objects.** Do not edit, drop, or change
  any table, column, constraint, trigger, function, or view the
  migrations create. A counterpart may **add** objects of its own (for
  example a framework's bookkeeping tables, or its own session storage),
  in migrations kept outside `db/migrations/` and applied after the
  shared ones. Those count as its own code.
- **Interchangeability is not required.** A counterpart need not be able
  to serve a database that tadmor populated, or the reverse. Password
  hashes, for example, are not contract (`domain.md` §12).

## Dependency policy

The counterparts exist to compare supply-chain exposure, so each one
**minimizes its transitive dependency set the way tadmor does**: as few
third-party packages as it can, from as few distinct maintainers, across
everything that runs (runtime, build, and test). These are the primary
metrics of `docs/counterpart-metrics.md` in tadmor. What that means in
practice depends on the ecosystem, so the specific choices are made per
stack, in the light of the trade-offs that ecosystem offers. The
principles are fixed:

- **Standard library first.** Use what the language and its platform ship
  before reaching for a package. No framework is adopted by default; one
  is taken on when it does a job that could not otherwise be done in
  reasonable code, and its whole transitive tree is weighed, not just its
  name.
- **Count maintainers, not lines.** A large package from one vendor can be
  a better choice than a few small ones from many authors.
- **Measure, don't guess.** Before adopting or rejecting a package, list
  its actual transitive tree and the maintainers behind it, and compare
  the realistic alternatives, including writing the code.
- **Hand-written code wins only when it truly does the same job** in a
  modest amount of code. Where the need is certain and hand-rolling would
  be a stopgap (routing was tadmor's example), take the standard package
  from the start.
- **Build, test, and development tools count**, because they run with
  full access on developer and CI machines.
- **Pin, verify, and isolate what remains**, as far as the ecosystem
  allows: exact versions with integrity hashes, vendored source where
  that is practical, a hermetic build, install-time scripts blocked, and
  a cooldown before adopting newly published versions.
- **New dependencies need a conversation first**, as in tadmor.
- **Commit a dependency manifest**, `dependencies.json`, written by the
  counterpart's own tooling and kept current with its lockfile. It lists
  every third-party package, its category, and the publishing identities
  behind it, in the format of `docs/counterpart-metrics.md` in tadmor,
  whose `tools/measure.py` reads it. The tooling that knows a package
  manager lives with the project that uses it.
- **Record each decision** in the counterpart's own docs: what was
  needed, the alternatives considered with their measured trees, what was
  chosen, and what would make it worth revisiting. tadmor's
  `docs/frontend-stack.md` and `docs/new-project-conventions.md` are
  worked examples; their Go and npm specifics apply only where a
  counterpart uses those ecosystems.

## Conformance

The suite in `conformance/` is a single stdlib-only Go program. It runs
against **a freshly initialized instance**, meaning the schema and seed
data are present, exactly one administrator login exists, and there is
nothing else (see "The shared schema" above). Several rules involve global state (fiscal-year ordering,
non-overlapping periods, the frozen base currency), so a used database
gives meaningless results.

```sh
cd conformance && go run . -base-url http://127.0.0.1:8090 \
    -email admin@example.com -password 'the-password'
```

In tadmor, `make conformance` does the whole run: it creates a throwaway
database, starts the server, bootstraps the admin, runs the suite, and
tears everything down. A counterpart should provide its own equivalent
wrapper. See [`conformance/README.md`](../conformance/README.md).

## Counterparts and versioning

Each counterpart lives in **its own repository**, named
`tadmor-<platform>` after the language or runtime that distinguishes its
stack: `tadmor-python`, `tadmor-rust`, `tadmor-dotnet`. The name is the
platform, not the framework, because under the dependency policy the
framework is decided inside the project, and may be none. A second
counterpart on the same platform adds a qualifier for what separates it
(`tadmor-python-htmx`), never a number; the first keeps the bare name.
tadmor itself, the reference, keeps its name. Counterpart repositories sit
beside tadmor's checkout and on GitHub as `russellw/tadmor-<platform>`.

Each one carries a **copy** of `spec/`, `conformance/`, and the migration
files of `db/migrations/`, taken at a specific tadmor commit:

```sh
spec/export.sh ../tadmor-python     # from the tadmor repo
```

The export script replaces all three directories in the destination
wholesale, leaves out tadmor-only files (`conformance/run-local.sh`,
`db/migrations/embed.go`, the script itself), and writes `spec/UPSTREAM`
naming the source commit. It refuses to export uncommitted changes.

- **tadmor owns the spec and the schema.** The copies are never edited in
  place. A change to the spec, the suite, or the schema is made in tadmor, in the same commit as any
  behavior change, and re-exported. tadmor must keep passing the suite.
- **A counterpart targets one spec commit.** It records which commit in
  `spec/UPSTREAM` and upgrades by re-exporting, deliberately, when it is
  ready to catch up. Comparisons between implementations should name the
  spec commit each one passes.
- **The suite is self-contained.** `conformance/` is its own Go module with
  no dependencies, so `cd conformance && go run . ...` works in any
  repository with a Go toolchain, whatever stack the counterpart uses.
