package main

import (
	"fmt"
)

// stockedProduct creates an inventory-tracked product with its own inventory
// and COGS accounts.
func (t *T) stockedProduct() (id, inventory, cogs int) {
	inventory, cogs = t.account("asset"), t.account("expense")
	id = t.create("/api/products", J{"sku": t.uniq("SKU"), "name": "Stocked item", "track_inventory": true,
		"inventory_account_id": inventory, "cogs_account_id": cogs})
	return id, inventory, cogs
}

func (t *T) warehouse() int {
	return t.create("/api/warehouses", J{"code": t.uniq("WH"), "name": "Conformance warehouse"})
}

// grni is the seeded Goods Received Not Invoiced account (spec/api.md §4).
func (t *T) grni() int {
	return t.int(t.mustFind(t.list("/api/accounts"), "code", "2150"), "id")
}

// orderLine finds a sales or purchase order's line by line number.
func (t *T) orderLines(collection string, id int, shape string) []J {
	lines := t.list(path("/api/%s/%d/lines", collection, id))
	for _, l := range lines {
		t.shape(l, shape)
	}
	return lines
}

// spec/api.md §5.10, domain §6.
func testSalesOrder(t *T) {
	y := t.openYear()
	d := func(md string) string { return fmt.Sprintf("%d-%s", y, md) }
	cust, l := t.customer()
	prod, inventory, cogs := t.stockedProduct()
	wh := t.warehouse()
	// Stock on hand: 10 at 4.00, so the moving-average cost is 4.
	t.create("/api/stock-movements", J{"product_id": prod, "warehouse_id": wh, "movement_type": "receipt", "movement_date": d("01-02"), "quantity": "10", "unit_cost": "4"})

	empty := t.create("/api/sales-orders", J{"order_number": t.uniq("SO"), "customer_id": cust, "order_date": d("01-05"), "currency_code": "USD"})
	t.status(422, "POST", path("/api/sales-orders/%d/confirm", empty), nil)
	t.status(422, "POST", "/api/sales-orders", J{"order_number": t.uniq("SO"), "customer_id": cust, "order_date": d("01-05"), "currency_code": "USD",
		"lines": []J{{"description": "Nothing", "quantity": "0"}}})
	t.status(400, "POST", "/api/sales-orders", J{"order_number": "", "customer_id": cust, "order_date": d("01-05"), "currency_code": "USD"})

	number := t.uniq("SO")
	body := J{"order_number": number, "customer_id": cust, "order_date": d("01-10"), "expected_ship_date": d("01-20"),
		"currency_code": "USD", "reference": "CUST-PO-1",
		"lines": []J{
			{"description": "Installation", "quantity": "1", "unit_price": "100", "revenue_account_id": l.income},
			{"description": "Widgets", "product_id": prod, "quantity": "5", "unit_price": "20", "revenue_account_id": l.income},
		}}
	so := t.create("/api/sales-orders", body)
	t.status(409, "POST", "/api/sales-orders", body)
	o := t.doc("sales-orders", so)
	t.shape(o, "SalesOrder")
	t.eq("status", t.str(o, "status"), "draft")
	t.eq("invoiced_status", t.str(o, "invoiced_status"), "none")
	t.eq("shipped_status", t.str(o, "shipped_status"), "none")
	t.eqDec(o, "total", "200")

	// Drafts are editable; the widget line grows to 6.
	body["lines"].([]J)[1]["quantity"] = "6"
	t.must(t.admin, 204, "PUT", path("/api/sales-orders/%d", so), body)
	t.eqDec(t.doc("sales-orders", so), "total", "220")
	lines := t.orderLines("sales-orders", so, "SalesOrderLine")
	if len(lines) != 2 {
		t.Fatalf("order has %d lines, want 2", len(lines))
	}
	service, widgets := t.int(lines[0], "order_line_id"), t.int(lines[1], "order_line_id")
	t.eqDec(lines[1], "qty_to_invoice", "6")
	t.eqDec(lines[1], "qty_to_ship", "6")
	t.eqDec(lines[0], "qty_to_ship", "0") // a service is never shipped

	// Fulfilment needs an open order.
	t.status(409, "POST", path("/api/sales-orders/%d/invoice", so), J{"invoice_number": t.uniq("INV"), "invoice_date": d("01-11")})
	t.status(409, "POST", path("/api/sales-orders/%d/ship", so), J{"warehouse_id": wh})
	t.status(409, "POST", path("/api/sales-orders/%d/close", so), nil)

	t.must(t.admin, 204, "POST", path("/api/sales-orders/%d/confirm", so), nil)
	t.eq("confirmed", t.str(t.doc("sales-orders", so), "status"), "open")
	t.status(409, "POST", path("/api/sales-orders/%d/confirm", so), nil)
	t.status(409, "PUT", path("/api/sales-orders/%d", so), body)
	t.status(409, "DELETE", path("/api/sales-orders/%d", so), nil)

	// Partial invoice: 2 widgets.
	t.status(400, "POST", path("/api/sales-orders/%d/invoice", so), J{"invoice_number": "", "invoice_date": d("01-11")})
	r := t.obj(t.must(t.admin, 201, "POST", path("/api/sales-orders/%d/invoice", so), J{
		"invoice_number": t.uniq("INV"), "invoice_date": d("01-11"),
		"lines": []J{{"order_line_id": widgets, "quantity": "2"}},
	}))
	partial := t.int(r, "invoice_id")
	inv := t.doc("sales-invoices", partial)
	t.eq("invoice customer", t.int(inv, "customer_id"), cust)
	t.eq("invoice currency", t.str(inv, "currency_code"), "USD")
	t.eq("invoice reference", t.str(inv, "reference"), number)
	t.eq("invoice status", t.str(inv, "status"), "draft")
	t.eqDec(inv, "total", "40")
	il := t.list(path("/api/sales-invoices/%d/lines", partial))
	if len(il) == 1 {
		t.eq("invoice line order link", t.int(il[0], "order_line_id"), widgets)
		t.eqDec(il[0], "quantity", "2")
		t.eqDec(il[0], "unit_price", "20")
		t.eq("revenue account copied", t.int(il[0], "revenue_account_id"), l.income)
	} else {
		t.Errorf("partial invoice has %d lines, want 1", len(il))
	}
	t.eq("invoiced_status", t.str(t.doc("sales-orders", so), "invoiced_status"), "partial")
	t.eqDec(t.mustFind(t.orderLines("sales-orders", so, "SalesOrderLine"), "order_line_id", widgets), "qty_to_invoice", "4")

	// An order-linked invoice cannot be edited, but deleting it returns the quantity.
	t.status(409, "PUT", path("/api/sales-invoices/%d", partial), J{"invoice_number": t.uniq("INV"), "customer_id": cust, "invoice_date": d("01-11"), "currency_code": "USD"})
	t.must(t.admin, 204, "DELETE", path("/api/sales-invoices/%d", partial), nil)
	t.eq("invoiced_status after delete", t.str(t.doc("sales-orders", so), "invoiced_status"), "none")

	// Requested quantities are capped at what remains: the service and 6 widgets.
	r = t.obj(t.must(t.admin, 201, "POST", path("/api/sales-orders/%d/invoice", so), J{"invoice_number": t.uniq("INV"), "invoice_date": d("01-12"),
		"lines": []J{{"order_line_id": widgets, "quantity": "99"}, {"order_line_id": service, "quantity": "1"}}}))
	full := t.int(r, "invoice_id")
	t.eqDec(t.doc("sales-invoices", full), "total", "220")
	t.eq("invoiced_status", t.str(t.doc("sales-orders", so), "invoiced_status"), "invoiced")
	t.status(422, "POST", path("/api/sales-orders/%d/invoice", so), J{"invoice_number": t.uniq("INV"), "invoice_date": d("01-12")})
	t.post("sales-invoices", full)
	t.status(409, "POST", path("/api/sales-orders/%d/cancel", so), nil) // fulfilled orders cannot be cancelled

	// Ship from the warehouse at its moving-average cost; the service line is skipped.
	t.status(400, "POST", path("/api/sales-orders/%d/ship", so), J{})
	ids := t.ints(t.obj(t.must(t.admin, 201, "POST", path("/api/sales-orders/%d/ship", so), J{"warehouse_id": wh, "movement_date": d("01-15")})), "movement_ids")
	if len(ids) != 1 {
		t.Fatalf("shipping created %d movements, want 1", len(ids))
	}
	m := t.doc("stock-movements", ids[0])
	t.shape(m, "StockMovement")
	t.eq("movement type", t.str(m, "movement_type"), "issue")
	t.eq("movement date", t.str(m, "movement_date"), d("01-15"))
	t.eq("movement status", t.str(m, "status"), "draft")
	t.eq("movement source", t.str(m, "source_type"), "sales_order_line")
	t.eqDec(m, "quantity", "-6")
	t.eqDec(m, "unit_cost", "4")
	t.eqDec(m, "total_cost", "-24")
	t.isNull(m, "journal_entry_id")
	t.eq("shipped_status", t.str(t.doc("sales-orders", so), "shipped_status"), "shipped")
	t.status(422, "POST", path("/api/sales-orders/%d/ship", so), J{"warehouse_id": wh})

	// Fulfilment movements cannot be edited, but may be deleted while unposted.
	t.status(409, "PUT", path("/api/stock-movements/%d", ids[0]), J{"product_id": prod, "warehouse_id": wh, "movement_type": "issue", "quantity": "-1"})
	t.must(t.admin, 204, "DELETE", path("/api/stock-movements/%d", ids[0]), nil)
	t.eq("shipped_status after delete", t.str(t.doc("sales-orders", so), "shipped_status"), "none")
	ids = t.ints(t.obj(t.must(t.admin, 201, "POST", path("/api/sales-orders/%d/ship", so), J{"warehouse_id": wh, "movement_date": d("01-16"),
		"lines": []J{{"order_line_id": widgets, "quantity": "2"}}})), "movement_ids")
	t.eq("partial shipment", t.str(t.doc("sales-orders", so), "shipped_status"), "partial")
	je := t.post("stock-movements", ids[0])
	t.eqLines("issue entry", t.entry(je), dr(cogs, "8"), cr(inventory, "8"))

	// Close is manual and final.
	t.must(t.admin, 204, "POST", path("/api/sales-orders/%d/close", so), nil)
	t.eq("closed", t.str(t.doc("sales-orders", so), "status"), "closed")
	t.status(409, "POST", path("/api/sales-orders/%d/close", so), nil)
	t.status(409, "POST", path("/api/sales-orders/%d/cancel", so), nil)
	t.status(409, "POST", path("/api/sales-orders/%d/ship", so), J{"warehouse_id": wh})

	// Cancellation: a draft always, an open order while unfulfilled.
	t.must(t.admin, 204, "POST", path("/api/sales-orders/%d/cancel", empty), nil)
	t.eq("cancelled", t.str(t.doc("sales-orders", empty), "status"), "cancelled")
	t.status(409, "POST", path("/api/sales-orders/%d/confirm", empty), nil)
	open := t.create("/api/sales-orders", J{"order_number": t.uniq("SO"), "customer_id": cust, "order_date": d("02-01"), "currency_code": "USD",
		"lines": []J{{"description": "x", "unit_price": "1"}}})
	t.must(t.admin, 204, "POST", path("/api/sales-orders/%d/confirm", open), nil)
	t.must(t.admin, 204, "POST", path("/api/sales-orders/%d/cancel", open), nil)
	draft := t.create("/api/sales-orders", J{"order_number": t.uniq("SO"), "customer_id": cust, "order_date": d("02-02"), "currency_code": "USD"})
	t.must(t.admin, 204, "DELETE", path("/api/sales-orders/%d", draft), nil)

	t.status(404, "POST", "/api/sales-orders/999999/confirm", nil)
	t.status(404, "POST", "/api/sales-orders/999999/invoice", J{"invoice_number": "x", "invoice_date": d("01-01")})
	t.status(404, "POST", "/api/sales-orders/999999/ship", J{"warehouse_id": wh})
	t.status(404, "GET", "/api/sales-orders/999999/lines", nil)

	var order []int
	for _, x := range t.list("/api/sales-orders") {
		t.shape(x, "SalesOrder")
		if n := t.int(x, "id"); n == so || n == open {
			order = append(order, n)
		}
	}
	t.eq("sales order list order", order, []int{open, so})
}

