-- 000001_init: shared database primitives used by every later migration.

-- citext gives us case-insensitive text, which is the correct type for
-- things like email addresses where casing is not semantically meaningful.
CREATE EXTENSION IF NOT EXISTS citext;

-- Generic trigger function that stamps updated_at on every row update.
-- Attach it to any table that carries an updated_at column:
--
--   CREATE TRIGGER set_updated_at BEFORE UPDATE ON <table>
--       FOR EACH ROW EXECUTE FUNCTION set_updated_at();
CREATE OR REPLACE FUNCTION set_updated_at()
RETURNS trigger AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;
