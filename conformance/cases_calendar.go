package main

import (
	"fmt"
)

// spec/api.md §5.7, domain §9.1.
func testFiscalYearsPeriods(t *T) {
	y := t.nextYear()
	name := t.uniq("FY")
	t.status(400, "POST", "/api/fiscal-years", J{"name": name, "start_date": "", "end_date": "2101-12-31"})
	t.status(422, "POST", "/api/fiscal-years", J{"name": name, "start_date": fmt.Sprintf("%d-12-31", y), "end_date": fmt.Sprintf("%d-01-01", y)})
	fy := t.create("/api/fiscal-years", J{"name": name, "start_date": fmt.Sprintf("%d-01-01", y), "end_date": fmt.Sprintf("%d-12-31", y)})
	t.status(409, "POST", "/api/fiscal-years", J{"name": name, "start_date": "2999-01-01", "end_date": "2999-12-31"})

	f := t.get(path("/api/fiscal-years/%d", fy))
	t.shape(f, "FiscalYear")
	t.eq("status", t.str(f, "status"), "open")
	t.eq("start_date", t.str(f, "start_date"), fmt.Sprintf("%d-01-01", y))

	// Status belongs to close/reopen, not to plain edits.
	t.status(204, "PUT", path("/api/fiscal-years/%d", fy), J{"name": name + "-renamed", "start_date": fmt.Sprintf("%d-01-01", y), "end_date": fmt.Sprintf("%d-12-31", y), "status": "closed"})
	f = t.get(path("/api/fiscal-years/%d", fy))
	t.eq("renamed", t.str(f, "name"), name+"-renamed")
	t.eq("status after PUT", t.str(f, "status"), "open")
	t.status(404, "PUT", "/api/fiscal-years/999999", J{"name": "x", "start_date": "2999-01-01", "end_date": "2999-12-31"})

	period := func(n, start, end string) J {
		return J{"fiscal_year_id": fy, "name": n, "start_date": fmt.Sprintf("%d-%s", y, start), "end_date": fmt.Sprintf("%d-%s", y, end)}
	}
	t.status(400, "POST", "/api/accounting-periods", J{"name": "Q1", "start_date": "2101-01-01", "end_date": "2101-03-31"})
	q1 := t.create("/api/accounting-periods", period("Q1", "01-01", "03-31"))
	p := t.get(path("/api/accounting-periods/%d", q1))
	t.shape(p, "AccountingPeriod")
	t.eq("period status", t.str(p, "status"), "open")
	t.eq("period fiscal year", t.int(p, "fiscal_year_id"), fy)
	t.status(409, "POST", "/api/accounting-periods", period("Q1", "04-01", "06-30"))      // name taken in this year
	t.status(422, "POST", "/api/accounting-periods", period("Overlap", "03-31", "04-30")) // shares 03-31
	t.status(422, "POST", "/api/accounting-periods", J{"fiscal_year_id": 999999, "name": "X", "start_date": "2998-01-01", "end_date": "2998-01-31"})

	// Periods never overlap, even across fiscal years.
	other := t.create("/api/fiscal-years", J{"name": t.uniq("FY"), "start_date": fmt.Sprintf("%d-01-01", y), "end_date": fmt.Sprintf("%d-12-31", y)})
	t.status(422, "POST", "/api/accounting-periods", J{"fiscal_year_id": other, "name": "Feb", "start_date": fmt.Sprintf("%d-02-01", y), "end_date": fmt.Sprintf("%d-02-28", y)})

	// Closing and reopening a period is an edit.
	closed := period("Q1", "01-01", "03-31")
	closed["status"] = "closed"
	t.status(204, "PUT", path("/api/accounting-periods/%d", q1), closed)
	t.eq("closed", t.str(t.get(path("/api/accounting-periods/%d", q1)), "status"), "closed")
	t.status(204, "PUT", path("/api/accounting-periods/%d", q1), period("Q1", "01-01", "03-31"))
	t.eq("reopened (status defaults to open)", t.str(t.get(path("/api/accounting-periods/%d", q1)), "status"), "open")
	t.status(404, "PUT", "/api/accounting-periods/999999", period("X", "07-01", "07-31"))

	periods := t.list("/api/accounting-periods")
	for i := 1; i < len(periods); i++ {
		if t.str(periods[i-1], "start_date") > t.str(periods[i], "start_date") {
			t.Errorf("periods are not ordered by start date")
		}
	}
	years := t.list("/api/fiscal-years")
	for i := 1; i < len(years); i++ {
		if t.str(years[i-1], "start_date") > t.str(years[i], "start_date") {
			t.Errorf("fiscal years are not ordered by start date")
		}
	}
}

