# Domain rules

The business behavior behind [`api.md`](api.md). Every rule here is
observable through the API. Every implementation runs on the shared
schema (`README.md`), and where it enforces a rule in the database, that
is noted in passing, so a counterpart knows the rule comes with the
schema.

---

## 1. Principles

1. **Exact decimals.** Money, quantities, rates, and percentages are exact
   decimals end to end. They are never binary floating point.
2. **Double entry.** Every financial effect is a balanced, posted journal
   entry. Nothing writes account balances directly. Every balance and
   report figure is derived from journal lines.
3. **Derived state is never stored.** Stock on hand, order fulfilment,
   document balances, and payment status are computed from the records
   that cause them.
4. **History is reversed, not rewritten.** A posted journal entry is never
   edited or deleted. Undoing a posting adds a mirror reversing entry, and
   both remain.
5. **Drafts are free; posted documents are frozen.** A document can be
   edited or deleted only in `draft`.
6. **Master data is deactivated, never deleted.**

---

## 2. Rounding and line arithmetic

All rounding is to **4 decimal places, half away from zero** (Postgres
`round(numeric, 4)`). What is rounded is the **exact** result. A quotient
in particular is rounded once, from its exact value: computing it to some
finite precision first and then rounding to 4 places can round twice and
give a different answer (`920907399.1189 / 123456789.0123` is
`7.45934999999999995…`, which is `7.4593`). Postgres `numeric` division
is not exact in this sense; tadmor multiplies by `0.01` for tax and uses
an integer-division helper for average costs. Inputs are first rounded to
their stored scale (`api.md` §1.2).

For every invoice, bill, credit-note, and order line:

```
line_subtotal = round(quantity × unit_price, 4)          (unit_cost for purchases)
tax_amount    = round(quantity × unit_price × tax_rate / 100, 4)
line_total    = line_subtotal + tax_amount
```

`tax_rate` is a percentage snapshot sent on the line. It is not copied
from the tax code. A document header's `subtotal`, `tax_total`, and
`total` are the sums over its lines, and they are kept current while the
document is a draft. Quantities may be negative on invoices, bills, and
credit notes (a returned or discounted line) but never zero. Order-line
quantities must be positive. Unit prices may be negative.

A stock movement's `total_cost = round(quantity × unit_cost, 4)`, and it
carries the quantity's sign.

---

## 3. Parties and master data

- An **organization** is any business entity. **Customer** and **supplier**
  are roles layered on top of one, at most one of each per organization,
  and an organization may hold both.
- At most one organization is flagged `is_self` (our own company). It
  supplies the issuer block on printed documents.
- The organization's `email` is the default recipient when a document is
  emailed.
- An organization's address is shown on printed documents. Addresses and
  contacts exist in the reference schema but have no API (§14).
- A customer's `ar_account_id` (a supplier's `ap_account_id`) is the
  control account its documents post to. It is optional on the record,
  but posting fails without it.
- Accounts form a tree through `parent_id`. Only **postable, active**
  accounts can carry journal lines. Summary accounts (`is_postable:
  false`) organize the tree. Only asset accounts may be `is_cash`.
- Products: `revenue_account_id` is the fallback revenue account for
  invoice and credit-note lines that do not name one.
  `inventory_account_id` is the fallback expense account for bill and
  supplier-credit lines, and the inventory account for stock postings.
  `cogs_account_id` is debited when stock is issued. Only products with
  `track_inventory` and `is_active` may appear on stock movements.

---

## 4. Document lifecycle and posting

### 4.1 States

Invoices, bills, both kinds of credit note, and both kinds of payment move
`draft → posted`. **Unpost** (administrators only) returns a posted
document to `draft`. `void` exists in the data model but nothing produces
it (§14).

A stock movement's status is derived: it is `posted` exactly when its
`journal_entry_id` is set, and `draft` otherwise. Only receipts and issues
post.

### 4.2 Posting checks

Posting runs as one transaction. It either fully succeeds or changes
nothing. The checks run in this order:

