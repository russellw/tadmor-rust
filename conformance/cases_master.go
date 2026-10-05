package main

import (
	"strings"
)

// spec/api.md §5.2.
func testOrganizations(t *T) {
	t.status(400, "POST", "/api/organizations", J{"name": ""})
	t.status(422, "POST", "/api/organizations", J{"name": t.uniq("Org"), "country_code": "ZZ"})
	t.status(422, "POST", "/api/organizations", J{"name": t.uniq("Org"), "default_currency": "ZZZ"})

	name := t.uniq("Org Full")
	id := t.create("/api/organizations", J{
		"name": name, "legal_name": "Full Org Ltd", "tax_id": "TAX-1", "country_code": "IE",
		"default_currency": "EUR", "email": "billing@full.example", "is_self": false,
	})
	o := t.get(path("/api/organizations/%d", id))
	t.shape(o, "Organization")
	t.eq("name", t.str(o, "name"), name)
	t.eq("legal_name", t.str(o, "legal_name"), "Full Org Ltd")
	t.eq("country_code", t.str(o, "country_code"), "IE")
	t.eq("default_currency", t.str(o, "default_currency"), "EUR")
	t.eq("email", t.str(o, "email"), "billing@full.example")

	// PUT is a full replacement: omitted optional fields become null.
	t.status(204, "PUT", path("/api/organizations/%d", id), J{"name": name + " Renamed"})
	o = t.get(path("/api/organizations/%d", id))
	t.eq("renamed", t.str(o, "name"), name+" Renamed")
	for _, k := range []string{"legal_name", "tax_id", "country_code", "default_currency", "email"} {
		t.isNull(o, k)
	}
	t.eq("is_self", t.boolean(o, "is_self"), false)

	t.status(404, "GET", "/api/organizations/999999", nil)
	t.status(404, "PUT", "/api/organizations/999999", J{"name": "Nobody"})

	// At most one organization is our own company.
	self := t.org(J{"is_self": true})
	t.status(409, "POST", "/api/organizations", J{"name": t.uniq("Second self"), "is_self": true})
	t.status(409, "PUT", path("/api/organizations/%d", id), J{"name": name, "is_self": true})
	t.eq("self flagged", t.boolean(t.get(path("/api/organizations/%d", self)), "is_self"), true)

	t.orderedBy("organizations", t.list("/api/organizations"), "name")
}

// spec/api.md §5.3.
func testCustomersSuppliers(t *T) {
	ar := t.account("asset")
	ap := t.account("liability")
	org := t.org(nil)

	t.status(400, "POST", "/api/customers", J{})
	t.status(422, "POST", "/api/customers", J{"organization_id": 999999})
	t.status(422, "POST", "/api/customers", J{"organization_id": org, "credit_limit": "-1"})
	t.status(422, "POST", "/api/customers", J{"organization_id": org, "payment_terms_code": "NOPE"})

	num := t.uniq("C")
	cust := t.create("/api/customers", J{
		"organization_id": org, "customer_number": num, "ar_account_id": ar,
		"payment_terms_code": "NET30", "currency_code": "USD", "tax_code": "STD", "credit_limit": "5000",
		"is_active": false, // ignored on create
	})
	c := t.get(path("/api/customers/%d", cust))
	t.shape(c, "Customer")
	t.eq("customer_number", t.str(c, "customer_number"), num)
	t.eq("ar_account_id", t.int(c, "ar_account_id"), ar)
	t.eq("payment_terms_code", t.str(c, "payment_terms_code"), "NET30")
	t.eqDec(c, "credit_limit", "5000")
	t.eq("created active", t.boolean(c, "is_active"), true)

	t.status(409, "POST", "/api/customers", J{"organization_id": org})                                // one role per org
	t.status(409, "POST", "/api/customers", J{"organization_id": t.org(nil), "customer_number": num}) // number taken

	// Full replacement: omitting is_active deactivates.
	t.status(204, "PUT", path("/api/customers/%d", cust), J{"organization_id": org, "customer_number": num, "ar_account_id": ar})
	c = t.get(path("/api/customers/%d", cust))
	t.eq("deactivated by omission", t.boolean(c, "is_active"), false)
	t.isNull(c, "credit_limit")
	t.isNull(c, "payment_terms_code")
	t.status(404, "PUT", "/api/customers/999999", J{"organization_id": org})

	// The same organization may also be a supplier.
	sup := t.create("/api/suppliers", J{"organization_id": org, "supplier_number": t.uniq("S"), "ap_account_id": ap, "currency_code": "USD"})
	s := t.get(path("/api/suppliers/%d", sup))
	t.shape(s, "Supplier")
	t.eq("supplier org", t.int(s, "organization_id"), org)
	t.eq("ap_account_id", t.int(s, "ap_account_id"), ap)
	t.status(409, "POST", "/api/suppliers", J{"organization_id": org})
	t.status(400, "POST", "/api/suppliers", J{"organization_id": 0})
	t.status(404, "GET", "/api/suppliers/999999", nil)
	t.status(204, "PUT", path("/api/suppliers/%d", sup), J{"organization_id": org, "ap_account_id": ap, "is_active": true})

	if find(t.list("/api/customers"), "id", cust) == nil {
		t.Errorf("customer %d missing from the list", cust)
	}
	if find(t.list("/api/suppliers"), "id", sup) == nil {
		t.Errorf("supplier %d missing from the list", sup)
	}
}

