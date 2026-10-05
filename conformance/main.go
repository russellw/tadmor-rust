// Command conformance checks a tadmor-compatible server against the
// stack-neutral specification in spec/. It is a black-box client: it talks to
// the server only over HTTP, imports nothing but the Go standard library, and
// never touches the database, so it can judge any implementation, in any
// language, that claims to follow the spec.
//
// It must run against a freshly initialized instance (schema and seed data
// present, exactly one administrator, nothing else); see spec/README.md.
//
//	go run ./conformance -base-url http://127.0.0.1:8090 -email admin@example.com -password secret
package main

import (
	"flag"
	"fmt"
	"os"
	"regexp"
	"strings"
	"time"
)

func main() {
	base := flag.String("base-url", "http://127.0.0.1:8090", "root URL of the server under test")
	email := flag.String("email", "", "the bootstrapped administrator's email")
	password := flag.String("password", "", "the bootstrapped administrator's password")
	run := flag.String("run", "", "only run cases whose name matches this regular expression")
	verbose := flag.Bool("v", false, "list passing cases too")
	flag.Parse()

	if *email == "" || *password == "" {
		fmt.Fprintln(os.Stderr, "conformance: -email and -password are required")
		os.Exit(2)
	}
	var filter *regexp.Regexp
	if *run != "" {
		var err error
		if filter, err = regexp.Compile(*run); err != nil {
			fmt.Fprintf(os.Stderr, "conformance: bad -run pattern: %v\n", err)
			os.Exit(2)
		}
	}

	env, err := newEnv(strings.TrimRight(*base, "/"), *email, *password)
	if err != nil {
		fmt.Fprintf(os.Stderr, "conformance: %v\n", err)
		os.Exit(1)
	}

	start := time.Now()
	var passed, failed int
	for _, c := range allCases() {
		if filter != nil && !filter.MatchString(c.name) {
			continue
		}
		t := &T{Env: env, name: c.name}
		t.run(c.fn)
		if len(t.failures) == 0 {
			passed++
			if *verbose {
				fmt.Printf("PASS  %s\n", c.name)
			}
			continue
		}
		failed++
		fmt.Printf("FAIL  %s\n", c.name)
		for _, f := range t.failures {
			fmt.Printf("        %s\n", f)
		}
	}
	fmt.Printf("\n%d passed, %d failed (%s)\n", passed, failed, time.Since(start).Round(time.Millisecond))
	if failed > 0 {
		os.Exit(1)
	}
}

// caseDef is one named conformance case. Cases run in the order allCases
// lists them, and later cases may rely on the instance having been fresh
// when the run began, but never on what an earlier case created.
type caseDef struct {
	name string
	fn   func(*T)
}

func allCases() []caseDef {
	return []caseDef{
		// The fresh-state check must run first, before anything is created.
		{"initial-state", testInitialState},
		{"probes", testProbes},
		{"auth/unauthenticated", testUnauthenticated},
		{"auth/login-logout", testLoginLogout},
		{"users/admin", testUserAdmin},
		{"users/sessions-and-roles", testSessionsAndRoles},
		{"master/organizations", testOrganizations},
		{"master/customers-suppliers", testCustomersSuppliers},
		{"master/products", testProducts},
		{"master/accounts", testAccounts},
		{"master/tax-codes-terms-warehouses", testTaxCodesTermsWarehouses},
		{"master/malformed-input", testMalformedInput},
		{"arithmetic/decimals", testDecimalArithmetic},
		{"calendar/today-is-utc", testTodayIsUTC},
		{"calendar/fiscal-years-periods", testFiscalYearsPeriods},
		{"settings/exchange-rates", testExchangeRates},
		{"settings/ledger-settings", testLedgerSettings},
		{"sales/invoice-lifecycle", testInvoiceLifecycle},
		{"sales/posting-refusals", testPostingRefusals},
		{"sales/negative-net-lines", testNegativeNetLines},
		{"purchases/bill-lifecycle", testBillLifecycle},
		{"settlement/customer-payments", testCustomerPayments},
		{"settlement/supplier-payments", testSupplierPayments},
		{"settlement/credit-notes", testCreditNotes},
		{"orders/sales-order", testSalesOrder},
		{"orders/purchase-order", testPurchaseOrder},
		{"inventory/stock-movements", testStockMovements},
		{"currency/foreign-documents", testForeignCurrency},
		{"currency/foreign-purchase-receipt", testForeignPurchaseReceipt},
		{"banking/reconciliation", testBankReconciliation},
		{"calendar/year-end", testYearEnd},
		{"reports/statements", testStatements},
		{"reports/aging", testAging},
		{"documents/pdf-and-email", testPDFAndEmail},
		{"api/routing-and-ids", testRoutingAndIDs},
	}
}
