package main

import (
	"fmt"
	"time"
)

// domain §2: request decimals round to their stored scale before anything
// uses them, values out of range are refused, and every rounded quotient is
// exact. The extreme values are there because they are where an inexact
// division (rounding the quotient once, then again to 4 places) gives a
// different answer.
func testDecimalArithmetic(t *T) {
	y := t.openYear()
	d := func(md string) string { return fmt.Sprintf("%d-%s", y, md) }
	cust, l := t.customer()
	line := func(qty, price, rate string) J {
		return J{"invoice_number": t.uniq("INV"), "customer_id": cust, "invoice_date": d("01-10"), "currency_code": "USD",
			"lines": []J{{"description": "x", "quantity": qty, "unit_price": price, "tax_rate": rate, "revenue_account_id": l.income}}}
	}
	firstLine := func(id int) J {
		lines := t.list(path("/api/sales-invoices/%d/lines", id))
		if len(lines) != 1 {
			t.Fatalf("invoice %d has %d lines, want 1", id, len(lines))
		}
		return lines[0]
	}

	// Inputs round half away from zero to 4 places (tax rates too), and the
	// line arithmetic uses the rounded values.
	ln := firstLine(t.create("/api/sales-invoices", line("1.00005", "10.00005", "7.12345")))
	t.eqDec(ln, "quantity", "1.0001")
	t.eqDec(ln, "unit_price", "10.0001")
	t.eqDec(ln, "tax_rate", "7.1235")
	t.eqDec(ln, "line_subtotal", "10.0011") // 10.00110001
	t.eqDec(ln, "tax_amount", "0.7124")     // 0.712528356...
	t.eqDec(firstLine(t.create("/api/sales-invoices", line("-2.00005", "1", "0"))), "quantity", "-2.0001")
	// A quantity that rounds to zero is zero.
	t.status(422, "POST", "/api/sales-invoices", line("0.00004", "1", "0"))

	// Tax is exact: 100.0003 × 1000000055833.3325 × 0.0001 / 100 is
	// 100000305.58334999999975, so 100000305.5833, not ...5834.
	ln = firstLine(t.create("/api/sales-invoices", line("100.0003", "1000000055833.3325", "0.0001")))
	t.eqDec(ln, "line_subtotal", "100000305583350")
	t.eqDec(ln, "tax_amount", "100000305.5833")

	// Out of range: amounts below 10^15, tax rates below 1000.
	t.status(422, "POST", "/api/sales-invoices", line("1", "1000000000000000", "0"))
	t.status(422, "POST", "/api/sales-invoices", line("10", "999999999999999", "0"))
	t.status(422, "POST", "/api/sales-invoices", line("1", "1", "1000"))

	// Exchange rates keep 8 places and stay below 10^11.
	t.must(t.admin, 201, "POST", "/api/exchange-rates", J{"currency_code": "EUR", "rate_date": d("01-01"), "rate": "1.123456785"})
	t.eqDec(t.mustFind(t.list("/api/exchange-rates"), "rate_date", d("01-01")), "rate", "1.12345679")
	t.status(422, "POST", "/api/exchange-rates", J{"currency_code": "EUR", "rate_date": d("01-02"), "rate": "100000000000"})

	// Average cost is exact: 920907399.1189 / 123456789.0123 is
	// 7.45934999999999995..., so 7.4593, not 7.4594.
	wh := t.warehouse()
	prod, _, _ := t.stockedProduct()
	receipt := func(prod int, qty, cost string) {
		t.create("/api/stock-movements", J{"product_id": prod, "warehouse_id": wh, "movement_type": "receipt",
			"movement_date": d("02-01"), "quantity": qty, "unit_cost": cost})
	}
	receipt(prod, "123456788.0123", "0")
	receipt(prod, "1", "920907399.1189")
	t.eqDec(t.mustFind(t.list("/api/inventory-valuation"), "product_id", prod), "avg_unit_cost", "7.4593")
	// A tie rounds away from zero: 0.0001 / 2.
	tie, _, _ := t.stockedProduct()
	receipt(tie, "1", "0.0001")
	receipt(tie, "1", "0")
	t.eqDec(t.mustFind(t.list("/api/inventory-valuation"), "product_id", tie), "avg_unit_cost", "0.0001")
}

// spec/api.md §1.2: "today" is the current UTC date.
func testTodayIsUTC(t *T) {
	prod, _, _ := t.stockedProduct()
	wh := t.warehouse()
	before := time.Now().UTC().Format("2006-01-02")
	id := t.create("/api/stock-movements", J{"product_id": prod, "warehouse_id": wh, "movement_type": "receipt", "quantity": "1", "unit_cost": "1"})
	after := time.Now().UTC().Format("2006-01-02")
	if got := t.str(t.doc("stock-movements", id), "movement_date"); got != before && got != after {
		t.Errorf("default movement_date = %s, want the UTC date %s", got, after)
	}
	t.must(t.admin, 204, "DELETE", path("/api/stock-movements/%d", id), nil)
}
