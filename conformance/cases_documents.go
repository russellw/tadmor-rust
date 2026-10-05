package main

import (
	"fmt"
)

// spec/api.md §5.9, domain §2 and §4.
func testInvoiceLifecycle(t *T) {
	y := t.openYear()
	cust, l := t.customer()
	tax := t.taxCode(l.tax, "8.25")
	date := fmt.Sprintf("%d-03-15", y)
	due := fmt.Sprintf("%d-04-14", y)
	number := t.uniq("INV")

	body := J{
		"invoice_number": number, "customer_id": cust, "invoice_date": date, "due_date": due,
		"currency_code": "USD", "reference": "PO-77", "memo": "Thanks",
		"lines": []J{
			{"description": "Consulting", "quantity": "2", "unit_price": "10.005", "revenue_account_id": l.income},
			{"description": "Licence", "quantity": "3", "unit_price": "7.333", "revenue_account_id": l.income, "tax_code": tax, "tax_rate": "8.25"},
		},
	}
	// Validation.
	bad := func(k string, v any) J {
		b := J{}
		for kk, vv := range body {
			b[kk] = vv
		}
		b[k] = v
		return b
	}
	t.status(400, "POST", "/api/sales-invoices", bad("invoice_number", ""))
	t.status(400, "POST", "/api/sales-invoices", bad("customer_id", 0))
	t.status(400, "POST", "/api/sales-invoices", bad("invoice_date", ""))
	t.status(400, "POST", "/api/sales-invoices", bad("currency_code", ""))
	t.status(400, "POST", "/api/sales-invoices", bad("lines", []J{{"description": "", "quantity": "1"}}))
	t.status(422, "POST", "/api/sales-invoices", bad("customer_id", 999999))
	t.status(422, "POST", "/api/sales-invoices", bad("lines", []J{{"description": "Zero", "quantity": "0"}}))
	t.status(422, "POST", "/api/sales-invoices", bad("due_date", fmt.Sprintf("%d-03-01", y)))
	t.status(422, "POST", "/api/sales-invoices", bad("currency_code", "ZZZ"))

	id := t.create("/api/sales-invoices", body)
	t.status(409, "POST", "/api/sales-invoices", body) // number is unique

	// Line money (domain §2): 2 × 10.005 = 20.01; 3 × 7.333 = 21.999, tax
	// round(21.999 × 8.25%, 4) = 1.8149.
	inv := t.doc("sales-invoices", id)
	t.shape(inv, "SalesInvoice")
	t.eq("invoice_number", t.str(inv, "invoice_number"), number)
	t.eq("customer_id", t.int(inv, "customer_id"), cust)
	t.eq("invoice_date", t.str(inv, "invoice_date"), date)
	t.eq("due_date", t.str(inv, "due_date"), due)
	t.eq("status", t.str(inv, "status"), "draft")
	t.eq("payment_status", t.str(inv, "payment_status"), "unpaid")
	t.eq("reference", t.str(inv, "reference"), "PO-77")
	t.eq("memo", t.str(inv, "memo"), "Thanks")
	t.eqDec(inv, "total", "43.8239")
	t.eqDec(inv, "amount_applied", "0")
	t.eqDec(inv, "balance", "43.8239")
	t.isNull(inv, "journal_entry_id")

	lines := t.list(path("/api/sales-invoices/%d/lines", id))
	if len(lines) != 2 {
		t.Fatalf("got %d lines, want 2", len(lines))
	}
	for i, ln := range lines {
		t.shape(ln, "InvoiceLine")
		t.eq("line_no", t.int(ln, "line_no"), i+1)
		t.isNull(ln, "order_line_id")
	}
	t.eqDec(lines[0], "line_subtotal", "20.01")
	t.eqDec(lines[0], "tax_amount", "0")
	t.eqDec(lines[1], "line_subtotal", "21.999")
	t.eqDec(lines[1], "tax_amount", "1.8149")
	t.eqDec(lines[1], "line_total", "23.8139")
	t.eqDec(lines[1], "tax_rate", "8.25")
	t.eq("tax_code", t.str(lines[1], "tax_code"), tax)

	// Draft edits replace the header and the whole line set.
	body["lines"] = []J{
		{"description": "Consulting", "quantity": "4", "unit_price": "25", "revenue_account_id": l.income},
		{"description": "Licence", "quantity": "1", "unit_price": "100", "revenue_account_id": l.income, "tax_code": tax, "tax_rate": "10"},
	}
	body["memo"] = nil
	t.must(t.admin, 204, "PUT", path("/api/sales-invoices/%d", id), body)
	inv = t.doc("sales-invoices", id)
	t.eqDec(inv, "total", "210")
	t.isNull(inv, "memo")
	t.status(400, "PUT", path("/api/sales-invoices/%d", id), bad("invoice_number", ""))
	t.status(404, "PUT", "/api/sales-invoices/999999", body)
	t.status(404, "GET", "/api/sales-invoices/999999", nil)
	t.status(404, "GET", "/api/sales-invoices/999999/lines", nil)

	// Posting: Dr A/R 210, Cr revenue 200, Cr tax 10. The fiscal year has no
	// periods, so posting creates the month's period (domain §9.2).
	je := t.post("sales-invoices", id)
	e := t.entry(je)
	t.eq("entry date", t.str(e, "entry_date"), date)
	t.eq("entry currency", t.str(e, "currency_code"), "USD")
	t.eqDec(e, "exchange_rate", "1")
	t.eq("entry status", t.str(e, "status"), "posted")
	t.eq("entry reference", t.str(e, "reference"), number)
	t.eqLines("invoice entry", e, dr(l.control, "210"), cr(l.income, "200"), cr(l.tax, "10"))

	month := fmt.Sprintf("%d-03", y)
	p := find(t.list("/api/accounting-periods"), "name", month)
	if p == nil {
		t.Errorf("posting did not create the %s period", month)
	} else {
		t.eq("auto period start", t.str(p, "start_date"), month+"-01")
		t.eq("auto period end", t.str(p, "end_date"), month+"-31")
		t.eq("auto period status", t.str(p, "status"), "open")
	}

	inv = t.doc("sales-invoices", id)
	t.eq("posted status", t.str(inv, "status"), "posted")
	t.eq("journal_entry_id", t.int(inv, "journal_entry_id"), je)
	t.status(409, "POST", path("/api/sales-invoices/%d/post", id), nil)
	t.status(409, "PUT", path("/api/sales-invoices/%d", id), body)
	t.status(409, "DELETE", path("/api/sales-invoices/%d", id), nil)

	ledger := t.list(path("/api/accounts/%d/ledger", l.control))
	if len(ledger) != 1 {
		t.Fatalf("A/R ledger has %d rows, want 1", len(ledger))
	}
	t.shape(ledger[0], "LedgerRow")
	t.eq("ledger entry", t.int(ledger[0], "journal_entry_id"), je)
	t.eqDec(ledger[0], "debit", "210")
	t.eqDec(ledger[0], "base_debit", "210")
	t.eq("ledger currency", t.str(ledger[0], "currency_code"), "USD")

	// Unpost (admin): a mirror reversal; the invoice returns to draft.
	t.expect(t.nonAdmin(), 403, "POST", path("/api/sales-invoices/%d/unpost", id), nil)
	rev := t.int(t.obj(t.must(t.admin, 200, "POST", path("/api/sales-invoices/%d/unpost", id), nil)), "reversal_entry_id")
	re := t.entry(rev)
	t.eq("reversal date", t.str(re, "entry_date"), date)
	t.eqLines("reversal entry", re, cr(l.control, "210"), dr(l.income, "200"), dr(l.tax, "10"))
	t.eq("original still posted", t.str(t.entry(je), "status"), "posted")
	inv = t.doc("sales-invoices", id)
	t.eq("unposted status", t.str(inv, "status"), "draft")
	t.isNull(inv, "journal_entry_id")
	t.status(409, "POST", path("/api/sales-invoices/%d/unpost", id), nil)
	t.status(404, "POST", "/api/sales-invoices/999999/unpost", nil)

	// A draft may be edited again and re-posted under a fresh entry.
	t.must(t.admin, 204, "PUT", path("/api/sales-invoices/%d", id), body)
	if je2 := t.post("sales-invoices", id); je2 == je || je2 == rev {
		t.Errorf("re-posting reused journal entry %d", je2)
	}
	tb := t.list("/api/trial-balance")
	ar := t.mustFind(tb, "account_id", l.control)
	t.eqDec(ar, "total_debit", "420")
	t.eqDec(ar, "total_credit", "210")
	t.eqDec(ar, "balance", "210")

	// Drafts can be deleted.
	draft := t.invoice(cust, fmt.Sprintf("%d-05-01", y), "USD", "1", l.income)
	t.must(t.admin, 204, "DELETE", path("/api/sales-invoices/%d", draft), nil)
	t.status(404, "GET", path("/api/sales-invoices/%d", draft), nil)
	t.status(404, "DELETE", path("/api/sales-invoices/%d", draft), nil)

	// The list is newest first.
	later := t.invoice(cust, fmt.Sprintf("%d-06-01", y), "USD", "1", l.income)
	earlier := t.invoice(cust, fmt.Sprintf("%d-01-10", y), "USD", "1", l.income)
	var order []int
	for _, d := range t.list("/api/sales-invoices") {
		t.shape(d, "SalesInvoice")
		switch n := t.int(d, "id"); n {
		case id, later, earlier:
			order = append(order, n)
		}
	}
	t.eq("invoice list order", order, []int{later, id, earlier})
}