// The purchasing mirror, through the GRNI cycle: receive and post at the
// order's cost against GRNI, then bill against GRNI so it nets to zero.
func testPurchaseOrder(t *T) {
	y := t.openYear()
	d := func(md string) string { return fmt.Sprintf("%d-%s", y, md) }
	sup, l := t.supplier()
	prod, inventory, _ := t.stockedProduct()
	wh := t.warehouse()
	grni := t.grni()

	number := t.uniq("PO")
	po := t.create("/api/purchase-orders", J{"order_number": number, "supplier_id": sup, "order_date": d("03-01"), "expected_receipt_date": d("03-10"),
		"currency_code": "USD", "lines": []J{
			{"description": "Widgets", "product_id": prod, "quantity": "4", "unit_cost": "10", "expense_account_id": grni},
		}})
	o := t.doc("purchase-orders", po)
	t.shape(o, "PurchaseOrder")
	t.eqDec(o, "total", "40")
	t.must(t.admin, 204, "POST", path("/api/purchase-orders/%d/confirm", po), nil)
	line := t.int(t.orderLines("purchase-orders", po, "PurchaseOrderLine")[0], "order_line_id")

	ids := t.ints(t.obj(t.must(t.admin, 201, "POST", path("/api/purchase-orders/%d/receive", po), J{"warehouse_id": wh, "movement_date": d("03-05"),
		"lines": []J{{"order_line_id": line, "quantity": "1"}}})), "movement_ids")
	t.eq("received_status", t.str(t.doc("purchase-orders", po), "received_status"), "partial")
	ids = append(ids, t.ints(t.obj(t.must(t.admin, 201, "POST", path("/api/purchase-orders/%d/receive", po), J{"warehouse_id": wh, "movement_date": d("03-06")})), "movement_ids")...)
	if len(ids) != 2 {
		t.Fatalf("receiving created %d movements, want 2", len(ids))
	}
	t.eq("received_status", t.str(t.doc("purchase-orders", po), "received_status"), "received")
	t.status(422, "POST", path("/api/purchase-orders/%d/receive", po), J{"warehouse_id": wh})
	m := t.doc("stock-movements", ids[1])
	t.eq("receipt type", t.str(m, "movement_type"), "receipt")
	t.eq("receipt source", t.str(m, "source_type"), "purchase_order_line")
	t.eqDec(m, "quantity", "3")
	t.eqDec(m, "unit_cost", "10")
	for _, id := range ids {
		t.must(t.admin, 200, "POST", path("/api/stock-movements/%d/post", id), J{"credit_account_id": grni})
	}

	r := t.obj(t.must(t.admin, 201, "POST", path("/api/purchase-orders/%d/bill", po), J{"bill_number": t.uniq("BILL"), "bill_date": d("03-20")}))
	b := t.int(r, "bill_id")
	t.eq("bill reference", t.str(t.doc("purchase-bills", b), "reference"), number)
	bl := t.list(path("/api/purchase-bills/%d/lines", b))
	if len(bl) == 1 {
		t.eq("bill line order link", t.int(bl[0], "order_line_id"), line)
	}
	t.eq("billed_status", t.str(t.doc("purchase-orders", po), "billed_status"), "billed")
	t.status(422, "POST", path("/api/purchase-orders/%d/bill", po), J{"bill_number": t.uniq("BILL"), "bill_date": d("03-21")})
	t.eqLines("bill against GRNI", t.entry(t.post("purchase-bills", b)), dr(grni, "40"), cr(l.control, "40"))

	t.eqDec(t.mustFind(t.list(path("/api/accounts/%d/ledger?from=%s&to=%s", inventory, d("01-01"), d("12-31"))), "entry_date", d("03-06")), "debit", "30")
	t.must(t.admin, 204, "POST", path("/api/purchase-orders/%d/close", po), nil)
	t.eq("closed", t.str(t.doc("purchase-orders", po), "status"), "closed")
}

