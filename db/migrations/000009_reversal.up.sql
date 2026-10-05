-- 000009_reversal: link a reversing journal entry back to the one it reverses.
--
-- Unposting a document does not delete its journal entry (that would destroy the
-- audit trail); instead a mirror entry is posted that nets the original to zero.
-- reverses_entry_id records that relationship and lets us prevent an entry from
-- being reversed twice.
ALTER TABLE journal_entries
    ADD COLUMN reverses_entry_id int REFERENCES journal_entries(id);

CREATE INDEX journal_entries_reverses_entry_id_idx ON journal_entries (reverses_entry_id);