// spec/api.md §5.8. Uses CAD on dates far from every other case, and deletes
// what it creates, so no posting elsewhere ever picks these rates up.
func testExchangeRates(t *T) {
	t.status(400, "POST", "/api/exchange-rates", J{"currency_code": "", "rate_date": "1901-01-01", "rate": "1.3"})
	t.status(422, "POST", "/api/exchange-rates", J{"currency_code": "CA", "rate_date": "1901-01-01", "rate": "1.3"})
	t.status(400, "POST", "/api/exchange-rates", J{"currency_code": "CAD", "rate_date": "", "rate": "1.3"})
	t.status(400, "POST", "/api/exchange-rates", J{"currency_code": "CAD", "rate_date": "1901-01-01", "rate": ""})
	t.status(422, "POST", "/api/exchange-rates", J{"currency_code": "ZZZ", "rate_date": "1901-01-01", "rate": "1.3"})
	t.status(422, "POST", "/api/exchange-rates", J{"currency_code": "CAD", "rate_date": "1901-01-01", "rate": "0"})

	r := t.must(t.admin, 201, "POST", "/api/exchange-rates", J{"currency_code": "CAD", "rate_date": "1901-01-01", "rate": "1.30"})
	k := t.obj(r)
	t.eq("created currency", t.str(k, "currency_code"), "CAD")
	t.eq("created date", t.str(k, "rate_date"), "1901-01-01")
	t.status(409, "POST", "/api/exchange-rates", J{"currency_code": "CAD", "rate_date": "1901-01-01", "rate": "1.4"})
	t.must(t.admin, 201, "POST", "/api/exchange-rates", J{"currency_code": "CAD", "rate_date": "1901-01-02", "rate": "1.31"})

	rates := t.list("/api/exchange-rates")
	var cad []J
	for _, x := range rates {
		t.shape(x, "ExchangeRate")
		if x["currency_code"] == "CAD" {
			cad = append(cad, x)
		}
	}
	if len(cad) != 2 {
		t.Fatalf("want 2 CAD rates, got %d", len(cad))
	}
	t.eq("newest first", t.str(cad[0], "rate_date"), "1901-01-02")
	t.eqDec(cad[1], "rate", "1.3")

	t.status(204, "PUT", "/api/exchange-rates/CAD/1901-01-01", J{"rate": "1.325"})
	t.eqDec(t.mustFind(t.list("/api/exchange-rates"), "rate_date", "1901-01-01"), "rate", "1.325")
	t.status(404, "PUT", "/api/exchange-rates/CAD/1901-02-01", J{"rate": "1.5"})
	t.status(204, "DELETE", "/api/exchange-rates/CAD/1901-01-01", nil)
	t.status(204, "DELETE", "/api/exchange-rates/CAD/1901-01-02", nil)

	// The created key is echoed as stored: currency codes are upper-cased.
	k = t.obj(t.must(t.admin, 201, "POST", "/api/exchange-rates", J{"currency_code": "cad", "rate_date": "1901-01-03", "rate": "1.3"}))
	t.eq("echoed currency", t.str(k, "currency_code"), "CAD")
	t.status(204, "DELETE", "/api/exchange-rates/CAD/1901-01-03", nil)
	t.status(404, "DELETE", "/api/exchange-rates/CAD/1901-01-01", nil)
}

