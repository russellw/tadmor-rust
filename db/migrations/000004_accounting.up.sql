-- 000004_accounting: the double-entry general ledger that other modules post into.
--
-- Model:
--   account_types       fixed lookup (asset/liability/equity/revenue/expense)
--   accounts            the chart of accounts (hierarchical)
--   fiscal_years        reporting years
--   accounting_periods  the bookkeeping periods that can be opened/closed
--   journal_entries     a balanced transaction (header)
--   journal_lines       the individual debits/credits (detail)
--
-- Invariants enforced in the database, not just the app:
--   * a *posted* entry's debits must equal its credits, and it must have lines;
--   * lines may only hit accounts flagged postable and active;
--   * nothing may be written against a closed period.

-- ---------------------------------------------------------------------------
-- Reference / lookup
-- ---------------------------------------------------------------------------

-- The five fundamental account classes. Natural key (the class name) per the
-- project rules; normal_balance is the side on which the account increases.
CREATE TABLE account_types (
    code           text PRIMARY KEY,                       -- 'asset', 'liability', ...
    name           text NOT NULL,
    normal_balance text NOT NULL CHECK (normal_balance IN ('debit', 'credit'))
);

INSERT INTO account_types (code, name, normal_balance) VALUES
    ('asset',     'Asset',     'debit'),
    ('liability', 'Liability', 'credit'),
    ('equity',    'Equity',    'credit'),
    ('revenue',   'Revenue',   'credit'),
    ('expense',   'Expense',   'debit');

-- The chart of accounts. Summary/header accounts (is_postable = false) organise
-- the tree; only leaf, postable accounts may carry journal lines.
CREATE TABLE accounts (
    id            int         GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    code          text        NOT NULL UNIQUE,             -- account number, e.g. '1000'
    name          text        NOT NULL,
    account_type  text        NOT NULL REFERENCES account_types(code),
    parent_id     int         REFERENCES accounts(id),     -- NULL for top-level accounts
    currency_code char(3)     REFERENCES currencies(code), -- optional single-currency restriction
    is_postable   boolean     NOT NULL DEFAULT true,
    is_active     boolean     NOT NULL DEFAULT true,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT accounts_no_self_parent CHECK (parent_id IS NULL OR parent_id <> id)
);

CREATE INDEX accounts_parent_id_idx    ON accounts (parent_id);
CREATE INDEX accounts_account_type_idx ON accounts (account_type);

CREATE TRIGGER set_updated_at BEFORE UPDATE ON accounts
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ---------------------------------------------------------------------------
-- Fiscal calendar
-- ---------------------------------------------------------------------------

CREATE TABLE fiscal_years (
    id         int         GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name       text        NOT NULL UNIQUE,                -- e.g. 'FY2026'
    start_date date        NOT NULL,
    end_date   date        NOT NULL,
    status     text        NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'closed')),
    CONSTRAINT fiscal_years_dates CHECK (end_date >= start_date)
);

CREATE TABLE accounting_periods (
    id             int    GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    fiscal_year_id int    NOT NULL REFERENCES fiscal_years(id),
    name           text   NOT NULL,                        -- e.g. '2026-06'
    start_date     date   NOT NULL,
    end_date       date   NOT NULL,
    status         text   NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'closed')),
    CONSTRAINT accounting_periods_dates CHECK (end_date >= start_date),
    CONSTRAINT accounting_periods_name_uq UNIQUE (fiscal_year_id, name),
    -- Periods must not overlap each other (inclusive bounds).
    CONSTRAINT accounting_periods_no_overlap
        EXCLUDE USING gist (daterange(start_date, end_date, '[]') WITH &&)
);

CREATE INDEX accounting_periods_fiscal_year_idx ON accounting_periods (fiscal_year_id);

-- ---------------------------------------------------------------------------
-- Journal (transactions)
-- ---------------------------------------------------------------------------

