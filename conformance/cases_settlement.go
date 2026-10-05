package main

import (
	"fmt"
)

// spec/api.md §5.9 payments, domain §5.
func testCustomerPayments(t *T) {
	y := t.openYear()
	cust, l := t.customer()
	bank := t.cashAccount()
	d := func(md string) string { return fmt.Sprintf("%d-%s", y, md) }

	inv1 := t.invoice(cust, d("01-10"), "USD", "100", l.income)
	inv2 := t.invoice(cust, d("02-10"), "USD", "80", l.income)
	inv3 := t.invoice(cust, d("03-10"), "USD", "50", l.income)
	eur := t.invoice(cust, d("01-05"), "EUR", "999", l.income) // other currency: never touched
	for _, id := range []int{inv1, inv2, inv3} {
		t.post("sales-invoices", id)
	}
	_ = eur // left as a draft; a draft is never open for application either

	t.status(400, "POST", "/api/customer-payments", J{"customer_id": cust, "payment_date": d("04-01"), "currency_code": "USD"})
	t.status(422, "POST", "/api/customer-payments", J{"customer_id": cust, "payment_date": d("04-01"), "currency_code": "USD", "amount": "0"})
	t.status(422, "POST", "/api/customer-payments", J{"customer_id": cust, "payment_date": d("04-01"), "currency_code": "USD", "amount": "5", "method": "barter"})

	// A payment without a deposit account cannot post.
	noBank := t.create("/api/customer-payments", J{"customer_id": cust, "payment_date": d("04-01"), "currency_code": "USD", "amount": "5"})
	t.status(422, "POST", path("/api/customer-payments/%d/post", noBank), nil)
	t.must(t.admin, 204, "DELETE", path("/api/customer-payments/%d", noBank), nil)

	pay := t.create("/api/customer-payments", J{
		"customer_id": cust, "payment_date": d("04-01"), "currency_code": "USD", "amount": "150",
		"method": "transfer", "reference": "WIRE-1", "deposit_account_id": bank,
	})
	p := t.doc("customer-payments", pay)
	t.shape(p, "CustomerPayment")
	t.eq("status", t.str(p, "status"), "draft")
	t.eqDec(p, "amount", "150")
	t.eqDec(p, "unapplied", "150")
	t.eq("deposit_account_id", t.int(p, "deposit_account_id"), bank)
	t.eq("customer_id", t.int(p, "customer_id"), cust)
	t.eq("payment_date", t.str(p, "payment_date"), d("04-01"))
	t.eq("method", t.str(p, "method"), "transfer")
	t.status(409, "POST", path("/api/customer-payments/%d/apply", pay), nil) // must be posted first
	t.must(t.admin, 204, "PUT", path("/api/customer-payments/%d", pay), J{
		"customer_id": cust, "payment_date": d("04-01"), "currency_code": "USD", "amount": "150",
		"method": "check", "reference": "CHK-9", "deposit_account_id": bank,
	})

	je := t.post("customer-payments", pay)
	t.eqLines("payment entry", t.entry(je), dr(bank, "150"), cr(l.control, "150"))
	t.status(409, "PUT", path("/api/customer-payments/%d", pay), J{"customer_id": cust, "payment_date": d("04-01"), "currency_code": "USD", "amount": "1"})

	// Auto-apply: oldest first, never over-applying.
	r := t.obj(t.must(t.admin, 200, "POST", path("/api/customer-payments/%d/apply", pay), nil))
	apps := t.arrOf(r, "applications")
	if len(apps) != 2 {
		t.Fatalf("apply created %d applications, want 2: %s", len(apps), brief(r))
	}
	t.eq("first application", t.int(apps[0], "document_id"), inv1)
	t.eqDec(apps[0], "amount_applied", "100")
	t.eq("second application", t.int(apps[1], "document_id"), inv2)
	t.eqDec(apps[1], "amount_applied", "50")

	i1, i2, i3 := t.doc("sales-invoices", inv1), t.doc("sales-invoices", inv2), t.doc("sales-invoices", inv3)
	t.eq("inv1 status", t.str(i1, "payment_status"), "paid")
	t.eqDec(i1, "balance", "0")
	t.eq("inv2 status", t.str(i2, "payment_status"), "partial")
	t.eqDec(i2, "amount_applied", "50")
	t.eqDec(i2, "balance", "30")
	t.eq("inv3 status", t.str(i3, "payment_status"), "unpaid")
	p = t.doc("customer-payments", pay)
	t.eqDec(p, "amount_applied", "150")
	t.eqDec(p, "unapplied", "0")

	list := t.list(path("/api/customer-payments/%d/applications", pay))
	if len(list) == 2 {
		t.shape(list[0], "Application")
		t.eq("application document", t.int(list[0], "document_id"), inv1)
		t.eq("application number", t.str(list[0], "document_number"), t.str(i1, "invoice_number"))
		t.eqDec(list[1], "amount_applied", "50")
	} else {
		t.Errorf("applications list has %d rows, want 2", len(list))
	}
	t.status(404, "GET", "/api/customer-payments/999999/applications", nil)

	// Nothing left: applying again is a no-op.
	r = t.obj(t.must(t.admin, 200, "POST", path("/api/customer-payments/%d/apply", pay), nil))
	if a := t.arrOf(r, "applications"); len(a) != 0 {
		t.Errorf("re-applying created %d applications", len(a))
	}

	// An invoice with applications cannot be unposted.
	t.status(409, "POST", path("/api/sales-invoices/%d/unpost", inv1), nil)

	// A second payment picks up where the first left off.
	pay2 := t.create("/api/customer-payments", J{"customer_id": cust, "payment_date": d("04-15"), "currency_code": "USD", "amount": "100", "deposit_account_id": bank})
	t.post("customer-payments", pay2)
	apps = t.arrOf(t.obj(t.must(t.admin, 200, "POST", path("/api/customer-payments/%d/apply", pay2), nil)), "applications")
	if len(apps) == 2 {
		t.eq("tops up inv2", t.int(apps[0], "document_id"), inv2)
		t.eqDec(apps[0], "amount_applied", "30")
		t.eq("then inv3", t.int(apps[1], "document_id"), inv3)
		t.eqDec(apps[1], "amount_applied", "50")
	} else {
		t.Errorf("second apply created %d applications, want 2", len(apps))
	}
	t.eqDec(t.doc("customer-payments", pay2), "unapplied", "20")

	// Unposting a payment deletes its applications and reopens the invoices.
	t.expect(t.nonAdmin(), 403, "POST", path("/api/customer-payments/%d/unpost", pay), nil)
	rev := t.int(t.obj(t.must(t.admin, 200, "POST", path("/api/customer-payments/%d/unpost", pay), nil)), "reversal_entry_id")
	t.eqLines("payment reversal", t.entry(rev), cr(bank, "150"), dr(l.control, "150"))
	p = t.doc("customer-payments", pay)
	t.eq("unposted", t.str(p, "status"), "draft")
	t.eqDec(p, "amount_applied", "0")
	t.isNull(p, "journal_entry_id")
	if a := t.list(path("/api/customer-payments/%d/applications", pay)); len(a) != 0 {
		t.Errorf("unposted payment still has %d applications", len(a))
	}
	t.eq("inv1 reopened", t.str(t.doc("sales-invoices", inv1), "payment_status"), "unpaid")
	t.eqDec(t.doc("sales-invoices", inv2), "balance", "50")

	// Now inv1 has no applications and may be unposted.
	t.must(t.admin, 200, "POST", path("/api/sales-invoices/%d/unpost", inv1), nil)
	t.must(t.admin, 204, "DELETE", path("/api/customer-payments/%d", pay), nil)
	t.status(404, "GET", path("/api/customer-payments/%d", pay), nil)

	var order []int
	for _, x := range t.list("/api/customer-payments") {
		t.shape(x, "CustomerPayment")
		if n := t.int(x, "id"); n == pay2 {
			order = append(order, n)
		}
	}
	t.eq("payment listed", order, []int{pay2})
}

