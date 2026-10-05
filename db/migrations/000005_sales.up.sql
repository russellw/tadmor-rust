-- 000005_sales: the Sales / Accounts-Receivable subledger.
--
-- Model:
--   payment_terms         lookup (Net 30, ...)
--   tax_codes             lookup of sales-tax rates -> GL liability account
--   products              sellable items, each mapped to a revenue account
--   customers             the "customer" role layered 1:1 onto an organization
--   sales_invoices        invoice header (+ lines)
--   sales_invoice_lines   invoice detail; line money is computed by the database
--   customer_payments     receipts from customers
--   payment_applications  allocation of receipts to invoices (m:n, partial OK)
--
-- Relationship to the GL (migration 000004): an invoice/payment links to the
-- journal_entry it produced via a nullable FK. Creating those journal entries
-- (Dr A/R, Cr revenue, Cr tax; Dr cash, Cr A/R) is the service layer's job; the
-- schema supplies the hooks and the subledger integrity rules, and leaves
-- posting policy to the application.

-- ---------------------------------------------------------------------------
-- Lookups
-- ---------------------------------------------------------------------------

CREATE TABLE payment_terms (
    code     text PRIMARY KEY,                             -- 'NET30'
    name     text NOT NULL,
    due_days int  NOT NULL DEFAULT 0 CHECK (due_days >= 0)
);

INSERT INTO payment_terms (code, name, due_days) VALUES
    ('DUE',   'Due on receipt', 0),
    ('NET15', 'Net 15',         15),
    ('NET30', 'Net 30',         30),
    ('NET60', 'Net 60',         60);

CREATE TABLE tax_codes (
    code           text          PRIMARY KEY,              -- 'STD', 'ZERO', 'EXEMPT'
    name           text          NOT NULL,
    rate           numeric(7,4)  NOT NULL DEFAULT 0 CHECK (rate >= 0),  -- percent, e.g. 8.2500
    tax_account_id int           REFERENCES accounts(id),  -- sales-tax-payable account
    is_active      boolean       NOT NULL DEFAULT true
);

-- Actual standard rates are jurisdiction-specific and configured per deployment.
INSERT INTO tax_codes (code, name, rate, tax_account_id) VALUES
    ('STD',    'Standard sales tax', 0, (SELECT id FROM accounts WHERE code = '2100')),
    ('ZERO',   'Zero-rated',         0, NULL),
    ('EXEMPT', 'Tax exempt',         0, NULL);

-- ---------------------------------------------------------------------------
-- Catalog
-- ---------------------------------------------------------------------------

CREATE TABLE products (
    id                 int         GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    sku                text        NOT NULL UNIQUE,
    name               text        NOT NULL,
    description        text,
    unit_price         numeric(19,4) NOT NULL DEFAULT 0,
    currency_code      char(3)     REFERENCES currencies(code),
    revenue_account_id int         REFERENCES accounts(id),
    tax_code           text        REFERENCES tax_codes(code),
    is_active          boolean     NOT NULL DEFAULT true,
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now()
);

CREATE TRIGGER set_updated_at BEFORE UPDATE ON products
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ---------------------------------------------------------------------------
-- Customers (a role on an organization)
-- ---------------------------------------------------------------------------

CREATE TABLE customers (
    id                  int         GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    organization_id     int         NOT NULL UNIQUE REFERENCES organizations(id),
    customer_number     text        UNIQUE,                -- optional human-facing code
    ar_account_id       int         REFERENCES accounts(id),   -- A/R control account
    payment_terms_code  text        REFERENCES payment_terms(code),
    currency_code       char(3)     REFERENCES currencies(code),
    tax_code            text        REFERENCES tax_codes(code), -- default tax treatment
    credit_limit        numeric(19,4) CHECK (credit_limit IS NULL OR credit_limit >= 0),
    is_active           boolean     NOT NULL DEFAULT true,
    created_at          timestamptz NOT NULL DEFAULT now(),
    updated_at          timestamptz NOT NULL DEFAULT now()
);

CREATE TRIGGER set_updated_at BEFORE UPDATE ON customers
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ---------------------------------------------------------------------------
-- Invoices
-- ---------------------------------------------------------------------------

CREATE TABLE sales_invoices (
    id                 int         GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    invoice_number     text        NOT NULL UNIQUE,
    customer_id        int         NOT NULL REFERENCES customers(id),
    invoice_date       date        NOT NULL,
    due_date           date,
    currency_code      char(3)     NOT NULL REFERENCES currencies(code),
    status             text        NOT NULL DEFAULT 'draft'
                                   CHECK (status IN ('draft', 'posted', 'void')),
    -- Set by the service layer when the invoice is posted to the GL.
    period_id          int         REFERENCES accounting_periods(id),
    journal_entry_id   int         REFERENCES journal_entries(id),
    -- Header money is maintained from the lines by trigger while in draft.
    subtotal           numeric(19,4) NOT NULL DEFAULT 0,
    tax_total          numeric(19,4) NOT NULL DEFAULT 0,
    total              numeric(19,4) NOT NULL DEFAULT 0,
    billing_address_id int         REFERENCES addresses(id),
    reference          text,
    memo               text,
    created_by         int         REFERENCES users(id),
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT sales_invoices_total_consistent CHECK (total = subtotal + tax_total),
    CONSTRAINT sales_invoices_due_after_date    CHECK (due_date IS NULL OR due_date >= invoice_date)
);