// spec/api.md §5.4.
func testProducts(t *T) {
	t.status(400, "POST", "/api/products", J{"sku": "", "name": "X"})
	t.status(400, "POST", "/api/products", J{"sku": t.uniq("SKU"), "name": ""})
	t.status(422, "POST", "/api/products", J{"sku": t.uniq("SKU"), "name": "X", "revenue_account_id": 999999})

	sku := t.uniq("SKU")
	id := t.create("/api/products", J{"sku": sku, "name": "Widget"})
	p := t.get(path("/api/products/%d", id))
	t.shape(p, "Product")
	t.eqDec(p, "unit_price", "0")
	t.eq("track_inventory", t.boolean(p, "track_inventory"), false)
	t.eq("is_active", t.boolean(p, "is_active"), true)
	t.status(409, "POST", "/api/products", J{"sku": sku, "name": "Duplicate"})

	rev := t.account("revenue")
	t.status(204, "PUT", path("/api/products/%d", id), J{
		"sku": sku, "name": "Widget Pro", "description": "Better", "unit_price": "12.5",
		"currency_code": "USD", "revenue_account_id": rev, "tax_code": "STD", "is_active": true,
	})
	p = t.get(path("/api/products/%d", id))
	t.eq("name", t.str(p, "name"), "Widget Pro")
	t.eqDec(p, "unit_price", "12.50")
	t.eq("revenue_account_id", t.int(p, "revenue_account_id"), rev)
	t.status(404, "PUT", "/api/products/999999", J{"sku": "x", "name": "x"})

	t.orderedBy("products", t.list("/api/products"), "sku")
}

// spec/api.md §5.5.
func testAccounts(t *T) {
	t.status(400, "POST", "/api/accounts", J{"code": "", "name": "X", "account_type": "asset"})
	t.status(400, "POST", "/api/accounts", J{"code": t.uniq("A"), "name": "X"})
	t.status(422, "POST", "/api/accounts", J{"code": t.uniq("A"), "name": "X", "account_type": "bogus"})
	t.status(422, "POST", "/api/accounts", J{"code": t.uniq("A"), "name": "X", "account_type": "liability", "is_cash": true})
	t.status(422, "POST", "/api/accounts", J{"code": t.uniq("A"), "name": "X", "account_type": "asset", "cash_flow_activity": "sideways"})
	t.status(422, "POST", "/api/accounts", J{"code": t.uniq("A"), "name": "X", "account_type": "asset", "parent_id": 999999})

	// is_postable defaults to false: a summary account.
	code := t.uniq("A")
	parent := t.create("/api/accounts", J{"code": code, "name": "Header", "account_type": "asset"})
	a := t.get(path("/api/accounts/%d", parent))
	t.shape(a, "Account")
	t.eq("is_postable default", t.boolean(a, "is_postable"), false)
	t.eq("cash_flow_activity default", t.str(a, "cash_flow_activity"), "operating")
	t.eq("is_active", t.boolean(a, "is_active"), true)
	t.status(409, "POST", "/api/accounts", J{"code": code, "name": "Dup", "account_type": "asset"})

	child := t.create("/api/accounts", J{
		"code": t.uniq("A"), "name": "Fixed assets", "account_type": "asset", "parent_id": parent,
		"is_postable": true, "cash_flow_activity": "investing",
	})
	a = t.get(path("/api/accounts/%d", child))
	t.eq("parent_id", t.int(a, "parent_id"), parent)
	t.eq("cash_flow_activity", t.str(a, "cash_flow_activity"), "investing")

	t.status(422, "PUT", path("/api/accounts/%d", child), J{"code": t.str(a, "code"), "name": "Self parent", "account_type": "asset", "parent_id": child, "is_postable": true, "is_active": true})
	t.status(204, "PUT", path("/api/accounts/%d", child), J{"code": t.str(a, "code"), "name": "Bank", "account_type": "asset", "is_postable": true, "is_active": true, "is_cash": true})
	a = t.get(path("/api/accounts/%d", child))
	t.eq("is_cash", t.boolean(a, "is_cash"), true)
	t.isNull(a, "parent_id")
	t.status(404, "PUT", "/api/accounts/999999", J{"code": "x", "name": "x", "account_type": "asset"})
	t.status(404, "GET", "/api/accounts/999999", nil)
	t.status(404, "GET", "/api/accounts/999999/ledger", nil)
	t.status(400, "GET", path("/api/accounts/%d/ledger?from=yesterday", child), nil)
	if l := t.list(path("/api/accounts/%d/ledger", child)); len(l) != 0 {
		t.Errorf("a new account's ledger has %d rows", len(l))
	}

	t.orderedBy("accounts", t.list("/api/accounts"), "code")
}

