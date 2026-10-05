package main

import (
	"bytes"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"math/big"
	"net/http"
	"net/http/cookiejar"
	"path/filepath"
	"regexp"
	"runtime"
	"sort"
	"strings"
	"time"
)

// ---------------------------------------------------------------------------
// Environment shared by all cases
// ---------------------------------------------------------------------------

// Env is the state shared across one run: the server, an administrator
// session, and generators for collision-free names and dates.
type Env struct {
	base    string
	admin   *Client
	adminID int
	// tag makes every code and number this run creates unique, so a rerun
	// against a dirty database fails loudly on the fresh-state check rather
	// than on confusing duplicate-key errors.
	tag  string
	seq  int
	year int // next fiscal year handed out by nextYear

	user     *Client // a non-administrator session, created on first use
	userID   int
	userPass string
}

func newEnv(base, email, password string) (*Env, error) {
	b := make([]byte, 3)
	if _, err := rand.Read(b); err != nil {
		return nil, err
	}
	e := &Env{base: base, tag: strings.ToUpper(hex.EncodeToString(b)), year: 2101}
	e.admin = newClient(base)
	r, err := e.admin.do("POST", "/api/auth/login", J{"email": email, "password": password})
	if err != nil {
		return nil, fmt.Errorf("cannot reach %s: %w", base, err)
	}
	if r.Status != http.StatusOK {
		return nil, fmt.Errorf("administrator login failed: HTTP %d %s", r.Status, r.Body)
	}
	v, err := decode(r.Body)
	if err != nil {
		return nil, fmt.Errorf("administrator login: %w", err)
	}
	u, _ := v.(map[string]any)
	n, _ := u["id"].(json.Number)
	id, _ := n.Int64()
	e.adminID = int(id)
	return e, nil
}

// uniq returns a run-unique identifier with the given prefix.
func (e *Env) uniq(prefix string) string {
	e.seq++
	return fmt.Sprintf("%s-%s-%d", prefix, e.tag, e.seq)
}

// nextYear hands each case its own calendar year (2101 onwards), so cases
// never share accounting periods, which may not overlap anywhere.
func (e *Env) nextYear() int {
	y := e.year
	e.year++
	return y
}

// ---------------------------------------------------------------------------
// HTTP client
// ---------------------------------------------------------------------------

// Client is one browser-like session: a cookie jar over the server.
type Client struct {
	base string
	hc   *http.Client
}

func newClient(base string) *Client {
	jar, _ := cookiejar.New(nil)
	return &Client{base: base, hc: &http.Client{Jar: jar, Timeout: 60 * time.Second}}
}

// Resp is a fully read HTTP response.
type Resp struct {
	Method, Path string
	Status       int
	Header       http.Header
	Body         []byte
}

// rawBody is sent verbatim, for malformed-request tests.
type rawBody string

func (c *Client) do(method, path string, body any) (*Resp, error) {
	var rd io.Reader
	switch b := body.(type) {
	case nil:
	case rawBody:
		rd = strings.NewReader(string(b))
	default:
		data, err := json.Marshal(b)
		if err != nil {
			return nil, err
		}
		rd = bytes.NewReader(data)
	}
	req, err := http.NewRequest(method, c.base+path, rd)
	if err != nil {
		return nil, err
	}
	if rd != nil {
		req.Header.Set("Content-Type", "application/json")
	}
	resp, err := c.hc.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	data, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, err
	}
	return &Resp{Method: method, Path: path, Status: resp.StatusCode, Header: resp.Header, Body: data}, nil
}

// ---------------------------------------------------------------------------
// JSON values
// ---------------------------------------------------------------------------

// J is a JSON object, for request bodies and decoded responses alike.
type J = map[string]any

func decode(b []byte) (any, error) {
	d := json.NewDecoder(bytes.NewReader(b))
	d.UseNumber()
	var v any
	if err := d.Decode(&v); err != nil {
		return nil, fmt.Errorf("invalid JSON (%v): %.200s", err, b)
	}
	return v, nil
}

var decimalRE = regexp.MustCompile(`^-?\d+(\.\d+)?$`)

// parseDec parses a spec decimal: a plain decimal string, never a JSON number.
func parseDec(s string) (*big.Rat, bool) {
	if !decimalRE.MatchString(s) {
		return nil, false
	}
	r, ok := new(big.Rat).SetString(s)
	return r, ok
}

func mustDec(s string) *big.Rat {
	r, ok := parseDec(s)
	if !ok {
		panic("bad decimal literal " + s)
	}
	return r
}

// decStr renders a value canonically for messages and comparisons.
func decStr(r *big.Rat) string { return r.FloatString(8) }