// domain §4.2: each posting refusal, on a fresh draft.
func testPostingRefusals(t *T) {
	y := t.openYear()
	date := fmt.Sprintf("%d-02-10", y)
	cust, l := t.customer()
	postStatus := func(want int, id int) {
		t.status(want, "POST", path("/api/sales-invoices/%d/post", id), nil)
	}
	mk := func(lines []J, customer int, d string) int {
		return t.create("/api/sales-invoices", J{"invoice_number": t.uniq("INV"), "customer_id": customer,
			"invoice_date": d, "currency_code": "USD", "lines": lines})
	}

	postStatus(404, 999999)
	t.status(400, "POST", "/api/sales-invoices/0/post", nil)
	t.status(400, "POST", "/api/sales-invoices/abc/post", nil)

	// Nothing to post: no lines, or a negative total.
	postStatus(422, mk(nil, cust, date))
	postStatus(422, mk([]J{{"description": "Refund", "quantity": "-1", "unit_price": "5", "revenue_account_id": l.income}}, cust, date))

	// Missing accounts.
	noAR := t.create("/api/customers", J{"organization_id": t.org(nil)})
	postStatus(422, mk([]J{{"description": "x", "unit_price": "5", "revenue_account_id": l.income}}, noAR, date))
	postStatus(422, mk([]J{{"description": "No revenue account", "unit_price": "5"}}, cust, date))
	postStatus(422, mk([]J{{"description": "Taxed, but ZERO has no tax account", "unit_price": "5", "revenue_account_id": l.income, "tax_code": "ZERO", "tax_rate": "5"}}, cust, date))

	// A product supplies the revenue account a line omits.
	prod := t.create("/api/products", J{"sku": t.uniq("SKU"), "name": "Fallback", "revenue_account_id": l.income})
	id := mk([]J{{"description": "Via product", "product_id": prod, "unit_price": "5"}}, cust, date)
	t.eqLines("product fallback", t.entry(t.post("sales-invoices", id)), dr(l.control, "5"), cr(l.income, "5"))

	// No open period: outside every fiscal year, or in a closed period.
	postStatus(422, mk([]J{{"description": "x", "unit_price": "5", "revenue_account_id": l.income}}, cust, fmt.Sprintf("%d-02-10", t.nextYear())))
	fy := t.mustFind(t.list("/api/fiscal-years"), "start_date", fmt.Sprintf("%d-01-01", y))
	closed := t.create("/api/accounting-periods", J{"fiscal_year_id": t.int(fy, "id"), "name": "Sep", "start_date": fmt.Sprintf("%d-09-01", y), "end_date": fmt.Sprintf("%d-09-30", y)})
	t.status(204, "PUT", path("/api/accounting-periods/%d", closed), J{"fiscal_year_id": t.int(fy, "id"), "name": "Sep", "start_date": fmt.Sprintf("%d-09-01", y), "end_date": fmt.Sprintf("%d-09-30", y), "status": "closed"})
	postStatus(422, mk([]J{{"description": "x", "unit_price": "5", "revenue_account_id": l.income}}, cust, fmt.Sprintf("%d-09-15", y)))

	// Unposting into a period closed since posting is refused too.
	inSep := mk([]J{{"description": "x", "unit_price": "5", "revenue_account_id": l.income}}, cust, fmt.Sprintf("%d-10-15", y))
	t.post("sales-invoices", inSep)
	oct := t.mustFind(t.list("/api/accounting-periods"), "name", fmt.Sprintf("%d-10", y))
	oct["status"] = "closed"
	t.status(204, "PUT", path("/api/accounting-periods/%d", t.int(oct, "id")), oct)
	t.status(422, "POST", path("/api/sales-invoices/%d/unpost", inSep), nil)
}