// spec/api.md §5.6.
func testTaxCodesTermsWarehouses(t *T) {
	acct := t.account("liability")

	// Tax codes, keyed by code.
	code := t.uniq("TX")
	t.status(400, "POST", "/api/tax-codes", J{"code": "", "name": "X"})
	t.status(422, "POST", "/api/tax-codes", J{"code": t.uniq("TX"), "name": "X", "rate": "-1"})
	r := t.must(t.admin, 201, "POST", "/api/tax-codes", J{"code": code, "name": "VAT", "rate": "20", "tax_account_id": acct})
	t.eq("created key", t.str(t.obj(r), "code"), code)
	t.status(409, "POST", "/api/tax-codes", J{"code": code, "name": "Dup"})
	tc := t.get("/api/tax-codes/" + code)
	t.shape(tc, "TaxCode")
	t.eqDec(tc, "rate", "20")
	t.eq("tax_account_id", t.int(tc, "tax_account_id"), acct)
	// The path's code wins over the body's.
	t.status(204, "PUT", "/api/tax-codes/"+code, J{"code": "IGNORED", "name": "VAT reduced", "rate": "13.5", "is_active": true})
	tc = t.get("/api/tax-codes/" + code)
	t.eq("name", t.str(tc, "name"), "VAT reduced")
	t.eqDec(tc, "rate", "13.5")
	t.isNull(tc, "tax_account_id")
	t.status(404, "GET", "/api/tax-codes/IGNORED", nil)
	t.status(404, "PUT", "/api/tax-codes/"+t.uniq("NONE"), J{"name": "X"})

	// Payment terms, keyed by code and listed by due_days then code.
	term := t.uniq("PT")
	t.status(422, "POST", "/api/payment-terms", J{"code": term, "name": "Bad", "due_days": -1})
	t.status(400, "POST", "/api/payment-terms", J{"code": term, "name": ""})
	r = t.must(t.admin, 201, "POST", "/api/payment-terms", J{"code": term, "name": "Net 45", "due_days": 45})
	t.eq("created key", t.str(t.obj(r), "code"), term)
	t.status(409, "POST", "/api/payment-terms", J{"code": term, "name": "Dup", "due_days": 1})
	t.status(204, "PUT", "/api/payment-terms/"+term, J{"name": "Net 45 days", "due_days": 45})
	pt := t.get("/api/payment-terms/" + term)
	t.shape(pt, "PaymentTerm")
	t.eq("name", t.str(pt, "name"), "Net 45 days")
	t.status(422, "PUT", "/api/payment-terms/"+term, J{"name": "Net 45", "due_days": -5})
	t.status(404, "PUT", "/api/payment-terms/"+t.uniq("NONE"), J{"name": "X", "due_days": 1})
	var codes []string
	for _, p := range t.list("/api/payment-terms") {
		codes = append(codes, t.str(p, "code"))
	}
	t.eq("payment terms order", strings.Join(codes, ","), "DUE,NET15,NET30,"+term+",NET60")

	// Warehouses.
	wcode := t.uniq("WH")
	t.status(400, "POST", "/api/warehouses", J{"code": wcode, "name": ""})
	wh := t.create("/api/warehouses", J{"code": wcode, "name": "Main"})
	t.status(409, "POST", "/api/warehouses", J{"code": wcode, "name": "Dup"})
	w := t.get(path("/api/warehouses/%d", wh))
	t.shape(w, "Warehouse")
	t.eq("is_active", t.boolean(w, "is_active"), true)
	t.status(204, "PUT", path("/api/warehouses/%d", wh), J{"code": wcode, "name": "Main (closed)"})
	t.eq("deactivated by omission", t.boolean(t.get(path("/api/warehouses/%d", wh)), "is_active"), false)
	t.status(404, "GET", "/api/warehouses/999999", nil)
}

// spec/api.md §1.4: unparseable bodies are 400; unparseable values inside
// well-formed JSON are 422; a server never answers bad input with 500.
func testMalformedInput(t *T) {
	t.status(400, "POST", "/api/organizations", rawBody(`{"name": `))
	t.status(400, "POST", "/api/products", rawBody(`[1,2,3]`))
	t.status(400, "POST", "/api/sales-invoices", rawBody(`not json at all`))
	t.status(422, "POST", "/api/products", J{"sku": t.uniq("SKU"), "name": "Bad price", "unit_price": "twelve"})
	t.status(422, "POST", "/api/fiscal-years", J{"name": t.uniq("FY"), "start_date": "2101-13-45", "end_date": "2101-12-31"})
	t.status(422, "POST", "/api/fiscal-years", J{"name": t.uniq("FY"), "start_date": "someday", "end_date": "2101-12-31"})
}
