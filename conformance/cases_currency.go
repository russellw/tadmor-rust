package main

import (
	"fmt"
)

// domain §7: rates, dual amounts, and realized FX on settlement.
func testForeignCurrency(t *T) {
	y := t.openYear()
	d := func(md string) string { return fmt.Sprintf("%d-%s", y, md) }
	for _, r := range []J{
		{"currency_code": "EUR", "rate_date": d("01-01"), "rate": "1.10"},
		{"currency_code": "EUR", "rate_date": d("02-01"), "rate": "1.123456"},
		{"currency_code": "EUR", "rate_date": d("03-01"), "rate": "1.20"},
	} {
		t.must(t.admin, 201, "POST", "/api/exchange-rates", r)
	}
	settings := t.get("/api/settings")
	fx := t.int(settings, "fx_gain_loss_account_id")
	bank := t.cashAccount()

	cust, l := t.customer()
	tax := t.taxCode(l.tax, "10")

	// Invoice A at 1.10: 100 EUR is 110 USD.
	invA := t.invoice(cust, d("01-15"), "EUR", "100", l.income)
	e := t.entry(t.post("sales-invoices", invA))
	t.eq("entry currency", t.str(e, "currency_code"), "EUR")
	t.eqDec(e, "exchange_rate", "1.1")
	t.eqLines("invoice A", e, dr(l.control, "100").base("110"), cr(l.income, "100").base("110"))

	// Invoice B at 1.123456: detail lines round to 4 places; A/R takes their sum.
	invB := t.create("/api/sales-invoices", J{"invoice_number": t.uniq("INV"), "customer_id": cust, "invoice_date": d("02-15"), "currency_code": "EUR",
		"lines": []J{{"description": "Taxed", "quantity": "1", "unit_price": "33.33", "revenue_account_id": l.income, "tax_code": tax, "tax_rate": "10"}}})
	e = t.entry(t.post("sales-invoices", invB))
	t.eqDec(e, "exchange_rate", "1.123456")
	// 33.33 × 1.123456 = 37.44478848 → 37.4448; 3.333 × 1.123456 = 3.744478848 → 3.7445.
	t.eqLines("invoice B", e, dr(l.control, "36.663").base("41.1893"), cr(l.income, "33.33").base("37.4448"), cr(l.tax, "3.333").base("3.7445"))

	// A rate dated on the document date applies to it.
	invC := t.invoice(cust, d("03-01"), "EUR", "10", l.income)
	t.eqDec(t.entry(t.post("sales-invoices", invC)), "exchange_rate", "1.2")

	// No rate on or before the date: the posting is refused, not a server error.
	aud := t.invoice(cust, d("03-01"), "AUD", "10", l.income)
	t.status(422, "POST", path("/api/sales-invoices/%d/post", aud), nil)

	// The base currency is frozen once entries exist.
	t.status(422, "PUT", "/api/settings", J{"base_currency": "EUR", "fx_gain_loss_account_id": fx})
	t.eq("base currency", t.str(t.get("/api/settings"), "base_currency"), "USD")

	// Payment at 1.20 settles invoice A (booked at 1.10): a realized gain of 10.
	pay := t.create("/api/customer-payments", J{"customer_id": cust, "payment_date": d("03-10"), "currency_code": "EUR", "amount": "100", "deposit_account_id": bank})
	t.eqLines("payment", t.entry(t.post("customer-payments", pay)), dr(bank, "100").base("120"), cr(l.control, "100").base("120"))

	// With no FX account configured, settlement at a different rate is refused.
	t.status(204, "PUT", "/api/settings", J{"base_currency": "USD", "fx_gain_loss_account_id": nil})
	t.status(422, "POST", path("/api/customer-payments/%d/apply", pay), nil)
	t.status(204, "PUT", "/api/settings", J{"base_currency": "USD", "fx_gain_loss_account_id": fx})
	if n := len(t.list(path("/api/customer-payments/%d/applications", pay))); n != 0 {
		t.Errorf("a refused apply left %d applications", n)
	}

	apps := t.arrOf(t.obj(t.must(t.admin, 200, "POST", path("/api/customer-payments/%d/apply", pay), nil)), "applications")
	if len(apps) != 1 || t.int(apps[0], "document_id") != invA {
		t.Fatalf("payment applied to %v, want invoice A only", apps)
	}
	fxEntry := t.findFXEntry(l.control, d("03-10"))
	if fxEntry != 0 {
		e = t.entry(fxEntry)
		t.eq("FX entry currency", t.str(e, "currency_code"), "USD")
		t.eqLines("FX gain", e, dr(l.control, "10"), cr(fx, "10"))
	}
	// A/R is settled in base: 110 + 41.1893 + 12 − 120 + 10.
	t.eqDec(t.mustFind(t.list("/api/trial-balance"), "account_id", l.control), "balance", "53.1893")

	// Unposting the payment reverses the FX entry too.
	t.must(t.admin, 200, "POST", path("/api/customer-payments/%d/unpost", pay), nil)
	t.eqDec(t.mustFind(t.list("/api/trial-balance"), "account_id", l.control), "balance", "163.1893")
	if fxEntry != 0 {
		var reversed bool
		for _, row := range t.list(path("/api/accounts/%d/ledger", fx)) {
			if t.int(row, "journal_entry_id") != fxEntry && t.dec(row, "base_debit").Cmp(mustDec("10")) == 0 {
				reversed = true
			}
		}
		if !reversed {
			t.Errorf("unposting the payment did not reverse FX entry %d", fxEntry)
		}
	}

	// Supplier side: a bill at 1.10 paid at 1.20 is a loss of 10.
	sup, sl := t.supplier()
	b := t.bill(sup, d("01-20"), "EUR", "100", sl.income)
	t.post("purchase-bills", b)
	sp := t.create("/api/supplier-payments", J{"supplier_id": sup, "payment_date": d("03-15"), "currency_code": "EUR", "amount": "100", "payment_account_id": bank})
	t.post("supplier-payments", sp)
	t.must(t.admin, 200, "POST", path("/api/supplier-payments/%d/apply", sp), nil)
	if id := t.findFXEntry(sl.control, d("03-15")); id != 0 {
		t.eqLines("FX loss", t.entry(id), cr(sl.control, "10"), dr(fx, "10"))
	}
	t.eqDec(t.mustFind(t.list("/api/trial-balance"), "account_id", sl.control), "balance", "0")

	// Cross-currency settlement is not a thing: a USD payment ignores EUR invoices.
	usd := t.create("/api/customer-payments", J{"customer_id": cust, "payment_date": d("03-20"), "currency_code": "USD", "amount": "5", "deposit_account_id": bank})
	t.post("customer-payments", usd)
	if a := t.arrOf(t.obj(t.must(t.admin, 200, "POST", path("/api/customer-payments/%d/apply", usd), nil)), "applications"); len(a) != 0 {
		t.Errorf("a USD payment was applied to EUR invoices: %v", a)
	}
}