| Check | Status |
| ----- | ------ |
| Document exists | 404 |
| Document is a draft (movements: not yet posted) | 409 |
| Total > 0 (payments: amount > 0; movements: total cost ≠ 0) | 422 |
| Party control account is set (invoices and credit notes: customer A/R; bills and supplier credits: supplier A/P; payments: both the control account and the deposit or payment account) | 422 |
| Every line with a non-zero subtotal resolves to an account (the line's own, else the product fallback, §3) | 422 |
| Every line with a non-zero tax amount has a tax code with a tax account | 422 |
| Movements: the type is `receipt` or `issue`. Issues need the product's COGS and inventory accounts. Receipts need the product's inventory account and a `credit_account_id` naming a postable, active account. | 422 |
| An open accounting period covers the document date (§9.2) | 422 |
| An exchange rate is available for a foreign-currency document (§7) | 422 |

### 4.3 The entries posting produces

Each posting creates one posted journal entry dated on the document date,
in the document's currency (movements use the base currency), with the
document number as `reference` (payments and movements have none). The
document records the entry's id.

| Document | Debit | Credit |
| -------- | ----- | ------ |
| Sales invoice | A/R: total | Revenue, per account: Σ line_subtotal. Tax, per tax account: Σ tax_amount. |
| Purchase bill | Expense, per account: Σ line_subtotal. Tax, per tax account: Σ tax_amount. | A/P: total |
| Sales credit note | Revenue, per account. Tax, per tax account. | A/R: total |
| Purchase credit note | A/P: total | Expense, per account. Tax, per tax account. |
| Customer payment | Deposit account: amount | A/R: amount |
| Supplier payment | A/P: amount | Payment account: amount |
| Stock issue | Product COGS: abs(total_cost) | Product inventory: abs(total_cost) |
| Stock receipt | Product inventory: total_cost | `credit_account_id`: total_cost |

Lines are summed per account (and tax per tax account). An account whose
sum nets to zero gets no line. An account whose sum nets **negative**,
such as a discount line on its own revenue account or a rebate on its own
expense account, is posted on the **opposite** side: an invoice debits
the discount account. The control line always carries the document total.
Its base amount is the net of the detail lines' base amounts. Line memos and
line ordering are not contract. The suite compares an entry's lines as a
multiset of (account, debit, credit, base_debit, base_credit).

A receipt's credit account is typically the seeded **Goods Received Not
Invoiced** account (2150). The matching bill line then debits 2150, so
GRNI nets to zero once the goods are billed (in base, only if the bill's
exchange rate equals the receipt's; see §14).

### 4.4 Unposting

Unposting is administrator-only. It posts a **reversal**: a new entry with
the same date, currency, and exchange rate as the original, every line's
debit and credit swapped (base amounts too), linked to the original. The
original stays posted, so the two net to zero. The document returns to
`draft` with no journal entry and can then be edited, deleted, or
re-posted (which creates a fresh entry).

| Refusal | Status |
| ------- | ------ |
| The document is not posted | 409 |
| Invoices and bills: any payment or credit note is applied to it | 409 |
| Credit notes: the note is applied to anything | 409 |
| Any line of the entry is matched on a bank statement | 409 |
| The entry was already reversed | 409 |
| No open period covers the original date (§9.2) | 422 |

Unposting a **payment** also deletes its applications and reverses any
realized-FX entries they created (§7.3). A stock movement's quantity record
is untouched. Only its GL link is removed.

---

## 5. Applications and settlement

An **application** allocates part of a posted payment or credit note to a
posted invoice (customer side) or bill (supplier side) of the **same party
and currency**. Applications create no journal entry of their own, because
both documents are already in the GL. The exception is a realized FX
difference (§7.3).

### 5.1 Auto-apply

`POST /{payments or credit notes}/{id}/apply` allocates the settling
document's unapplied remainder across the party's open documents:

1. The settling document must be posted (409 otherwise).
2. Its **remaining** amount is the payment amount (or credit-note total)
   less what is already applied.
3. **Open documents** are the party's posted invoices (or bills) in the
   same currency whose **available** amount, total less everything
   applied from payments **and** credit notes, is greater than 0.
4. Open documents are taken oldest first, by document date, then id. Each
   receives `min(available, remaining still unallocated)`.
5. It returns the applications created, which may be none. Applying again
   with nothing remaining is a no-op.

Nothing may ever be over-applied, neither the settler nor the document.

### 5.2 Balances and statuses

For invoices and bills:

- `amount_applied` is the sum of applications from **posted** payments and
  credit notes.
- `balance = total − amount_applied`.
- `payment_status` is `void` if the document is void; otherwise `paid`
  when total > 0 and balance ≤ 0; otherwise `partial` when anything is
  applied; otherwise `unpaid`. Drafts are `unpaid`.

For credit notes:

- `amount_applied` is the sum of the note's applications.
- `balance = total − amount_applied`.
- `application_status` is `open`, `partial`, or
  `applied`, by the same rule.

For payments, `unapplied = amount − amount_applied`.

---

## 6. Orders

Orders are commercial documents. They never post to the GL.

### 6.1 Lifecycle

```
draft --confirm--> open --close--> closed
  \                  \
   --cancel-->        --cancel--> cancelled   (open: only while nothing has been fulfilled)
```

- **Confirm** requires at least one line (422).
- **Close** is manual, for an open order that will get no more fulfilment.
  Fully fulfilled orders are **not** closed automatically.
- **Cancel**: a draft always cancels. An open order cancels only while
  nothing is invoiced, billed, shipped, or received against it (409).
  Closed and cancelled orders cannot be cancelled (409).
- Only drafts can be edited or deleted (409).

### 6.2 Fulfilment quantities (derived per line)

Sales orders:

- `qty_invoiced` is the sum of quantities on invoice lines linked to the
  order line, across all non-void invoices **including drafts**.
- `qty_shipped` is the negated sum of linked issue movements, posted or
  not.
- `qty_to_invoice = max(quantity − qty_invoiced, 0)`.
- `qty_to_ship = max(quantity − qty_shipped, 0)` when the line's product
  is inventory-tracked, and **0** otherwise. Services are invoiced, never
  shipped.

Purchase orders mirror these with `billed`, `received`, `to_bill`, and
`to_receive`.

Header statuses, per axis: `none` when nothing has been fulfilled on that
axis; `invoiced`, `billed`, `shipped`, or `received` when nothing remains
outstanding on it; `partial` otherwise.

### 6.3 Fulfilment operations (order must be `open`, otherwise 409)

- **Invoice** a sales order (and **bill** a purchase order) creates a
  **draft** invoice or bill. The header takes the order's party and
  currency, with `reference` set to the order number. Each line copies
  the order line's product, description, price or cost, account, tax
  code, and tax rate, carries `order_line_id`, and takes quantity
  `min(remaining, requested)`, or the whole remainder when `lines` is
  empty. Lines that come to 0 are skipped. The remaining lines are
  numbered 1..n in order-line id order. If every line is skipped, nothing
  is created (422).
- **Ship** a sales order creates one **draft issue** movement per eligible
  line: tracked, active products with quantity to ship. Each movement
  draws from the given warehouse, is dated `movement_date` (default
  today), and has `source_type = "sales_order_line"`. Its `unit_cost` is
  that warehouse's current **moving-average cost** for the product,
  `round(Σ total_cost / Σ quantity, 4)` over all of the pair's movements,
  or 0 when there are none.
- **Receive** a purchase order creates **draft receipt** movements in the
  same way, with `source_type = "purchase_order_line"`. Stock is valued in
  the base currency, so each movement's `unit_cost` is
  `round(order line unit_cost × rate, 4)`, where the rate is 1 for a
  base-currency order and otherwise the order currency's latest rate on or
  before the movement date (§7.1). With no such rate, nothing is created
  (422).

Linked invoice and bill lines are only valid while the order is open,
belongs to the same party and currency, and is not over-invoiced (or
over-billed). The created documents are posted separately through their
own endpoints.

### 6.4 Order-linked documents

An invoice or bill with any order-linked line, and a fulfilment stock
movement, **cannot be edited** (409), because their lines are what draw
the order down. While still draft or unposted they **can be deleted**,
which returns the quantities to the order.

---

## 7. Multi-currency

### 7.1 Base currency and rates

- The ledger has one **base currency** (fresh instances use `USD`). It
  cannot change once any journal entry exists (422).
- An exchange rate states how many base units one foreign unit buys on a
  given date. There is at most one rate per currency per date, and rates
  are maintained by hand.
- An entry's **rate** is 1 for the base currency. Otherwise it is the
  currency's **latest rate dated on or before the entry date**. With no
  such rate the posting fails (422). The entry stores the rate it used, so
  editing rates later never changes posted history.

### 7.2 Dual amounts

Every journal line carries `debit`/`credit` in the entry's currency and
`base_debit`/`base_credit` in the base currency. Every report sums base
amounts. A posted entry must balance in **both**.

- Detail lines (revenue, expense, tax): `base = round(amount × rate, 4)`,
  on the same side as the amount (§4.3).
- The gross control line (A/R or A/P on invoices, bills, and credit notes)
  takes as its base amount the **net of the detail lines' base amounts**,
  so the entry balances in base exactly.
- Payment lines both use `round(amount × rate, 4)`.
- Stock postings, closing entries, and FX entries are in the base
  currency at rate 1.

### 7.3 Realized FX on settlement

When an application is created (§5.1), let

```
diff = round(applied × settler_rate, 4) − round(applied × document_rate, 4)
```

where `settler_rate` is the payment's or credit note's entry rate and
`document_rate` is the invoice's or bill's. If `diff ≠ 0`, an **FX entry**
is posted for that application. It is in the base currency, dated on the
settling document's date, and has two lines between the party's control
account and the configured FX gain/loss account:

| Side | Effect for diff > 0 | Effect for diff < 0 |
| ---- | ------------------- | ------------------- |
| Customer (A/R) | Dr A/R diff, Cr FX diff (gain) | Cr A/R \|diff\|, Dr FX (loss) |
| Supplier (A/P) | Cr A/P diff, Dr FX diff (loss) | Dr A/P \|diff\|, Cr FX (gain) |

With no FX account configured, the apply fails (422) and creates nothing.
The FX entry is linked to its application and reversed if the payment is
unposted.

Settlement across currencies is not supported, because an application
requires matching currencies. There is no revaluation of open balances
at period end (§14).

---

## 8. Bank reconciliation

### 8.1 Statements

A statement belongs to one **postable, active, cash** account. It has a
statement date, opening and closing balances, and lines. Line amounts are
signed from the books' side: a deposit is positive. A statement is `open`
until reconciled. While open, its header and lines may change. A
**reconciled** statement and its lines are frozen. Reopening is
administrator-only.

### 8.2 CSV import

The request body carries CSV text with the columns
`date,description,amount[,reference]`:

- Fields are trimmed. Blank records are ignored.
- The **first** record is treated as a header and skipped if its first
  field is not a `YYYY-MM-DD` date.
- Every data record needs 3 or 4 fields, a valid date, a non-empty
  description, and an amount matching `-?(\d+(\.\d*)?|\.\d+)` that is not
  zero.
- Any bad record rejects the whole import (422) and adds nothing. CSV with
  no data rows is also a 422. An empty `csv` field is a 400 (`api.md`
  §5.13).
- Lines are appended after any existing ones.

### 8.3 Matching

A statement line matches **one** posted journal line on the statement's
account whose **transaction-currency** signed amount (`debit − credit`)
equals the line's amount. A journal line backs at most one statement line
across all statements.

**Match candidates** are the account's posted journal lines that no
statement line has claimed.

**Auto-match** visits unmatched lines in `line_no` order. Each takes the
unclaimed candidate with an equal amount and the **nearest entry date**
(ties go to the lowest journal-line id), or stays unmatched if there is
none. It returns the number matched.

**Reconcile** requires every line to be matched and `opening + Σ lines =
closing` (422 otherwise).

---

## 9. Fiscal calendar and year-end

### 9.1 Years and periods

Fiscal years have a name (unique), start and end dates, and an
open/closed status. Accounting periods belong to a year, have unique
names within it, and **must not overlap any other period**, inclusive of
their bounds and across all years. A period may be opened or closed by
editing it, but a period in a closed year cannot be opened (422). No
journal entry may be written into a closed period.

### 9.2 Period resolution when posting

Posting (and reversing) needs the open period that covers the entry date.

- If an open period covers the date, it is used.
- Otherwise, if a **closed** period covers it, the posting fails (422).
- Otherwise, if an **open fiscal year** covers the date, a period for that
  calendar month is created automatically. It is named `YYYY-MM` and
  clipped to the year's bounds. If that period would overlap an existing
  one, the posting fails (422).
- Otherwise the posting fails (422).

### 9.3 Year-end close (administrator)

Closing an open year requires that **no earlier year** (by start date) is
still open (422), and a postable, active **equity** account for retained
earnings (422). Closing the year:

1. Posts a **closing entry**, unless no revenue or expense account has a
   balance. The entry is dated the year's end date, in the base currency,
   and flagged as closing. It carries:
   - one line per revenue and expense account with a non-zero cumulative
     base balance up to the year end, on the side that zeroes it;
   - one line to the retained-earnings account for the net: a credit for
     net income, a debit for a net loss.

   The entry lands in the period covering the end date. That period is
   created if missing, or used even if it is already closed.
2. Closes every period of the year, then the year.
3. **Rolls forward.** If no fiscal year covers the day after the end date,
   it creates one starting that day, one year long (ending the day before
   the same date a year later), named `FY` plus the calendar year of its
   end date. If that name is taken, nothing is created and the id is
   `null`.

The response gives the closing entry's id (`null` if there was nothing to
sweep) and the new year's id (`null` if none was created).

**Reopen** (administrator) applies only to a closed year with **no later
closed year** (422). It sets the year open, reopens the period that holds
the closing entry, and reverses the closing entry. The reversal is also
flagged as closing. Other periods stay closed until reopened by hand. The
response gives the reversal's id, or `null` if the close posted no entry.

Closing entries and their reversals are excluded from the profit and loss
statement and the cash-flow statement, but included in the trial balance,
the balance sheet, and ledgers.

---

## 10. Reports

All reports cover **posted** entries and **base** amounts.

- **Trial balance**: every account, with `total_debit = Σ base_debit`,
  `total_credit = Σ base_credit`, and `balance = total_debit −
  total_credit`.
- **Profit and loss** (`from`, `to` inclusive, either optional): revenue
  and expense accounts with at least one line in range, excluding closing
  entries. Amounts are in natural sign: revenue = Σ(credit − debit),
  expense = Σ(debit − credit). Offsetting activity shows as 0.
- **Balance sheet** (`as_of` inclusive, optional): asset, liability, and
  equity accounts with lines on or before the date. Assets are
  debit-positive; liabilities and equity are credit-positive.
  `current_earnings` is Σ(credit − debit) over revenue and expense lines
  up to the date, **including** closing entries, so after a year-end
  close the swept income has moved into retained earnings. The identity
  is Σ assets = Σ liabilities + Σ equity + current_earnings.
- **Cash flow** (indirect method, `from`/`to` inclusive):
  - `net_income` is P&L net income over the range.
  - `rows` covers every **non-cash** asset, liability, and equity account
    with lines in range, excluding closing entries. Each row's amount is
    Σ(credit − debit), so a source of cash is positive, and it is labeled
    with the account's `cash_flow_activity`.
  - `opening_cash` is the cash accounts' debit-positive balance strictly
    before `from` (0 when `from` is absent).
  - `net_cash_flow` is their movement within the range.
  - `closing_cash` is their balance up to `to`.
  - The identities are `net_income + Σ rows = net_cash_flow` and
    `opening_cash + net_cash_flow = closing_cash`.
- **Account ledger**: an account's posted lines in range, with both
  amount pairs and the line's memo, falling back to the entry's.
- **A/R and A/P aging**: per party, the posted invoices (or bills) with a
  balance > 0, bucketed by `due_date` against **today** (the UTC date,
  `api.md` §1.2):
  - `not_yet_due` when there is no due date or it is today or later;
  - `days_1_30` when it falls in [today − 30, today);
  - `days_31_60`, `days_61_90`, and `days_over_90` likewise.

  `party_name` is the organization's name. Unapplied payments and credits
  do not reduce aging, because only applications do.
- **Inventory valuation**: per product across all warehouses and all
  movements, **posted or not**: `qty_on_hand = Σ quantity`,
  `value_on_hand = Σ total_cost`, and `avg_unit_cost = round(value / qty,
  4)`, or 0 when the quantity is 0.

---

## 11. Printed documents

Each printable document (invoices, bills, both kinds of credit note, both
kinds of order) renders as one shared layout, which should include:

- the document kind as a title, its number, dates, status, and currency;
- the issuer block from the `is_self` organization (name, legal name, tax
  id, address), omitted when none is flagged;
- the counterparty block (name, legal name, tax id, address);
- a line table with number, description, quantity, unit price or cost,
  tax rate, and subtotal;
- subtotal, tax, and total, plus the applied amount and balance for
  invoices, bills, and credit notes when anything is applied;
- reference and memo.

tadmor renders A4 pages with the standard-14 Helvetica font.

---

## 12. Users and roles

There are two levels: **administrators** and everyone else. Only
administrators may:

- manage users;
- unpost documents and stock movements;
- close and reopen fiscal years;
- reopen bank statements;
- change ledger settings.

Administrators cannot deactivate or demote themselves, which guards
against locking out the last administrator one click at a time.
Passwords are at least 8 characters. tadmor hashes them with
PBKDF2-HMAC-SHA256 at 600,000 iterations, though the scheme is not
contract.

---

## 13. User interface (required, not checked by the suite)

Every implementation ships its own user interface (`README.md`). This
section is the checklist it must satisfy. Each item is a capability, not a
layout: grouping, navigation style, wording, and technology are free.
Items are numbered so that a coverage report can cite them, and a
counterpart is complete only when a walk-through ticks every one
(`docs/counterpart-metrics.md`). tadmor's SPA is the reference for each
item.

Where an item says "a form", the form edits every writable field of the
corresponding request body in `api.md`, with a picker over active records
for each reference (accounts, parties, products, tax codes, payment terms,
warehouses, organizations). Where it says "a list", the list shows the
records in the API's order, each one opening its edit form or detail
screen, with a way to create a new one.

### 13.1 General

- **G1** A login screen is the only page shown without a session. Failed
  logins show an error. A request that returns 401 brings the login
  screen back.
- **G2** The signed-in user's name is shown, with a way to sign out.
- **G3** Every screen in this section is reachable by navigation from
  every other, without typing a URL.
- **G4** Administrator-only actions (§12) are hidden or disabled for other
  users, including the Users screen. The server still enforces them.
- **G5** A refused action shows the server's error message next to it.
  Nothing fails silently.
- **G6** Deleting a document, payment, order, stock movement, bank
  statement, or exchange rate asks for confirmation first.
- **G7** Amounts are shown as exact decimals. Wherever a document is in a
  foreign currency, its currency is shown with its amounts.
- **G8** An unknown address shows a not-found message.

### 13.2 Home

- **H1** Receivables and payables outstanding (posted documents with a
  positive balance), totaled per currency, each with its overdue portion.
- **H2** Counts of open sales and purchase orders, and of draft invoices
  and bills.
- **H3** The most overdue invoices, oldest due date first, each linking to
  the invoice, with a link to the AR aging report.
- **H4** Bills due within the next 14 days, each linking to the bill,
  with a link to the AP aging report.
- **H5** One-step starts for a new invoice, customer payment, bill,
  supplier payment, sales order, and purchase order.

### 13.3 Master data

- **M1** Organizations: a list (name, legal name, tax id, country,
  currency) and a form.
- **M2** Customers and suppliers: a list each (organization name, number,
  currency, tax code, terms, credit limit for customers, active status)
  and a form. The organization is chosen on create and read-only on edit.
- **M3** Products: a list (sku, name, unit price, currency, tax code,
  whether inventory is tracked, active status) and a form with the
  revenue, inventory, and COGS accounts.
- **M4** Chart of accounts: a list (code, name, type, currency, postable,
  active status) and a form. A parent picker never offers the account
  itself.
- **M5** Tax codes (code, name, rate, active status), payment terms (code,
  name, due days), and warehouses (code, name, active status): a list and
  a form each.
- **M6** Every record with `is_active` shows its active status in its list
  and is deactivated through its form. Master data has no delete.
- **M7** Users (administrators only): a list (email, name, role, active
  status), a create form with a password, an edit form, and a separate
  password reset. Deactivating or demoting oneself is refused visibly.
- **M8** Settings: base currency and FX gain/loss account. Read-only for
  non-administrators.

### 13.4 Invoices, bills, and credit notes

These items apply to each of the four collections: sales invoices,
purchase bills, sales credit notes, and supplier credits.

- **D1** A list: number, party name, date, due date (invoices and bills),
  total, balance or unapplied amount, and status. Newest first.
- **D2** A form with header fields and any number of lines, which can be
  added and removed. Picking a product fills the line's description and
  tax code from it, and on the sales side its price and revenue account
  too. Picking a tax code fills
  the line's tax rate from it. Everything stays editable. Line totals and
  document totals are previewed as the user types, computed as in §2.
- **D3** A detail screen with the header, every line (quantity, price,
  tax rate, tax amount, line total), the totals, the status and payment
  or application status, and, when posted, a link to the journal entry
  (R6).
- **D4** Actions offered by state. A draft can be posted, edited (unless
  produced from an order, §6.4), and deleted. A posted document can be
  unposted by an administrator. A posted credit note with something
  unapplied can be applied.
- **D5** A credit note's detail lists the documents it is applied to,
  each with its amount and a link.
- **D6** A PDF action that opens the document's PDF.
- **D7** An email action: optional recipients, blank meaning the
  counterparty's email on file. It shows the address the server used, or
  the error (including 501 when sending is disabled).

### 13.5 Payments

These items apply to customer payments and supplier payments.

- **P1** A list: party, date, method, amount, applied, unapplied, and
  status. Newest first.
- **P2** A form: party, date, currency, amount, method, reference, and
  the deposit or payment account.
- **P3** A detail screen with the payment's facts and, when posted, a
  link to the journal entry. A draft can be posted, edited, and deleted.
  A posted payment with something unapplied can be applied. A posted
  payment can be unposted by an administrator.
- **P4** The detail lists the documents the payment is applied to, each
  with its amount and a link.

### 13.6 Orders

These items apply to sales orders and purchase orders.

- **O1** A list: number, party, date, total, status, and both fulfilment
  statuses. Newest first.
- **O2** A form like D2. Drafts only.
- **O3** A detail screen with the header and statuses, and per line the
  ordered quantity, the quantities invoiced (or billed) and shipped (or
  received), and what remains on each axis.
- **O4** A draft can be confirmed, edited, deleted, and cancelled. An open
  order can be closed, and cancelled while nothing has been fulfilled.
- **O5** On an open order, an invoice (or bill) action asks for the
  number, date, and due date, and offers each outstanding line with its
  remaining quantity filled in. The quantities can be lowered for a
  partial invoice. On success it goes to the new draft document.
- **O6** On an open order, a ship (or receive) action asks for the
  warehouse and date, offers only the stocked outstanding lines, with
  remaining quantities filled in and lowerable, and links to the
  movements it created.
- **O7** PDF and email actions, as D6 and D7.

### 13.7 Inventory

- **S1** Stock movements: a list (date, product, warehouse, type,
  quantity, unit cost, total cost, and whether it is posted). Newest
  first.
- **S2** A form where the quantity is entered as a magnitude and signed by
  the type (an adjustment takes its sign as typed).
- **S3** A detail screen with the movement's facts and, when posted, a
  link to the journal entry. An
  unposted receipt or issue can be posted, a receipt asking for the
  account to credit. An unposted movement can be deleted, and edited
  unless it came from an order. A posted one can be unposted by an
  administrator.

### 13.8 Reports

Every date bound is optional, and a blank one is unbounded.

- **R1** Profit and loss for a date range: revenue and expense accounts
  with totals per section, and net income.
- **R2** Balance sheet as of a date: assets, liabilities, and equity with
  totals per section, plus current earnings, so that the identity of
  §10 is visible.
- **R3** Cash flow for a date range: operating (starting from net
  income), investing, and financing sections with subtotals, then
  opening cash, net cash flow, and closing cash.
- **R4** Trial balance: every account with debit, credit, and balance,
  and totals. Each account links to its ledger.
- **R5** Account ledger for a date range: each line with its date, a link
  to its journal entry, memo, debit and credit, and a running balance.
  Where lines are in a foreign currency, it shows the currency and the
  base amounts too.
- **R6** Journal entry: date, currency, exchange rate, reference, memo,
  status, and every line with its account (linking to the ledger), memo,
  debit, credit, and base amounts, with totals.
- **R7** AR and AP aging: one row per party with the five buckets and the
  total, plus a total row.
- **R8** Inventory valuation: sku, product, quantity on hand, average
  unit cost, and value on hand, plus a total value.

### 13.9 Accounting

- **A1** Periods: fiscal years, each with its periods. Forms for a fiscal
  year and a period. The new-period form proposes the month after the
  latest existing period. Each period can be closed or reopened in one
  step from the list.
- **A2** Year-end (administrators): closing a year asks for the retained
  earnings account, proposing the seeded Retained Earnings, and states
  what will happen before doing it. Reopening the latest closed year is
  offered too.
- **A3** Exchange rates: a list (currency, date, rate), a form to create
  and change a rate, and delete.
- **A4** Bank statements: a list (account, date, reference, closing
  balance, lines matched of total, difference, status) and a form, which
  offers only cash accounts.
- **A5** A statement's detail: opening, closing, and difference; its
  lines, each with its match. While open: lines can be added by hand,
  imported by pasting CSV (§8.2), and deleted; auto-match can be run;
  each unmatched line offers the candidates of its amount, with a way to
  see all candidates, and matches one; a matched line can be unmatched;
  and the statement can be reconciled, edited, or deleted. A reconciled
  statement can be reopened by an administrator.

## 14. Known gaps (deliberately unspecified)

These are absent from tadmor today. A counterpart need not implement
them, and the suite does not test them.

- There is no void operation. The `void` status exists in the data model
  only.
- Credit-note applications cannot be removed, so an applied credit note,
  and any invoice or bill it was applied to, can never be unposted.
- Cross-currency settlement and period-end revaluation of open
  foreign-currency balances are not supported. A sub-cent rounding
  residual can remain on a foreign document settled by several partial
  payments.
- Addresses, contacts, and reorder levels have no API. They exist in the
  reference schema, and addresses feed the PDFs.
- Transfers and adjustments never post to the GL.
- Receiving against a foreign-currency purchase order converts at the
  movement date's rate, and the bill converts at the bill date's. When the
  two rates differ, the base difference stays in GRNI: no FX entry clears
  it.
- Payment terms are informational. Due dates are supplied by the client.
