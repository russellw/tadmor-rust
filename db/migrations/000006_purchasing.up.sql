-- 000006_purchasing: the Purchasing / Accounts-Payable subledger.
--
-- This is the mirror image of the Sales / AR module (000005):
--   suppliers           the "supplier" role layered 1:1 onto an organization
--   purchase_bills       vendor bill header (+ lines)
--   purchase_bill_lines  bill detail; line money is computed by the database
--   supplier_payments    payments made to suppliers
--   bill_applications    allocation of payments to bills (m:n, partial OK)
--
-- As with sales, bills/payments link to the GL journal entry they produce via a
-- nullable FK; posting (Dr expense, Cr A/P; Dr A/P, Cr cash) is the service
-- layer's job. Lookups (payment_terms, tax_codes) and products are reused.

-- ---------------------------------------------------------------------------
-- Suppliers (a role on an organization)
-- ---------------------------------------------------------------------------

CREATE TABLE suppliers (
    id                 int         GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    organization_id    int         NOT NULL UNIQUE REFERENCES organizations(id),
    supplier_number    text        UNIQUE,                -- optional human-facing code
    ap_account_id      int         REFERENCES accounts(id),   -- A/P control account
    payment_terms_code text        REFERENCES payment_terms(code),
    currency_code      char(3)     REFERENCES currencies(code),
    tax_code           text        REFERENCES tax_codes(code),
    is_active          boolean     NOT NULL DEFAULT true,
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now()
);

CREATE TRIGGER set_updated_at BEFORE UPDATE ON suppliers
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ---------------------------------------------------------------------------
-- Bills
-- ---------------------------------------------------------------------------

CREATE TABLE purchase_bills (
    id               int         GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    -- The supplier's own invoice number: unique per supplier, not globally.
    bill_number      text        NOT NULL,
    supplier_id      int         NOT NULL REFERENCES suppliers(id),
    bill_date        date        NOT NULL,
    due_date         date,
    currency_code    char(3)     NOT NULL REFERENCES currencies(code),
    status           text        NOT NULL DEFAULT 'draft'
                                 CHECK (status IN ('draft', 'posted', 'void')),
    -- Set by the service layer when the bill is posted to the GL.
    period_id        int         REFERENCES accounting_periods(id),
    journal_entry_id int         REFERENCES journal_entries(id),
    -- Header money is maintained from the lines by trigger while in draft.
    subtotal         numeric(19,4) NOT NULL DEFAULT 0,
    tax_total        numeric(19,4) NOT NULL DEFAULT 0,
    total            numeric(19,4) NOT NULL DEFAULT 0,
    reference        text,                               -- our internal reference
    memo             text,
    created_by       int         REFERENCES users(id),
    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT purchase_bills_number_uq        UNIQUE (supplier_id, bill_number),
    CONSTRAINT purchase_bills_total_consistent CHECK (total = subtotal + tax_total),
    CONSTRAINT purchase_bills_due_after_date   CHECK (due_date IS NULL OR due_date >= bill_date)
);

CREATE INDEX purchase_bills_supplier_id_idx ON purchase_bills (supplier_id);
CREATE INDEX purchase_bills_status_idx      ON purchase_bills (status);
CREATE INDEX purchase_bills_due_date_idx    ON purchase_bills (due_date);

CREATE TRIGGER set_updated_at BEFORE UPDATE ON purchase_bills
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE purchase_bill_lines (
    id                 int           GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    bill_id            int           NOT NULL REFERENCES purchase_bills(id) ON DELETE CASCADE,
    line_no            int           NOT NULL,
    product_id         int           REFERENCES products(id),   -- NULL = free-form line
    description        text          NOT NULL,
    quantity           numeric(19,4) NOT NULL DEFAULT 1 CHECK (quantity <> 0),
    unit_cost          numeric(19,4) NOT NULL DEFAULT 0,
    expense_account_id int           REFERENCES accounts(id),
    tax_code           text          REFERENCES tax_codes(code),
    tax_rate           numeric(7,4)  NOT NULL DEFAULT 0 CHECK (tax_rate >= 0),  -- rate snapshot
    -- Line money is derived by the database so it can never drift from inputs.
    line_subtotal numeric(19,4)
        GENERATED ALWAYS AS (round(quantity * unit_cost, 4)) STORED,
    tax_amount    numeric(19,4)
        GENERATED ALWAYS AS (round(quantity * unit_cost * tax_rate / 100, 4)) STORED,
    line_total    numeric(19,4)
        GENERATED ALWAYS AS (round(quantity * unit_cost, 4)
                             + round(quantity * unit_cost * tax_rate / 100, 4)) STORED,
    CONSTRAINT purchase_bill_lines_line_no_uq UNIQUE (bill_id, line_no)
);

