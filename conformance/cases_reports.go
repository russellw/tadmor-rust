package main

import (
	"fmt"
	"math/big"
	"strings"
	"time"
)

// domain §10: the financial statements, built from one case-owned year of
// activity, plus the identities that must hold across the whole ledger.
func testStatements(t *T) {
	y := t.openYear()
	d := func(md string) string { return fmt.Sprintf("%d-%s", y, md) }
	cash := t.cashAccount()
	cust, cl := t.customer()
	sup, sl := t.supplier()
	fixed := t.create("/api/accounts", J{"code": t.uniq("A"), "name": "Equipment", "account_type": "asset", "is_postable": true, "cash_flow_activity": "investing"})
	loan := t.create("/api/accounts", J{"code": t.uniq("A"), "name": "Bank loan", "account_type": "liability", "is_postable": true, "cash_flow_activity": "financing"})

	// Sales 1000, of which 600 collected.
	t.post("sales-invoices", t.invoice(cust, d("01-10"), "USD", "1000", cl.income))
	receive := func(date, amount string) {
		id := t.create("/api/customer-payments", J{"customer_id": cust, "payment_date": date, "currency_code": "USD", "amount": amount, "deposit_account_id": cash})
		t.post("customer-payments", id)
	}
	receive(d("01-20"), "600")
	// Expenses 300 and equipment 500 on account; 700 paid.
	t.post("purchase-bills", t.bill(sup, d("02-05"), "USD", "300", sl.income))
	t.post("purchase-bills", t.bill(sup, d("02-10"), "USD", "500", fixed))
	sp := t.create("/api/supplier-payments", J{"supplier_id": sup, "payment_date": d("02-20"), "currency_code": "USD", "amount": "700", "payment_account_id": cash})
	t.post("supplier-payments", sp)
	// A 2000 loan, drawn through the receivable and received in cash.
	t.post("sales-invoices", t.invoice(cust, d("03-01"), "USD", "2000", loan))
	receive(d("03-05"), "2000")

	// Profit and loss: natural signs, own year only.
	pl := t.list(path("/api/profit-and-loss?from=%s&to=%s", d("01-01"), d("12-31")))
	for _, r := range pl {
		t.shape(r, "ActivityRow")
	}
	t.eqDec(t.mustFind(pl, "account_id", cl.income), "amount", "1000")
	t.eqDec(t.mustFind(pl, "account_id", sl.income), "amount", "300")
	if find(pl, "account_id", fixed) != nil || find(pl, "account_id", cash) != nil {
		t.Errorf("profit and loss lists a balance-sheet account")
	}
	pl = t.list(path("/api/profit-and-loss?from=%s&to=%s", d("02-01"), d("12-31")))
	if find(pl, "account_id", cl.income) != nil {
		t.Errorf("profit and loss from %s includes January revenue", d("02-01"))
	}

	// Balance sheet: own rows, plus the global identity.
	bs := t.get(path("/api/balance-sheet?as_of=%s", d("12-31")))
	rows := t.arrOf(bs, "rows")
	for _, r := range rows {
		t.shape(r, "ActivityRow")
	}
	want := map[int]string{cash: "1900", cl.control: "400", fixed: "500", sl.control: "100", loan: "2000"}
	for acct, amount := range want {
		t.eqDec(t.mustFind(rows, "account_id", acct), "amount", amount)
	}
	if find(rows, "account_id", cl.income) != nil {
		t.Errorf("balance sheet lists a revenue account")
	}
	t.balanceSheetIdentity(t.get("/api/balance-sheet"))
	t.balanceSheetIdentity(bs)
	early := t.arrOf(t.get(path("/api/balance-sheet?as_of=%s", d("01-31"))), "rows")
	t.eqDec(t.mustFind(early, "account_id", cash), "amount", "600")
	if find(early, "account_id", fixed) != nil {
		t.Errorf("balance sheet as of %s lists equipment bought later", d("01-31"))
	}

	// Cash flow (indirect): 700 net income − 400 receivables + 100 payables
	// (operating) − 500 equipment (investing) + 2000 loan (financing) = 1900.
	cf := t.get(path("/api/cash-flow?from=%s&to=%s", d("01-01"), d("12-31")))
	t.shape(cf, "CashFlow")
	t.eqDec(cf, "net_income", "700")
	t.eqDec(cf, "net_cash_flow", "1900")
	cfRows := t.arrOf(cf, "rows")
	for _, r := range cfRows {
		t.shape(r, "CashFlowRow")
	}
	type flow struct{ amount, activity string }
	for acct, f := range map[int]flow{cl.control: {"-400", "operating"}, sl.control: {"100", "operating"}, fixed: {"-500", "investing"}, loan: {"2000", "financing"}} {
		r := t.mustFind(cfRows, "account_id", acct)
		t.eqDec(r, "amount", f.amount)
		t.eq("cash flow activity", t.str(r, "activity"), f.activity)
	}
	if find(cfRows, "account_id", cash) != nil {
		t.Errorf("cash flow rows include a cash account")
	}
	t.cashFlowIdentities(cf)
	t.cashFlowIdentities(t.get("/api/cash-flow"))
	opening := t.get(path("/api/cash-flow?from=%s&to=%s", d("02-01"), d("12-31")))
	t.eqDec(opening, "net_cash_flow", "1300")
	t.cashFlowIdentities(opening)

	// Trial balance: every account, balanced overall.
	tb := t.list("/api/trial-balance")
	var dr, cr big.Rat
	for _, r := range tb {
		t.shape(r, "TrialBalanceRow")
		dr.Add(&dr, t.dec(r, "total_debit"))
		cr.Add(&cr, t.dec(r, "total_credit"))
		bal := new(big.Rat).Sub(t.dec(r, "total_debit"), t.dec(r, "total_credit"))
		if bal.Cmp(t.dec(r, "balance")) != 0 {
			t.Errorf("trial balance row %s: balance is not debit − credit", t.str(r, "code"))
		}
	}
	if dr.Cmp(&cr) != 0 {
		t.Errorf("trial balance does not balance: debits %s, credits %s", decStr(&dr), decStr(&cr))
	}
	t.eqDec(t.mustFind(tb, "account_id", cash), "balance", "1900")
	if len(tb) != len(t.list("/api/accounts")) {
		t.Errorf("trial balance has %d rows, want one per account", len(tb))
	}

	// Account ledger: dated, ordered, and range-filtered.
	ledger := t.list(path("/api/accounts/%d/ledger", cash))
	var dates []string
	for _, r := range ledger {
		dates = append(dates, t.str(r, "entry_date"))
	}
	t.eq("cash ledger dates", strings.Join(dates, ","), strings.Join([]string{d("01-20"), d("02-20"), d("03-05")}, ","))
	if l := t.list(path("/api/accounts/%d/ledger?from=%s&to=%s", cash, d("02-01"), d("02-28"))); len(l) == 1 {
		t.eqDec(l[0], "credit", "700")
	} else {
		t.Errorf("February cash ledger has %d rows, want 1", len(l))
	}

	for _, p := range []string{"/api/profit-and-loss?from=2101-02-30", "/api/balance-sheet?as_of=soon", "/api/cash-flow?to=31/12/2101"} {
		t.status(400, "GET", p, nil)
	}
	t.status(404, "GET", "/api/journal-entries/999999", nil)
}

