# HTTP API

The contract every implementation exposes. Business meaning (what posting
does, how balances are computed) lives in [`domain.md`](domain.md); this
document pins down the wire format.

Key words: **must** is checked by the conformance suite or is required for
it to work, and **should** is the reference behavior that a counterpart
ought to match but that nothing checks.

---

## 1. Conventions

### 1.1 Transport

- The JSON API is served under the path prefix `/api/`. The probes
  (§2) sit at the root. Everything else is free for the UI.
- Request bodies are JSON (`Content-Type: application/json`). Unknown
  request fields are ignored.
- Responses with a body are JSON (`Content-Type: application/json`),
  except PDFs (§5.11). An unknown path, or a known path with an unknown
  method, under `/api/` is a JSON 404 once the caller is authenticated.
  Without a session it returns 401 (§3), because authentication wraps the
  whole API.

### 1.2 Value types

| Kind | JSON representation |
| ---- | ------------------- |
| Identifier | Positive integer. Most entities use synthetic, auto-incrementing ids. Tax codes and payment terms use their `code`. Exchange rates use `(currency_code, rate_date)`. |
| Decimal (money, quantity, rate) | **String**, e.g. `"1234.5000"`. Requests accept any plain decimal string (`"12"`, `"12.5"`, `"-3.25"`). Responses must be compared **by value**, not textually. tadmor renders money and quantities at scale 4 (`"9.9900"`) and exchange rates with trailing zeros trimmed (`"1.125"`). Never a JSON number. |
| Date | String `YYYY-MM-DD`. Wherever the spec says **today** (a default date, aging), it means the current date in **UTC**, whatever the server's or database's timezone. |
| Currency | ISO 4217 alphabetic code, e.g. `"USD"`. |
| Country | ISO 3166-1 alpha-2 code, e.g. `"US"`. |
| Optional value | `null` in responses. In requests, either `null` or omitted. |

Every documented response field must be present, even when it is `null`.
Responses may carry extra fields.

**Decimal scale and range.** Each decimal is stored at a fixed scale, and a
request value with more fractional digits is rounded to it, half away from
zero, **before** it is checked or used. So a quantity of `"1.00005"` is
`1.0001`, its line arithmetic uses `1.0001`, and a quantity of `"0.00004"`
is zero and refused as zero. A value whose magnitude reaches the limit,
directly or as a computed amount such as a line subtotal, is a 422.

| Kind | Scale | Magnitude below |
| ---- | ----- | --------------- |
| Money, quantity, unit price or cost | 4 | 10^15 |
| Tax rate (percent) | 4 | 1000 |
| Exchange rate | 8 | 10^11 |

### 1.3 Read and write semantics

- **One vocabulary.** A resource's read representation names its fields
  exactly as its create/update body does: an invoice is written with
  `invoice_number`, `customer_id`, and `invoice_date`, and read back with
  the same names plus read-only extras (`status`, `total`, `balance`, and
  so on). A client can edit what it read and send it back.
- **Create** (`POST` to a collection) returns **201** with the new key:
  `{"id": n}` (synthetic keys), `{"code": "..."}` (tax codes, payment
  terms), or `{"currency_code": "...", "rate_date": "..."}` (exchange
  rates). The key is echoed as stored, so a currency code comes back
  upper-cased. The fulfilment operations, which create one resource from
  another, name the key by its kind instead (§5.10).
- **Update** (`PUT` to an item) is a **full replacement**. A boolean
  omitted from the body is `false`, and an omitted optional field becomes
  `null`, so clients must send the whole record.