// findFXEntry finds the base-currency entry on a control account dated on
// the settlement date: the realized-FX entry.
func (t *T) findFXEntry(control int, date string) int {
	for _, row := range t.list(path("/api/accounts/%d/ledger?from=%s&to=%s", control, date, date)) {
		if t.str(row, "currency_code") == "USD" {
			return t.int(row, "journal_entry_id")
		}
	}
	t.Errorf("no realized-FX entry on account %d dated %s", control, date)
	return 0
}

// domain §8.
func testBankReconciliation(t *T) {
	y := t.openYear()
	d := func(md string) string { return fmt.Sprintf("%d-%s", y, md) }
	bank := t.cashAccount()
	cust, l := t.customer()
	sup, _ := t.supplier()

	payIn := func(date, amount string) int {
		id := t.create("/api/customer-payments", J{"customer_id": cust, "payment_date": date, "currency_code": "USD", "amount": amount, "deposit_account_id": bank})
		t.post("customer-payments", id)
		return id
	}
	p1 := payIn(d("05-02"), "100")
	sp := t.create("/api/supplier-payments", J{"supplier_id": sup, "payment_date": d("05-03"), "currency_code": "USD", "amount": "40", "payment_account_id": bank})
	t.post("supplier-payments", sp)
	payIn(d("05-20"), "25")

	t.status(400, "POST", "/api/bank-statements", J{"account_id": bank, "statement_date": d("05-31"), "opening_balance": "0"})
	t.status(422, "POST", "/api/bank-statements", J{"account_id": l.control, "statement_date": d("05-31"), "opening_balance": "0", "closing_balance": "0"})
	st := t.create("/api/bank-statements", J{"account_id": bank, "statement_date": d("05-31"), "opening_balance": "0", "closing_balance": "85", "reference": "MAY"})
	s := t.doc("bank-statements", st)
	t.shape(s, "BankStatement")
	t.eq("status", t.str(s, "status"), "open")
	t.eq("line_count", t.int(s, "line_count"), 0)
	t.eqDec(s, "difference", "-85")

	t.status(400, "POST", path("/api/bank-statements/%d/lines", st), J{"txn_date": d("05-02"), "description": "", "amount": "1"})
	t.status(422, "POST", path("/api/bank-statements/%d/lines", st), J{"txn_date": d("05-02"), "description": "Nothing", "amount": "0"})
	line1 := t.create(path("/api/bank-statements/%d/lines", st), J{"txn_date": d("05-02"), "description": "Deposit", "amount": "100"})

	// CSV import: all-or-nothing.
	t.status(400, "POST", path("/api/bank-statements/%d/import", st), J{"csv": ""})
	for _, bad := range []string{
		"date,description,amount\n",
		fmt.Sprintf("%s,Ok,1\nnot-a-date,Bad,1\n", d("05-04")),
		fmt.Sprintf("%s,Zero,0.00\n", d("05-04")),
		fmt.Sprintf("%s,Too few\n", d("05-04")),
		fmt.Sprintf("%s,Not a number,1e3\n", d("05-04")),
	} {
		t.status(422, "POST", path("/api/bank-statements/%d/import", st), J{"csv": bad})
	}
	csv := fmt.Sprintf("date,description,amount,reference\n%s, Cheque to supplier ,-40,CHQ-1\n\n%s,Deposit,25\n", d("05-04"), d("05-21"))
	r := t.obj(t.must(t.admin, 200, "POST", path("/api/bank-statements/%d/import", st), J{"csv": csv}))
	t.eq("imported", t.int(r, "imported"), 2)
	lines := t.list(path("/api/bank-statements/%d/lines", st))
	if len(lines) != 3 {
		t.Fatalf("statement has %d lines, want 3", len(lines))
	}
	for i, ln := range lines {
		t.shape(ln, "BankStatementLine")
		t.eq("line_no", t.int(ln, "line_no"), i+1)
		t.isNull(ln, "journal_line_id")
	}
	t.eq("trimmed description", t.str(lines[1], "description"), "Cheque to supplier")
	t.eq("reference", t.str(lines[1], "reference"), "CHQ-1")
	t.eqDec(lines[1], "amount", "-40")

	cands := t.list(path("/api/bank-statements/%d/candidates", st))
	if len(cands) != 3 {
		t.Fatalf("got %d match candidates, want 3", len(cands))
	}
	for _, c := range cands {
		t.shape(c, "MatchCandidate")
	}
	byAmount := func(amount string) int {
		for _, c := range cands {
			if t.dec(c, "amount").Cmp(mustDec(amount)) == 0 {
				return t.int(c, "journal_line_id")
			}
		}
		t.Fatalf("no candidate for %s", amount)
		return 0
	}
	jl100, jl40 := byAmount("100"), byAmount("-40")

	t.status(422, "POST", path("/api/bank-statements/%d/reconcile", st), nil) // unmatched lines
	t.status(400, "POST", path("/api/bank-statement-lines/%d/match", line1), J{})
	t.status(422, "POST", path("/api/bank-statement-lines/%d/match", line1), J{"journal_line_id": jl40}) // amounts differ
	t.must(t.admin, 204, "POST", path("/api/bank-statement-lines/%d/match", line1), J{"journal_line_id": jl100})
	t.status(409, "POST", path("/api/bank-statement-lines/%d/match", line1), J{"journal_line_id": jl100})

	// A journal line backs at most one statement line, across statements.
	other := t.create("/api/bank-statements", J{"account_id": bank, "statement_date": d("06-30"), "opening_balance": "0", "closing_balance": "0"})
	dup := t.create(path("/api/bank-statements/%d/lines", other), J{"txn_date": d("05-02"), "description": "Again", "amount": "100"})
	t.status(409, "POST", path("/api/bank-statement-lines/%d/match", dup), J{"journal_line_id": jl100})
	t.must(t.admin, 204, "DELETE", path("/api/bank-statements/%d", other), nil)
	t.status(404, "GET", path("/api/bank-statements/%d", other), nil)

	r = t.obj(t.must(t.admin, 200, "POST", path("/api/bank-statements/%d/auto-match", st), nil))
	t.eq("auto-matched", t.int(r, "matched"), 2)
	lines = t.list(path("/api/bank-statements/%d/lines", st))
	t.eq("line 2 matched to the -40 line", t.int(lines[1], "journal_line_id"), jl40)
	t.notNull(lines[1], "journal_entry_id")
	if c := t.list(path("/api/bank-statements/%d/candidates", st)); len(c) != 0 {
		t.Errorf("%d candidates remain after matching everything", len(c))
	}

	// Must add up: 0 + 100 − 40 + 25 = 85.
	t.must(t.admin, 204, "PUT", path("/api/bank-statements/%d", st), J{"account_id": bank, "statement_date": d("05-31"), "opening_balance": "0", "closing_balance": "90"})
	t.status(422, "POST", path("/api/bank-statements/%d/reconcile", st), nil)
	t.must(t.admin, 204, "PUT", path("/api/bank-statements/%d", st), J{"account_id": bank, "statement_date": d("05-31"), "opening_balance": "0", "closing_balance": "85", "reference": "MAY"})
	s = t.doc("bank-statements", st)
	t.eq("matched_count", t.int(s, "matched_count"), 3)
	t.eqDec(s, "lines_total", "85")
	t.eqDec(s, "difference", "0")
	t.must(t.admin, 204, "POST", path("/api/bank-statements/%d/reconcile", st), nil)
	t.eq("reconciled", t.str(t.doc("bank-statements", st), "status"), "reconciled")

	// Reconciled statements are frozen, and so are the entries they match.
	t.status(409, "POST", path("/api/bank-statements/%d/lines", st), J{"txn_date": d("05-30"), "description": "Late", "amount": "1"})
	t.status(409, "PUT", path("/api/bank-statements/%d", st), J{"account_id": bank, "statement_date": d("05-31"), "opening_balance": "0", "closing_balance": "85"})
	t.status(409, "DELETE", path("/api/bank-statements/%d", st), nil)
	t.status(409, "POST", path("/api/bank-statements/%d/import", st), J{"csv": d("05-30") + ",Late,1\n"})
	t.status(409, "POST", path("/api/bank-statements/%d/auto-match", st), nil)
	t.status(409, "POST", path("/api/bank-statements/%d/reconcile", st), nil)
	t.status(409, "POST", path("/api/bank-statement-lines/%d/unmatch", line1), nil)
	t.status(409, "DELETE", path("/api/bank-statement-lines/%d", line1), nil)
	t.status(409, "POST", path("/api/customer-payments/%d/unpost", p1), nil)

	t.expect(t.nonAdmin(), 403, "POST", path("/api/bank-statements/%d/reopen", st), nil)
	t.must(t.admin, 204, "POST", path("/api/bank-statements/%d/reopen", st), nil)
	t.status(409, "POST", path("/api/bank-statements/%d/reopen", st), nil)
	t.eq("reopened", t.str(t.doc("bank-statements", st), "status"), "open")

	// Once released, the entry may be unposted again.
	t.must(t.admin, 204, "POST", path("/api/bank-statement-lines/%d/unmatch", line1), nil)
	t.must(t.admin, 204, "POST", path("/api/bank-statement-lines/%d/unmatch", line1), nil) // no-op
	t.must(t.admin, 200, "POST", path("/api/customer-payments/%d/unpost", p1), nil)
	t.must(t.admin, 204, "DELETE", path("/api/bank-statement-lines/%d", line1), nil)
	t.eq("line_count", t.int(t.doc("bank-statements", st), "line_count"), 2)

	t.status(404, "GET", "/api/bank-statements/999999", nil)
	t.status(404, "GET", "/api/bank-statements/999999/lines", nil)
	t.status(404, "GET", "/api/bank-statements/999999/candidates", nil)
	t.status(404, "POST", "/api/bank-statements/999999/auto-match", nil)
	t.status(404, "POST", "/api/bank-statements/999999/reconcile", nil)
	t.status(404, "POST", "/api/bank-statement-lines/999999/match", J{"journal_line_id": jl40})
	t.status(404, "DELETE", "/api/bank-statement-lines/999999", nil)

	var order []int
	for _, x := range t.list("/api/bank-statements") {
		t.shape(x, "BankStatement")
		if n := t.int(x, "id"); n == st {
			order = append(order, n)
		}
	}
	t.eq("statement listed", order, []int{st})
}