CREATE INDEX purchase_bill_lines_bill_id_idx    ON purchase_bill_lines (bill_id);
CREATE INDEX purchase_bill_lines_product_id_idx ON purchase_bill_lines (product_id);

-- Keep draft bill headers in sync with their lines. Posted/void bills are frozen.
CREATE FUNCTION trg_purchase_bill_recompute() RETURNS trigger AS $$
DECLARE
    v_bill   int;
    v_status text;
BEGIN
    v_bill := COALESCE(NEW.bill_id, OLD.bill_id);
    SELECT status INTO v_status FROM purchase_bills WHERE id = v_bill;
    IF v_status IS NULL OR v_status <> 'draft' THEN
        RETURN NULL;
    END IF;
    UPDATE purchase_bills pb SET
        subtotal  = COALESCE(t.sub, 0),
        tax_total = COALESCE(t.tax, 0),
        total     = COALESCE(t.tot, 0)
    FROM (
        SELECT sum(line_subtotal) AS sub, sum(tax_amount) AS tax, sum(line_total) AS tot
        FROM purchase_bill_lines WHERE bill_id = v_bill
    ) t
    WHERE pb.id = v_bill;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER purchase_bill_lines_recompute
    AFTER INSERT OR UPDATE OR DELETE ON purchase_bill_lines
    FOR EACH ROW EXECUTE FUNCTION trg_purchase_bill_recompute();

-- ---------------------------------------------------------------------------
-- Payments to suppliers
-- ---------------------------------------------------------------------------