// The supplier-side mirror of customer payments.
func testSupplierPayments(t *T) {
	y := t.openYear()
	sup, l := t.supplier()
	bank := t.cashAccount()
	d := func(md string) string { return fmt.Sprintf("%d-%s", y, md) }

	b1 := t.bill(sup, d("01-10"), "USD", "60", l.income)
	b2 := t.bill(sup, d("02-10"), "USD", "60", l.income)
	t.post("purchase-bills", b1)
	t.post("purchase-bills", b2)

	t.status(400, "POST", "/api/supplier-payments", J{"supplier_id": 0, "payment_date": d("03-01"), "currency_code": "USD", "amount": "1"})
	pay := t.create("/api/supplier-payments", J{"supplier_id": sup, "payment_date": d("03-01"), "currency_code": "USD", "amount": "90", "payment_account_id": bank})
	p := t.doc("supplier-payments", pay)
	t.shape(p, "SupplierPayment")
	t.eq("payment_account_id", t.int(p, "payment_account_id"), bank)
	je := t.post("supplier-payments", pay)
	t.eqLines("supplier payment entry", t.entry(je), dr(l.control, "90"), cr(bank, "90"))

	apps := t.arrOf(t.obj(t.must(t.admin, 200, "POST", path("/api/supplier-payments/%d/apply", pay), nil)), "applications")
	if len(apps) != 2 {
		t.Fatalf("apply created %d applications, want 2", len(apps))
	}
	t.eq("oldest bill first", t.int(apps[0], "document_id"), b1)
	t.eqDec(apps[0], "amount_applied", "60")
	t.eqDec(apps[1], "amount_applied", "30")
	t.eq("b1 paid", t.str(t.doc("purchase-bills", b1), "payment_status"), "paid")
	t.eq("b2 partial", t.str(t.doc("purchase-bills", b2), "payment_status"), "partial")
	t.status(409, "POST", path("/api/purchase-bills/%d/unpost", b1), nil)

	list := t.list(path("/api/supplier-payments/%d/applications", pay))
	if len(list) != 2 {
		t.Errorf("applications list has %d rows, want 2", len(list))
	}
	t.must(t.admin, 200, "POST", path("/api/supplier-payments/%d/unpost", pay), nil)
	t.eq("b1 reopened", t.str(t.doc("purchase-bills", b1), "payment_status"), "unpaid")
}

