package main

import (
	"net/http"
	"strings"
)

// spec/api.md §4: what a fresh instance contains.
func testInitialState(t *T) {
	type seedAccount struct {
		code, name, typ, activity string
		cash                      bool
	}
	seed := []seedAccount{
		{"1000", "Cash", "asset", "operating", true},
		{"1100", "Accounts Receivable", "asset", "operating", false},
		{"1200", "Inventory", "asset", "operating", false},
		{"2000", "Accounts Payable", "liability", "operating", false},
		{"2100", "Sales Tax Payable", "liability", "operating", false},
		{"2150", "Goods Received Not Invoiced", "liability", "operating", false},
		{"3000", "Retained Earnings", "equity", "financing", false},
		{"3100", "Common Stock", "equity", "financing", false},
		{"4000", "Sales Revenue", "revenue", "operating", false},
		{"5000", "Cost of Goods Sold", "expense", "operating", false},
		{"6000", "Operating Expenses", "expense", "operating", false},
		{"7000", "Foreign Exchange Gain/Loss", "expense", "operating", false},
	}
	accounts := t.list("/api/accounts")
	if len(accounts) != len(seed) {
		t.Fatalf("a fresh instance has %d accounts, want the %d seeded ones; is the database fresh?", len(accounts), len(seed))
	}
	for i, s := range seed {
		a := accounts[i] // ordered by code
		t.shape(a, "Account")
		t.eq("account code", t.str(a, "code"), s.code)
		t.eq(s.code+" name", t.str(a, "name"), s.name)
		t.eq(s.code+" type", t.str(a, "account_type"), s.typ)
		t.eq(s.code+" is_postable", t.boolean(a, "is_postable"), true)
		t.eq(s.code+" is_active", t.boolean(a, "is_active"), true)
		t.eq(s.code+" is_cash", t.boolean(a, "is_cash"), s.cash)
		t.eq(s.code+" cash_flow_activity", t.str(a, "cash_flow_activity"), s.activity)
		t.isNull(a, "parent_id")
		t.isNull(a, "currency_code")
	}

	terms := t.list("/api/payment-terms")
	var got []string
	for _, p := range terms {
		t.shape(p, "PaymentTerm")
		got = append(got, t.str(p, "code")+"/"+t.str(p, "name")+"/"+itoa(t.int(p, "due_days")))
	}
	t.eq("payment terms", strings.Join(got, ", "), "DUE/Due on receipt/0, NET15/Net 15/15, NET30/Net 30/30, NET60/Net 60/60")

	taxes := t.list("/api/tax-codes")
	got = nil
	for _, tc := range taxes {
		t.shape(tc, "TaxCode")
		t.eqDec(tc, "rate", "0")
		t.eq(t.str(tc, "code")+" is_active", t.boolean(tc, "is_active"), true)
		got = append(got, t.str(tc, "code"))
	}
	t.eq("tax codes", strings.Join(got, ","), "EXEMPT,STD,ZERO")
	if std := find(taxes, "code", "STD"); std != nil {
		t.eq("STD tax account", t.int(std, "tax_account_id"), t.int(find(accounts, "code", "2100"), "id"))
	}
	if zero := find(taxes, "code", "ZERO"); zero != nil {
		t.isNull(zero, "tax_account_id")
	}

	s := t.get("/api/settings")
	t.shape(s, "Settings")
	t.eq("base currency", t.str(s, "base_currency"), "USD")
	t.eq("FX gain/loss account", t.int(s, "fx_gain_loss_account_id"), t.int(find(accounts, "code", "7000"), "id"))

	users := t.list("/api/users")
	if len(users) != 1 {
		t.Errorf("a fresh instance has %d users, want exactly the bootstrapped administrator", len(users))
	} else {
		t.shape(users[0], "UserRecord")
		t.eq("bootstrapped user is_admin", t.boolean(users[0], "is_admin"), true)
		t.eq("bootstrapped user is_active", t.boolean(users[0], "is_active"), true)
	}

	for _, p := range []string{
		"organizations", "customers", "suppliers", "products", "warehouses", "fiscal-years",
		"accounting-periods", "exchange-rates", "sales-invoices", "purchase-bills",
		"sales-credit-notes", "purchase-credit-notes", "customer-payments", "supplier-payments",
		"sales-orders", "purchase-orders", "stock-movements", "bank-statements",
		"ar-aging", "ap-aging", "inventory-valuation",
	} {
		if l := t.list("/api/" + p); len(l) != 0 {
			t.Errorf("GET /api/%s on a fresh instance has %d rows, want []", p, len(l))
		}
	}
	for _, row := range t.list("/api/trial-balance") {
		t.shape(row, "TrialBalanceRow")
		t.eqDec(row, "balance", "0")
	}
}

