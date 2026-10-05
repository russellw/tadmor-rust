-- 000018_multi_currency: give the ledger a base (functional) currency.
--
-- Model:
--   gl_settings     one-row table: the base currency and the account that
--                   absorbs realized exchange gains/losses
--   exchange_rates  manually maintained rates, one per currency per date;
--                   posting uses the latest rate on or before the document
--                   date (rate = base-currency units per 1 foreign unit)
--
-- Journal entries keep their transaction currency and now record the
-- exchange rate they were converted at; journal lines carry both the
-- transaction amounts (debit/credit — what bank reconciliation matches
-- against) and the base-currency amounts (base_debit/base_credit — what
-- every financial report sums). Applications (payment→invoice and the
-- credit-note/AP mirrors) may link to a realized-FX journal entry posted
-- when the two documents carried different rates.
--
-- Invariants enforced in the database, not just the app:
--   * the base currency cannot change once journal entries exist;
--   * a posted entry balances in base amounts as well as transaction ones;
--   * a base amount sits on the same side as its transaction amount;
--   * applications may only be created between two *posted* documents
--     (their journal entries are where the FX rates come from).

-- ---------------------------------------------------------------------------
-- Settings
-- ---------------------------------------------------------------------------

CREATE TABLE gl_settings (
    id                      int     PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    base_currency           char(3) NOT NULL REFERENCES currencies(code),
    fx_gain_loss_account_id int     REFERENCES accounts(id),
    updated_at              timestamptz NOT NULL DEFAULT now()
);

CREATE TRIGGER set_updated_at BEFORE UPDATE ON gl_settings
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- Seed: the ledger's predominant entry currency (all existing entries were
-- posted 1:1, i.e. in what is now declared the base), falling back to USD on
-- an empty ledger. The FX gain/loss account is created if its code is free.
WITH fx AS (
    INSERT INTO accounts (code, name, account_type, is_postable)
    VALUES ('7000', 'Foreign Exchange Gain/Loss', 'expense', true)
    ON CONFLICT (code) DO NOTHING
    RETURNING id
)
INSERT INTO gl_settings (base_currency, fx_gain_loss_account_id)
SELECT COALESCE(
           (SELECT currency_code FROM journal_entries
            GROUP BY currency_code ORDER BY count(*) DESC, currency_code LIMIT 1),
           'USD'),
       (SELECT id FROM fx);

-- The base currency is the unit every stored base amount is denominated in:
-- once any journal entry exists, changing it would silently redenominate
-- history. The row itself must also stay put.
CREATE FUNCTION trg_gl_settings_guard() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'gl_settings must not be deleted';
    END IF;
    IF NEW.base_currency <> OLD.base_currency
       AND EXISTS (SELECT 1 FROM journal_entries) THEN
        RAISE EXCEPTION 'base currency cannot change once journal entries exist';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER gl_settings_guard BEFORE UPDATE OR DELETE ON gl_settings
    FOR EACH ROW EXECUTE FUNCTION trg_gl_settings_guard();

-- ---------------------------------------------------------------------------
-- Exchange rates
-- ---------------------------------------------------------------------------

-- One rate per currency per date, maintained by hand: rate is how many
-- base-currency units one unit of currency_code buys. Posting a document
-- dated D in a foreign currency uses that currency's latest rate on or
-- before D and fails when there is none. Editing a rate never rewrites
-- posted history — entries store the rate they actually used.
CREATE TABLE exchange_rates (
    currency_code char(3)       NOT NULL REFERENCES currencies(code),
    rate_date     date          NOT NULL,
    rate          numeric(19,8) NOT NULL CHECK (rate > 0),
    created_at    timestamptz   NOT NULL DEFAULT now(),
    updated_at    timestamptz   NOT NULL DEFAULT now(),
    PRIMARY KEY (currency_code, rate_date)
);

CREATE TRIGGER set_updated_at BEFORE UPDATE ON exchange_rates
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ---------------------------------------------------------------------------
-- Journal: dual amounts
-- ---------------------------------------------------------------------------

-- The rate the entry's transaction currency was converted to base at
-- (1 for base-currency entries, and for all pre-multi-currency history).
ALTER TABLE journal_entries
    ADD COLUMN exchange_rate numeric(19,8) NOT NULL DEFAULT 1
        CONSTRAINT journal_entries_rate_positive CHECK (exchange_rate > 0);

-- Add the base columns and their checks together, before the backfill: the
-- backfill UPDATE queues deferred balance-trigger events on journal_lines,
-- and Postgres forbids ALTER TABLE on a table with pending trigger events, so
-- every journal_lines DDL statement must run first. The checks hold at add
-- time because the columns start at 0 (base amount 0 sits on either side).
ALTER TABLE journal_lines
    ADD COLUMN base_debit  numeric(19,4) NOT NULL DEFAULT 0,
    ADD COLUMN base_credit numeric(19,4) NOT NULL DEFAULT 0,
    ADD CONSTRAINT journal_lines_base_nonneg
        CHECK (base_debit >= 0 AND base_credit >= 0),
    -- A base amount may round to zero, but never lands on the opposite side
    -- of its transaction amount.
    ADD CONSTRAINT journal_lines_base_side
        CHECK ((base_debit = 0 OR debit > 0) AND (base_credit = 0 OR credit > 0));

-- Everything posted so far was implicitly in the base currency.
UPDATE journal_lines SET base_debit = debit, base_credit = credit;