// balanceSheetIdentity: assets = liabilities + equity + current earnings.
func (t *T) balanceSheetIdentity(bs J) {
	var assets, claims big.Rat
	for _, r := range t.arrOf(bs, "rows") {
		if t.str(r, "account_type") == "asset" {
			assets.Add(&assets, t.dec(r, "amount"))
		} else {
			claims.Add(&claims, t.dec(r, "amount"))
		}
	}
	claims.Add(&claims, t.dec(bs, "current_earnings"))
	if assets.Cmp(&claims) != 0 {
		t.Errorf("balance sheet does not balance: assets %s, liabilities + equity + current earnings %s", decStr(&assets), decStr(&claims))
	}
}

// cashFlowIdentities: net income + rows = net cash flow; opening + net = closing.
func (t *T) cashFlowIdentities(cf J) {
	sum := new(big.Rat).Set(t.dec(cf, "net_income"))
	for _, r := range t.arrOf(cf, "rows") {
		sum.Add(sum, t.dec(r, "amount"))
	}
	if sum.Cmp(t.dec(cf, "net_cash_flow")) != 0 {
		t.Errorf("cash flow: net income + rows = %s, net_cash_flow = %s", decStr(sum), decStr(t.dec(cf, "net_cash_flow")))
	}
	close := new(big.Rat).Add(t.dec(cf, "opening_cash"), t.dec(cf, "net_cash_flow"))
	if close.Cmp(t.dec(cf, "closing_cash")) != 0 {
		t.Errorf("cash flow: opening + net = %s, closing_cash = %s", decStr(close), decStr(t.dec(cf, "closing_cash")))
	}
}