- **Nothing to report means 204.** Every update, every delete, and every
  state transition that produces no new data (confirming an order,
  reconciling a statement, matching a line) returns **204 No Content**.
  Operations that do produce data return 200 with it, for example posting
  (the journal entry's id), applying (the applications created), or
  importing (the number of lines).
- **Delete** exists only where it is listed. Master data is never deleted,
  only deactivated (`is_active: false`), because it may carry history.
- On create, `is_active` is ignored and new records start active. A
  record can only be deactivated through update.

### 1.4 Errors

Every error with a JSON body has this shape:

```json
{"error": "human-readable message"}
```

Error **messages are not contract**. Status codes are. The general mapping
is:

| Status | Meaning |
| ------ | ------- |
| 400 | The request cannot be interpreted: unparseable JSON, a missing required field, or a path id or query parameter that is malformed (an id that is not a positive integer, a date that is not `YYYY-MM-DD`). |
| 401 | No session, or an invalid or expired session (§3). Also wrong login credentials. |
| 403 | Authenticated, but the endpoint is administrator-only. |
| 404 | The addressed record does not exist. |
| 409 | Conflict with existing state: a duplicate unique key, or the record is in the wrong lifecycle state for the operation (not draft, not open, already posted, already matched, and so on). |
| 422 | The request is interpretable but a value in it is not acceptable: an unknown foreign key, a value out of range or badly formed (a negative `due_days`, an email without `@`, a password under 8 characters, an invalid date or decimal in the body), or a business rule (a missing GL account configuration, no open period, an unbalanced statement, an administrator deactivating themselves, and so on). |
| 501 | The feature is present but disabled in this deployment (email, §5.11). |
| 500 | Server fault. Never expected from any request in this spec. |

Precedence: authentication (401) comes before authorization (403), which
comes before request validation (400), which comes before existence (404),
which comes before state and rule checks (409/422).

### 1.5 List ordering

Every list endpoint returns a JSON array, which is `[]` when empty and
never `null`. The documented order is part of the contract.

Numeric and date orderings are exact. Ordering by a text key (a name,
code, sku, or email) uses the database's collation, which may ignore case
and punctuation. tadmor uses Postgres's default collation for the
database. The suite therefore compares text keys only by their letters
and digits, case-folded.

---

## 2. Probes (no authentication)

| Method & path | Response |
| ------------- | -------- |
| `GET /healthz` | 200 `{"status":"ok"}` while the process is up. |
| `GET /readyz` | 200 `{"status":"ready"}` when the database is reachable, otherwise 503 `{"status":"database unavailable"}`. |

---

## 3. Authentication and sessions

Sessions are cookie-based. The cookie's name and token format are up to
the implementation. It must be `HttpOnly` and `SameSite=Lax` (other
cookies may be set alongside it), should be
`Secure` when the request arrived over HTTPS (directly or per
`X-Forwarded-Proto`), and should be stored server-side only as a hash.

Every `/api/` route except `POST /api/auth/login` and `POST /api/auth/logout`
requires a live session. Without one the response is 401. A session ends:

- at logout,
- when it expires (tadmor: a fixed 30 days from login, not sliding),
- when its user is deactivated (immediately, on the next request),
- when an administrator resets that user's password (all of the user's
  sessions are revoked).

Administrator status is re-read on every request, so demotion takes effect
immediately.

Email addresses are **case-insensitive** for login and uniqueness, and
leading and trailing whitespace is trimmed.

| Method & path | Body | Response |
| ------------- | ---- | -------- |
| `POST /api/auth/login` | `{"email", "password"}` | 200 `User` and sets the session cookie. 400 if either field is empty. 401 for an unknown email, a wrong password, or a deactivated user. All three are indistinguishable, and should take indistinguishable time. |
| `POST /api/auth/logout` | none | 204. Revokes the session if there is one and clears the cookie. Idempotent, and succeeds without a session. |
| `GET /api/auth/me` | none | 200 `User` for the session's user. |

`User`: `{"id", "email", "full_name", "is_admin"}`.

**Bootstrap.** How the first administrator comes to exist is
implementation-defined and out of band. tadmor uses
`server -adduser -email ... -name ...` with the password read from stdin.
The conformance suite is handed that administrator's credentials.

---

## 4. Initial state of a fresh instance

A freshly initialized instance must contain exactly the following
reference and seed data, plus the bootstrapped administrator, and no
other rows.

**Currencies** (`minor_unit` in parentheses): USD (2), GBP (2), EUR (2),
CAD (2), AUD (2), JPY (0). **Countries**: US, GB, IE, CA, AU, DE, FR, JP.
Loading the full ISO lists is an optional extra (tadmor: `make seed-iso`)
and is not part of the fresh state. Neither list has an endpoint. They are
observable only as valid and invalid foreign-key values.

**Chart of accounts.** All of these are postable and active, with no
parent and no currency:

| Code | Name | Type | is_cash | cash_flow_activity |
| ---- | ---- | ---- | ------- | ------------------ |
| 1000 | Cash | asset | true | operating |
| 1100 | Accounts Receivable | asset | false | operating |
| 1200 | Inventory | asset | false | operating |
| 2000 | Accounts Payable | liability | false | operating |
| 2100 | Sales Tax Payable | liability | false | operating |
| 2150 | Goods Received Not Invoiced | liability | false | operating |
| 3000 | Retained Earnings | equity | false | financing |
| 3100 | Common Stock | equity | false | financing |
| 4000 | Sales Revenue | revenue | false | operating |
| 5000 | Cost of Goods Sold | expense | false | operating |
| 6000 | Operating Expenses | expense | false | operating |
| 7000 | Foreign Exchange Gain/Loss | expense | false | operating |

