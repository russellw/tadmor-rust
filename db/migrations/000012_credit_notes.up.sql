-- 000012_credit_notes: credit notes for both subledgers.
--
--   sales_credit_notes / _lines        credit we issue to a customer
--   sales_credit_applications          allocation of a credit note to invoices
--   purchase_credit_notes / _lines     credit a supplier issues to us
--   purchase_credit_applications       allocation of a credit note to bills
--
-- A credit note is the posting-side mirror of its document: a sales credit
-- note credits A/R and debits revenue/tax; a purchase credit note debits A/P
-- and credits expense/tax. Applying a credit note to an invoice/bill is a pure
-- subledger allocation (like a payment application): the GL already carries
-- both sides, so applications create no journal entries.
--
-- Because an invoice can now be settled from two tables (payments and credit
-- notes), the over-application checks and the balance views are re-created
-- here to count both.

-- ---------------------------------------------------------------------------
-- Sales credit notes (issued by us; our numbering, globally unique)
-- ---------------------------------------------------------------------------

CREATE TABLE sales_credit_notes (
    id                 int         GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    credit_note_number text        NOT NULL UNIQUE,
    customer_id        int         NOT NULL REFERENCES customers(id),
    credit_note_date   date        NOT NULL,
    currency_code      char(3)     NOT NULL REFERENCES currencies(code),
    status             text        NOT NULL DEFAULT 'draft'
                                   CHECK (status IN ('draft', 'posted', 'void')),
    -- Set by the service layer when the credit note is posted to the GL.
    period_id          int         REFERENCES accounting_periods(id),
    journal_entry_id   int         REFERENCES journal_entries(id),
    -- Header money is maintained from the lines by trigger while in draft.
    subtotal           numeric(19,4) NOT NULL DEFAULT 0,
    tax_total          numeric(19,4) NOT NULL DEFAULT 0,
    total              numeric(19,4) NOT NULL DEFAULT 0,
    reference          text,                               -- e.g. the invoice being credited
    memo               text,
    created_by         int         REFERENCES users(id),
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT sales_credit_notes_total_consistent CHECK (total = subtotal + tax_total)
);

CREATE INDEX sales_credit_notes_customer_id_idx ON sales_credit_notes (customer_id);
CREATE INDEX sales_credit_notes_status_idx      ON sales_credit_notes (status);

CREATE TRIGGER set_updated_at BEFORE UPDATE ON sales_credit_notes
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE sales_credit_note_lines (
    id                 int           GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    credit_note_id     int           NOT NULL REFERENCES sales_credit_notes(id) ON DELETE CASCADE,
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
    CONSTRAINT sales_credit_note_lines_line_no_uq UNIQUE (credit_note_id, line_no)
);

CREATE INDEX sales_credit_note_lines_credit_note_id_idx ON sales_credit_note_lines (credit_note_id);
CREATE INDEX sales_credit_note_lines_product_id_idx     ON sales_credit_note_lines (product_id);

-- Keep draft credit-note headers in sync with their lines. Posted/void notes
-- are frozen and left untouched.
CREATE FUNCTION trg_sales_credit_note_recompute() RETURNS trigger AS $$
DECLARE
    v_note   int;
    v_status text;
BEGIN
    v_note := COALESCE(NEW.credit_note_id, OLD.credit_note_id);
    SELECT status INTO v_status FROM sales_credit_notes WHERE id = v_note;
    IF v_status IS NULL OR v_status <> 'draft' THEN
        RETURN NULL;
    END IF;
    UPDATE sales_credit_notes cn SET
        subtotal  = COALESCE(t.sub, 0),
        tax_total = COALESCE(t.tax, 0),
        total     = COALESCE(t.tot, 0)
    FROM (
        SELECT sum(line_subtotal) AS sub, sum(tax_amount) AS tax, sum(line_total) AS tot
        FROM sales_credit_note_lines WHERE credit_note_id = v_note
    ) t
    WHERE cn.id = v_note;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER sales_credit_note_lines_recompute
    AFTER INSERT OR UPDATE OR DELETE ON sales_credit_note_lines
    FOR EACH ROW EXECUTE FUNCTION trg_sales_credit_note_recompute();

-- ---------------------------------------------------------------------------
-- Purchase credit notes (issued by the supplier; their numbering, unique
-- per supplier)
-- ---------------------------------------------------------------------------