// domain §10 aging. Due dates sit mid-bucket relative to today (the UTC
// date), so a request that straddles midnight cannot move them.
func testAging(t *T) {
	today := time.Now().UTC()
	day := func(offset int) string { return today.AddDate(0, 0, offset).Format("2006-01-02") }
	t.create("/api/fiscal-years", J{"name": t.uniq("FY-aging"), "start_date": day(-200), "end_date": day(60)})

	orgName := t.uniq("Aging Customer")
	org := t.create("/api/organizations", J{"name": orgName})
	ar, rev := t.account("asset"), t.account("revenue")
	cust := t.create("/api/customers", J{"organization_id": org, "ar_account_id": ar})
	invoice := func(due *int, amount string, post bool) int {
		body := J{"invoice_number": t.uniq("INV"), "customer_id": cust, "currency_code": "USD",
			"lines": []J{{"description": "x", "unit_price": amount, "revenue_account_id": rev}}}
		if due == nil {
			body["invoice_date"] = day(-1)
		} else {
			body["invoice_date"], body["due_date"] = day(*due-5), day(*due)
		}
		id := t.create("/api/sales-invoices", body)
		if post {
			t.post("sales-invoices", id)
		}
		return id
	}
	offs := func(n int) *int { return &n }
	invoice(offs(10), "10", true)
	invoice(nil, "1", true)
	invoice(offs(-10), "20", true)
	invoice(offs(-45), "40", true)
	invoice(offs(-75), "80", true)
	invoice(offs(-120), "160", true)
	invoice(offs(-45), "999", false) // drafts do not age
	// A payment settles 60 of the oldest invoice.
	bank := t.cashAccount()
	pay := t.create("/api/customer-payments", J{"customer_id": cust, "payment_date": day(0), "currency_code": "USD", "amount": "60", "deposit_account_id": bank})
	t.post("customer-payments", pay)
	t.must(t.admin, 200, "POST", path("/api/customer-payments/%d/apply", pay), nil)

	row := t.mustFind(t.list("/api/ar-aging"), "party_id", cust)
	t.shape(row, "AgingRow")
	t.eq("party_name", t.str(row, "party_name"), orgName)
	t.eqDec(row, "total_outstanding", "251")
	t.eqDec(row, "not_yet_due", "11")
	t.eqDec(row, "days_1_30", "20")
	t.eqDec(row, "days_31_60", "40")
	t.eqDec(row, "days_61_90", "80")
	t.eqDec(row, "days_over_90", "100")

	sup, sl := t.supplier()
	b := t.create("/api/purchase-bills", J{"bill_number": t.uniq("BILL"), "supplier_id": sup, "bill_date": day(-50), "due_date": day(-45), "currency_code": "USD",
		"lines": []J{{"description": "x", "unit_cost": "70", "expense_account_id": sl.income}}})
	t.post("purchase-bills", b)
	ap := t.mustFind(t.list("/api/ap-aging"), "party_id", sup)
	t.shape(ap, "AgingRow")
	t.eqDec(ap, "total_outstanding", "70")
	t.eqDec(ap, "days_31_60", "70")
	t.eqDec(ap, "not_yet_due", "0")

	// Fully settled parties drop out.
	c2, l2 := t.customer()
	inv := t.invoice(c2, day(-3), "USD", "5", l2.income)
	t.post("sales-invoices", inv)
	if find(t.list("/api/ar-aging"), "party_id", c2) == nil {
		t.Errorf("customer with an open invoice missing from aging")
	}
	p2 := t.create("/api/customer-payments", J{"customer_id": c2, "payment_date": day(0), "currency_code": "USD", "amount": "5", "deposit_account_id": bank})
	t.post("customer-payments", p2)
	t.must(t.admin, 200, "POST", path("/api/customer-payments/%d/apply", p2), nil)
	if find(t.list("/api/ar-aging"), "party_id", c2) != nil {
		t.Errorf("fully paid customer still listed in aging")
	}
}

