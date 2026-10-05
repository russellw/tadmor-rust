-- 000009_reversal (down)
DROP INDEX IF EXISTS journal_entries_reverses_entry_id_idx;
ALTER TABLE journal_entries DROP COLUMN IF EXISTS reverses_entry_id;