CREATE TABLE purchase_credit_notes (
    id                 int         GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    credit_note_number text        NOT NULL,
    supplier_id        int         NOT NULL REFERENCES suppliers(id),
    credit_note_date   date        NOT NULL,
    currency_code      char(3)     NOT NULL REFERENCES currencies(code),
    status             text        NOT NULL DEFAULT 'draft'
                                   CHECK (status IN ('draft', 'posted', 'void')),
    period_id          int         REFERENCES accounting_periods(id),
    journal_entry_id   int         REFERENCES journal_entries(id),
    subtotal           numeric(19,4) NOT NULL DEFAULT 0,
    tax_total          numeric(19,4) NOT NULL DEFAULT 0,
    total              numeric(19,4) NOT NULL DEFAULT 0,
    reference          text,                               -- our internal reference
    memo               text,
    created_by         int         REFERENCES users(id),
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT purchase_credit_notes_number_uq        UNIQUE (supplier_id, credit_note_number),
    CONSTRAINT purchase_credit_notes_total_consistent CHECK (total = subtotal + tax_total)
);

CREATE INDEX purchase_credit_notes_supplier_id_idx ON purchase_credit_notes (supplier_id);
CREATE INDEX purchase_credit_notes_status_idx      ON purchase_credit_notes (status);

CREATE TRIGGER set_updated_at BEFORE UPDATE ON purchase_credit_notes
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE purchase_credit_note_lines (
    id                 int           GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    credit_note_id     int           NOT NULL REFERENCES purchase_credit_notes(id) ON DELETE CASCADE,
    line_no            int           NOT NULL,
    product_id         int           REFERENCES products(id),   -- NULL = free-form line
    description        text          NOT NULL,
    quantity           numeric(19,4) NOT NULL DEFAULT 1 CHECK (quantity <> 0),
    unit_cost          numeric(19,4) NOT NULL DEFAULT 0,
    expense_account_id int           REFERENCES accounts(id),
    tax_code           text          REFERENCES tax_codes(code),
    tax_rate           numeric(7,4)  NOT NULL DEFAULT 0 CHECK (tax_rate >= 0),  -- rate snapshot
    line_subtotal numeric(19,4)
        GENERATED ALWAYS AS (round(quantity * unit_cost, 4)) STORED,
    tax_amount    numeric(19,4)
        GENERATED ALWAYS AS (round(quantity * unit_cost * tax_rate / 100, 4)) STORED,
    line_total    numeric(19,4)
        GENERATED ALWAYS AS (round(quantity * unit_cost, 4)
                             + round(quantity * unit_cost * tax_rate / 100, 4)) STORED,
    CONSTRAINT purchase_credit_note_lines_line_no_uq UNIQUE (credit_note_id, line_no)
);

CREATE INDEX purchase_credit_note_lines_credit_note_id_idx ON purchase_credit_note_lines (credit_note_id);
CREATE INDEX purchase_credit_note_lines_product_id_idx     ON purchase_credit_note_lines (product_id);

CREATE FUNCTION trg_purchase_credit_note_recompute() RETURNS trigger AS $$
DECLARE
    v_note   int;
    v_status text;
BEGIN
    v_note := COALESCE(NEW.credit_note_id, OLD.credit_note_id);
    SELECT status INTO v_status FROM purchase_credit_notes WHERE id = v_note;
    IF v_status IS NULL OR v_status <> 'draft' THEN
        RETURN NULL;
    END IF;
    UPDATE purchase_credit_notes cn SET
        subtotal  = COALESCE(t.sub, 0),
        tax_total = COALESCE(t.tax, 0),
        total     = COALESCE(t.tot, 0)
    FROM (
        SELECT sum(line_subtotal) AS sub, sum(tax_amount) AS tax, sum(line_total) AS tot
        FROM purchase_credit_note_lines WHERE credit_note_id = v_note
    ) t
    WHERE cn.id = v_note;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER purchase_credit_note_lines_recompute
    AFTER INSERT OR UPDATE OR DELETE ON purchase_credit_note_lines
    FOR EACH ROW EXECUTE FUNCTION trg_purchase_credit_note_recompute();

-- ---------------------------------------------------------------------------
-- Applications: allocating a credit note to invoices / bills
-- ---------------------------------------------------------------------------

CREATE TABLE sales_credit_applications (
    id             int           GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    credit_note_id int           NOT NULL REFERENCES sales_credit_notes(id) ON DELETE CASCADE,
    invoice_id     int           NOT NULL REFERENCES sales_invoices(id),
    amount_applied numeric(19,4) NOT NULL CHECK (amount_applied > 0),
    created_at     timestamptz   NOT NULL DEFAULT now(),
    CONSTRAINT sales_credit_applications_uq UNIQUE (credit_note_id, invoice_id)
);

