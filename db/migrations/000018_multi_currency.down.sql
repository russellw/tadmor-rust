-- Revert 000018_multi_currency.

DROP TRIGGER purchase_credit_applications_both_posted ON purchase_credit_applications;
DROP TRIGGER sales_credit_applications_both_posted ON sales_credit_applications;
DROP TRIGGER bill_applications_both_posted ON bill_applications;
DROP TRIGGER payment_applications_both_posted ON payment_applications;
DROP FUNCTION trg_application_both_posted();

ALTER TABLE purchase_credit_applications DROP COLUMN fx_journal_entry_id;
ALTER TABLE sales_credit_applications    DROP COLUMN fx_journal_entry_id;
ALTER TABLE bill_applications            DROP COLUMN fx_journal_entry_id;
ALTER TABLE payment_applications         DROP COLUMN fx_journal_entry_id;

-- Restore 000004's trial balance over transaction amounts.
CREATE OR REPLACE VIEW trial_balance AS
SELECT a.id   AS account_id,
       a.code,
       a.name,
       a.account_type,
       COALESCE(sum(jl.debit)  FILTER (WHERE je.status = 'posted'), 0) AS total_debit,
       COALESCE(sum(jl.credit) FILTER (WHERE je.status = 'posted'), 0) AS total_credit,
       COALESCE(sum(jl.debit - jl.credit) FILTER (WHERE je.status = 'posted'), 0) AS balance
FROM accounts a
LEFT JOIN journal_lines   jl ON jl.account_id = a.id
LEFT JOIN journal_entries je ON je.id = jl.journal_entry_id
GROUP BY a.id, a.code, a.name, a.account_type;

-- Restore 000004's balance assertion (transaction amounts only).
CREATE OR REPLACE FUNCTION accounting_assert_entry_balanced(p_entry_id int)
RETURNS void AS $$
DECLARE
    v_status text;
    v_debit  numeric(19,4);
    v_credit numeric(19,4);
BEGIN
    SELECT status INTO v_status FROM journal_entries WHERE id = p_entry_id;
    IF v_status IS NULL OR v_status <> 'posted' THEN
        RETURN;
    END IF;

    SELECT COALESCE(sum(debit), 0), COALESCE(sum(credit), 0)
        INTO v_debit, v_credit
        FROM journal_lines WHERE journal_entry_id = p_entry_id;

    IF v_debit = 0 AND v_credit = 0 THEN
        RAISE EXCEPTION 'journal entry % is posted but has no lines', p_entry_id;
    END IF;
    IF v_debit <> v_credit THEN
        RAISE EXCEPTION 'journal entry % is unbalanced: debits %, credits %',
            p_entry_id, v_debit, v_credit;
    END IF;
END;
$$ LANGUAGE plpgsql;

ALTER TABLE journal_lines
    DROP CONSTRAINT journal_lines_base_side,
    DROP CONSTRAINT journal_lines_base_nonneg,
    DROP COLUMN base_credit,
    DROP COLUMN base_debit;

ALTER TABLE journal_entries DROP COLUMN exchange_rate;

DROP TABLE exchange_rates;

DROP TRIGGER gl_settings_guard ON gl_settings;
DROP FUNCTION trg_gl_settings_guard();
DROP TABLE gl_settings;