// domain §6.3: receiving a foreign-currency purchase order values the stock
// in the base currency, at the rate for the movement date.
func testForeignPurchaseReceipt(t *T) {
	y := t.openYear()
	d := func(md string) string { return fmt.Sprintf("%d-%s", y, md) }
	sup, _ := t.supplier()
	prod, inventory, _ := t.stockedProduct()
	wh := t.warehouse()
	grni := t.grni()

	po := t.create("/api/purchase-orders", J{"order_number": t.uniq("PO"), "supplier_id": sup, "order_date": d("01-05"),
		"currency_code": "JPY", "lines": []J{
			{"description": "Widgets", "product_id": prod, "quantity": "2", "unit_cost": "1234.5", "expense_account_id": grni},
		}})
	t.must(t.admin, 204, "POST", path("/api/purchase-orders/%d/confirm", po), nil)

	// No JPY rate exists on or before the date, so nothing can be valued.
	t.status(422, "POST", path("/api/purchase-orders/%d/receive", po), J{"warehouse_id": wh, "movement_date": d("01-10")})
	for _, m := range t.list("/api/stock-movements") {
		if t.int(m, "product_id") == prod {
			t.Errorf("a refused receipt created movement %d", t.int(m, "id"))
		}
	}

	t.must(t.admin, 201, "POST", "/api/exchange-rates", J{"currency_code": "JPY", "rate_date": d("01-01"), "rate": "0.006712"})
	t.must(t.admin, 201, "POST", "/api/exchange-rates", J{"currency_code": "JPY", "rate_date": d("03-01"), "rate": "0.0069"})
	// 1234.5 × 0.006712 = 8.285964, so 8.2860 in base.
	ids := t.ints(t.obj(t.must(t.admin, 201, "POST", path("/api/purchase-orders/%d/receive", po), J{"warehouse_id": wh, "movement_date": d("02-15")})), "movement_ids")
	if len(ids) != 1 {
		t.Fatalf("receiving created %d movements, want 1", len(ids))
	}
	m := t.doc("stock-movements", ids[0])
	t.eqDec(m, "unit_cost", "8.286")
	t.eqDec(m, "total_cost", "16.572")
	je := t.int(t.obj(t.must(t.admin, 200, "POST", path("/api/stock-movements/%d/post", ids[0]), J{"credit_account_id": grni})), "journal_entry_id")
	e := t.entry(je)
	t.eq("receipt entry currency", t.str(e, "currency_code"), "USD")
	t.eqLines("receipt entry", e, dr(inventory, "16.572"), cr(grni, "16.572"))
}
