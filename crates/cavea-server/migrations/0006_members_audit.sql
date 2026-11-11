-- Members and the audit log (docs/08-server.md §5, docs/09-console.md §7).
-- Invitations to a workspace, by a one-time link: the token's SHA-256 only.
-- One pending invitation per (workspace, email); accepting or revoking one
-- deletes it.
CREATE TABLE invitations (
  id           uuid PRIMARY KEY,
  workspace_id uuid NOT NULL REFERENCES workspaces,
  email        text NOT NULL CHECK (email = lower(email)),
  role         text NOT NULL CHECK (role IN ('owner', 'admin', 'editor', 'operator', 'viewer')),
  token_hash   bytea NOT NULL UNIQUE,
  created_by   uuid,
  created_at   timestamptz NOT NULL DEFAULT now(),
  expires_at   timestamptz NOT NULL,
  UNIQUE (workspace_id, email)
);
-- Migration 1's audit_log, never written before this migration: who acted
-- (`usr_…`, `key_…`, `admin`, or `anonymous` for a failed sign-in), and the
-- request's id. `user_id` is the console user the row concerns.
ALTER TABLE audit_log ADD COLUMN actor text NOT NULL, ADD COLUMN request_id text;
CREATE INDEX audit_log_workspace ON audit_log (workspace_id, id);
CREATE INDEX audit_log_user ON audit_log (user_id, id) WHERE workspace_id IS NULL;