// spec/api.md §2.
func testProbes(t *T) {
	c := newClient(t.base)
	r := t.must(c, 200, "GET", "/healthz", nil)
	t.eq("healthz status", t.str(t.obj(r), "status"), "ok")
	r = t.must(c, 200, "GET", "/readyz", nil)
	t.eq("readyz status", t.str(t.obj(r), "status"), "ready")
}

// spec/api.md §3: everything under /api/ but login and logout needs a session.
func testUnauthenticated(t *T) {
	c := newClient(t.base)
	t.expect(c, 401, "GET", "/api/accounts", nil)
	t.expect(c, 401, "GET", "/api/auth/me", nil)
	t.expect(c, 401, "POST", "/api/sales-invoices", J{})
	t.expect(c, 401, "GET", "/api/users", nil)
	t.expect(c, 401, "GET", "/api/no-such-endpoint", nil)
	t.expect(c, 204, "POST", "/api/auth/logout", nil) // idempotent without a session
}

func testLoginLogout(t *T) {
	c := newClient(t.base)
	email := strings.ToLower(t.uniq("login")) + "@conformance.test"
	t.create("/api/users", J{"email": email, "full_name": "Login Tester", "password": "correct horse", "is_admin": false})

	t.expect(c, 400, "POST", "/api/auth/login", J{"email": email, "password": ""})
	t.expect(c, 400, "POST", "/api/auth/login", J{"email": "", "password": "correct horse"})
	t.expect(c, 400, "POST", "/api/auth/login", rawBody("{not json"))
	t.expect(c, 401, "POST", "/api/auth/login", J{"email": email, "password": "wrong password"})
	t.expect(c, 401, "POST", "/api/auth/login", J{"email": "nobody-" + email, "password": "correct horse"})
	t.expect(c, 401, "GET", "/api/auth/me", nil)

	// Emails are case-insensitive and trimmed.
	r := t.must(c, 200, "POST", "/api/auth/login", J{"email": "  " + strings.ToUpper(email) + " ", "password": "correct horse"})
	u := t.obj(r)
	t.shape(u, "User")
	t.eq("login email", strings.ToLower(t.str(u, "email")), email)
	t.eq("login is_admin", t.boolean(u, "is_admin"), false)
	// The session cookie is HttpOnly and SameSite=Lax (§3). Its name is free,
	// and other cookies may be set alongside it, so one such cookie suffices.
	session := false
	for _, ck := range (&http.Response{Header: r.Header}).Cookies() {
		session = session || ck.HttpOnly && ck.SameSite == http.SameSiteLaxMode
	}
	if !session {
		t.Errorf("login set no HttpOnly, SameSite=Lax cookie: %q", r.Header.Values("Set-Cookie"))
	}

	me := t.obj(t.must(c, 200, "GET", "/api/auth/me", nil))
	t.shape(me, "User")
	t.eq("me id", t.int(me, "id"), t.int(u, "id"))
	t.expect(c, 200, "GET", "/api/accounts", nil)

	t.expect(c, 204, "POST", "/api/auth/logout", nil)
	t.expect(c, 401, "GET", "/api/auth/me", nil)
	t.expect(c, 401, "GET", "/api/accounts", nil)
}