// ---------------------------------------------------------------------------
// Test context
// ---------------------------------------------------------------------------

// T is the context of one running case. Errorf records a failure and
// continues; Fatalf records one and abandons the case.
type T struct {
	*Env
	name     string
	failures []string
}

type fatalSignal struct{}

func (t *T) run(fn func(*T)) {
	defer func() {
		if r := recover(); r != nil {
			if _, ok := r.(fatalSignal); !ok {
				t.failures = append(t.failures, fmt.Sprintf("panic: %v", r))
			}
		}
	}()
	fn(t)
}

// where finds the innermost cases_*.go frame, so a failure points at the
// assertion's line in the case rather than inside a helper.
func where() string {
	pcs := make([]uintptr, 32)
	n := runtime.Callers(3, pcs)
	frames := runtime.CallersFrames(pcs[:n])
	for {
		f, more := frames.Next()
		if strings.HasPrefix(filepath.Base(f.File), "cases_") {
			return fmt.Sprintf("%s:%d", filepath.Base(f.File), f.Line)
		}
		if !more {
			return "?"
		}
	}
}

func (t *T) Errorf(format string, args ...any) {
	t.failures = append(t.failures, where()+": "+fmt.Sprintf(format, args...))
}

func (t *T) Fatalf(format string, args ...any) {
	t.Errorf(format, args...)
	panic(fatalSignal{})
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

func (t *T) send(c *Client, method, path string, body any) *Resp {
	r, err := c.do(method, path, body)
	if err != nil {
		t.Fatalf("%s %s: %v", method, path, err)
	}
	return r
}

// check reports whether r has the wanted status, recording a failure if not.
// Error responses must carry the spec's {"error": "..."} body.
func (t *T) check(r *Resp, want int) bool {
	if r.Status != want {
		t.Errorf("%s %s: status %d, want %d; body: %.300s", r.Method, r.Path, r.Status, want, r.Body)
		return false
	}
	if want == http.StatusNoContent && len(r.Body) != 0 {
		t.Errorf("%s %s: 204 response has a body: %.200s", r.Method, r.Path, r.Body)
	}
	if want >= 400 {
		v, err := decode(r.Body)
		o, isObj := v.(map[string]any)
		if err != nil || !isObj {
			t.Errorf("%s %s: error body is not a JSON object: %.200s", r.Method, r.Path, r.Body)
			return true
		}
		if s, ok := o["error"].(string); !ok || s == "" {
			t.Errorf(`%s %s: error body lacks a non-empty "error" string: %.200s`, r.Method, r.Path, r.Body)
		}
	}
	return true
}

// expect sends a request as client c and checks the status, continuing on
// mismatch.
func (t *T) expect(c *Client, want int, method, path string, body any) *Resp {
	r := t.send(c, method, path, body)
	t.check(r, want)
	return r
}

// must sends a request as client c and abandons the case on a wrong status.
func (t *T) must(c *Client, want int, method, path string, body any) *Resp {
	r := t.send(c, method, path, body)
	if !t.check(r, want) {
		panic(fatalSignal{})
	}
	return r
}

// Administrator shorthands.

func (t *T) status(want int, method, path string, body any) *Resp {
	return t.expect(t.admin, want, method, path, body)
}

func (t *T) get(path string) J { return t.obj(t.must(t.admin, 200, "GET", path, nil)) }

func (t *T) list(path string) []J { return t.arr(t.must(t.admin, 200, "GET", path, nil)) }

// create POSTs as the administrator, requires 201, and returns {"id"}.
func (t *T) create(path string, body any) int {
	return t.int(t.obj(t.must(t.admin, 201, "POST", path, body)), "id")
}

// ---------------------------------------------------------------------------
// Response decoding and field access
// ---------------------------------------------------------------------------

func (t *T) obj(r *Resp) J {
	v, err := decode(r.Body)
	if err != nil {
		t.Fatalf("%s %s: %v", r.Method, r.Path, err)
	}
	o, ok := v.(map[string]any)
	if !ok {
		t.Fatalf("%s %s: want a JSON object, got %.200s", r.Method, r.Path, r.Body)
	}
	return o
}

func (t *T) arr(r *Resp) []J {
	v, err := decode(r.Body)
	if err != nil {
		t.Fatalf("%s %s: %v", r.Method, r.Path, err)
	}
	a, ok := v.([]any)
	if !ok {
		t.Fatalf("%s %s: want a JSON array, got %.200s", r.Method, r.Path, r.Body)
	}
	out := make([]J, 0, len(a))
	for i, e := range a {
		o, ok := e.(map[string]any)
		if !ok {
			t.Fatalf("%s %s: element %d is not an object", r.Method, r.Path, i)
		}
		out = append(out, o)
	}
	return out
}

func (t *T) field(o J, k string) (any, bool) {
	v, ok := o[k]
	if !ok {
		t.Errorf("missing field %q in %v", k, brief(o))
	}
	return v, ok
}

func (t *T) int(o J, k string) int {
	v, ok := t.field(o, k)
	if !ok {
		return 0
	}
	n, isNum := v.(json.Number)
	i, err := n.Int64()
	if !isNum || err != nil {
		t.Errorf("field %q = %v, want an integer", k, v)
		return 0
	}
	return int(i)
}

// intPtr reads a nullable integer: nil for JSON null.
func (t *T) intPtr(o J, k string) *int {
	v, ok := t.field(o, k)
	if !ok || v == nil {
		return nil
	}
	i := t.int(o, k)
	return &i
}

func (t *T) str(o J, k string) string {
	v, ok := t.field(o, k)
	if !ok {
		return ""
	}
	s, isStr := v.(string)
	if !isStr {
		t.Errorf("field %q = %v, want a string", k, v)
	}
	return s
}

func (t *T) boolean(o J, k string) bool {
	v, ok := t.field(o, k)
	if !ok {
		return false
	}
	b, isBool := v.(bool)
	if !isBool {
		t.Errorf("field %q = %v, want a boolean", k, v)
	}
	return b
}

// dec reads a decimal field, which the spec requires to be a JSON string.
func (t *T) dec(o J, k string) *big.Rat {
	v, ok := t.field(o, k)
	if !ok {
		return new(big.Rat)
	}
	s, isStr := v.(string)
	if !isStr {
		t.Errorf("field %q = %v (%T), want a decimal string", k, v, v)
		return new(big.Rat)
	}
	r, valid := parseDec(s)
	if !valid {
		t.Errorf("field %q = %q is not a plain decimal", k, s)
		return new(big.Rat)
	}
	return r
}

func brief(o J) string {
	b, _ := json.Marshal(o)
	if len(b) > 300 {
		return string(b[:300]) + "..."
	}
	return string(b)
}

// ---------------------------------------------------------------------------
// Assertions
// ---------------------------------------------------------------------------

func (t *T) eq(what string, got, want any) {
	if fmt.Sprint(got) != fmt.Sprint(want) {
		t.Errorf("%s = %v, want %v", what, got, want)
	}
}

func (t *T) isNull(o J, k string) {
	if v, ok := t.field(o, k); ok && v != nil {
		t.Errorf("field %q = %v, want null", k, v)
	}
}

func (t *T) notNull(o J, k string) {
	if v, ok := t.field(o, k); ok && v == nil {
		t.Errorf("field %q is null, want a value", k)
	}
}

// eqDec compares a decimal field by value.
func (t *T) eqDec(o J, k, want string) {
	got := t.dec(o, k)
	if got.Cmp(mustDec(want)) != 0 {
		t.Errorf("%s = %s, want %s (in %s)", k, decStr(got), want, brief(o))
	}
}

// shape checks that every documented field of a response type is present.
func (t *T) shape(o J, typ string) {
	keys, ok := shapes[typ]
	if !ok {
		panic("unknown shape " + typ)
	}
	var missing []string
	for _, k := range keys {
		if _, ok := o[k]; !ok {
			missing = append(missing, k)
		}
	}
	if len(missing) > 0 {
		t.Errorf("%s is missing fields %v: %s", typ, missing, brief(o))
	}
}

// textOrdered reports whether a sorts no later than b under any reasonable
// collation. Implementations sort text keys by their database's collation,
// which may ignore punctuation and case, so only letters and digits are
// compared, case-folded.
func textOrdered(a, b string) bool {
	key := func(s string) string {
		var out []rune
		for _, r := range strings.ToLower(s) {
			if (r >= 'a' && r <= 'z') || (r >= '0' && r <= '9') {
				out = append(out, r)
			}
		}
		return string(out)
	}
	return key(a) <= key(b)
}

// orderedBy checks that list is sorted by the text field k (see textOrdered).
func (t *T) orderedBy(what string, list []J, k string) {
	for i := 1; i < len(list); i++ {
		if a, b := t.str(list[i-1], k), t.str(list[i], k); !textOrdered(a, b) {
			t.Errorf("%s are not ordered by %s: %q before %q", what, k, a, b)
			return
		}
	}
}

// find returns the element of list whose field k equals v (compared as
// text), or nil.
func find(list []J, k string, v any) J {
	want := fmt.Sprint(v)
	for _, o := range list {
		if fmt.Sprint(o[k]) == want {
			return o
		}
	}
	return nil
}

func (t *T) mustFind(list []J, k string, v any) J {
	o := find(list, k, v)
	if o == nil {
		t.Fatalf("no element with %s = %v", k, v)
	}
	return o
}

// ---------------------------------------------------------------------------
// Journal entries
// ---------------------------------------------------------------------------

// L is one expected journal line: an account and its four amounts. Base
// amounts default to the transaction amounts when left empty.
type L struct {
	Account            int
	Debit, Credit      string
	BaseDebit, BaseCrd string
}

func dr(account int, amount string) L { return L{Account: account, Debit: amount, Credit: "0"} }
func cr(account int, amount string) L { return L{Account: account, Debit: "0", Credit: amount} }

// base sets the line's base-currency amounts on its own side.
func (l L) base(amount string) L {
	if mustDec(l.Debit).Sign() != 0 {
		l.BaseDebit, l.BaseCrd = amount, "0"
	} else {
		l.BaseDebit, l.BaseCrd = "0", amount
	}
	return l
}

func (l L) key() string {
	bd, bc := l.BaseDebit, l.BaseCrd
	if bd == "" {
		bd = l.Debit
	}
	if bc == "" {
		bc = l.Credit
	}
	return fmt.Sprintf("acct %d Dr %s Cr %s (base Dr %s Cr %s)", l.Account,
		decStr(mustDec(l.Debit)), decStr(mustDec(l.Credit)), decStr(mustDec(bd)), decStr(mustDec(bc)))
}

// entry fetches a journal entry and checks its shape.
func (t *T) entry(id int) J {
	e := t.get(fmt.Sprintf("/api/journal-entries/%d", id))
	t.shape(e, "JournalEntry")
	return e
}

// eqLines compares a journal entry's lines with want as a multiset; line
// numbering and memos are not contract.
func (t *T) eqLines(what string, e J, want ...L) {
	raw, _ := e["lines"].([]any)
	var got []string
	for _, x := range raw {
		o, _ := x.(map[string]any)
		t.shape(o, "JournalLine")
		got = append(got, L{
			Account: t.int(o, "account_id"),
			Debit:   decStr(t.dec(o, "debit")), Credit: decStr(t.dec(o, "credit")),
			BaseDebit: decStr(t.dec(o, "base_debit")), BaseCrd: decStr(t.dec(o, "base_credit")),
		}.key())
	}
	var exp []string
	for _, l := range want {
		exp = append(exp, l.key())
	}
	sort.Strings(got)
	sort.Strings(exp)
	if strings.Join(got, "\n") != strings.Join(exp, "\n") {
		t.Errorf("%s: journal lines\n          got:  %s\n          want: %s", what,
			strings.Join(got, "\n                "), strings.Join(exp, "\n                "))
	}
}

// ---------------------------------------------------------------------------
// Documented response shapes (spec/api.md)
// ---------------------------------------------------------------------------

var shapes = map[string][]string{
	"User":               {"id", "email", "full_name", "is_admin"},
	"UserRecord":         {"id", "email", "full_name", "is_active", "is_admin"},
	"Organization":       {"id", "name", "legal_name", "tax_id", "country_code", "default_currency", "email", "is_self"},
	"Customer":           {"id", "organization_id", "customer_number", "ar_account_id", "payment_terms_code", "currency_code", "tax_code", "credit_limit", "is_active"},
	"Supplier":           {"id", "organization_id", "supplier_number", "ap_account_id", "payment_terms_code", "currency_code", "tax_code", "is_active"},
	"Product":            {"id", "sku", "name", "description", "unit_price", "currency_code", "revenue_account_id", "tax_code", "track_inventory", "inventory_account_id", "cogs_account_id", "is_active"},
	"Account":            {"id", "code", "name", "account_type", "parent_id", "currency_code", "is_postable", "is_active", "is_cash", "cash_flow_activity"},
	"TaxCode":            {"code", "name", "rate", "tax_account_id", "is_active"},
	"PaymentTerm":        {"code", "name", "due_days"},
	"Warehouse":          {"id", "code", "name", "address_id", "is_active"},
	"FiscalYear":         {"id", "name", "start_date", "end_date", "status"},
	"AccountingPeriod":   {"id", "fiscal_year_id", "name", "start_date", "end_date", "status"},
	"Settings":           {"base_currency", "fx_gain_loss_account_id"},
	"ExchangeRate":       {"currency_code", "rate_date", "rate"},
	"SalesInvoice":       {"id", "invoice_number", "customer_id", "invoice_date", "due_date", "payment_status", "currency_code", "status", "total", "amount_applied", "balance", "journal_entry_id", "reference", "memo"},
	"PurchaseBill":       {"id", "bill_number", "supplier_id", "bill_date", "due_date", "payment_status", "currency_code", "status", "total", "amount_applied", "balance", "journal_entry_id", "reference", "memo"},
	"SalesCreditNote":    {"id", "credit_note_number", "customer_id", "credit_note_date", "application_status", "currency_code", "status", "total", "amount_applied", "balance", "journal_entry_id", "reference", "memo"},
	"PurchaseCreditNote": {"id", "credit_note_number", "supplier_id", "credit_note_date", "application_status", "currency_code", "status", "total", "amount_applied", "balance", "journal_entry_id", "reference", "memo"},
	"InvoiceLine":        {"line_no", "product_id", "description", "quantity", "unit_price", "tax_code", "tax_rate", "line_subtotal", "tax_amount", "line_total", "revenue_account_id", "order_line_id"},
	"BillLine":           {"line_no", "product_id", "description", "quantity", "unit_cost", "tax_code", "tax_rate", "line_subtotal", "tax_amount", "line_total", "expense_account_id", "order_line_id"},
	"CustomerPayment":    {"id", "customer_id", "payment_date", "deposit_account_id", "currency_code", "amount", "method", "reference", "status", "amount_applied", "unapplied", "journal_entry_id"},
	"SupplierPayment":    {"id", "supplier_id", "payment_date", "payment_account_id", "currency_code", "amount", "method", "reference", "status", "amount_applied", "unapplied", "journal_entry_id"},
	"Application":        {"document_id", "document_number", "amount_applied"},
	"SalesOrder":         {"id", "order_number", "customer_id", "order_date", "expected_ship_date", "currency_code", "status", "total", "invoiced_status", "shipped_status", "reference", "memo"},
	"PurchaseOrder":      {"id", "order_number", "supplier_id", "order_date", "expected_receipt_date", "currency_code", "status", "total", "billed_status", "received_status", "reference", "memo"},
	"SalesOrderLine":     {"line_no", "order_line_id", "product_id", "description", "quantity", "unit_price", "tax_code", "tax_rate", "line_subtotal", "tax_amount", "line_total", "revenue_account_id", "qty_invoiced", "qty_shipped", "qty_to_invoice", "qty_to_ship"},
	"PurchaseOrderLine":  {"line_no", "order_line_id", "product_id", "description", "quantity", "unit_cost", "tax_code", "tax_rate", "line_subtotal", "tax_amount", "line_total", "expense_account_id", "qty_billed", "qty_received", "qty_to_bill", "qty_to_receive"},
	"StockMovement":      {"id", "product_id", "warehouse_id", "movement_date", "movement_type", "status", "quantity", "unit_cost", "total_cost", "reference", "notes", "journal_entry_id", "source_type"},
	"BankStatement":      {"id", "account_id", "account_code", "account_name", "statement_date", "opening_balance", "closing_balance", "reference", "status", "line_count", "matched_count", "lines_total", "difference"},
	"BankStatementLine":  {"id", "line_no", "txn_date", "description", "reference", "amount", "journal_line_id", "journal_entry_id", "entry_date", "entry_memo"},
	"MatchCandidate":     {"journal_line_id", "journal_entry_id", "entry_date", "reference", "memo", "amount"},
	"JournalEntry":       {"id", "entry_date", "currency_code", "exchange_rate", "reference", "memo", "status", "lines"},
	"JournalLine":        {"line_no", "account_id", "account_code", "account_name", "memo", "debit", "credit", "base_debit", "base_credit"},
	"LedgerRow":          {"journal_entry_id", "entry_date", "reference", "memo", "currency_code", "debit", "credit", "base_debit", "base_credit"},
	"TrialBalanceRow":    {"account_id", "code", "name", "account_type", "total_debit", "total_credit", "balance"},
	"ActivityRow":        {"account_id", "code", "name", "account_type", "amount"},
	"CashFlow":           {"net_income", "rows", "net_cash_flow", "opening_cash", "closing_cash"},
	"CashFlowRow":        {"account_id", "code", "name", "activity", "amount"},
	"AgingRow":           {"party_id", "party_name", "total_outstanding", "not_yet_due", "days_1_30", "days_31_60", "days_61_90", "days_over_90"},
	"ValuationRow":       {"product_id", "sku", "name", "qty_on_hand", "value_on_hand", "avg_unit_cost"},
}