// spec/api.md §5.9 for the purchasing mirror.
func testBillLifecycle(t *T) {
	y := t.openYear()
	sup, l := t.supplier()
	tax := t.taxCode(l.tax, "20")
	inventory := t.account("asset")
	prod := t.create("/api/products", J{"sku": t.uniq("SKU"), "name": "Stocked", "inventory_account_id": inventory})
	number := t.uniq("BILL")
	body := J{
		"bill_number": number, "supplier_id": sup, "bill_date": fmt.Sprintf("%d-04-02", y),
		"due_date": fmt.Sprintf("%d-05-02", y), "currency_code": "USD",
		"lines": []J{
			{"description": "Office supplies", "quantity": "2", "unit_cost": "50", "expense_account_id": l.income, "tax_code": tax, "tax_rate": "20"},
			{"description": "Stock", "quantity": "10", "unit_cost": "3", "product_id": prod},
		},
	}
	t.status(400, "POST", "/api/purchase-bills", J{"bill_number": "", "supplier_id": sup, "bill_date": "2101-01-01", "currency_code": "USD"})
	id := t.create("/api/purchase-bills", body)
	t.status(409, "POST", "/api/purchase-bills", body) // same supplier, same number

	// Bill numbers are the supplier's own: another supplier may reuse one.
	sup2, _ := t.supplier()
	other := J{}
	for k, v := range body {
		other[k] = v
	}
	other["supplier_id"] = sup2
	t.create("/api/purchase-bills", other)

	b := t.doc("purchase-bills", id)
	t.shape(b, "PurchaseBill")
	t.eqDec(b, "total", "150") // 100 + 20 tax + 30
	lines := t.list(path("/api/purchase-bills/%d/lines", id))
	for _, ln := range lines {
		t.shape(ln, "BillLine")
	}
	t.eqDec(lines[0], "unit_cost", "50")
	t.eqDec(lines[0], "tax_amount", "20")

	// Dr expense 100 + Dr inventory (product fallback) 30 + Dr input tax 20; Cr A/P 150.
	je := t.post("purchase-bills", id)
	t.eqLines("bill entry", t.entry(je), dr(l.income, "100"), dr(inventory, "30"), dr(l.tax, "20"), cr(l.control, "150"))
	t.eq("posted", t.str(t.doc("purchase-bills", id), "status"), "posted")
	t.status(409, "PUT", path("/api/purchase-bills/%d", id), body)

	rev := t.int(t.obj(t.must(t.admin, 200, "POST", path("/api/purchase-bills/%d/unpost", id), nil)), "reversal_entry_id")
	t.eqLines("bill reversal", t.entry(rev), cr(l.income, "100"), cr(inventory, "30"), cr(l.tax, "20"), dr(l.control, "150"))
	t.must(t.admin, 204, "DELETE", path("/api/purchase-bills/%d", id), nil)
	t.status(404, "GET", path("/api/purchase-bills/%d/lines", id), nil)
}