**Payment terms**: `DUE` "Due on receipt" 0 days, `NET15` "Net 15" 15,
`NET30` "Net 30" 30, `NET60` "Net 60" 60.

**Tax codes**, all at rate 0 and active: `STD` "Standard sales tax" (tax
account 2100), `ZERO` "Zero-rated" (no account), `EXEMPT` "Tax exempt" (no
account).

**Settings**: base currency `USD`, FX gain/loss account = account 7000.

No organizations, customers, suppliers, products, warehouses, fiscal years,
periods, exchange rates, or documents.

---

## 5. Endpoints

All paths below are relative to `/api`. **(admin)** marks
administrator-only endpoints, which return 403 for other users.

### 5.1 Users (all admin)

`UserRecord`: `{"id", "email", "full_name", "is_active", "is_admin"}`. The
password hash never appears in any response.

| Method & path | Body | Response |
| ------------- | ---- | -------- |
| `GET /users` | | 200 `[UserRecord]`, ordered by email. |
| `GET /users/{id}` | | 200 `UserRecord`, 404. |
| `POST /users` | `{"email", "full_name", "password", "is_admin"}` | 201 `{"id"}`. 400 if the email, name, or password is missing. 422 if the email lacks `@` or the password is under 8 characters. 409 for a duplicate email (case-insensitive). |
| `PUT /users/{id}` | `{"email", "full_name", "is_active", "is_admin"}` | 204. 400 or 422 for the same field rules. 422 if the caller would deactivate or demote **themselves**. 404, or 409 for a duplicate email. |
| `POST /users/{id}/password` | `{"password"}` | 204, and revokes all of that user's sessions. 400 if the password is missing, 422 if it is under 8 characters. 404. |

### 5.2 Organizations

`Organization`: `{"id", "name", "legal_name", "tax_id", "country_code",
"default_currency", "email", "is_self"}`. All are nullable except `id`,
`name`, and `is_self`.

| Method & path | Body | Response |
| ------------- | ---- | -------- |
| `GET /organizations` | | 200, ordered by name. |
| `GET /organizations/{id}` | | 200, 404. |
| `POST /organizations` | all fields except `id` | 201. 400 without a name. 422 for an unknown country or currency. 409 if `is_self` is true and another organization already has it (at most one). |
| `PUT /organizations/{id}` | same | 204, 404, 409, 422. |

### 5.3 Customers and suppliers

Customers and suppliers are roles on an organization, at most one of each
per organization.

`Customer`: `{"id", "organization_id", "customer_number", "ar_account_id",
"payment_terms_code", "currency_code", "tax_code", "credit_limit"
(decimal|null), "is_active"}`.

`Supplier`: `{"id", "organization_id", "supplier_number", "ap_account_id",
"payment_terms_code", "currency_code", "tax_code", "is_active"}`.

| Method & path | Response |
| ------------- | -------- |
| `GET /customers`, `GET /suppliers` | 200, ordered by id. |
| `GET /customers/{id}`, `GET /suppliers/{id}` | 200, 404. |
| `POST /customers`, `POST /suppliers` | 201. 400 if `organization_id` is missing or ≤ 0. 409 if the organization already has the role, or the customer or supplier number is taken. 422 for unknown references or a negative `credit_limit`. |
| `PUT /customers/{id}`, `PUT /suppliers/{id}` | 204, 404, 409, 422. |

### 5.4 Products

`Product`: `{"id", "sku", "name", "description", "unit_price" (decimal),
"currency_code", "revenue_account_id", "tax_code", "track_inventory",
"inventory_account_id", "cogs_account_id", "is_active"}`.

| Method & path | Response |
| ------------- | -------- |
| `GET /products` | 200, ordered by sku. |
| `GET /products/{id}` | 200, 404. |
| `POST /products` | 201. 400 without `sku` or `name`. `unit_price` defaults to 0. 409 for a duplicate sku. 422 for unknown references. |
| `PUT /products/{id}` | 204, 404, 409, 422. |

### 5.5 Chart of accounts

`Account`: `{"id", "code", "name", "account_type", "parent_id",
"currency_code", "is_postable", "is_active", "is_cash",
"cash_flow_activity"}`.

`account_type` is one of `asset`, `liability`, `equity`, `revenue`,
`expense`. `cash_flow_activity` is one of `operating` (the default when
omitted), `investing`, `financing`.

