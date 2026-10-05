-- 000011_roles: administrator flag on users.
--
-- The auth model gains one level: administrators manage login users and may
-- unpost documents; everyone else gets the day-to-day surface. One boolean
-- rather than a roles table — two levels is all the product needs today, and
-- a wider role system can grow out of this column if that changes.

ALTER TABLE users ADD COLUMN is_admin boolean NOT NULL DEFAULT false;

-- Everyone who could sign in before this migration had the full surface;
-- grandfather them so the upgrade locks nobody out of anything.
UPDATE users SET is_admin = true;
