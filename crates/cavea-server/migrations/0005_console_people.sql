-- Console people (docs/08-server.md §5, docs/09-console.md §2). `users`,
-- `memberships` and `console_sessions` are migration 1's; no server before
-- this migration wrote them. Emails are stored trimmed and lowercased, so
-- `users.email`'s UNIQUE is case-insensitive; this makes sure of it.
ALTER TABLE users ADD CONSTRAINT users_email_normalised CHECK (email = lower(email));
-- A user's sessions, for revoking them all (a password change).
CREATE INDEX console_sessions_user ON console_sessions (user_id);
-- The first-run setup token (09 §2): one row, its SHA-256 only.
CREATE TABLE console_setup (
  id         smallint PRIMARY KEY CHECK (id = 1),
  token_hash bytea NOT NULL,
  expires_at timestamptz NOT NULL,
  used_at    timestamptz
);
