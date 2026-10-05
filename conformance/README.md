# Conformance suite

A black-box test of the HTTP API specified in [`../spec/`](../spec/). It
talks to a running server only over HTTP, so it can judge any
implementation, including counterparts on other stacks.

**Supply chain.** The suite is one Go program that imports only the
standard library, and `imports_test.go` enforces that. It is its own Go
module with no dependencies, so a counterpart repository can carry a copy
(exported by `spec/export.sh`) and run it unchanged. Running it needs a Go
toolchain and nothing else, which is no more trust than the reference
implementation already asks for.

## Running it against tadmor

With Postgres up:

```sh
make conformance                 # wipe tadmor_conformance, build, bootstrap, serve, test, tear down
make conformance ARGS=-v         # also list passing cases
make conformance ARGS='-run banking'
```

`run-local.sh` refuses to wipe any database whose name does not end in
`_conformance`. Server output goes to `bin/conformance-server.log`.

## Running it against another implementation

1. Start the implementation against a **fresh** database: the shared
   migrations applied (`spec/README.md`, "The shared schema"), which
   carry the seed data of `spec/api.md` §4, plus exactly one
   administrator login.
2. Disable outbound email, so the email endpoints answer 501.
3. Run the suite from the `conformance/` directory:

   ```sh
   cd conformance && go run . -base-url http://127.0.0.1:8091 \
       -email admin@example.com -password 'the-password' [-v] [-run regexp]
   ```

   A counterpart should wrap steps 1 to 3 in its own one-shot script, the
   equivalent of tadmor's `run-local.sh`.

The exit status is 0 only if every selected case passes. To run only some
cases, pass `-run` a regular expression over case names. Group prefixes
include `master/`, `orders/`, and `banking/`.

## How the suite is built

- **One fresh instance, many independent cases.** `initial-state` runs
  first and checks the seed data. Every later case creates its own
  accounts, parties, and fiscal year, so its report figures are checked
  per account and cannot be disturbed by other cases. Each case gets its
  own calendar year from 2101 onwards, because accounting periods may not
  overlap anywhere. The year-end case uses 2001–2003, because closing a
  year requires every earlier year to be closed. The aging case uses
  dates around today (UTC).
- **Exchange rates are global.** No case may create an AUD rate (one case
  relies on AUD having none), and JPY rates appear only in the
  foreign-receipt case, which first checks that it has none.
- **Ledger-wide checks are identities, not totals.** For example, the
  trial balance balances, the balance sheet satisfies assets = liabilities
  + equity + current earnings, and the cash flow ties to cash.
- **Decimals are compared by value.** `"9.9900"` equals `"9.99"`, but a
  JSON number where the spec says decimal string is a failure.
- **Journal entries are compared as multisets** of (account, debit,
  credit, base debit, base credit). Line order and memos are not contract.
- **Error responses** must have the right status and a
  `{"error": "..."}` body. Message wording is not checked.
- **Text ordering** is compared on letters and digits only, case-folded,
  because collations differ (`spec/api.md` §1.5).

## Adding a case

1. Write `func testSomething(t *T)` in the `cases_*.go` file for its area.
   Use the fixtures in `fixtures.go` (`t.customer()`, `t.openYear()`,
   `t.post(...)`, and so on) so the case owns all of its data.
2. Register it in `allCases()` in `main.go`.
3. If the behavior is new, specify it in `spec/` in the same commit.

`t.status(want, method, path, body)` checks a status and continues.
`t.must(...)` and `t.create(...)` abandon the case on a mismatch. Failures
report the `cases_*.go` line that made the assertion.
