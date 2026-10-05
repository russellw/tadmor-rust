-- 000004_accounting (down)
DROP VIEW IF EXISTS trial_balance;

DROP TABLE IF EXISTS journal_lines;
DROP TABLE IF EXISTS journal_entries;
DROP TABLE IF EXISTS accounting_periods;
DROP TABLE IF EXISTS fiscal_years;
DROP TABLE IF EXISTS accounts;
DROP TABLE IF EXISTS account_types;

DROP FUNCTION IF EXISTS trg_journal_entries_period_open();
DROP FUNCTION IF EXISTS trg_journal_lines_account_ok();
DROP FUNCTION IF EXISTS trg_journal_entries_balanced();
DROP FUNCTION IF EXISTS trg_journal_lines_balanced();
DROP FUNCTION IF EXISTS accounting_assert_entry_balanced(int);