// spec/api.md §5.1.
func testUserAdmin(t *T) {
	email := strings.ToLower(t.uniq("admin-ui")) + "@conformance.test"
	t.status(422, "POST", "/api/users", J{"email": "no-at-sign", "full_name": "X", "password": "longenough"})
	t.status(400, "POST", "/api/users", J{"email": email, "full_name": "", "password": "longenough"})
	t.status(422, "POST", "/api/users", J{"email": email, "full_name": "X", "password": "short"})
	t.status(400, "POST", "/api/users", J{"email": email, "full_name": "X", "password": ""})
	t.status(400, "POST", "/api/users", J{"email": "", "full_name": "X", "password": "longenough"})

	id := t.create("/api/users", J{"email": email, "full_name": "Ann Example", "password": "longenough", "is_admin": false})
	t.status(409, "POST", "/api/users", J{"email": strings.ToUpper(email), "full_name": "Dup", "password": "longenough"})

	u := t.get(path("/api/users/%d", id))
	t.shape(u, "UserRecord")
	t.eq("full_name", t.str(u, "full_name"), "Ann Example")
	t.eq("is_active", t.boolean(u, "is_active"), true)
	t.eq("is_admin", t.boolean(u, "is_admin"), false)
	t.status(404, "GET", "/api/users/999999", nil)

	t.orderedBy("users", t.list("/api/users"), "email")

	t.status(204, "PUT", path("/api/users/%d", id), J{"email": email, "full_name": "Ann Renamed", "is_active": true, "is_admin": false})
	t.eq("renamed", t.str(t.get(path("/api/users/%d", id)), "full_name"), "Ann Renamed")
	t.status(400, "PUT", path("/api/users/%d", id), J{"email": email, "full_name": "", "is_active": true})
	t.status(404, "PUT", "/api/users/999999", J{"email": "x@y.z", "full_name": "X", "is_active": true})
	t.status(422, "POST", path("/api/users/%d/password", id), J{"password": "short"})
	t.status(400, "POST", path("/api/users/%d/password", id), J{})
	t.status(404, "POST", "/api/users/999999/password", J{"password": "longenough"})

	// Administrators may not lock themselves out.
	me := t.get(path("/api/users/%d", t.adminID))
	t.status(422, "PUT", path("/api/users/%d", t.adminID), J{"email": t.str(me, "email"), "full_name": t.str(me, "full_name"), "is_active": false, "is_admin": true})
	t.status(422, "PUT", path("/api/users/%d", t.adminID), J{"email": t.str(me, "email"), "full_name": t.str(me, "full_name"), "is_active": true, "is_admin": false})
	t.eq("admin still admin", t.boolean(t.get(path("/api/users/%d", t.adminID)), "is_admin"), true)
}

// spec/api.md §3 session endings and §5 administrator gating.
func testSessionsAndRoles(t *T) {
	email := strings.ToLower(t.uniq("roles")) + "@conformance.test"
	id := t.create("/api/users", J{"email": email, "full_name": "Role Tester", "password": "first-password", "is_admin": false})
	c := newClient(t.base)
	t.must(c, 200, "POST", "/api/auth/login", J{"email": email, "password": "first-password"})
	record := func(active, admin bool) J {
		return J{"email": email, "full_name": "Role Tester", "is_active": active, "is_admin": admin}
	}

	// Ordinary users get the day-to-day surface but not the admin endpoints.
	t.expect(c, 200, "GET", "/api/settings", nil)
	t.expect(c, 403, "GET", "/api/users", nil)
	t.expect(c, 403, "POST", "/api/users", J{"email": "x@y.z", "full_name": "X", "password": "longenough"})
	t.expect(c, 403, "PUT", "/api/settings", J{"base_currency": "USD"})
	t.expect(c, 403, "POST", "/api/sales-invoices/1/unpost", nil)
	t.expect(c, 403, "POST", "/api/fiscal-years/1/close", J{"retained_earnings_account_id": 1})

	// Promotion and demotion take effect on the existing session.
	t.status(204, "PUT", path("/api/users/%d", id), record(true, true))
	t.expect(c, 200, "GET", "/api/users", nil)
	t.status(204, "PUT", path("/api/users/%d", id), record(true, false))
	t.expect(c, 403, "GET", "/api/users", nil)

	// A password reset revokes every session of that user.
	t.status(204, "POST", path("/api/users/%d/password", id), J{"password": "second-password"})
	t.expect(c, 401, "GET", "/api/auth/me", nil)
	t.expect(c, 401, "POST", "/api/auth/login", J{"email": email, "password": "first-password"})
	t.must(c, 200, "POST", "/api/auth/login", J{"email": email, "password": "second-password"})

	// Deactivation ends sessions immediately and blocks login.
	t.status(204, "PUT", path("/api/users/%d", id), record(false, false))
	t.expect(c, 401, "GET", "/api/auth/me", nil)
	t.expect(c, 401, "POST", "/api/auth/login", J{"email": email, "password": "second-password"})
	t.eq("deactivated", t.boolean(t.get(path("/api/users/%d", id)), "is_active"), false)

	t.status(204, "PUT", path("/api/users/%d", id), record(true, false))
	t.must(c, 200, "POST", "/api/auth/login", J{"email": email, "password": "second-password"})
}
