-- The initial schema (docs/08-server.md §5). Forward-only: never edit a
-- migration once released; add a new file instead (crates/cavea-server/src/store/migrations.rs).

CREATE TABLE workspaces (
  id          uuid PRIMARY KEY,
  key         text NOT NULL UNIQUE,
  name        text NOT NULL,
  settings    jsonb NOT NULL DEFAULT '{}',
  created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE api_keys (
  id              uuid PRIMARY KEY,
  workspace_id    uuid NOT NULL REFERENCES workspaces,
  kind            text NOT NULL CHECK (kind IN ('secret', 'publishable')),
  prefix          text NOT NULL,                 -- shown in the console
  hash            bytea NOT NULL UNIQUE,         -- SHA-256 of the key
  name            text NOT NULL,
  allowed_origins text[],                        -- publishable keys only
  created_at      timestamptz NOT NULL DEFAULT now(),
  last_used_at    timestamptz,
  revoked_at      timestamptz
);

CREATE TABLE custom_statuses (
  workspace_id uuid NOT NULL REFERENCES workspaces,
  code         smallint NOT NULL CHECK (code BETWEEN 4 AND 15),
  key          text NOT NULL,
  label        text NOT NULL,
  color        text NOT NULL,
  PRIMARY KEY (workspace_id, code),
  UNIQUE (workspace_id, key)
);

CREATE TABLE charts (
  id                   uuid PRIMARY KEY,
  workspace_id         uuid NOT NULL REFERENCES workspaces,
  key                  text NOT NULL,
  name                 text NOT NULL,
  draft                jsonb NOT NULL,
  draft_etag           bigint NOT NULL DEFAULT 1,
  draft_updated_at     timestamptz NOT NULL DEFAULT now(),
  draft_updated_by     uuid,
  published_version_id uuid,
  archived_at          timestamptz,
  created_at           timestamptz NOT NULL DEFAULT now(),
  UNIQUE (workspace_id, key)
);

CREATE TABLE chart_versions (
  id            uuid PRIMARY KEY,
  chart_id      uuid NOT NULL REFERENCES charts,
  number        int  NOT NULL,
  hash          bytea NOT NULL,                  -- 16 bytes, content hash
  source        jsonb NOT NULL,                  -- the document that was compiled
  public_ccf    bytea NOT NULL,
  public_ccf_br bytea NOT NULL,                  -- precompressed
  full_ccf      bytea NOT NULL,
  stats         jsonb NOT NULL,
  thumbnail     bytea,
  created_at    timestamptz NOT NULL DEFAULT now(),
  created_by    uuid,
  UNIQUE (chart_id, number),
  UNIQUE (chart_id, hash)
);

CREATE TABLE assets (
  hash         bytea PRIMARY KEY,                -- 16 bytes
  workspace_id uuid NOT NULL REFERENCES workspaces,
  content_type text NOT NULL,
  size         int  NOT NULL,
  bytes        bytea,                            -- null when stored in object storage
  created_at   timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE events (
  id               uuid PRIMARY KEY,
  workspace_id     uuid NOT NULL REFERENCES workspaces,
  key              text NOT NULL,
  name             text,
  chart_version_id uuid NOT NULL REFERENCES chart_versions,
  settings         jsonb NOT NULL DEFAULT '{}',  -- hold_ttl, max_objects, orphan_policy, ...
  seq              bigint NOT NULL DEFAULT 0,
  owner_node       text,
  owner_epoch      bigint NOT NULL DEFAULT 0,
  lease_until      timestamptz,
  archived_at      timestamptz,
  created_at       timestamptz NOT NULL DEFAULT now(),
  UNIQUE (workspace_id, key)
);

CREATE TABLE channels (
  event_id uuid NOT NULL REFERENCES events,
  idx      smallint NOT NULL,
  key      text NOT NULL,
  name     text NOT NULL,
  color    text NOT NULL,
  PRIMARY KEY (event_id, idx),
  UNIQUE (event_id, key)
);

CREATE TABLE holds (
  event_id    uuid NOT NULL REFERENCES events,
  id          bigint NOT NULL,                   -- seq of the CreateHold command; assigned by the actor
  token_hash  bytea NOT NULL UNIQUE,
  expires_at  timestamptz NOT NULL,
  max_objects int,                               -- null: unlimited (secret-key tokens)
  session_sub text,
  created_by  uuid,
  created_at  timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (event_id, id)
);

-- A row exists only for objects that are not free, or that have a channel.
CREATE TABLE object_state (
  event_id    uuid NOT NULL REFERENCES events,
  oid         int  NOT NULL,                     -- u32 stored bit for bit: values from 2^31 read as negative
  status      smallint NOT NULL,
  hold_id     bigint,
  channel     smallint,
  order_ref   text,
  ticket_type text,
  extra       json,                              -- not jsonb: kept byte for byte, and jsonb rejects \u0000
  seq         bigint NOT NULL,
  PRIMARY KEY (event_id, oid)
);
CREATE INDEX object_state_order ON object_state (event_id, order_ref) WHERE order_ref IS NOT NULL;

CREATE TABLE area_alloc (
  id          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  event_id    uuid NOT NULL REFERENCES events,
  oid         int  NOT NULL,
  status      smallint NOT NULL,                 -- 1 held, 2 booked
  hold_id     bigint,
  order_ref   text,
  ticket_type text,
  quantity    int NOT NULL CHECK (quantity > 0),
  seq         bigint NOT NULL
);
CREATE INDEX area_alloc_event ON area_alloc (event_id, oid);

CREATE TABLE event_log (
  event_id  uuid NOT NULL,
  seq       bigint NOT NULL,
  at        timestamptz NOT NULL DEFAULT now(),
  op        smallint NOT NULL,
  actor     text NOT NULL,                       -- key id, user id, or 'system'
  hold_id   bigint,
  order_ref text,
  oids      int[] NOT NULL,
  detail    json,                                -- not jsonb, which rejects \u0000 in valid JSON
  PRIMARY KEY (event_id, seq, at)
) PARTITION BY RANGE (at);                       -- monthly partitions, configurable retention
-- Until monthly partitions are created (SRV-040), every row lands here, so an
-- insert never fails for lack of a partition. SRV-040 must move the rows a new
-- monthly partition covers out of it first, or create partitions ahead of use.
CREATE TABLE event_log_default PARTITION OF event_log DEFAULT;

CREATE TABLE webhook_endpoints (
  id           uuid PRIMARY KEY,
  workspace_id uuid NOT NULL REFERENCES workspaces,
  url          text NOT NULL,
  secret       bytea NOT NULL,
  types        text[] NOT NULL,
  enabled      boolean NOT NULL DEFAULT true,
  created_at   timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE webhook_deliveries (
  id              bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  endpoint_id     uuid NOT NULL REFERENCES webhook_endpoints,
  type            text NOT NULL,
  event_id        uuid,
  seq             bigint,
  payload         jsonb NOT NULL,
  attempts        int NOT NULL DEFAULT 0,
  next_attempt_at timestamptz NOT NULL DEFAULT now(),
  delivered_at    timestamptz,
  last_status     int,
  last_error      text
);
CREATE INDEX webhook_due ON webhook_deliveries (next_attempt_at) WHERE delivered_at IS NULL;

CREATE TABLE idempotency (
  workspace_id uuid NOT NULL,
  event_id     uuid,                             -- set for seat commands; loaded with the event
  key          text NOT NULL,
  request_hash bytea NOT NULL,                   -- the caller's fingerprint of the request
  status       int NOT NULL,                     -- the reply's HTTP status, for reports
  response     json NOT NULL,                    -- the engine's reply, as the store encodes it; not jsonb (\u0000)
  created_at   timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (workspace_id, key)
);
CREATE INDEX idempotency_event ON idempotency (event_id, created_at) WHERE event_id IS NOT NULL;

CREATE TABLE users (
  id            uuid PRIMARY KEY,
  email         text NOT NULL UNIQUE,
  password_hash text,                            -- argon2id; null for SSO-only users
  totp_secret   bytea,
  disabled_at   timestamptz,
  created_at    timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE memberships (
  user_id      uuid NOT NULL REFERENCES users,
  workspace_id uuid NOT NULL REFERENCES workspaces,
  role         text NOT NULL CHECK (role IN ('owner', 'admin', 'editor', 'operator', 'viewer')),
  PRIMARY KEY (user_id, workspace_id)
);

CREATE TABLE console_sessions (
  id_hash    bytea PRIMARY KEY,
  user_id    uuid NOT NULL REFERENCES users,
  expires_at timestamptz NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE audit_log (
  id           bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  workspace_id uuid,
  user_id      uuid,
  at           timestamptz NOT NULL DEFAULT now(),
  action       text NOT NULL,
  target       text,
  detail       jsonb
);
