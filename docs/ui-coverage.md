# UI coverage

**Status:** written 2026-10-06, when the UI was first completed.

This records where each item of the UI checklist in `spec/domain.md` §13
is met, and how it was checked. tadmor's `docs/counterpart-metrics.md`
asks for a walk-through, item by item, before a counterpart's figures
count; this is the record of it.

**How it was checked.**

- **Test** means `tests/ui.rs` drives it: requests through the whole
  router with the session cookie kept, forms posted with their tokens,
  and the pages read back. They run with `make test`.
- **Browser** means a scripted walk-through in headless Chromium on
  2026-10-06, against a release build serving the database a full
  conformance run leaves behind. It used tadmor's Playwright install from
  outside this repo, so it is not a dependency here and nothing in the
  repo runs it. It made 52 checks, all passing, among them the line
  editor's fills and exact previews (`static/app.js`), adding and
  removing lines, the delete confirmation, opening a row of each
  document list, and role-based hiding. It found no console errors and
  no Content Security Policy violations. The home, invoice form, invoice
  detail, credit note, and profit and loss screens were screenshotted and
  inspected.
- **Reviewed** means the screen was opened and inspected, but no test
  asserts the item's content.

The UI is server-rendered Askama templates (`templates/`) over the same
service modules as the JSON API; the handlers are in `src/ui/`. The only
script is `static/app.js` and the only stylesheet `static/app.css`; the
Content Security Policy allows no inline script or style. Every form
posts a token derived from the session.

## General

| Item | Where | Checked |
| ---- | ----- | ------- |
| G1 | `/login`; every other UI path sends the browser there without a session (`ui::require_login`), and back to the page asked for afterwards | Test (`signing_in_and_out`: redirect, failed login, only local `next`), Browser |
| G2 | The user's name and Sign out, in the header of every page | Test, Browser |
| G3 | The sidebar on every page (`ui::nav`) | Test (every master data link), Browser (every link opens) |
| G4 | Users hidden from the sidebar and refused with 403; unpost, statement reopen, year-end close and reopen shown only to administrators; settings read-only. The services refuse them regardless | Test (`administrator_only_screens`, `ordinary_users_are_not_offered_unpost`), Browser |
| G5 | A refusal shows the server's message beside the form or action, keeping what was typed | Test (missing name, negative credit limit, posting twice, demoting oneself, cancelling a fulfilled order, a bad report date), Browser (failed login) |
| G6 | Deleting a document, payment, order, stock movement, bank statement, or exchange rate goes through a confirmation page | Test (invoice, exchange rate), Browser |
| G7 | Amounts are the database's exact decimals, grouped, never floats; totals are summed as integers of ten-thousandths (`ui::reports::units`). A foreign-currency amount carries its currency in lists, details and home | Test (exact line money, running balance), Reviewed (home's per-currency outstanding) |
| G8 | Any unknown address, or a known one with the wrong method, shows a not-found page | Test (`forms_need_their_token_and_pages_have_a_policy`, `accounting_screens`) |

## Home

| Item | Where | Checked |
| ---- | ----- | ------- |
| H1 | Outstanding receivables and payables per currency, with the overdue part (`ui::home`) | Reviewed (screenshot) |
| H2 | Open sales and purchase orders, draft invoices and bills, each linking to its list | Reviewed |
| H3 | The ten most overdue invoices, oldest due first, linked, and a link to A/R aging | Reviewed (screenshot) |
| H4 | Bills due in the next 14 days, linked, and a link to A/P aging | Reviewed |
| H5 | New invoice, customer payment, bill, supplier payment, sales order, purchase order | Reviewed (screenshot), Test (each form opens) |

## Master data

| Item | Where | Checked |
| ---- | ----- | ------- |
| M1 | `/organizations` (`ui::master`, a `Resource` per kind) | Test (create, refusal, edit) |
| M2 | `/customers`, `/suppliers`; the organization is a picker on create and read-only after | Test (refusal, create, list) |
| M3 | `/products`, with revenue, inventory and COGS accounts | Reviewed |
| M4 | `/accounts`; the parent picker never offers the account itself; each row links to its ledger | Test (no self-parent) |
| M5 | `/tax-codes`, `/payment-terms`, `/warehouses` | Test (pages open), Reviewed |
| M6 | Lists show active status; forms deactivate; nothing deletes master data | Test (customer list), Reviewed |
| M7 | `/users` (administrators only): list, create with password, edit, and `/users/{id}/password` | Test (self-demotion refused visibly) |
| M8 | `/settings`: a form for administrators, read-only facts otherwise | Test |