CREATE INDEX sales_invoices_customer_id_idx ON sales_invoices (customer_id);
CREATE INDEX sales_invoices_status_idx      ON sales_invoices (status);
CREATE INDEX sales_invoices_due_date_idx    ON sales_invoices (due_date);

CREATE TRIGGER set_updated_at BEFORE UPDATE ON sales_invoices
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE sales_invoice_lines (
    id                 int           GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    invoice_id         int           NOT NULL REFERENCES sales_invoices(id) ON DELETE CASCADE,
    line_no            int           NOT NULL,
    product_id         int           REFERENCES products(id),   -- NULL = free-form line
    description        text          NOT NULL,
    quantity           numeric(19,4) NOT NULL DEFAULT 1 CHECK (quantity <> 0),
    unit_price         numeric(19,4) NOT NULL DEFAULT 0,
    revenue_account_id int           REFERENCES accounts(id),
    tax_code           text          REFERENCES tax_codes(code),
    tax_rate           numeric(7,4)  NOT NULL DEFAULT 0 CHECK (tax_rate >= 0),  -- rate snapshot
    -- Line money is derived by the database so it can never drift from inputs.
    line_subtotal numeric(19,4)
        GENERATED ALWAYS AS (round(quantity * unit_price, 4)) STORED,
    tax_amount    numeric(19,4)
        GENERATED ALWAYS AS (round(quantity * unit_price * tax_rate / 100, 4)) STORED,
    line_total    numeric(19,4)
        GENERATED ALWAYS AS (round(quantity * unit_price, 4)
                             + round(quantity * unit_price * tax_rate / 100, 4)) STORED,
    CONSTRAINT sales_invoice_lines_line_no_uq UNIQUE (invoice_id, line_no)
);

CREATE INDEX sales_invoice_lines_invoice_id_idx ON sales_invoice_lines (invoice_id);
CREATE INDEX sales_invoice_lines_product_id_idx ON sales_invoice_lines (product_id);

-- Keep draft invoice headers in sync with their lines. Posted/void invoices are
-- frozen and left untouched.
CREATE FUNCTION trg_sales_invoice_recompute() RETURNS trigger AS $$
DECLARE
    v_invoice int;
    v_status  text;
BEGIN
    v_invoice := COALESCE(NEW.invoice_id, OLD.invoice_id);
    SELECT status INTO v_status FROM sales_invoices WHERE id = v_invoice;
    IF v_status IS NULL OR v_status <> 'draft' THEN
        RETURN NULL;
    END IF;
    UPDATE sales_invoices si SET
        subtotal  = COALESCE(t.sub, 0),
        tax_total = COALESCE(t.tax, 0),
        total     = COALESCE(t.tot, 0)
    FROM (
        SELECT sum(line_subtotal) AS sub, sum(tax_amount) AS tax, sum(line_total) AS tot
        FROM sales_invoice_lines WHERE invoice_id = v_invoice
    ) t
    WHERE si.id = v_invoice;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER sales_invoice_lines_recompute
    AFTER INSERT OR UPDATE OR DELETE ON sales_invoice_lines
    FOR EACH ROW EXECUTE FUNCTION trg_sales_invoice_recompute();

-- ---------------------------------------------------------------------------
-- Payments / receipts
-- ---------------------------------------------------------------------------