// spec/api.md §5.9 credit notes, domain §4.3 and §5.
func testCreditNotes(t *T) {
	y := t.openYear()
	cust, l := t.customer()
	tax := t.taxCode(l.tax, "10")
	d := func(md string) string { return fmt.Sprintf("%d-%s", y, md) }

	inv := t.create("/api/sales-invoices", J{"invoice_number": t.uniq("INV"), "customer_id": cust, "invoice_date": d("01-10"), "currency_code": "USD",
		"lines": []J{{"description": "Goods", "quantity": "1", "unit_price": "200", "revenue_account_id": l.income, "tax_code": tax, "tax_rate": "10"}}})
	t.post("sales-invoices", inv)

	number := t.uniq("CN")
	cnBody := J{"credit_note_number": number, "customer_id": cust, "credit_note_date": d("02-01"), "currency_code": "USD", "reference": "Returned",
		"lines": []J{{"description": "Return", "quantity": "1", "unit_price": "50", "revenue_account_id": l.income, "tax_code": tax, "tax_rate": "10"}}}
	t.status(400, "POST", "/api/sales-credit-notes", J{"credit_note_number": "", "customer_id": cust, "credit_note_date": d("02-01"), "currency_code": "USD"})
	cn := t.create("/api/sales-credit-notes", cnBody)
	t.status(409, "POST", "/api/sales-credit-notes", cnBody)

	c := t.doc("sales-credit-notes", cn)
	t.shape(c, "SalesCreditNote")
	t.eqDec(c, "total", "55")
	t.eq("credit_note_number", t.str(c, "credit_note_number"), number)
	t.eq("application status", t.str(c, "application_status"), "open")
	lines := t.list(path("/api/sales-credit-notes/%d/lines", cn))
	if len(lines) == 1 {
		t.shape(lines[0], "InvoiceLine")
		t.isNull(lines[0], "order_line_id")
	}
	t.status(409, "POST", path("/api/sales-credit-notes/%d/apply", cn), nil) // still a draft

	// Dr revenue 50, Dr tax 5, Cr A/R 55.
	je := t.post("sales-credit-notes", cn)
	t.eqLines("credit note entry", t.entry(je), dr(l.income, "50"), dr(l.tax, "5"), cr(l.control, "55"))

	apps := t.arrOf(t.obj(t.must(t.admin, 200, "POST", path("/api/sales-credit-notes/%d/apply", cn), nil)), "applications")
	if len(apps) != 1 {
		t.Fatalf("apply created %d applications, want 1", len(apps))
	}
	t.eq("applied to", t.int(apps[0], "document_id"), inv)
	t.eqDec(apps[0], "amount_applied", "55")
	c = t.doc("sales-credit-notes", cn)
	t.eq("fully applied", t.str(c, "application_status"), "applied")
	t.eqDec(c, "balance", "0")
	i := t.doc("sales-invoices", inv)
	t.eq("invoice partially settled", t.str(i, "payment_status"), "partial")
	t.eqDec(i, "balance", "165")
	if a := t.list(path("/api/sales-credit-notes/%d/applications", cn)); len(a) == 1 {
		t.shape(a[0], "Application")
	} else {
		t.Errorf("credit note applications list has %d rows, want 1", len(a))
	}

	// Payments and credit notes share the invoice's availability.
	bank := t.cashAccount()
	pay := t.create("/api/customer-payments", J{"customer_id": cust, "payment_date": d("03-01"), "currency_code": "USD", "amount": "500", "deposit_account_id": bank})
	t.post("customer-payments", pay)
	apps = t.arrOf(t.obj(t.must(t.admin, 200, "POST", path("/api/customer-payments/%d/apply", pay), nil)), "applications")
	if len(apps) == 1 {
		t.eqDec(apps[0], "amount_applied", "165")
	} else {
		t.Errorf("payment apply created %d applications, want 1", len(apps))
	}
	t.eq("invoice paid", t.str(t.doc("sales-invoices", inv), "payment_status"), "paid")
	t.eqDec(t.doc("customer-payments", pay), "unapplied", "335")

	// An applied credit note, and the invoice it settles, cannot be unposted.
	t.status(409, "POST", path("/api/sales-credit-notes/%d/unpost", cn), nil)
	t.status(409, "POST", path("/api/sales-invoices/%d/unpost", inv), nil)

	// An unapplied credit note unposts cleanly.
	cn2 := t.create("/api/sales-credit-notes", J{"credit_note_number": t.uniq("CN"), "customer_id": cust, "credit_note_date": d("04-01"), "currency_code": "USD",
		"lines": []J{{"description": "Goodwill", "unit_price": "10", "revenue_account_id": l.income}}})
	t.post("sales-credit-notes", cn2)
	rev := t.int(t.obj(t.must(t.admin, 200, "POST", path("/api/sales-credit-notes/%d/unpost", cn2), nil)), "reversal_entry_id")
	t.eqLines("credit note reversal", t.entry(rev), cr(l.income, "10"), dr(l.control, "10"))
	t.must(t.admin, 204, "DELETE", path("/api/sales-credit-notes/%d", cn2), nil)

	// Supplier credit: Dr A/P, Cr expense and input tax; applied to a bill.
	sup, sl := t.supplier()
	stax := t.taxCode(sl.tax, "10")
	b := t.create("/api/purchase-bills", J{"bill_number": t.uniq("BILL"), "supplier_id": sup, "bill_date": d("01-20"), "currency_code": "USD",
		"lines": []J{{"description": "Parts", "unit_cost": "100", "expense_account_id": sl.income, "tax_code": stax, "tax_rate": "10"}}})
	t.post("purchase-bills", b)
	scnNumber := t.uniq("SCN")
	scn := t.create("/api/purchase-credit-notes", J{"credit_note_number": scnNumber, "supplier_id": sup, "credit_note_date": d("02-20"), "currency_code": "USD",
		"lines": []J{{"description": "Damaged", "unit_cost": "20", "expense_account_id": sl.income, "tax_code": stax, "tax_rate": "10"}}})
	sup2, _ := t.supplier()
	t.create("/api/purchase-credit-notes", J{"credit_note_number": scnNumber, "supplier_id": sup2, "credit_note_date": d("02-20"), "currency_code": "USD"})
	lines = t.list(path("/api/purchase-credit-notes/%d/lines", scn))
	if len(lines) == 1 {
		t.shape(lines[0], "BillLine")
	}
	je = t.post("purchase-credit-notes", scn)
	t.eqLines("supplier credit entry", t.entry(je), dr(sl.control, "22"), cr(sl.income, "20"), cr(sl.tax, "2"))
	apps = t.arrOf(t.obj(t.must(t.admin, 200, "POST", path("/api/purchase-credit-notes/%d/apply", scn), nil)), "applications")
	if len(apps) == 1 {
		t.eq("applied to bill", t.int(apps[0], "document_id"), b)
		t.eqDec(apps[0], "amount_applied", "22")
	} else {
		t.Errorf("supplier credit apply created %d applications, want 1", len(apps))
	}
	t.eqDec(t.doc("purchase-bills", b), "balance", "88")
	scnRead := t.doc("purchase-credit-notes", scn)
	t.shape(scnRead, "PurchaseCreditNote")
	t.eq("supplier credit applied", t.str(scnRead, "application_status"), "applied")
	t.status(409, "POST", path("/api/purchase-credit-notes/%d/unpost", scn), nil)
	if a := t.list(path("/api/purchase-credit-notes/%d/applications", scn)); len(a) != 1 {
		t.Errorf("supplier credit applications list has %d rows, want 1", len(a))
	}
}