CREATE TABLE supplier_payments (
    id                 int         GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    supplier_id        int         NOT NULL REFERENCES suppliers(id),
    payment_date       date        NOT NULL,
    currency_code      char(3)     NOT NULL REFERENCES currencies(code),
    amount             numeric(19,4) NOT NULL CHECK (amount > 0),
    method             text        CHECK (method IN ('cash', 'check', 'card', 'transfer', 'other')),
    reference          text,
    status             text        NOT NULL DEFAULT 'draft'
                                   CHECK (status IN ('draft', 'posted', 'void')),
    period_id          int         REFERENCES accounting_periods(id),
    journal_entry_id   int         REFERENCES journal_entries(id),
    payment_account_id int         REFERENCES accounts(id),   -- cash/bank account credited
    created_by         int         REFERENCES users(id),
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX supplier_payments_supplier_id_idx ON supplier_payments (supplier_id);

CREATE TRIGGER set_updated_at BEFORE UPDATE ON supplier_payments
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE bill_applications (
    id             int           GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    payment_id     int           NOT NULL REFERENCES supplier_payments(id) ON DELETE CASCADE,
    bill_id        int           NOT NULL REFERENCES purchase_bills(id),
    amount_applied numeric(19,4) NOT NULL CHECK (amount_applied > 0),
    created_at     timestamptz   NOT NULL DEFAULT now(),
    CONSTRAINT bill_applications_uq UNIQUE (payment_id, bill_id)
);

CREATE INDEX bill_applications_payment_id_idx ON bill_applications (payment_id);
CREATE INDEX bill_applications_bill_id_idx    ON bill_applications (bill_id);

-- A payment and the bill it pays must belong to the same supplier and currency,
-- and neither the payment nor the bill may be over-applied.
CREATE FUNCTION trg_bill_application_check() RETURNS trigger AS $$
DECLARE
    v_payment    int;
    v_bill       int;
    v_pay_supp   int;
    v_pay_ccy    char(3);
    v_pay_amount numeric(19,4);
    v_bill_supp  int;
    v_bill_ccy   char(3);
    v_bill_total numeric(19,4);
    v_applied    numeric(19,4);
BEGIN
    v_payment := COALESCE(NEW.payment_id, OLD.payment_id);
    v_bill    := COALESCE(NEW.bill_id, OLD.bill_id);

    IF TG_OP <> 'DELETE' THEN
        SELECT supplier_id, currency_code, amount
            INTO v_pay_supp, v_pay_ccy, v_pay_amount
            FROM supplier_payments WHERE id = v_payment;
        SELECT supplier_id, currency_code, total
            INTO v_bill_supp, v_bill_ccy, v_bill_total
            FROM purchase_bills WHERE id = v_bill;

        IF v_pay_supp <> v_bill_supp THEN
            RAISE EXCEPTION 'payment % and bill % belong to different suppliers',
                v_payment, v_bill;
        END IF;
        IF v_pay_ccy <> v_bill_ccy THEN
            RAISE EXCEPTION 'payment % (%) and bill % (%) are in different currencies',
                v_payment, v_pay_ccy, v_bill, v_bill_ccy;
        END IF;
    END IF;

    -- Payment not over-applied.
    SELECT amount INTO v_pay_amount FROM supplier_payments WHERE id = v_payment;
    IF v_pay_amount IS NOT NULL THEN
        SELECT COALESCE(sum(amount_applied), 0) INTO v_applied
            FROM bill_applications WHERE payment_id = v_payment;
        IF v_applied > v_pay_amount THEN
            RAISE EXCEPTION 'payment % over-applied: applied %, available %',
                v_payment, v_applied, v_pay_amount;
        END IF;
    END IF;

    -- Bill not over-applied.
    SELECT total INTO v_bill_total FROM purchase_bills WHERE id = v_bill;
    IF v_bill_total IS NOT NULL THEN
        SELECT COALESCE(sum(amount_applied), 0) INTO v_applied
            FROM bill_applications WHERE bill_id = v_bill;
        IF v_applied > v_bill_total THEN
            RAISE EXCEPTION 'bill % over-applied: applied %, bill total %',
                v_bill, v_applied, v_bill_total;
        END IF;
    END IF;

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE CONSTRAINT TRIGGER bill_applications_check
    AFTER INSERT OR UPDATE OR DELETE ON bill_applications
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION trg_bill_application_check();

-- ---------------------------------------------------------------------------
-- Reporting convenience
-- ---------------------------------------------------------------------------

-- Outstanding balance and payment status per bill. Only *posted* payments count
-- toward what's been paid.
CREATE VIEW purchase_bill_balances AS
SELECT pb.id          AS bill_id,
       pb.bill_number,
       pb.supplier_id,
       pb.currency_code,
       pb.bill_date,
       pb.due_date,
       pb.status,
       pb.total,
       COALESCE(ba.applied, 0)              AS amount_applied,
       pb.total - COALESCE(ba.applied, 0)   AS balance,
       CASE
           WHEN pb.status = 'void'                                       THEN 'void'
           WHEN pb.total > 0 AND pb.total - COALESCE(ba.applied, 0) <= 0 THEN 'paid'
           WHEN COALESCE(ba.applied, 0) > 0                              THEN 'partial'
           ELSE 'unpaid'
       END AS payment_status
FROM purchase_bills pb
LEFT JOIN (
    SELECT ba.bill_id, sum(ba.amount_applied) AS applied
    FROM bill_applications ba
    JOIN supplier_payments sp ON sp.id = ba.payment_id AND sp.status = 'posted'
    GROUP BY ba.bill_id
) ba ON ba.bill_id = pb.id;

-- Accounts-payable aging by supplier, bucketed on due date as of today.
CREATE VIEW ap_aging AS
SELECT b.supplier_id,
       sum(b.balance)                                                                       AS total_outstanding,
       sum(b.balance) FILTER (WHERE b.due_date IS NULL OR b.due_date >= current_date)        AS not_yet_due,
       sum(b.balance) FILTER (WHERE b.due_date <  current_date      AND b.due_date >= current_date - 30) AS days_1_30,
       sum(b.balance) FILTER (WHERE b.due_date <  current_date - 30 AND b.due_date >= current_date - 60) AS days_31_60,
       sum(b.balance) FILTER (WHERE b.due_date <  current_date - 60 AND b.due_date >= current_date - 90) AS days_61_90,
       sum(b.balance) FILTER (WHERE b.due_date <  current_date - 90)                         AS days_over_90
FROM purchase_bill_balances b
WHERE b.status = 'posted' AND b.balance > 0
GROUP BY b.supplier_id;