CREATE TABLE customer_payments (
    id                 int         GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    customer_id        int         NOT NULL REFERENCES customers(id),
    payment_date       date        NOT NULL,
    currency_code      char(3)     NOT NULL REFERENCES currencies(code),
    amount             numeric(19,4) NOT NULL CHECK (amount > 0),
    method             text        CHECK (method IN ('cash', 'check', 'card', 'transfer', 'other')),
    reference          text,
    status             text        NOT NULL DEFAULT 'draft'
                                   CHECK (status IN ('draft', 'posted', 'void')),
    period_id          int         REFERENCES accounting_periods(id),
    journal_entry_id   int         REFERENCES journal_entries(id),
    deposit_account_id int         REFERENCES accounts(id),   -- cash/bank account debited
    created_by         int         REFERENCES users(id),
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX customer_payments_customer_id_idx ON customer_payments (customer_id);

CREATE TRIGGER set_updated_at BEFORE UPDATE ON customer_payments
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE payment_applications (
    id             int           GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    payment_id     int           NOT NULL REFERENCES customer_payments(id) ON DELETE CASCADE,
    invoice_id     int           NOT NULL REFERENCES sales_invoices(id),
    amount_applied numeric(19,4) NOT NULL CHECK (amount_applied > 0),
    created_at     timestamptz   NOT NULL DEFAULT now(),
    CONSTRAINT payment_applications_uq UNIQUE (payment_id, invoice_id)
);

CREATE INDEX payment_applications_payment_id_idx ON payment_applications (payment_id);
CREATE INDEX payment_applications_invoice_id_idx ON payment_applications (invoice_id);

-- A payment and the invoice it pays must belong to the same customer and
-- currency, and neither the payment nor the invoice may be over-applied.
CREATE FUNCTION trg_payment_application_check() RETURNS trigger AS $$
DECLARE
    v_payment      int;
    v_invoice      int;
    v_pay_cust     int;
    v_pay_ccy      char(3);
    v_pay_amount   numeric(19,4);
    v_inv_cust     int;
    v_inv_ccy      char(3);
    v_inv_total    numeric(19,4);
    v_applied      numeric(19,4);
BEGIN
    v_payment := COALESCE(NEW.payment_id, OLD.payment_id);
    v_invoice := COALESCE(NEW.invoice_id, OLD.invoice_id);

    IF TG_OP <> 'DELETE' THEN
        SELECT customer_id, currency_code, amount
            INTO v_pay_cust, v_pay_ccy, v_pay_amount
            FROM customer_payments WHERE id = v_payment;
        SELECT customer_id, currency_code, total
            INTO v_inv_cust, v_inv_ccy, v_inv_total
            FROM sales_invoices WHERE id = v_invoice;

        IF v_pay_cust <> v_inv_cust THEN
            RAISE EXCEPTION 'payment % and invoice % belong to different customers',
                v_payment, v_invoice;
        END IF;
        IF v_pay_ccy <> v_inv_ccy THEN
            RAISE EXCEPTION 'payment % (%) and invoice % (%) are in different currencies',
                v_payment, v_pay_ccy, v_invoice, v_inv_ccy;
        END IF;
    END IF;

    -- Payment not over-applied.
    SELECT amount INTO v_pay_amount FROM customer_payments WHERE id = v_payment;
    IF v_pay_amount IS NOT NULL THEN
        SELECT COALESCE(sum(amount_applied), 0) INTO v_applied
            FROM payment_applications WHERE payment_id = v_payment;
        IF v_applied > v_pay_amount THEN
            RAISE EXCEPTION 'payment % over-applied: applied %, available %',
                v_payment, v_applied, v_pay_amount;
        END IF;
    END IF;

    -- Invoice not over-applied.
    SELECT total INTO v_inv_total FROM sales_invoices WHERE id = v_invoice;
    IF v_inv_total IS NOT NULL THEN
        SELECT COALESCE(sum(amount_applied), 0) INTO v_applied
            FROM payment_applications WHERE invoice_id = v_invoice;
        IF v_applied > v_inv_total THEN
            RAISE EXCEPTION 'invoice % over-applied: applied %, invoice total %',
                v_invoice, v_applied, v_inv_total;
        END IF;
    END IF;

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE CONSTRAINT TRIGGER payment_applications_check
    AFTER INSERT OR UPDATE OR DELETE ON payment_applications
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION trg_payment_application_check();

-- ---------------------------------------------------------------------------
-- Reporting convenience
-- ---------------------------------------------------------------------------

-- Outstanding balance and payment status per invoice. Only *posted* payments
-- count toward what's been collected.
CREATE VIEW sales_invoice_balances AS
SELECT si.id            AS invoice_id,
       si.invoice_number,
       si.customer_id,
       si.currency_code,
       si.invoice_date,
       si.due_date,
       si.status,
       si.total,
       COALESCE(pa.applied, 0)              AS amount_applied,
       si.total - COALESCE(pa.applied, 0)   AS balance,
       CASE
           WHEN si.status = 'void'                                  THEN 'void'
           WHEN si.total > 0 AND si.total - COALESCE(pa.applied, 0) <= 0 THEN 'paid'
           WHEN COALESCE(pa.applied, 0) > 0                         THEN 'partial'
           ELSE 'unpaid'
       END AS payment_status
FROM sales_invoices si
LEFT JOIN (
    SELECT pa.invoice_id, sum(pa.amount_applied) AS applied
    FROM payment_applications pa
    JOIN customer_payments cp ON cp.id = pa.payment_id AND cp.status = 'posted'
    GROUP BY pa.invoice_id
) pa ON pa.invoice_id = si.id;

-- Accounts-receivable aging by customer, bucketed on due date as of today.
CREATE VIEW ar_aging AS
SELECT b.customer_id,
       sum(b.balance)                                                                       AS total_outstanding,
       sum(b.balance) FILTER (WHERE b.due_date IS NULL OR b.due_date >= current_date)        AS not_yet_due,
       sum(b.balance) FILTER (WHERE b.due_date <  current_date      AND b.due_date >= current_date - 30) AS days_1_30,
       sum(b.balance) FILTER (WHERE b.due_date <  current_date - 30 AND b.due_date >= current_date - 60) AS days_31_60,
       sum(b.balance) FILTER (WHERE b.due_date <  current_date - 60 AND b.due_date >= current_date - 90) AS days_61_90,
       sum(b.balance) FILTER (WHERE b.due_date <  current_date - 90)                         AS days_over_90
FROM sales_invoice_balances b
WHERE b.status = 'posted' AND b.balance > 0
GROUP BY b.customer_id;
