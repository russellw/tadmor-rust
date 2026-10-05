-- 000012_credit_notes (down)

DROP VIEW purchase_credit_note_balances;
DROP VIEW sales_credit_note_balances;

-- Restore the original balance views (payments only), as created in
-- 000005/000006.
CREATE OR REPLACE VIEW sales_invoice_balances AS
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

CREATE OR REPLACE VIEW purchase_bill_balances AS
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

-- Restore the original over-application checks (payments only), as created in
-- 000005/000006.
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

DROP TABLE purchase_credit_applications;
DROP TABLE sales_credit_applications;
DROP FUNCTION trg_purchase_credit_application_check();
DROP FUNCTION trg_sales_credit_application_check();
DROP FUNCTION bill_amount_settled(int);
DROP FUNCTION invoice_amount_settled(int);

DROP TABLE purchase_credit_note_lines;
DROP TABLE sales_credit_note_lines;
DROP FUNCTION trg_purchase_credit_note_recompute();
DROP FUNCTION trg_sales_credit_note_recompute();
DROP TABLE purchase_credit_notes;
DROP TABLE sales_credit_notes;