// spec/api.md §5.12, domain §2 and §4.
func testStockMovements(t *T) {
	y := t.openYear()
	d := func(md string) string { return fmt.Sprintf("%d-%s", y, md) }
	prod, inventory, cogs := t.stockedProduct()
	wh := t.warehouse()
	grni := t.grni()
	untracked := t.create("/api/products", J{"sku": t.uniq("SKU"), "name": "Service"})
	mv := func(typ, qty, cost string) J {
		return J{"product_id": prod, "warehouse_id": wh, "movement_type": typ, "movement_date": d("02-01"), "quantity": qty, "unit_cost": cost}
	}

	t.status(400, "POST", "/api/stock-movements", J{"warehouse_id": wh, "movement_type": "receipt", "quantity": "1"})
	t.status(400, "POST", "/api/stock-movements", J{"product_id": prod, "warehouse_id": wh, "movement_type": "receipt"})
	t.status(422, "POST", "/api/stock-movements", mv("receipt", "-1", "5"))
	t.status(422, "POST", "/api/stock-movements", mv("issue", "1", "5"))
	t.status(422, "POST", "/api/stock-movements", mv("adjustment", "0", "5"))
	t.status(422, "POST", "/api/stock-movements", mv("teleport", "1", "5"))
	t.status(422, "POST", "/api/stock-movements", mv("receipt", "1", "-5"))
	t.status(422, "POST", "/api/stock-movements", J{"product_id": untracked, "warehouse_id": wh, "movement_type": "receipt", "quantity": "1"})

	receipt := t.create("/api/stock-movements", mv("receipt", "10", "5"))
	m := t.doc("stock-movements", receipt)
	t.shape(m, "StockMovement")
	t.eqDec(m, "total_cost", "50")
	t.isNull(m, "source_type")
	t.isNull(m, "journal_entry_id")
	t.create("/api/stock-movements", mv("adjustment", "-1", "5"))
	t.create("/api/stock-movements", mv("adjustment", "2", "5"))
	issue := t.create("/api/stock-movements", mv("issue", "-4", "5"))

	val := t.mustFind(t.list("/api/inventory-valuation"), "product_id", prod)
	t.shape(val, "ValuationRow")
	t.eqDec(val, "qty_on_hand", "7")
	t.eqDec(val, "value_on_hand", "35")
	t.eqDec(val, "avg_unit_cost", "5")

	// Unposted movements are editable.
	t.must(t.admin, 204, "PUT", path("/api/stock-movements/%d", receipt), mv("receipt", "12", "5"))
	t.eqDec(t.doc("stock-movements", receipt), "total_cost", "60")
	t.eqDec(t.mustFind(t.list("/api/inventory-valuation"), "product_id", prod), "qty_on_hand", "9")

	// Posting: receipts need a postable credit account.
	t.status(422, "POST", path("/api/stock-movements/%d/post", receipt), nil)
	summary := t.create("/api/accounts", J{"code": t.uniq("A"), "name": "Header", "account_type": "liability"})
	t.status(422, "POST", path("/api/stock-movements/%d/post", receipt), J{"credit_account_id": summary})
	je := t.int(t.obj(t.must(t.admin, 200, "POST", path("/api/stock-movements/%d/post", receipt), J{"credit_account_id": grni})), "journal_entry_id")
	t.eqLines("receipt entry", t.entry(je), dr(inventory, "60"), cr(grni, "60"))
	m = t.doc("stock-movements", receipt)
	t.eq("posted movement", t.int(m, "journal_entry_id"), je)
	t.eq("posted status", t.str(m, "status"), "posted")
	t.status(409, "POST", path("/api/stock-movements/%d/post", receipt), J{"credit_account_id": grni})
	t.status(409, "PUT", path("/api/stock-movements/%d", receipt), mv("receipt", "1", "5"))
	t.status(409, "DELETE", path("/api/stock-movements/%d", receipt), nil)

	adj := t.create("/api/stock-movements", mv("adjustment", "1", "5"))
	t.status(422, "POST", path("/api/stock-movements/%d/post", adj), nil)
	je = t.post("stock-movements", issue)
	t.eqLines("issue entry", t.entry(je), dr(cogs, "20"), cr(inventory, "20"))

	// Issues need the product's COGS account; a zero-cost movement has nothing to post.
	bare := t.create("/api/products", J{"sku": t.uniq("SKU"), "name": "No accounts", "track_inventory": true})
	noCogs := t.create("/api/stock-movements", J{"product_id": bare, "warehouse_id": wh, "movement_type": "issue", "movement_date": d("02-02"), "quantity": "-1", "unit_cost": "3"})
	t.status(422, "POST", path("/api/stock-movements/%d/post", noCogs), nil)
	free := t.create("/api/stock-movements", mv("issue", "-1", "0"))
	t.status(422, "POST", path("/api/stock-movements/%d/post", free), nil)

	// Unpost (admin) unlinks the entry and leaves the quantity record.
	t.expect(t.nonAdmin(), 403, "POST", path("/api/stock-movements/%d/unpost", receipt), nil)
	rev := t.int(t.obj(t.must(t.admin, 200, "POST", path("/api/stock-movements/%d/unpost", receipt), nil)), "reversal_entry_id")
	t.eqLines("receipt reversal", t.entry(rev), cr(inventory, "60"), dr(grni, "60"))
	m = t.doc("stock-movements", receipt)
	t.isNull(m, "journal_entry_id")
	t.eqDec(m, "quantity", "12")
	t.status(409, "POST", path("/api/stock-movements/%d/unpost", receipt), nil)
	t.must(t.admin, 204, "DELETE", path("/api/stock-movements/%d", receipt), nil)
	t.status(404, "GET", path("/api/stock-movements/%d", receipt), nil)
	t.status(404, "POST", "/api/stock-movements/999999/post", nil)

	// Newest first.
	later := t.create("/api/stock-movements", J{"product_id": prod, "warehouse_id": wh, "movement_type": "receipt", "movement_date": d("03-01"), "quantity": "1", "unit_cost": "5"})
	all := t.list("/api/stock-movements")
	var order []int
	for _, x := range all {
		if n := t.int(x, "id"); n == later || n == issue {
			order = append(order, n)
		}
	}
	t.eq("movement list order", order, []int{later, issue})
}

// ints reads an array-of-integers field.
func (t *T) ints(o J, k string) []int {
	v, ok := t.field(o, k)
	if !ok {
		return nil
	}
	a, isArr := v.([]any)
	if !isArr {
		t.Errorf("field %q is not an array", k)
		return nil
	}
	var out []int
	for _, e := range a {
		out = append(out, t.int(J{"v": e}, "v"))
	}
	return out
}