// spec/api.md §5.11. Email sending must be disabled on the server under test.
func testPDFAndEmail(t *T) {
	y := t.openYear()
	d := fmt.Sprintf("%d-04-01", y)
	cust, l := t.customer()
	sup, sl := t.supplier()
	line := func(price string, acct string, id int) []J {
		return []J{{"description": "Item", "quantity": "2", price: "12.50", acct: id}}
	}
	number := "INV/" + t.uniq("P") + " 01"
	inv := t.create("/api/sales-invoices", J{"invoice_number": number, "customer_id": cust, "invoice_date": d, "currency_code": "USD", "lines": line("unit_price", "revenue_account_id", l.income)})
	docs := []struct {
		collection, prefix string
		id                 int
		number             string
	}{
		{"sales-invoices", "invoice", inv, strings.NewReplacer("/", "-", " ", "-").Replace(number)},
	}
	add := func(collection, prefix, numberField string, body J) {
		n := t.uniq("D")
		body[numberField] = n
		docs = append(docs, struct {
			collection, prefix string
			id                 int
			number             string
		}{collection, prefix, t.create("/api/"+collection, body), n})
	}
	add("purchase-bills", "bill", "bill_number", J{"supplier_id": sup, "bill_date": d, "currency_code": "USD", "lines": line("unit_cost", "expense_account_id", sl.income)})
	add("sales-credit-notes", "credit-note", "credit_note_number", J{"customer_id": cust, "credit_note_date": d, "currency_code": "USD", "lines": line("unit_price", "revenue_account_id", l.income)})
	add("purchase-credit-notes", "supplier-credit", "credit_note_number", J{"supplier_id": sup, "credit_note_date": d, "currency_code": "USD", "lines": line("unit_cost", "expense_account_id", sl.income)})
	add("sales-orders", "sales-order", "order_number", J{"customer_id": cust, "order_date": d, "currency_code": "USD", "lines": line("unit_price", "revenue_account_id", l.income)})
	add("purchase-orders", "purchase-order", "order_number", J{"supplier_id": sup, "order_date": d, "currency_code": "USD", "lines": line("unit_cost", "expense_account_id", sl.income)})

	for _, doc := range docs {
		r := t.must(t.admin, 200, "GET", path("/api/%s/%d/pdf", doc.collection, doc.id), nil)
		if ct := r.Header.Get("Content-Type"); ct != "application/pdf" {
			t.Errorf("%s PDF Content-Type = %q", doc.collection, ct)
		}
		want := fmt.Sprintf(`inline; filename="%s-%s.pdf"`, doc.prefix, doc.number)
		if cd := r.Header.Get("Content-Disposition"); cd != want {
			t.Errorf("%s PDF Content-Disposition = %q, want %q", doc.collection, cd, want)
		}
		if !strings.HasPrefix(string(r.Body), "%PDF-") {
			t.Errorf("%s PDF body does not start with %%PDF-: %.20q", doc.collection, r.Body)
		}
		t.status(404, "GET", path("/api/%s/999999/pdf", doc.collection), nil)

		// Email: an explicit recipient reaches the (disabled) mailer.
		t.status(501, "POST", path("/api/%s/%d/email", doc.collection, doc.id), J{"to": []string{"someone@example.com"}})
		t.status(404, "POST", path("/api/%s/999999/email", doc.collection), J{"to": []string{"someone@example.com"}})
	}

	// Without "to", the counterparty organization's email is the recipient.
	t.status(422, "POST", path("/api/sales-invoices/%d/email", inv), nil)
	t.status(422, "POST", path("/api/sales-invoices/%d/email", inv), J{"to": []string{}})
	t.status(400, "POST", path("/api/sales-invoices/%d/email", inv), rawBody("{oops"))
	c := t.get(path("/api/customers/%d", cust))
	org := t.get(path("/api/organizations/%d", t.int(c, "organization_id")))
	t.status(204, "PUT", path("/api/organizations/%d", t.int(org, "id")), J{"name": t.str(org, "name"), "email": "ap@customer.example"})
	t.status(501, "POST", path("/api/sales-invoices/%d/email", inv), nil)
}

// spec/api.md §1: path ids and unknown routes.
func testRoutingAndIDs(t *T) {
	for _, p := range []string{"/api/accounts/abc", "/api/accounts/0", "/api/accounts/-3", "/api/sales-invoices/1.5", "/api/customers/9999999999999999999999"} {
		t.status(400, "GET", p, nil)
	}
	// Unknown routes and methods are a JSON 404 like any other error.
	for _, req := range [][2]string{{"GET", "/api/no-such-endpoint"}, {"DELETE", "/api/accounts"}, {"PATCH", "/api/organizations"}} {
		t.status(404, req[0], req[1], nil)
	}
}