CREATE TABLE journal_entries (
    id            int         GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    entry_date    date        NOT NULL,
    period_id     int         NOT NULL REFERENCES accounting_periods(id),
    currency_code char(3)     NOT NULL REFERENCES currencies(code),
    reference     text,                                    -- external document reference
    memo          text,
    status        text        NOT NULL DEFAULT 'draft'
                              CHECK (status IN ('draft', 'posted', 'void')),
    posted_at     timestamptz,
    created_by    int         REFERENCES users(id),
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX journal_entries_period_id_idx  ON journal_entries (period_id);
CREATE INDEX journal_entries_entry_date_idx ON journal_entries (entry_date);

CREATE TRIGGER set_updated_at BEFORE UPDATE ON journal_entries
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE journal_lines (
    id               int           GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    journal_entry_id int           NOT NULL REFERENCES journal_entries(id) ON DELETE CASCADE,
    line_no          int           NOT NULL,               -- ordering within the entry
    account_id       int           NOT NULL REFERENCES accounts(id),
    debit            numeric(19,4) NOT NULL DEFAULT 0,
    credit           numeric(19,4) NOT NULL DEFAULT 0,
    memo             text,
    CONSTRAINT journal_lines_line_no_uq UNIQUE (journal_entry_id, line_no),
    CONSTRAINT journal_lines_nonneg     CHECK (debit >= 0 AND credit >= 0),
    -- Exactly one side of each line is positive.
    CONSTRAINT journal_lines_one_side   CHECK ((debit = 0) <> (credit = 0))
);

CREATE INDEX journal_lines_account_id_idx ON journal_lines (account_id);

-- ---------------------------------------------------------------------------
-- Integrity triggers
-- ---------------------------------------------------------------------------

-- A posted entry must balance and must have at least one line. Checked at
-- transaction commit (deferred) so a multi-statement build of an entry is fine.
CREATE FUNCTION accounting_assert_entry_balanced(p_entry_id int)
RETURNS void AS $$
DECLARE
    v_status text;
    v_debit  numeric(19,4);
    v_credit numeric(19,4);
BEGIN
    SELECT status INTO v_status FROM journal_entries WHERE id = p_entry_id;
    -- Entry gone (e.g. cascade delete) or still a draft: nothing to enforce.
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

CREATE FUNCTION trg_journal_lines_balanced() RETURNS trigger AS $$
BEGIN
    PERFORM accounting_assert_entry_balanced(
        COALESCE(NEW.journal_entry_id, OLD.journal_entry_id));
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE FUNCTION trg_journal_entries_balanced() RETURNS trigger AS $$
BEGIN
    PERFORM accounting_assert_entry_balanced(NEW.id);
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE CONSTRAINT TRIGGER journal_lines_balanced
    AFTER INSERT OR UPDATE OR DELETE ON journal_lines
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION trg_journal_lines_balanced();

-- Catches the draft -> posted transition for an entry whose lines already exist.
CREATE CONSTRAINT TRIGGER journal_entries_balanced
    AFTER INSERT OR UPDATE ON journal_entries
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION trg_journal_entries_balanced();

-- Lines may only hit postable, active accounts.
CREATE FUNCTION trg_journal_lines_account_ok() RETURNS trigger AS $$
DECLARE
    v_postable boolean;
    v_active   boolean;
BEGIN
    SELECT is_postable, is_active INTO v_postable, v_active
        FROM accounts WHERE id = NEW.account_id;
    IF NOT v_postable THEN
        RAISE EXCEPTION 'account % is a summary account and cannot be posted to', NEW.account_id;
    END IF;
    IF NOT v_active THEN
        RAISE EXCEPTION 'account % is inactive', NEW.account_id;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER journal_lines_account_ok
    BEFORE INSERT OR UPDATE ON journal_lines
    FOR EACH ROW EXECUTE FUNCTION trg_journal_lines_account_ok();

-- Nothing may be written against a closed period.
CREATE FUNCTION trg_journal_entries_period_open() RETURNS trigger AS $$
DECLARE
    v_status text;
BEGIN
    SELECT status INTO v_status FROM accounting_periods WHERE id = NEW.period_id;
    IF v_status = 'closed' THEN
        RAISE EXCEPTION 'accounting period % is closed', NEW.period_id;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER journal_entries_period_open
    BEFORE INSERT OR UPDATE ON journal_entries
    FOR EACH ROW EXECUTE FUNCTION trg_journal_entries_period_open();

-- ---------------------------------------------------------------------------
-- Reporting convenience
-- ---------------------------------------------------------------------------

-- Trial balance over posted entries only. Every account appears, even with no
-- activity; balance is expressed debit-positive (debit - credit).
CREATE VIEW trial_balance AS
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

-- ---------------------------------------------------------------------------
-- Starter chart of accounts
-- ---------------------------------------------------------------------------

INSERT INTO accounts (code, name, account_type, is_postable) VALUES
    ('1000', 'Cash',                  'asset',     true),
    ('1100', 'Accounts Receivable',   'asset',     true),
    ('1200', 'Inventory',             'asset',     true),
    ('2000', 'Accounts Payable',      'liability', true),
    ('2100', 'Sales Tax Payable',     'liability', true),
    ('3000', 'Retained Earnings',     'equity',    true),
    ('3100', 'Common Stock',          'equity',    true),
    ('4000', 'Sales Revenue',         'revenue',   true),
    ('5000', 'Cost of Goods Sold',    'expense',   true),
    ('6000', 'Operating Expenses',    'expense',   true);