// spec/api.md §5.8 settings. The base currency itself is exercised in the
// foreign-currency case, once entries exist.
func testLedgerSettings(t *T) {
	s := t.get("/api/settings")
	base, fx := t.str(s, "base_currency"), t.int(s, "fx_gain_loss_account_id")
	summary := t.create("/api/accounts", J{"code": t.uniq("A"), "name": "Header", "account_type": "expense"})

	t.status(400, "PUT", "/api/settings", J{"base_currency": "", "fx_gain_loss_account_id": fx})
	t.status(422, "PUT", "/api/settings", J{"base_currency": "US", "fx_gain_loss_account_id": fx})
	t.status(422, "PUT", "/api/settings", J{"base_currency": base, "fx_gain_loss_account_id": summary})
	t.status(422, "PUT", "/api/settings", J{"base_currency": base, "fx_gain_loss_account_id": 999999})

	other := t.account("expense")
	t.status(204, "PUT", "/api/settings", J{"base_currency": base, "fx_gain_loss_account_id": other})
	t.eq("fx account", t.int(t.get("/api/settings"), "fx_gain_loss_account_id"), other)
	t.status(204, "PUT", "/api/settings", J{"base_currency": base, "fx_gain_loss_account_id": fx})
	t.eq("fx account restored", t.int(t.get("/api/settings"), "fx_gain_loss_account_id"), fx)
}