CREATE INDEX sales_credit_applications_credit_note_id_idx ON sales_credit_applications (credit_note_id);
CREATE INDEX sales_credit_applications_invoice_id_idx     ON sales_credit_applications (invoice_id);

CREATE TABLE purchase_credit_applications (
    id             int           GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    credit_note_id int           NOT NULL REFERENCES purchase_credit_notes(id) ON DELETE CASCADE,
    bill_id        int           NOT NULL REFERENCES purchase_bills(id),
    amount_applied numeric(19,4) NOT NULL CHECK (amount_applied > 0),
    created_at     timestamptz   NOT NULL DEFAULT now(),
    CONSTRAINT purchase_credit_applications_uq UNIQUE (credit_note_id, bill_id)
);

CREATE INDEX purchase_credit_applications_credit_note_id_idx ON purchase_credit_applications (credit_note_id);
CREATE INDEX purchase_credit_applications_bill_id_idx        ON purchase_credit_applications (bill_id);

-- An invoice's settled amount now spans payments and credit notes; these
-- helpers total both so every over-application check sees the whole picture.
CREATE FUNCTION invoice_amount_settled(p_invoice int) RETURNS numeric AS $$
    SELECT COALESCE((SELECT sum(amount_applied) FROM payment_applications
                     WHERE invoice_id = p_invoice), 0)
         + COALESCE((SELECT sum(amount_applied) FROM sales_credit_applications
                     WHERE invoice_id = p_invoice), 0);
$$ LANGUAGE sql STABLE;

CREATE FUNCTION bill_amount_settled(p_bill int) RETURNS numeric AS $$
    SELECT COALESCE((SELECT sum(amount_applied) FROM bill_applications
                     WHERE bill_id = p_bill), 0)
         + COALESCE((SELECT sum(amount_applied) FROM purchase_credit_applications
                     WHERE bill_id = p_bill), 0);
$$ LANGUAGE sql STABLE;

-- A credit note and the invoice it credits must belong to the same customer
-- and currency, and neither the credit note nor the invoice may be
-- over-applied.
CREATE FUNCTION trg_sales_credit_application_check() RETURNS trigger AS $$
DECLARE
    v_note       int;
    v_invoice    int;
    v_note_cust  int;
    v_note_ccy   char(3);
    v_note_total numeric(19,4);
    v_inv_cust   int;
    v_inv_ccy    char(3);
    v_inv_total  numeric(19,4);
    v_applied    numeric(19,4);
BEGIN
    v_note    := COALESCE(NEW.credit_note_id, OLD.credit_note_id);
    v_invoice := COALESCE(NEW.invoice_id, OLD.invoice_id);

    IF TG_OP <> 'DELETE' THEN
        SELECT customer_id, currency_code, total
            INTO v_note_cust, v_note_ccy, v_note_total
            FROM sales_credit_notes WHERE id = v_note;
        SELECT customer_id, currency_code, total
            INTO v_inv_cust, v_inv_ccy, v_inv_total
            FROM sales_invoices WHERE id = v_invoice;

        IF v_note_cust <> v_inv_cust THEN
            RAISE EXCEPTION 'credit note % and invoice % belong to different customers',
                v_note, v_invoice;
        END IF;
        IF v_note_ccy <> v_inv_ccy THEN
            RAISE EXCEPTION 'credit note % (%) and invoice % (%) are in different currencies',
                v_note, v_note_ccy, v_invoice, v_inv_ccy;
        END IF;
    END IF;

    -- Credit note not over-applied.
    SELECT total INTO v_note_total FROM sales_credit_notes WHERE id = v_note;
    IF v_note_total IS NOT NULL THEN
        SELECT COALESCE(sum(amount_applied), 0) INTO v_applied
            FROM sales_credit_applications WHERE credit_note_id = v_note;
        IF v_applied > v_note_total THEN
            RAISE EXCEPTION 'credit note % over-applied: applied %, available %',
                v_note, v_applied, v_note_total;
        END IF;
    END IF;

    -- Invoice not over-applied (payments and credit notes combined).
    SELECT total INTO v_inv_total FROM sales_invoices WHERE id = v_invoice;
    IF v_inv_total IS NOT NULL AND invoice_amount_settled(v_invoice) > v_inv_total THEN
        RAISE EXCEPTION 'invoice % over-applied: applied %, invoice total %',
            v_invoice, invoice_amount_settled(v_invoice), v_inv_total;
    END IF;

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE CONSTRAINT TRIGGER sales_credit_applications_check
    AFTER INSERT OR UPDATE OR DELETE ON sales_credit_applications
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION trg_sales_credit_application_check();

