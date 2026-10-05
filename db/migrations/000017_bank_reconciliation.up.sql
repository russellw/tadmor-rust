-- 000017_bank_reconciliation: match bank statements against the ledger.
--
-- Model:
--   bank_statements       one imported/keyed statement for a cash account
--   bank_statement_lines  the statement's transactions, each matchable 1:1
--                         to a posted journal line on that account
--
-- A statement is captured (CSV import or manual entry) while 'open', its
-- lines are matched to journal lines, and it is then marked 'reconciled'.
-- Amounts are signed from the book's perspective: a deposit is positive
-- (a debit to the cash account), a withdrawal negative.
--
-- Invariants enforced in the database, not just the app:
--   * a statement may only be drawn on a postable, active, cash account;
--   * a match must point at a posted journal line on the statement's
--     account whose signed amount (debit - credit) equals the line's;
--   * a journal line backs at most one statement line (unique index);
--   * reconciling requires every line matched and
--     opening_balance + sum(lines) = closing_balance;
--   * a reconciled statement and its lines are frozen (except reopening).

CREATE TABLE bank_statements (
    id              int           GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    account_id      int           NOT NULL REFERENCES accounts(id),
    statement_date  date          NOT NULL,                -- the statement's closing date
    opening_balance numeric(19,4) NOT NULL DEFAULT 0,
    closing_balance numeric(19,4) NOT NULL DEFAULT 0,
    reference       text,                                  -- e.g. the bank's statement number
    status          text          NOT NULL DEFAULT 'open'
                                  CHECK (status IN ('open', 'reconciled')),
    reconciled_at   timestamptz,
    created_by      int           REFERENCES users(id),
    created_at      timestamptz   NOT NULL DEFAULT now(),
    updated_at      timestamptz   NOT NULL DEFAULT now()
);

CREATE INDEX bank_statements_account_id_idx ON bank_statements (account_id);

CREATE TRIGGER set_updated_at BEFORE UPDATE ON bank_statements
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE bank_statement_lines (
    id              int           GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    statement_id    int           NOT NULL REFERENCES bank_statements(id) ON DELETE CASCADE,
    line_no         int           NOT NULL,
    txn_date        date          NOT NULL,
    description     text          NOT NULL,
    reference       text,
    amount          numeric(19,4) NOT NULL CHECK (amount <> 0),  -- signed; deposits positive
    journal_line_id int           REFERENCES journal_lines(id),  -- the match; NULL = unmatched
    CONSTRAINT bank_statement_lines_line_no_uq UNIQUE (statement_id, line_no)
);

CREATE INDEX bank_statement_lines_statement_id_idx ON bank_statement_lines (statement_id);

-- A journal line backs at most one statement line, across all statements.
CREATE UNIQUE INDEX bank_statement_lines_journal_line_uq
    ON bank_statement_lines (journal_line_id) WHERE journal_line_id IS NOT NULL;

-- ---------------------------------------------------------------------------
-- Integrity triggers
-- ---------------------------------------------------------------------------

-- Statements may only be drawn on postable, active, cash accounts, and once a
-- statement is reconciled its substance is frozen: only the reopen transition
-- (reconciled -> open) may touch it. Changing the account out from under
-- matched lines is also refused while open.
CREATE FUNCTION trg_bank_statements_check() RETURNS trigger AS $$
DECLARE
    v_ok bool;
BEGIN
    IF TG_OP = 'DELETE' THEN
        IF OLD.status = 'reconciled' THEN
            RAISE EXCEPTION 'bank statement % is reconciled and cannot be deleted', OLD.id;
        END IF;
        RETURN OLD;
    END IF;

    IF TG_OP = 'UPDATE' AND OLD.status = 'reconciled' AND NEW.status = 'reconciled' THEN
        RAISE EXCEPTION 'bank statement % is reconciled and cannot be modified', OLD.id;
    END IF;

    SELECT is_postable AND is_active AND is_cash INTO v_ok
        FROM accounts WHERE id = NEW.account_id;
    IF NOT v_ok THEN
        RAISE EXCEPTION 'account % is not a postable, active cash account', NEW.account_id;
    END IF;

    IF TG_OP = 'UPDATE' AND NEW.account_id <> OLD.account_id
       AND EXISTS (SELECT 1 FROM bank_statement_lines
                   WHERE statement_id = OLD.id AND journal_line_id IS NOT NULL) THEN
        RAISE EXCEPTION 'bank statement % has matched lines; unmatch them before changing its account', OLD.id;
    END IF;

    -- Reconciling: every line matched, and the statement itself must add up.
    IF NEW.status = 'reconciled' AND (TG_OP = 'INSERT' OR OLD.status = 'open') THEN
        IF EXISTS (SELECT 1 FROM bank_statement_lines
                   WHERE statement_id = NEW.id AND journal_line_id IS NULL) THEN
            RAISE EXCEPTION 'bank statement % has unmatched lines', NEW.id;
        END IF;
        IF NEW.opening_balance
           + COALESCE((SELECT sum(amount) FROM bank_statement_lines WHERE statement_id = NEW.id), 0)
           <> NEW.closing_balance THEN
            RAISE EXCEPTION 'bank statement % does not balance: opening + lines <> closing', NEW.id;
        END IF;
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER bank_statements_check
    BEFORE INSERT OR UPDATE OR DELETE ON bank_statements
    FOR EACH ROW EXECUTE FUNCTION trg_bank_statements_check();

-- Lines of a reconciled statement are frozen, and a match must point at a
-- posted journal line on the statement's account with the same signed amount.
CREATE FUNCTION trg_bank_statement_lines_check() RETURNS trigger AS $$
DECLARE
    v_statement   int;
    v_status      text;
    v_account     int;
    v_jl_account  int;
    v_jl_amount   numeric(19,4);
    v_je_status   text;
BEGIN
    v_statement := COALESCE(NEW.statement_id, OLD.statement_id);
    SELECT status, account_id INTO v_status, v_account
        FROM bank_statements WHERE id = v_statement;
    -- Statement gone (cascade delete): nothing to enforce.
    IF v_status IS NULL THEN
        RETURN COALESCE(NEW, OLD);
    END IF;
    IF v_status = 'reconciled' THEN
        RAISE EXCEPTION 'bank statement % is reconciled; its lines cannot be modified', v_statement;
    END IF;

    IF TG_OP <> 'DELETE' AND NEW.journal_line_id IS NOT NULL THEN
        SELECT jl.account_id, jl.debit - jl.credit, je.status
            INTO v_jl_account, v_jl_amount, v_je_status
            FROM journal_lines jl
            JOIN journal_entries je ON je.id = jl.journal_entry_id
            WHERE jl.id = NEW.journal_line_id;
        IF v_je_status IS DISTINCT FROM 'posted' THEN
            RAISE EXCEPTION 'journal line % does not belong to a posted entry', NEW.journal_line_id;
        END IF;
        IF v_jl_account <> v_account THEN
            RAISE EXCEPTION 'journal line % is not on the statement''s account', NEW.journal_line_id;
        END IF;
        IF v_jl_amount <> NEW.amount THEN
            RAISE EXCEPTION 'journal line % amount % does not equal statement line amount %',
                NEW.journal_line_id, v_jl_amount, NEW.amount;
        END IF;
    END IF;

    RETURN COALESCE(NEW, OLD);
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER bank_statement_lines_check
    BEFORE INSERT OR UPDATE OR DELETE ON bank_statement_lines
    FOR EACH ROW EXECUTE FUNCTION trg_bank_statement_lines_check();