// domain §9.3. Runs in the years 2001-2003, earlier than every other case's
// fiscal year, because closing requires every earlier year to be closed.
func testYearEnd(t *T) {
	cust, cl := t.customer()
	sup, sl := t.supplier()
	re := t.account("equity")
	fy1 := t.create("/api/fiscal-years", J{"name": t.uniq("Y1"), "start_date": "2001-01-01", "end_date": "2001-12-31"})

	inv := t.invoice(cust, "2001-06-15", "USD", "1000", cl.income)
	t.post("sales-invoices", inv)
	b := t.bill(sup, "2001-07-10", "USD", "400", sl.income)
	t.post("purchase-bills", b)

	// Refusals before anything changes.
	t.status(422, "POST", path("/api/fiscal-years/%d/close", fy1), J{"retained_earnings_account_id": t.account("asset")})
	headerEquity := t.create("/api/accounts", J{"code": t.uniq("A"), "name": "Equity header", "account_type": "equity"})
	t.status(422, "POST", path("/api/fiscal-years/%d/close", fy1), J{"retained_earnings_account_id": headerEquity})
	t.status(400, "POST", path("/api/fiscal-years/%d/close", fy1), J{})
	t.status(404, "POST", "/api/fiscal-years/999999/close", J{"retained_earnings_account_id": re})
	t.status(409, "POST", path("/api/fiscal-years/%d/reopen", fy1), nil)

	// Close 2001: sweep 1000 revenue and 400 expense into retained earnings.
	r := t.obj(t.must(t.admin, 200, "POST", path("/api/fiscal-years/%d/close", fy1), J{"retained_earnings_account_id": re}))
	closing := t.intPtr(r, "closing_entry_id")
	next := t.intPtr(r, "next_fiscal_year_id")
	if closing == nil || next == nil {
		t.Fatalf("close returned %v, want both a closing entry and a new fiscal year", brief(r))
	}
	e := t.entry(*closing)
	t.eq("closing entry date", t.str(e, "entry_date"), "2001-12-31")
	t.eq("closing entry currency", t.str(e, "currency_code"), "USD")
	t.eqLines("closing entry", e, dr(cl.income, "1000"), cr(sl.income, "400"), cr(re, "600"))

	fy2 := *next
	n := t.get(path("/api/fiscal-years/%d", fy2))
	t.eq("rolled-forward name", t.str(n, "name"), "FY2002")
	t.eq("rolled-forward start", t.str(n, "start_date"), "2002-01-01")
	t.eq("rolled-forward end", t.str(n, "end_date"), "2002-12-31")
	t.eq("rolled-forward status", t.str(n, "status"), "open")

	t.eq("closed year", t.str(t.get(path("/api/fiscal-years/%d", fy1)), "status"), "closed")
	for _, p := range t.list("/api/accounting-periods") {
		if t.int(p, "fiscal_year_id") == fy1 && t.str(p, "status") != "closed" {
			t.Errorf("period %s of the closed year is %s", t.str(p, "name"), t.str(p, "status"))
		}
	}
	if p := find(t.list("/api/accounting-periods"), "fiscal_year_id", fy1); p != nil {
		reopened := J{"fiscal_year_id": fy1, "name": t.str(p, "name"), "start_date": t.str(p, "start_date"), "end_date": t.str(p, "end_date"), "status": "open"}
		t.status(422, "PUT", path("/api/accounting-periods/%d", t.int(p, "id")), reopened)
	}
	t.status(422, "POST", "/api/sales-invoices/"+itoa(t.invoice(cust, "2001-08-01", "USD", "5", cl.income))+"/post", nil)
	t.status(409, "POST", path("/api/fiscal-years/%d/close", fy1), J{"retained_earnings_account_id": re})

	// The income statement ignores the closing entry; the balance sheet does not.
	pl := t.list("/api/profit-and-loss?from=2001-01-01&to=2001-12-31")
	t.eqDec(t.mustFind(pl, "account_id", cl.income), "amount", "1000")
	t.eqDec(t.mustFind(pl, "account_id", sl.income), "amount", "400")
	bs := t.get("/api/balance-sheet?as_of=2001-12-31")
	rows := t.arrOf(bs, "rows")
	t.eqDec(t.mustFind(rows, "account_id", re), "amount", "600")
	t.eqDec(bs, "current_earnings", "0")

	// Close the empty 2002: nothing to sweep, and 2003 is created.
	r = t.obj(t.must(t.admin, 200, "POST", path("/api/fiscal-years/%d/close", fy2), J{"retained_earnings_account_id": re}))
	t.isNull(r, "closing_entry_id")
	fy3 := t.intPtr(r, "next_fiscal_year_id")
	if fy3 == nil {
		t.Fatalf("closing 2002 created no 2003")
	}
	t.eq("2003 name", t.str(t.get(path("/api/fiscal-years/%d", *fy3)), "name"), "FY2003")

	// Reopen newest first.
	t.status(422, "POST", path("/api/fiscal-years/%d/reopen", fy1), nil)
	r = t.obj(t.must(t.admin, 200, "POST", path("/api/fiscal-years/%d/reopen", fy2), nil))
	t.isNull(r, "reversal_entry_id")
	// Reopen 2001 too; then 2002 cannot close ahead of it.
	r = t.obj(t.must(t.admin, 200, "POST", path("/api/fiscal-years/%d/reopen", fy1), nil))
	rev := t.intPtr(r, "reversal_entry_id")
	if rev == nil {
		t.Fatalf("reopening 2001 reversed nothing")
	}
	t.eqLines("closing reversal", t.entry(*rev), cr(cl.income, "1000"), dr(sl.income, "400"), dr(re, "600"))
	t.eq("reopened year", t.str(t.get(path("/api/fiscal-years/%d", fy1)), "status"), "open")
	t.status(422, "POST", path("/api/fiscal-years/%d/close", fy2), J{"retained_earnings_account_id": re})

	pl = t.list("/api/profit-and-loss?from=2001-01-01&to=2001-12-31")
	t.eqDec(t.mustFind(pl, "account_id", cl.income), "amount", "1000")
	bs = t.get("/api/balance-sheet?as_of=2001-12-31")
	t.eqDec(t.mustFind(t.arrOf(bs, "rows"), "account_id", re), "amount", "0")
	t.eqDec(bs, "current_earnings", "600")

	// The reopened period accepts postings again.
	t.post("sales-invoices", t.invoice(cust, "2001-12-20", "USD", "5", cl.income))
}

// arrOf reads an array-of-objects field.
func (t *T) arrOf(o J, k string) []J {
	v, ok := t.field(o, k)
	if !ok {
		return nil
	}
	a, isArr := v.([]any)
	if !isArr {
		t.Errorf("field %q is not an array", k)
		return nil
	}
	out := make([]J, 0, len(a))
	for _, e := range a {
		m, _ := e.(map[string]any)
		out = append(out, m)
	}
	return out
}