CREATE FUNCTION trg_purchase_credit_application_check() RETURNS trigger AS $$
DECLARE
    v_note       int;
    v_bill       int;
    v_note_supp  int;
    v_note_ccy   char(3);
    v_note_total numeric(19,4);
    v_bill_supp  int;
    v_bill_ccy   char(3);
    v_bill_total numeric(19,4);
    v_applied    numeric(19,4);
BEGIN
    v_note := COALESCE(NEW.credit_note_id, OLD.credit_note_id);
    v_bill := COALESCE(NEW.bill_id, OLD.bill_id);

    IF TG_OP <> 'DELETE' THEN
        SELECT supplier_id, currency_code, total
            INTO v_note_supp, v_note_ccy, v_note_total
            FROM purchase_credit_notes WHERE id = v_note;
        SELECT supplier_id, currency_code, total
            INTO v_bill_supp, v_bill_ccy, v_bill_total
            FROM purchase_bills WHERE id = v_bill;

        IF v_note_supp <> v_bill_supp THEN
            RAISE EXCEPTION 'credit note % and bill % belong to different suppliers',
                v_note, v_bill;
        END IF;
        IF v_note_ccy <> v_bill_ccy THEN
            RAISE EXCEPTION 'credit note % (%) and bill % (%) are in different currencies',
                v_note, v_note_ccy, v_bill, v_bill_ccy;
        END IF;
    END IF;

    -- Credit note not over-applied.
    SELECT total INTO v_note_total FROM purchase_credit_notes WHERE id = v_note;
    IF v_note_total IS NOT NULL THEN
        SELECT COALESCE(sum(amount_applied), 0) INTO v_applied
            FROM purchase_credit_applications WHERE credit_note_id = v_note;
        IF v_applied > v_note_total THEN
            RAISE EXCEPTION 'credit note % over-applied: applied %, available %',
                v_note, v_applied, v_note_total;
        END IF;
    END IF;

    -- Bill not over-applied (payments and credit notes combined).
    SELECT total INTO v_bill_total FROM purchase_bills WHERE id = v_bill;
    IF v_bill_total IS NOT NULL AND bill_amount_settled(v_bill) > v_bill_total THEN
        RAISE EXCEPTION 'bill % over-applied: applied %, bill total %',
            v_bill, bill_amount_settled(v_bill), v_bill_total;
    END IF;

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE CONSTRAINT TRIGGER purchase_credit_applications_check
    AFTER INSERT OR UPDATE OR DELETE ON purchase_credit_applications
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION trg_purchase_credit_application_check();

-- The payment-application checks (000005/000006) must also count credit
-- applications when testing invoice/bill over-application; only that part of
-- each function body changes.
CREATE OR REPLACE FUNCTION trg_payment_application_check() RETURNS trigger AS $$
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

    -- Invoice not over-applied (payments and credit notes combined).
    SELECT total INTO v_inv_total FROM sales_invoices WHERE id = v_invoice;
    IF v_inv_total IS NOT NULL AND invoice_amount_settled(v_invoice) > v_inv_total THEN
        RAISE EXCEPTION 'invoice % over-applied: applied %, invoice total %',
            v_invoice, invoice_amount_settled(v_invoice), v_inv_total;
    END IF;

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION trg_bill_application_check() RETURNS trigger AS $$
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

    -- Bill not over-applied (payments and credit notes combined).
    SELECT total INTO v_bill_total FROM purchase_bills WHERE id = v_bill;
    IF v_bill_total IS NOT NULL AND bill_amount_settled(v_bill) > v_bill_total THEN
        RAISE EXCEPTION 'bill % over-applied: applied %, bill total %',
            v_bill, bill_amount_settled(v_bill), v_bill_total;
    END IF;

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

-- ---------------------------------------------------------------------------
-- Reporting convenience
-- ---------------------------------------------------------------------------

-- The invoice/bill balance views now count posted credit notes alongside
-- posted payments. Column lists are unchanged, so dependents (ar_aging,
-- ap_aging) pick the new logic up automatically.
CREATE OR REPLACE VIEW sales_invoice_balances AS
SELECT si.id            AS invoice_id,
       si.invoice_number,
       si.customer_id,
       si.currency_code,
       si.invoice_date,
       si.due_date,
       si.status,
       si.total,
       COALESCE(pa.applied, 0) + COALESCE(ca.applied, 0)              AS amount_applied,
       si.total - COALESCE(pa.applied, 0) - COALESCE(ca.applied, 0)   AS balance,
       CASE
           WHEN si.status = 'void' THEN 'void'
           WHEN si.total > 0
                AND si.total - COALESCE(pa.applied, 0) - COALESCE(ca.applied, 0) <= 0 THEN 'paid'
           WHEN COALESCE(pa.applied, 0) + COALESCE(ca.applied, 0) > 0 THEN 'partial'
           ELSE 'unpaid'
       END AS payment_status