| Method & path | Response |
| ------------- | -------- |
| `GET /accounts` | 200, ordered by code. |
| `GET /accounts/{id}` | 200, 404. |
| `POST /accounts` | 201. 400 without `code`, `name`, or `account_type`. `is_postable` defaults to **false**, which makes a summary account. 409 for a duplicate code. 422 for an unknown type or activity, `is_cash` on a non-asset, an unknown parent, or self-parenting. |
| `PUT /accounts/{id}` | 204, 404, 409, 422. |
| `GET /accounts/{id}/ledger?from=&to=` | 200 `[LedgerRow]` (§5.14). 404 for an unknown account. 400 for a malformed date. |

### 5.6 Tax codes, payment terms, warehouses

`TaxCode`: `{"code", "name", "rate" (percent, decimal), "tax_account_id",
"is_active"}`.

`PaymentTerm`: `{"code", "name", "due_days"}`.

`Warehouse`: `{"id", "code", "name", "address_id", "is_active"}`.

| Method & path | Response |
| ------------- | -------- |
| `GET /tax-codes` | 200, ordered by code. |
| `GET /tax-codes/{code}` | 200, 404. |
| `POST /tax-codes` | 201 `{"code"}`. 400 without `code` or `name`. `rate` defaults to 0. 409 for a duplicate. 422 for a negative rate or an unknown account. |
| `PUT /tax-codes/{code}` | 204. The path's code wins over the body's. 404, 422. |
| `GET /payment-terms` | 200, ordered by `due_days` then code. |
| `GET /payment-terms/{code}` | 200, 404. |
| `POST /payment-terms` | 201 `{"code"}`. 400 without `code` or `name`. 422 for a negative `due_days`. 409 for a duplicate. |
| `PUT /payment-terms/{code}` | 204. The path's code wins. 400, 404, 422. |
| `GET /warehouses` | 200, ordered by code. |
| `GET /warehouses/{id}` | 200, 404. |
| `POST /warehouses` | 201. 400 without `code` or `name`. 409 for a duplicate code. |
| `PUT /warehouses/{id}` | 204, 404, 409. |

### 5.7 Fiscal years and accounting periods

`FiscalYear`: `{"id", "name", "start_date", "end_date", "status"}`.

`AccountingPeriod`: `{"id", "fiscal_year_id", "name", "start_date",
"end_date", "status"}`.

Status is `open` or `closed`.

| Method & path | Body | Response |
| ------------- | ---- | -------- |
| `GET /fiscal-years` | | 200, ordered by start date. |
| `GET /fiscal-years/{id}` | | 200, 404. |
| `POST /fiscal-years` | `{"name", "start_date", "end_date"}` | 201, created open. 400 for a missing field. 409 for a duplicate name. 422 if end is before start or a date is invalid. |
| `PUT /fiscal-years/{id}` | same | 204. **Status cannot be set here**: open and closed belong to close and reopen. 404, 409, 422. |
| `POST /fiscal-years/{id}/close` **(admin)** | `{"retained_earnings_account_id"}` | 200 `{"closing_entry_id": int|null, "next_fiscal_year_id": int|null}`. 400 without the account. 404. 409 if the year is not open. 422 if an earlier year is still open, or the account is not a postable, active **equity** account. See domain §9. |
| `POST /fiscal-years/{id}/reopen` **(admin)** | none | 200 `{"reversal_entry_id": int|null}`. 404. 409 if the year is not closed. 422 if a later year is closed. |
| `GET /accounting-periods` | | 200, ordered by start date. |
| `GET /accounting-periods/{id}` | | 200, 404. |
| `POST /accounting-periods` | `{"fiscal_year_id", "name", "start_date", "end_date"}` | 201, created open. 400 for a missing field. 409 if the name is a duplicate within its fiscal year. 422 if it overlaps **any** existing period, is in an unknown or closed fiscal year, or end is before start. |
| `PUT /accounting-periods/{id}` | the same plus `"status"` (`open` or `closed`, default `open`) | 204. This is how a period is closed or reopened. 404, 409. 422 if it overlaps, or would open a period in a closed fiscal year. |

### 5.8 Ledger settings and exchange rates

`Settings`: `{"base_currency", "fx_gain_loss_account_id"}`.

`ExchangeRate`: `{"currency_code", "rate_date", "rate"}`. The rate is the
number of base-currency units bought by one unit of `currency_code`.

| Method & path | Body | Response |
| ------------- | ---- | -------- |
| `GET /settings` | | 200 `Settings`. |
| `PUT /settings` **(admin)** | `Settings` | 204. The currency is upper-cased. 400 without `base_currency`. 422 for an unknown or malformed currency, an FX account that is not postable and active, or a change of base currency **once any journal entry exists**. |
| `GET /exchange-rates` | | 200, ordered by currency, then date newest first. |
| `POST /exchange-rates` | `ExchangeRate` | 201 `{"currency_code", "rate_date"}`, with the currency upper-cased as stored. 400 if the currency, date, or rate is missing. 409 for a duplicate (currency, date). 422 for an unknown or malformed currency or a rate ≤ 0. |
| `PUT /exchange-rates/{currency}/{date}` | `{"rate"}` | 204. 404 if no rate exists for that key. |
| `DELETE /exchange-rates/{currency}/{date}` | | 204, 404. |