-- A posted entry must now balance in base amounts too. (Replaces 000004's
-- version; the deferred triggers calling it are unchanged.)
CREATE OR REPLACE FUNCTION accounting_assert_entry_balanced(p_entry_id int)
RETURNS void AS $$
DECLARE
    v_status      text;
    v_debit       numeric(19,4);
    v_credit      numeric(19,4);
    v_base_debit  numeric(19,4);
    v_base_credit numeric(19,4);
BEGIN
    SELECT status INTO v_status FROM journal_entries WHERE id = p_entry_id;
    -- Entry gone (e.g. cascade delete) or still a draft: nothing to enforce.
    IF v_status IS NULL OR v_status <> 'posted' THEN
        RETURN;
    END IF;

    SELECT COALESCE(sum(debit), 0), COALESCE(sum(credit), 0),
           COALESCE(sum(base_debit), 0), COALESCE(sum(base_credit), 0)
        INTO v_debit, v_credit, v_base_debit, v_base_credit
        FROM journal_lines WHERE journal_entry_id = p_entry_id;

    IF v_debit = 0 AND v_credit = 0 THEN
        RAISE EXCEPTION 'journal entry % is posted but has no lines', p_entry_id;
    END IF;
    IF v_debit <> v_credit THEN
        RAISE EXCEPTION 'journal entry % is unbalanced: debits %, credits %',
            p_entry_id, v_debit, v_credit;
    END IF;
    IF v_base_debit <> v_base_credit THEN
        RAISE EXCEPTION 'journal entry % is unbalanced in base currency: debits %, credits %',
            p_entry_id, v_base_debit, v_base_credit;
    END IF;
END;
$$ LANGUAGE plpgsql;

-- The trial balance — and every report built on it — reads base amounts, so
-- mixed-currency activity aggregates in one unit.
CREATE OR REPLACE VIEW trial_balance AS
SELECT a.id   AS account_id,
       a.code,
       a.name,
       a.account_type,
       COALESCE(sum(jl.base_debit)  FILTER (WHERE je.status = 'posted'), 0) AS total_debit,
       COALESCE(sum(jl.base_credit) FILTER (WHERE je.status = 'posted'), 0) AS total_credit,
       COALESCE(sum(jl.base_debit - jl.base_credit) FILTER (WHERE je.status = 'posted'), 0) AS balance
FROM accounts a
LEFT JOIN journal_lines   jl ON jl.account_id = a.id
LEFT JOIN journal_entries je ON je.id = jl.journal_entry_id
GROUP BY a.id, a.code, a.name, a.account_type;

-- ---------------------------------------------------------------------------
-- Applications: realized FX
-- ---------------------------------------------------------------------------

-- When the two documents of an application were posted at different rates,
-- settling the transaction-currency amount realizes a base-currency gain or
-- loss; the entry that books it is linked here so unwinding the application
-- can reverse it.
ALTER TABLE payment_applications
    ADD COLUMN fx_journal_entry_id int REFERENCES journal_entries(id);
ALTER TABLE bill_applications
    ADD COLUMN fx_journal_entry_id int REFERENCES journal_entries(id);
ALTER TABLE sales_credit_applications
    ADD COLUMN fx_journal_entry_id int REFERENCES journal_entries(id);
ALTER TABLE purchase_credit_applications
    ADD COLUMN fx_journal_entry_id int REFERENCES journal_entries(id);

-- Applications need both documents' journal entries (that is where their
-- exchange rates live), so both sides must already be posted.
CREATE FUNCTION trg_application_both_posted() RETURNS trigger AS $$
DECLARE
    -- TG_ARGV: payment-side table, payment-side id column,
    --          document-side table, document-side id column.
    v_status text;
BEGIN
    EXECUTE format('SELECT status FROM %I WHERE id = $1', TG_ARGV[0])
        INTO v_status USING (row_to_json(NEW) ->> TG_ARGV[1])::int;
    IF v_status IS DISTINCT FROM 'posted' THEN
        RAISE EXCEPTION '% %: applications require a posted document',
            TG_ARGV[0], (row_to_json(NEW) ->> TG_ARGV[1])::int;
    END IF;
    EXECUTE format('SELECT status FROM %I WHERE id = $1', TG_ARGV[2])
        INTO v_status USING (row_to_json(NEW) ->> TG_ARGV[3])::int;
    IF v_status IS DISTINCT FROM 'posted' THEN
        RAISE EXCEPTION '% %: applications require a posted document',
            TG_ARGV[2], (row_to_json(NEW) ->> TG_ARGV[3])::int;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER payment_applications_both_posted
    BEFORE INSERT ON payment_applications
    FOR EACH ROW EXECUTE FUNCTION trg_application_both_posted(
        'customer_payments', 'payment_id', 'sales_invoices', 'invoice_id');
CREATE TRIGGER bill_applications_both_posted
    BEFORE INSERT ON bill_applications
    FOR EACH ROW EXECUTE FUNCTION trg_application_both_posted(
        'supplier_payments', 'payment_id', 'purchase_bills', 'bill_id');
CREATE TRIGGER sales_credit_applications_both_posted
    BEFORE INSERT ON sales_credit_applications
    FOR EACH ROW EXECUTE FUNCTION trg_application_both_posted(
        'sales_credit_notes', 'credit_note_id', 'sales_invoices', 'invoice_id');
CREATE TRIGGER purchase_credit_applications_both_posted
    BEFORE INSERT ON purchase_credit_applications
    FOR EACH ROW EXECUTE FUNCTION trg_application_both_posted(
        'purchase_credit_notes', 'credit_note_id', 'purchase_bills', 'bill_id');