FROM sales_invoices si
LEFT JOIN (
    SELECT pa.invoice_id, sum(pa.amount_applied) AS applied
    FROM payment_applications pa
    JOIN customer_payments cp ON cp.id = pa.payment_id AND cp.status = 'posted'
    GROUP BY pa.invoice_id
) pa ON pa.invoice_id = si.id
LEFT JOIN (
    SELECT ca.invoice_id, sum(ca.amount_applied) AS applied
    FROM sales_credit_applications ca
    JOIN sales_credit_notes cn ON cn.id = ca.credit_note_id AND cn.status = 'posted'
    GROUP BY ca.invoice_id
) ca ON ca.invoice_id = si.id;

CREATE OR REPLACE VIEW purchase_bill_balances AS
SELECT pb.id          AS bill_id,
       pb.bill_number,
       pb.supplier_id,
       pb.currency_code,
       pb.bill_date,
       pb.due_date,
       pb.status,
       pb.total,
       COALESCE(ba.applied, 0) + COALESCE(ca.applied, 0)              AS amount_applied,
       pb.total - COALESCE(ba.applied, 0) - COALESCE(ca.applied, 0)   AS balance,
       CASE
           WHEN pb.status = 'void' THEN 'void'
           WHEN pb.total > 0
                AND pb.total - COALESCE(ba.applied, 0) - COALESCE(ca.applied, 0) <= 0 THEN 'paid'
           WHEN COALESCE(ba.applied, 0) + COALESCE(ca.applied, 0) > 0 THEN 'partial'
           ELSE 'unpaid'
       END AS payment_status
FROM purchase_bills pb
LEFT JOIN (
    SELECT ba.bill_id, sum(ba.amount_applied) AS applied
    FROM bill_applications ba
    JOIN supplier_payments sp ON sp.id = ba.payment_id AND sp.status = 'posted'
    GROUP BY ba.bill_id
) ba ON ba.bill_id = pb.id
LEFT JOIN (
    SELECT ca.bill_id, sum(ca.amount_applied) AS applied
    FROM purchase_credit_applications ca
    JOIN purchase_credit_notes cn ON cn.id = ca.credit_note_id AND cn.status = 'posted'
    GROUP BY ca.bill_id
) ca ON ca.bill_id = pb.id;

-- Remaining (unapplied) credit per credit note. Applications count regardless
-- of the note's status here: they are the note's own allocations.
CREATE VIEW sales_credit_note_balances AS
SELECT cn.id            AS credit_note_id,
       cn.credit_note_number,
       cn.customer_id,
       cn.currency_code,
       cn.credit_note_date,
       cn.status,
       cn.total,
       COALESCE(ca.applied, 0)             AS amount_applied,
       cn.total - COALESCE(ca.applied, 0)  AS balance,
       CASE
           WHEN cn.status = 'void'                                       THEN 'void'
           WHEN cn.total > 0 AND cn.total - COALESCE(ca.applied, 0) <= 0 THEN 'applied'
           WHEN COALESCE(ca.applied, 0) > 0                              THEN 'partial'
           ELSE 'open'
       END AS application_status
FROM sales_credit_notes cn
LEFT JOIN (
    SELECT credit_note_id, sum(amount_applied) AS applied
    FROM sales_credit_applications GROUP BY credit_note_id
) ca ON ca.credit_note_id = cn.id;

CREATE VIEW purchase_credit_note_balances AS
SELECT cn.id            AS credit_note_id,
       cn.credit_note_number,
       cn.supplier_id,
       cn.currency_code,
       cn.credit_note_date,
       cn.status,
       cn.total,
       COALESCE(ca.applied, 0)             AS amount_applied,
       cn.total - COALESCE(ca.applied, 0)  AS balance,
       CASE
           WHEN cn.status = 'void'                                       THEN 'void'
           WHEN cn.total > 0 AND cn.total - COALESCE(ca.applied, 0) <= 0 THEN 'applied'
           WHEN COALESCE(ca.applied, 0) > 0                              THEN 'partial'
           ELSE 'open'
       END AS application_status
FROM purchase_credit_notes cn
LEFT JOIN (
    SELECT credit_note_id, sum(amount_applied) AS applied
    FROM purchase_credit_applications GROUP BY credit_note_id
) ca ON ca.credit_note_id = cn.id;