### 5.9 Subledger documents

Six document kinds share one lifecycle (domain §4):

| Collection | Party field | Number field | Date field(s) | Line price field | Line account field |
| ---------- | ----------- | ------------ | ------------- | ---------------- | ------------------ |
| `sales-invoices` | `customer_id` | `invoice_number` | `invoice_date`, `due_date` | `unit_price` | `revenue_account_id` |
| `purchase-bills` | `supplier_id` | `bill_number` | `bill_date`, `due_date` | `unit_cost` | `expense_account_id` |
| `sales-credit-notes` | `customer_id` | `credit_note_number` | `credit_note_date` | `unit_price` | `revenue_account_id` |
| `purchase-credit-notes` | `supplier_id` | `credit_note_number` | `credit_note_date` | `unit_cost` | `expense_account_id` |
| `customer-payments` | `customer_id` | none | `payment_date` | none | none |
| `supplier-payments` | `supplier_id` | none | `payment_date` | none | none |

Number uniqueness: invoice numbers and sales credit-note numbers are
unique globally. Bill numbers and purchase credit-note numbers are unique
**per supplier**, since they are the supplier's own numbering.

**Invoice, bill, and credit-note request body.** The header carries the
party, number, and date fields above, plus `currency_code` (required),
`reference` and `memo` (optional), and `lines`. Each line is
`{"product_id", "description" (required), "quantity" (default "1"),
"unit_price" or "unit_cost" (default "0"), "<account field>", "tax_code",
"tax_rate" (percent, default "0")}`. The tax rate is a snapshot carried on
the line. It is **not** looked up from `tax_code`, which only selects the
GL tax account at posting. `due_date` is never derived from payment
terms; the client supplies it.

**Payment request body**: `{"customer_id" or "supplier_id", "payment_date",
"currency_code", "amount" (required, > 0), "method" (cash, check, card,
transfer, other, or null), "reference", "deposit_account_id"}`. Supplier
payments use `payment_account_id` in place of `deposit_account_id`.

Validation (400) covers the required header fields (party > 0, number,
date, currency, and amount for payments) and a non-empty `description` on
every line.

**Read shapes.** Each document is read with its own key fields, named as
in its request body, followed by the fields all four share:

| Collection | Key fields |
| ---------- | ---------- |
| `sales-invoices` (`SalesInvoice`) | `id`, `invoice_number`, `customer_id`, `invoice_date`, `due_date`, `payment_status` |
| `purchase-bills` (`PurchaseBill`) | `id`, `bill_number`, `supplier_id`, `bill_date`, `due_date`, `payment_status` |
| `sales-credit-notes` (`SalesCreditNote`) | `id`, `credit_note_number`, `customer_id`, `credit_note_date`, `application_status` |
| `purchase-credit-notes` (`PurchaseCreditNote`) | `id`, `credit_note_number`, `supplier_id`, `credit_note_date`, `application_status` |

Shared: `"currency_code", "status", "total", "amount_applied", "balance",
"journal_entry_id", "reference", "memo"`.

- `status`: `draft`, `posted`, or `void`. Nothing in the API produces
  `void` today.
- `payment_status` (invoices and bills): `unpaid`, `partial`, `paid`, or
  `void`.
- `application_status` (credit notes): `open`, `partial`, `applied`, or
  `void`. Credit notes do not age, so they have no due date.
- `journal_entry_id` is set while the document is posted.

`InvoiceLine`: `{"line_no", "product_id", "description", "quantity",
"unit_price", "tax_code", "tax_rate", "line_subtotal", "tax_amount",
"line_total", "revenue_account_id", "order_line_id"}`. Bill lines carry
`unit_cost` and `expense_account_id` instead. Credit-note lines have the
same shape with `order_line_id` always null.

`CustomerPayment`: `{"id", "customer_id", "payment_date",
"deposit_account_id", "currency_code", "amount", "method", "reference",
"status", "amount_applied", "unapplied", "journal_entry_id"}`.
`SupplierPayment` is the same with `supplier_id` and `payment_account_id`.

`Application`: `{"document_id", "document_number", "amount_applied"}`.

**Endpoints**, with `{c}` ranging over the six collections unless noted:

| Method & path | Response |
| ------------- | -------- |
| `POST /{c}` | 201 `{"id"}`, created as a draft. 400 per the validation rules above. 409 for a duplicate number. 422 for unknown references, quantity 0, a due date before the document date, an invalid date or decimal, an amount ≤ 0, or an unknown method. |
| `PUT /{c}/{id}` | 204. Replaces the header **and the full line set**. 400, 404. 409 if the document is not a draft, or was produced from an order (domain §6.4). 409 or 422 as for create. |
| `DELETE /{c}/{id}` | 204. 404. 409 if the document is not a draft. Order-produced drafts **may** be deleted. |
| `GET /{c}` | 200, a list of the collection's read shape, ordered by date newest first, then id descending. |
| `GET /{c}/{id}` | 200, the collection's read shape. 404. |
| `GET /{c}/{id}/lines` (not payments) | 200 `[InvoiceLine]` or `[BillLine]`, ordered by line_no. 404 for an unknown document. |
| `GET /{c}/{id}/applications` (payments and credit notes only) | 200 `[Application]` in creation order. 404 for an unknown document. |
| `POST /{c}/{id}/post` | 200 `{"journal_entry_id"}`. See domain §4.2 for the checks and their codes. |
| `POST /{c}/{id}/unpost` **(admin)** | 200 `{"reversal_entry_id"}`. See domain §4.4. |
| `POST /{c}/{id}/apply` (payments and credit notes only) | 200 `{"applications": [{"document_id", "amount_applied"}]}`, which may be empty. 404. 409 if the payment or note is not posted. 422 if a realized FX difference arises and no FX account is configured. See domain §5. |

### 5.10 Sales and purchase orders

| Collection | Party | Number | Dates | Line price | Line account |
| ---------- | ----- | ------ | ----- | ---------- | ------------ |
| `sales-orders` | `customer_id` | `order_number` | `order_date`, `expected_ship_date` | `unit_price` | `revenue_account_id` |
| `purchase-orders` | `supplier_id` | `order_number` | `order_date`, `expected_receipt_date` | `unit_cost` | `expense_account_id` |

Order numbers are unique globally within each collection. The request body
mirrors invoices and bills, and line `quantity` must be > 0 (422 otherwise).

`SalesOrder`: `{"id", "order_number", "customer_id", "order_date",
"expected_ship_date", "currency_code", "status", "total",
"invoiced_status", "shipped_status", "reference", "memo"}`.

`PurchaseOrder` is the mirror, with `supplier_id`,
`expected_receipt_date`, `billed_status`, and `received_status`.

- `status`: `draft`, `open`, `closed`, or `cancelled`.
- `invoiced_status` and `billed_status`: `none`, `partial`, `invoiced` or
  `billed`. `shipped_status` and `received_status`: `none`, `partial`,
  `shipped` or `received`.

`SalesOrderLine`: the invoice-line fields (without `order_line_id`'s
nullability) plus `"order_line_id"` (the line's own id, used to address
it in fulfilment requests), `"qty_invoiced"`, `"qty_shipped"`,
`"qty_to_invoice"`, and `"qty_to_ship"`. `PurchaseOrderLine` mirrors it
with `qty_billed`, `qty_received`, `qty_to_bill`, and `qty_to_receive`.

| Method & path | Body | Response |
| ------------- | ---- | -------- |
| `POST /{c}`, `PUT /{c}/{id}`, `DELETE /{c}/{id}` | as for invoices and bills | As §5.9. Edit and delete are allowed only in `draft` (409 otherwise). |
| `GET /{c}` | | 200, ordered by order date newest first, then id descending. |
| `GET /{c}/{id}`, `GET /{c}/{id}/lines` | | 200, 404. |
| `POST /{c}/{id}/confirm` | | 204 (draft → open). 404. 409 if not a draft. 422 if the order has no lines. |
| `POST /{c}/{id}/close` | | 204 (open → closed). 404. 409 if not open. |
| `POST /{c}/{id}/cancel` | | 204 (draft or open → cancelled). 404. 409 if closed or cancelled, or if it is open but anything has been invoiced, billed, shipped, or received against it. |
| `POST /sales-orders/{id}/invoice` | `{"invoice_number", "invoice_date", "due_date", "lines": [{"order_line_id", "quantity"}]}` | 201 `{"invoice_id"}`, a draft invoice. 400 without a number or date. 404. 409 if the order is not open. 422 if there is nothing left to invoice. 409 for a duplicate invoice number. |
| `POST /purchase-orders/{id}/bill` | `{"bill_number", "bill_date", "due_date", "lines"}` | 201 `{"bill_id"}`, the mirror of the above. |
| `POST /sales-orders/{id}/ship` | `{"warehouse_id", "movement_date", "reference", "lines"}` | 201 `{"movement_ids": [...]}`, draft issue movements. 400 without a warehouse. 404, 409, 422 as above. |
| `POST /purchase-orders/{id}/receive` | same | 201 `{"movement_ids": [...]}`, draft receipt movements, valued in the base currency (domain §6.3). 422 also when the order is in a foreign currency with no exchange rate on or before the movement date. |