// domain §4.3: an account whose lines net negative posts on the opposite side,
// in base currency too (domain §7.2).
func testNegativeNetLines(t *T) {
	y := t.openYear()
	t.must(t.admin, 201, "POST", "/api/exchange-rates", J{"currency_code": "GBP", "rate_date": fmt.Sprintf("%d-01-01", y), "rate": "1.123456"})
	cust, l := t.customer()
	discount := t.account("revenue")
	inv := t.create("/api/sales-invoices", J{"invoice_number": t.uniq("INV"), "customer_id": cust, "invoice_date": fmt.Sprintf("%d-03-01", y), "currency_code": "GBP",
		"lines": []J{
			{"description": "Goods", "unit_price": "100", "revenue_account_id": l.income},
			{"description": "Discount", "unit_price": "-10", "revenue_account_id": discount},
		}})
	// 100 × 1.123456 = 112.3456 and 10 × 1.123456 = 11.2346; A/R takes their net.
	t.eqLines("discounted invoice", t.entry(t.post("sales-invoices", inv)),
		dr(l.control, "90").base("101.111"), cr(l.income, "100").base("112.3456"), dr(discount, "10").base("11.2346"))

	sup, sl := t.supplier()
	rebate := t.account("expense")
	b := t.create("/api/purchase-bills", J{"bill_number": t.uniq("BILL"), "supplier_id": sup, "bill_date": fmt.Sprintf("%d-03-02", y), "currency_code": "USD",
		"lines": []J{
			{"description": "Materials", "unit_cost": "50", "expense_account_id": sl.income},
			{"description": "Rebate", "unit_cost": "-5", "expense_account_id": rebate},
		}})
	t.eqLines("bill with rebate", t.entry(t.post("purchase-bills", b)), dr(sl.income, "50"), cr(rebate, "5"), cr(sl.control, "45"))
}
