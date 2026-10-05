-- 000014_year_end: the year-end close.
--
-- Closing a fiscal year posts a "closing entry" that sweeps every revenue and
-- expense balance into retained earnings, closes all of the year's periods,
-- and marks the year closed. Two schema additions support that:
--
--   * journal_entries.is_closing flags the closing entry (and its reversal,
--     should the year be reopened) so income statements can exclude it —
--     otherwise the year it closes would report zero revenue and expenses;
--   * fiscal_years.closing_entry_id records which entry closed the year, so
--     reopening knows what to reverse.
--
-- One new invariant is enforced in the database: an accounting period in a
-- closed fiscal year may not be open. Journal entries already cannot touch a
-- closed period, so together the two triggers make a closed year immutable
-- until it is explicitly reopened.

ALTER TABLE journal_entries
    ADD COLUMN is_closing boolean NOT NULL DEFAULT false;

ALTER TABLE fiscal_years
    ADD COLUMN closing_entry_id int REFERENCES journal_entries(id);

CREATE FUNCTION trg_accounting_periods_year_open() RETURNS trigger AS $$
DECLARE
    v_year_status text;
BEGIN
    IF NEW.status = 'open' THEN
        SELECT status INTO v_year_status FROM fiscal_years WHERE id = NEW.fiscal_year_id;
        IF v_year_status = 'closed' THEN
            RAISE EXCEPTION 'fiscal year % is closed; its periods cannot be open', NEW.fiscal_year_id;
        END IF;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER accounting_periods_year_open
    BEFORE INSERT OR UPDATE ON accounting_periods
    FOR EACH ROW EXECUTE FUNCTION trg_accounting_periods_year_open();