An empty or absent `lines` array means "the full remaining quantity of
every eligible line". Requested quantities are capped at what remains. See
domain §6.

### 5.11 Printing and email

The printable collections are `sales-invoices`, `purchase-bills`,
`sales-credit-notes`, `purchase-credit-notes`, `sales-orders`, and
`purchase-orders`.

| Method & path | Response |
| ------------- | -------- |
| `GET /{c}/{id}/pdf` | 200 with `Content-Type: application/pdf`, a body that begins `%PDF-`, and `Content-Disposition: inline; filename="<prefix>-<number>.pdf"`. Every character of the document number outside `[A-Za-z0-9._-]` is replaced by `-`. 404 for an unknown document. |
| `POST /{c}/{id}/email` | Body `{"to": ["addr", ...]}`, which is optional. 404 for an unknown document. With an empty or absent `to`, the recipient is the counterparty organization's `email`, and if that is null the response is 422. When email sending is **disabled** (the conformance configuration), it then returns 501. When enabled, it sends the PDF as an attachment and returns 200 `{"status":"sent","to":[...]}`. |

Filename prefixes: `invoice`, `bill`, `credit-note`, `supplier-credit`,
`sales-order`, and `purchase-order`. For example, the invoice
`INV/2026 01` downloads as `invoice-INV-2026-01.pdf`. The email subject
should be `"<Label> <number>"`, where the label is Invoice, Bill, Credit
Note, Sales Order, or Purchase Order. The PDF content is described in
domain §11.

### 5.12 Stock movements

`StockMovement`: `{"id", "product_id", "warehouse_id", "movement_date",
"movement_type", "status", "quantity", "unit_cost", "total_cost",
"reference", "notes", "journal_entry_id", "source_type"}`.

- `movement_type` is one of `receipt`, `issue`, `adjustment`,
  `transfer_in`, `transfer_out`.
- `source_type` is null for hand-entered movements and
  `sales_order_line` or `purchase_order_line` for fulfilment.
- `status` is `posted` exactly when `journal_entry_id` is non-null, and
  `draft` otherwise.

| Method & path | Body | Response |
| ------------- | ---- | -------- |
| `GET /stock-movements` | | 200, ordered by date newest first, then id descending. |
| `GET /stock-movements/{id}` | | 200, 404. |
| `POST /stock-movements` | `{"product_id", "warehouse_id", "movement_type", "movement_date" (default today), "quantity" (signed), "unit_cost" (default 0), "reference", "notes"}` | 201. 400 for a missing product, warehouse, type, or quantity. 422 if the quantity sign disagrees with the type, the quantity is 0, the product is not inventory-tracked or not active, or `unit_cost` < 0. |
| `PUT /stock-movements/{id}` | same | 204. 404. 409 if posted or produced by fulfilment. |
| `DELETE /stock-movements/{id}` | | 204. 404. 409 if posted. Fulfilment movements may be deleted while unposted. |
| `POST /stock-movements/{id}/post` | `{"credit_account_id"}`, required for receipts and ignored otherwise. The body may be absent. | 200 `{"journal_entry_id"}`. 404. 409 if already posted. 422 for a type other than receipt or issue, zero cost, missing product accounts, a receipt without a valid postable credit account, or no open period. |
| `POST /stock-movements/{id}/unpost` **(admin)** | | 200 `{"reversal_entry_id"}`. 404. 409 if not posted. |

### 5.13 Bank reconciliation

`BankStatement`: `{"id", "account_id", "account_code", "account_name",
"statement_date", "opening_balance", "closing_balance", "reference",
"status", "line_count", "matched_count", "lines_total", "difference"}`.

- `status` is `open` or `reconciled`.
- `difference` = opening + lines_total − closing.

`BankStatementLine`: `{"id", "line_no", "txn_date", "description",
"reference", "amount", "journal_line_id", "journal_entry_id",
"entry_date", "entry_memo"}`. The `journal_*` and `entry_*` fields are
null while the line is unmatched.

`MatchCandidate`: `{"journal_line_id", "journal_entry_id", "entry_date",
"reference", "memo", "amount"}`.

Amounts are signed from the books' point of view: a deposit is positive
(a debit to the cash account).

