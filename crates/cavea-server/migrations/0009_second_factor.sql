-- The console's second factor and password reset links (docs/09-console.md
-- §2, §10; docs/12-security-licensing.md §3 T9; D-89, D-90).
--
-- `users.totp_secret` (migration 1) is the TOTP secret, kept as it is: the
-- server computes codes from it. While `totp_enabled_at` is null it is an
-- enrolment not yet confirmed and signs nobody in. `totp_last_step` is the
-- last 30-second step whose code was accepted: a code is accepted once.
ALTER TABLE users
  ADD COLUMN totp_enabled_at timestamptz,
  ADD COLUMN totp_last_step bigint;
-- One-time recovery codes: their SHA-256 only; using one deletes it.
CREATE TABLE totp_recovery_codes (
  user_id   uuid NOT NULL REFERENCES users,
  code_hash bytea NOT NULL,
  PRIMARY KEY (user_id, code_hash)
);
-- A sign-in waiting for its second step: the challenge's SHA-256, given
-- once the password was verified; the password hash it was verified against
-- (a change since refuses it); wrong codes so far. Using it deletes it.
CREATE TABLE console_challenges (
  id_hash       bytea PRIMARY KEY,
  user_id       uuid NOT NULL REFERENCES users,
  password_hash text NOT NULL,
  expires_at    timestamptz NOT NULL,
  failures      int NOT NULL DEFAULT 0
);
CREATE INDEX console_challenges_user ON console_challenges (user_id);
-- A password reset link: the token's SHA-256 only; one live link per user;
-- whether using it also clears the second factor; who made it (`usr_…` or
-- `admin`). Using it deletes it.
CREATE TABLE password_resets (
  id                  uuid PRIMARY KEY,
  user_id             uuid NOT NULL UNIQUE REFERENCES users,
  token_hash          bytea NOT NULL UNIQUE,
  clear_second_factor boolean NOT NULL,
  created_by          text NOT NULL,
  created_at          timestamptz NOT NULL DEFAULT now(),
  expires_at          timestamptz NOT NULL
);