## Invoices, bills and credit notes

| Item | Where | Checked |
| ---- | ----- | ------- |
| D1 | `/sales-invoices`, `/purchase-bills`, `/sales-credit-notes`, `/purchase-credit-notes` (`ui::documents`) | Test, Browser (a row of each opens) |
| D2 | The line editor (`templates/docform.html`, `static/app.js`): add and remove lines; a product fills description and tax code, and on the sales side price and revenue account; a tax code fills its rate; exact previews | Test (create, edit, refusal kept), Browser (fills; 21.999, 1.8149, 23.8139, 43.8239) |
| D3 | The detail screen: header, lines with tax amounts and totals, statuses, journal entry link once posted | Test, Browser (screenshot) |
| D4 | Post, edit (not when produced from an order), delete for drafts; unpost for administrators; apply for a posted credit note with something unapplied | Test |
| D5 | A credit note lists what it is applied to, linked | Reviewed (screenshot), Test (the same table for payments) |
| D6 | A PDF link to `/api/{collection}/{id}/pdf`, which the session cookie opens | Test |
| D7 | An email form; blank means the counterparty's email; shows where it went, or the error (no address on file, sending off) | Test |

## Payments

| Item | Where | Checked |
| ---- | ----- | ------- |
| P1 | `/customer-payments`, `/supplier-payments` | Test, Browser |
| P2 | The form: party, date, currency, amount, method, reference, account | Test |
| P3 | The detail screen with post, edit, delete, apply, unpost by state | Test |
| P4 | What the payment is applied to, linked | Test |

## Orders

| Item | Where | Checked |
| ---- | ----- | ------- |
| O1 | `/sales-orders`, `/purchase-orders` (`ui::orders`) | Test, Browser |
| O2 | The line editor, shared with documents | Test |
| O3 | Per line: ordered, invoiced (billed), shipped (received), and what remains on each | Test |
| O4 | Confirm, edit, delete, cancel for drafts; close and cancel for open orders | Test (confirm; cancelling a fulfilled order refused) |
| O5 | `/{order}/invoice` (`/bill`): number, date, due date, each outstanding line with its remainder, lowerable; on to the new draft | Test |
| O6 | `/{order}/ship` (`/receive`): warehouse, date, stocked outstanding lines; links to the movements made | Test |
| O7 | PDF link and email form on the detail screen | Reviewed |

## Inventory

| Item | Where | Checked |
| ---- | ----- | ------- |
| S1 | `/stock-movements` | Test, Browser |
| S2 | The quantity is a magnitude signed by the type; an adjustment keeps its sign | Test (an issue typed as 1 is stored as -1) |
| S3 | The detail screen; posting a receipt asks for the account to credit, proposing GRNI; edit unless from an order; delete; unpost for administrators | Test |

## Reports

| Item | Where | Checked |
| ---- | ----- | ------- |
| R1 | `/reports/profit-and-loss` (`ui::reports`) | Test, Browser |
| R2 | `/reports/balance-sheet`, both sides of the identity shown | Test |
| R3 | `/reports/cash-flow` | Test |
| R4 | `/reports/trial-balance`, each account linking to its ledger | Test, Browser |
| R5 | `/reports/ledger/{account}`, with a running balance, and currencies where any line is foreign | Test (running balance) |
| R6 | `/journal-entries/{id}`, each account linking to its ledger | Test |
| R7 | `/reports/ar-aging`, `/reports/ap-aging`, with a total row | Test |
| R8 | `/reports/inventory-valuation`, with a total value | Test |

## Accounting

| Item | Where | Checked |
| ---- | ----- | ------- |
| A1 | `/periods`: years with their periods; year and period forms; the new period proposes the month after the latest; close or reopen in one step | Test |
| A2 | `/fiscal-years/{id}/close` states what closing does and proposes Retained Earnings; the latest closed year can be reopened | Test (the page), Reviewed |
| A3 | `/exchange-rates`: list, create, change, confirmed delete | Test |
| A4 | `/bank-statements`: list, and a form offering only cash accounts | Test |
| A5 | The statement screen: add lines, import pasted CSV, delete lines, auto-match, match to a candidate of the line's amount or see all, unmatch, reconcile, edit, delete, reopen | Test (import, candidates offered, auto-match, reconcile) |
