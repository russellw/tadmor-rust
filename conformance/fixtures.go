package main

import (
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"strconv"
)

// Fixture builders. Every case creates its own accounts, parties, and fiscal
// year, so report figures can be checked per account without interference
// from other cases.

// account creates a postable, active account of the given type.
func (t *T) account(typ string) int {
	return t.create("/api/accounts", J{
		"code": t.uniq("A"), "name": "Conformance " + typ, "account_type": typ, "is_postable": true,
	})
}

// cashAccount creates a postable cash (bank) account.
func (t *T) cashAccount() int {
	return t.create("/api/accounts", J{
		"code": t.uniq("A"), "name": "Conformance bank", "account_type": "asset",
		"is_postable": true, "is_cash": true,
	})
}

// fiscalYear creates an open fiscal year covering the whole calendar year y,
// with no periods: posting creates monthly periods on demand (domain §9.2).
func (t *T) fiscalYear(y int) int {
	return t.create("/api/fiscal-years", J{
		"name": t.uniq(fmt.Sprintf("FY%d", y)), "start_date": fmt.Sprintf("%d-01-01", y),
		"end_date": fmt.Sprintf("%d-12-31", y),
	})
}

// openYear allocates this case's own year and opens a fiscal year for it.
func (t *T) openYear() int {
	y := t.nextYear()
	t.fiscalYear(y)
	return y
}

func (t *T) org(fields J) int {
	body := J{"name": t.uniq("Org")}
	for k, v := range fields {
		body[k] = v
	}
	return t.create("/api/organizations", body)
}

// ledger is one party's chart: control, revenue/expense, and tax accounts.
type ledger struct {
	control, income, tax int
}

// customer creates an organization with the customer role, its own A/R,
// revenue, and tax accounts.
func (t *T) customer() (id int, l ledger) {
	l = ledger{control: t.account("asset"), income: t.account("revenue"), tax: t.account("liability")}
	id = t.create("/api/customers", J{"organization_id": t.org(nil), "ar_account_id": l.control})
	return id, l
}

// supplier creates an organization with the supplier role, its own A/P,
// expense, and tax accounts.
func (t *T) supplier() (id int, l ledger) {
	l = ledger{control: t.account("liability"), income: t.account("expense"), tax: t.account("asset")}
	id = t.create("/api/suppliers", J{"organization_id": t.org(nil), "ap_account_id": l.control})
	return id, l
}

// taxCode creates an active tax code posting to account.
func (t *T) taxCode(account int, rate string) string {
	code := t.uniq("TX")
	t.must(t.admin, 201, "POST", "/api/tax-codes", J{
		"code": code, "name": "Conformance tax", "rate": rate, "tax_account_id": account,
	})
	return code
}

// invoice creates a draft sales invoice with one untaxed line.
func (t *T) invoice(customer int, date, currency, amount string, revenue int) int {
	return t.create("/api/sales-invoices", J{
		"invoice_number": t.uniq("INV"), "customer_id": customer, "invoice_date": date,
		"currency_code": currency,
		"lines":         []J{{"description": "Services", "quantity": "1", "unit_price": amount, "revenue_account_id": revenue}},
	})
}

// bill creates a draft purchase bill with one untaxed line.
func (t *T) bill(supplier int, date, currency, amount string, expense int) int {
	return t.create("/api/purchase-bills", J{
		"bill_number": t.uniq("BILL"), "supplier_id": supplier, "bill_date": date,
		"currency_code": currency,
		"lines":         []J{{"description": "Supplies", "quantity": "1", "unit_cost": amount, "expense_account_id": expense}},
	})
}

// post posts a document and returns its journal entry id.
func (t *T) post(collection string, id int) int {
	r := t.must(t.admin, 200, "POST", fmt.Sprintf("/api/%s/%d/post", collection, id), nil)
	return t.int(t.obj(r), "journal_entry_id")
}

// doc fetches one document's read model.
func (t *T) doc(collection string, id int) J {
	return t.get(fmt.Sprintf("/api/%s/%d", collection, id))
}

// nonAdmin returns a session for an ordinary (non-administrator) user,
// creating the user on first use.
func (t *T) nonAdmin() *Client {
	if t.user != nil {
		return t.user
	}
	b := make([]byte, 12)
	_, _ = rand.Read(b)
	t.userPass = hex.EncodeToString(b)
	email := fmt.Sprintf("user-%s@conformance.test", t.tag)
	t.userID = t.create("/api/users", J{"email": email, "full_name": "Ordinary User", "password": t.userPass, "is_admin": false})
	c := newClient(t.base)
	t.must(c, 200, "POST", "/api/auth/login", J{"email": email, "password": t.userPass})
	t.user = c
	return c
}

func path(format string, args ...any) string { return fmt.Sprintf(format, args...) }

func itoa(i int) string { return strconv.Itoa(i) }
