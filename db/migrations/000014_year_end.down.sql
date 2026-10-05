-- 000014_year_end (down)

DROP TRIGGER accounting_periods_year_open ON accounting_periods;
DROP FUNCTION trg_accounting_periods_year_open();

ALTER TABLE fiscal_years DROP COLUMN closing_entry_id;
ALTER TABLE journal_entries DROP COLUMN is_closing;
