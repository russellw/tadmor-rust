-- 000010_auth: server-side login sessions.
--
-- The natural key is the SHA-256 hash of the bearer token: the raw token lives
-- only in the client's cookie, so a leaked database dump cannot be replayed as
-- a session. Expired rows are deleted opportunistically on login.

CREATE TABLE sessions (
    token_hash bytea       PRIMARY KEY,
    user_id    int         NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL
);

CREATE INDEX sessions_user_id_idx ON sessions (user_id);