| Method & path | Body | Response |
| ------------- | ---- | -------- |
| `GET /bank-statements` | | 200, ordered by statement date newest first, then id descending. |
| `GET /bank-statements/{id}` | | 200, 404. |
| `POST /bank-statements` | `{"account_id", "statement_date", "opening_balance", "closing_balance", "reference"}` | 201 `{"id"}`. 400 for a missing field. 422 unless the account is a postable, active, **cash** account. |
| `PUT /bank-statements/{id}` | same | 204. 404. 409 if not open. 422 when changing the account while lines are matched. |
| `DELETE /bank-statements/{id}` | | 204. 404. 409 if not open. |
| `GET /bank-statements/{id}/lines` | | 200, ordered by line_no. 404 for an unknown statement. |
| `POST /bank-statements/{id}/lines` | `{"txn_date", "description", "reference", "amount"}` | 201 `{"id"}`, appended as the next line_no. 400 for a missing field. 404. 409 if not open. 422 for a zero amount. |
| `POST /bank-statements/{id}/import` | `{"csv": "..."}` | 200 `{"imported": n}`. 400 if `csv` is empty. 422 for invalid CSV content (domain §8.2). 404. 409 if not open. |
| `GET /bank-statements/{id}/candidates` | | 200 `[MatchCandidate]`, ordered by entry date, then entry id, then line. 404. |
| `POST /bank-statements/{id}/auto-match` | | 200 `{"matched": n}`. 404. 409 if not open. |
| `POST /bank-statements/{id}/reconcile` | | 204. 404. 409 if not open. 422 if any line is unmatched or the statement does not balance. |
| `POST /bank-statements/{id}/reopen` **(admin)** | | 204. 404. 409 if not reconciled. |
| `POST /bank-statement-lines/{id}/match` | `{"journal_line_id"}` | 204. 400 without a journal line. 404. 409 if the statement is not open, the line is already matched, or the journal line already backs another statement line. 422 if the journal line does not exist, is not posted, is on another account, or has a different signed amount. |
| `POST /bank-statement-lines/{id}/unmatch` | | 204, a no-op if unmatched. 404. 409 if the statement is not open. |
| `DELETE /bank-statement-lines/{id}` | | 204. 404. 409 if the statement is not open. |

### 5.14 Journal and reports

All report figures are in the **base currency** (domain §7) and cover
**posted** entries only.

| Method & path | Response |
| ------------- | -------- |
| `GET /journal-entries/{id}` | 200 `JournalEntry`, 404. |
| `GET /accounts/{id}/ledger?from=&to=` | 200 `[LedgerRow]`, with inclusive optional date bounds, ordered by entry date, then entry id, then line. |
| `GET /trial-balance` | 200 `[TrialBalanceRow]`, **every** account including those with no activity, ordered by code. |
| `GET /profit-and-loss?from=&to=` | 200 `[ActivityRow]`, revenue and expense accounts with activity in range, ordered by code. Year-end closing entries are excluded. |
| `GET /balance-sheet?as_of=` | 200 `{"rows": [ActivityRow], "current_earnings"}`, covering asset, liability, and equity accounts with activity on or before the date, ordered by code. |
| `GET /cash-flow?from=&to=` | 200 `CashFlow`. |
| `GET /ar-aging`, `GET /ap-aging` | 200 `[AgingRow]` for each party with a positive outstanding posted balance, ordered by party id. |
| `GET /inventory-valuation` | 200 `[ValuationRow]` for each product that has movements, ordered by sku. |

A malformed date parameter is 400. An absent bound is unbounded.

`JournalEntry`: `{"id", "entry_date", "currency_code", "exchange_rate",
"reference", "memo", "status", "lines": [{"line_no", "account_id",
"account_code", "account_name", "memo", "debit", "credit", "base_debit",
"base_credit"}]}`.

`LedgerRow`: `{"journal_entry_id", "entry_date", "reference", "memo",
"currency_code", "debit", "credit", "base_debit", "base_credit"}`.

`TrialBalanceRow`: `{"account_id", "code", "name", "account_type",
"total_debit", "total_credit", "balance"}`. The balance is debit-positive.

`ActivityRow`: `{"account_id", "code", "name", "account_type", "amount"}`,
in each account's natural sign (domain §10).

`CashFlow`: `{"net_income", "rows": [{"account_id", "code", "name",
"activity", "amount"}], "net_cash_flow", "opening_cash", "closing_cash"}`.

`AgingRow`: `{"party_id", "party_name", "total_outstanding", "not_yet_due",
"days_1_30", "days_31_60", "days_61_90", "days_over_90"}`.

`ValuationRow`: `{"product_id", "sku", "name", "qty_on_hand",
"value_on_hand", "avg_unit_cost"}`.
