# 08 · Server

**Status:** Draft v0.1 · 2026-11-10

`cavea-server` is one Rust binary that the customer runs next to PostgreSQL. It stores charts, owns per-event seat state, serves realtime updates, sends webhooks and hosts the console.

Vocabulary is defined in `02-domain-model.md`. Budgets are in `01-architecture.md` §5.

## 1. Goals

1. **Never double-book.** Correctness outranks every other property.
2. **Fast under contention.** An on-sale means thousands of buyers racing for the same seats.
3. **Boring to operate.** One binary, one database, sane defaults, clear metrics.
4. **Recover by itself.** After a crash, state is rebuilt from PostgreSQL with nothing to repair by hand.

## 2. Process layout

```
                      ┌──────────────────────── cavea-server ─────────────────────────┐
HTTPS ──▶ axum router │  REST handlers ──▶ Actor registry ──▶ Event actor (per event) │
                      │        │                                  │   ▲               │
                      │        │                            batch │   │ commit result │
                      │        ▼                                  ▼   │               │
WS / SSE ◀── Realtime hub ◀── per-event broadcast            Committer ──▶ PostgreSQL │
                      │                                                               │
                      │  Background: webhook dispatcher · log partition maintenance · │
                      │  idle-actor eviction · lease heartbeat · idempotency cleanup  │
                      │  Static: console assets · compiled charts · images            │
                      └───────────────────────────────────────────────────────────────┘
```

## 3. Crate modules

| Module | Responsibility |
|---|---|
| `http` | Router, extractors, auth, problem+json errors, OpenAPI |
| `public` | Publishable-key endpoints, CORS, rate limiting |
| `registry` | Finds or loads the actor for an event; eviction |
| `engine` (crate `cavea-engine`) | Actor, state machine, hold expiry, best-available |
| `store` | `SeatStore` trait and its PostgreSQL implementation |
| `realtime` | WebSocket, SSE, polling; frame encoding; back-pressure |
| `charts` | Draft storage, validation, compile on publish, asset storage |
| `webhooks` | Outbox reader, signer, retry scheduler |
| `reports` | Summaries and exports |
| `console` | Sessions, users, roles, embedded static assets |
| `licence` | Licence file verification |
| `telemetry` | Tracing, metrics, health |

## 4. The event actor

### 4.1 State

```rust
struct EventActor {
    event_id: Uuid,
    chart: Arc<CompiledChart>,          // full CCF: ranks, oids, segments
    status: Vec<u8>,                    // per idx, domain status code
    holder: Vec<u32>,                   // per idx, hold slot or 0
    channel: Vec<u16>,                  // per idx, channel index or 0
    areas: Vec<AreaState>,              // booked_qty, held_qty, per-hold quantities
    holds: SlotMap<HoldSlot, Hold>,     // token hash, expiry, object list, limits
    by_token: HashMap<[u8; 32], HoldSlot>,
    expiry: BinaryHeap<(Instant, HoldSlot)>,
    seq: u64,
    epoch: u64,                         // fencing token
    pending: Batch,                     // applied in memory, not yet committed
    recent: RingBuffer<DeltaFrame>,     // last 60 s, for resume
    public: ArcSwap<PublicSnapshot>,    // pre-encoded; read without the mailbox
    tx: broadcast::Sender<Arc<Bytes>>,  // pre-encoded delta frames
}
```

Memory: about 7 bytes per object plus holds. A 100,000-seat event is under 1 MB.

### 4.2 Commands

| Command | Effect |
|---|---|
| `CreateHold` | New hold token with expiry and limits |
| `Hold` | Free → held under a token |
| `Release` | Held → free (by token) or booked → free (secret key) |
| `ExtendHold` | New expiry |
| `Book` | Held → booked (with token) or free → booked (direct) |
| `ChangeStatus` | Free ↔ blocked or custom |
| `AssignChannel` | Set or clear channel on objects |
| `BestAvailable` | Choose, then hold, atomically |
| `Expire` | Internal: release an expired token's objects |
| `Upgrade` | Move to a new chart version, remapping by `oid` |
| `Inspect` | Read-only: object details for the manager |

### 4.3 Loop

```
loop {
    select! {
        cmd = mailbox.recv()      => apply(cmd),         // validate + mutate memory, queue reply
        _   = next_expiry()       => apply(Expire),
        res = commit_in_flight    => on_commit(res),
    }
    if !commit_in_flight && !pending.is_empty() {
        commit_in_flight = committer.commit(take(pending));   // start immediately
    }
}
```

Properties:

- **Natural batching.** A batch is whatever arrived while the previous commit was in flight. At low load each command commits alone, with minimal latency. Under load, batches grow by themselves. No timer is involved.
- **Acknowledge after commit.** Replies and deltas for a batch are released only when its transaction commits (INV-7, INV-8).
- **Validation sees uncommitted state.** Commands in batch *n+1* are checked against memory that already includes batch *n*. This is safe because batches commit in order and a failed commit poisons the actor (§4.6).
- **Caps.** A batch holds at most 2,000 commands. The mailbox holds at most 10,000; beyond that, callers get `503 overloaded` with `Retry-After`. While a commit is in flight and the next batch is full, the actor stops reading the mailbox, so the excess waits there in order and forms the batch after.
- **Refusals and no-ops.** A refused command, or one that changes nothing (holding only objects the token already holds, releasing a token that holds nothing), takes no `seq` and writes no log row. Its reply carries the `seq` it observed and is released in order with the batch it was applied in, since it may depend on that batch's uncommitted state. A batch of only such commands needs no transaction.
- **Stopping.** When a commit fails, callers whose commands are in that batch, in the next one, or still in the mailbox all get `503 unavailable`, and the mailbox is closed.

### 4.4 Commit

One transaction per batch:

```sql
-- 1. Fence and advance. Zero rows updated means we lost ownership: abort.
UPDATE events
   SET seq = $new_seq, lease_until = now() + $lease
 WHERE id = $event AND owner_epoch = $epoch;

-- 2. New and changed holds.
INSERT INTO holds (...) SELECT ... FROM UNNEST(...);
UPDATE holds SET expires_at = u.expires_at FROM UNNEST(...) u WHERE holds.id = u.id;
DELETE FROM holds WHERE id = ANY($released_or_expired);

-- 3. Objects that are no longer in the default state.
INSERT INTO object_state (event_id, oid, status, hold_id, channel, order_ref, ticket_type, extra, seq)
SELECT $event, * FROM UNNEST($oids, $statuses, $hold_ids, $channels, $order_refs, $ticket_types, $extras, $seqs)
ON CONFLICT (event_id, oid) DO UPDATE SET
  status = EXCLUDED.status, hold_id = EXCLUDED.hold_id, channel = EXCLUDED.channel,
  order_ref = EXCLUDED.order_ref, ticket_type = EXCLUDED.ticket_type,
  extra = EXCLUDED.extra, seq = EXCLUDED.seq;

-- 4. Objects that returned to the default state.
DELETE FROM object_state WHERE event_id = $event AND oid = ANY($freed);

-- 5. Area allocations (inserts and deletes), same pattern.

-- 6. The log: one row per command.
INSERT INTO event_log (event_id, seq, op, actor, hold_id, order_ref, oids, detail)
SELECT ... FROM UNNEST(...);

-- 7. Webhook outbox rows for subscribed types.
INSERT INTO webhook_deliveries (...) SELECT ...;

-- 8. Idempotency records for commands that carried a key.
INSERT INTO idempotency (...) SELECT ...;
```

All eight statements go in one round trip (a pipelined transaction).

A batch carries the net effect of its commands as of its last one: each object appears at most once, in either the upsert list (with the `seq` of the last command that changed it) or the freed list, and a hold created and extended in the same batch is inserted once with its final expiry. The statement order above is then correct without the store knowing engine internals.

### 4.5 Reads

- **Public snapshot:** after each commit the actor re-encodes the public snapshot (at most every 50 ms) and swaps it into `public`. HTTP handlers read it without sending a message to the actor.
- **Deltas:** encoded once per coalescing interval and broadcast as shared bytes. No per-subscriber encoding.
- **Object details and reports:** `Inspect` commands for live data; SQL for historical data.

### 4.6 Failure

| Failure | Response |
|---|---|
| Commit fails (database down, timeout) | Fail every command in that batch and in later uncommitted batches with `503 unavailable`. Drop the actor. The next request reloads it from PostgreSQL |
| Fencing check updates zero rows | Same, and log loudly: another node owns the event |
| Actor panics | The registry catches it, fails pending callers, drops the actor |
| Process crash | Nothing acknowledged is lost. On restart, events with outstanding holds are loaded so that expiries still fire |

There is no in-memory rollback path. Reloading from the database is the single recovery mechanism, which keeps it tested.

### 4.7 Loading and eviction

Load:

1. Acquire ownership: `UPDATE events SET owner_node = $me, owner_epoch = owner_epoch + 1, lease_until = now() + $lease WHERE id = $1 AND (owner_node = $me OR lease_until IS NULL OR lease_until < now()) RETURNING owner_epoch, seq, chart_version_id`.
2. Load the full CCF for the pinned version (cached per version).
3. Read `object_state`, `holds`, `area_alloc`, `channels`; map `oid` to `idx`; fill the arrays.
4. Schedule expiries, including any already past.

Eviction: an actor with no subscribers, no outstanding holds and no command for 10 minutes is dropped. At startup the server loads every event that has rows in `holds`.

### 4.8 Ownership and failover

- v1 runs one node. The lease and epoch are implemented from the start so that a second node can never corrupt state.
- **Active–passive (P2):** a standby takes over events whose lease has lapsed. Lease 15 s, renewed with every commit and by a 5 s heartbeat when idle. Only database time is used.
- **Multi-node (P2):** a node receiving a command for an event it does not own proxies it to the owner. Realtime subscribers on other nodes are fed by a node-to-node stream.
- A deposed node may serve a stale snapshot for up to one lease period. It cannot commit.

## 5. Storage

PostgreSQL 15 or newer. Migrations are embedded and run at startup under an advisory lock.

```sql
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
  oid         int  NOT NULL,
  status      smallint NOT NULL,
  hold_id     bigint,
  channel     smallint,
  order_ref   text,
  ticket_type text,
  extra       jsonb,
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
  detail    jsonb,
  PRIMARY KEY (event_id, seq, at)
) PARTITION BY RANGE (at);                       -- monthly partitions, configurable retention

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
  key          text NOT NULL,
  request_hash bytea NOT NULL,
  status       int NOT NULL,
  response     jsonb NOT NULL,
  created_at   timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (workspace_id, key)
);

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
```

The `SeatStore` trait hides all of this from the engine: `load_event`, `commit_batch`, `acquire`, `heartbeat`. Tests use an in-memory implementation with fault injection.

## 6. REST API

### 6.1 Conventions

- Base path `/v1`. JSON bodies, `snake_case` fields. Times are RFC 3339 UTC.
- Authentication: `Authorization: Bearer sk_…` or `Bearer pk_…`. The workspace is implied by the key.
- Mutating requests accept `Idempotency-Key` (up to 255 characters, kept 24 hours). Replaying a key with a different body returns `409 idempotency_mismatch`.
- Objects are addressed by label. A request may instead send `{"version": "<hash>", "idx": [..]}`.
- Lists use cursor pagination: `?limit=100&after=<cursor>`; responses carry `next`.
- Every response carries `Cavea-Request-Id`.
- The OpenAPI 3.1 document is served at `/v1/openapi.json`.

### 6.2 Endpoints

Secret key unless marked.

**Charts**

| Method and path | Purpose |
|---|---|
| `GET /v1/charts` | List |
| `POST /v1/charts` | Create (`key`, `name`, optional document) |
| `GET /v1/charts/{chart}` | Metadata, draft etag, published version |
| `GET /v1/charts/{chart}/draft` | The draft document |
| `PUT /v1/charts/{chart}/draft` | Replace the draft; requires `If-Match: <etag>` |
| `POST /v1/charts/{chart}/validate` | Run validation on the draft |
| `POST /v1/charts/{chart}/publish` | Compile and publish; returns the new version |
| `GET /v1/charts/{chart}/versions` | List versions |
| `GET /v1/charts/{chart}/versions/{number}/diff` | Objects added, removed and relabelled against the previous version |
| `POST /v1/charts/{chart}/copy` | Duplicate |
| `DELETE /v1/charts/{chart}` | Archive |
| `POST /v1/assets` | Upload an image; returns its hash |

**Events**

| Method and path | Purpose |
|---|---|
| `GET /v1/events` | List |
| `POST /v1/events` | Create (`key`, `chart`, optional `version`, `settings`) |
| `GET /v1/events/{event}` | Metadata, settings, counts by status |
| `PATCH /v1/events/{event}` | Change name or settings |
| `POST /v1/events/{event}/upgrade` | Move to a newer chart version |
| `DELETE /v1/events/{event}` | Archive |
| `GET /v1/events/{event}/rt` | Manager realtime stream (WebSocket) with domain statuses |
| `PUT /v1/events/{event}/channels` | Replace the channel list |
| `POST /v1/events/{event}/channels/assign` | Put objects in a channel, or clear |

**Seat operations**

| Method and path | Purpose |
|---|---|
| `POST /v1/hold-tokens` | Create a hold token (`event`, optional `ttl_seconds`, `max_objects`) |
| `GET /v1/hold-tokens/{token}` | Expiry and held objects |
| `POST /v1/hold-tokens/{token}/extend` | New expiry |
| `DELETE /v1/hold-tokens/{token}` | Release everything it holds |
| `POST /v1/events/{event}/hold` | Hold objects under a token |
| `POST /v1/events/{event}/release` | Release held or booked objects |
| `POST /v1/events/{event}/book` | Book by hold token or directly |
| `POST /v1/events/{event}/change-status` | Block, unblock, set a custom status |
| `POST /v1/events/{event}/best-available` | Choose and hold |
| `GET /v1/events/{event}/objects` | Status of given labels, or a filtered page |

**Reports and log**

| Method and path | Purpose |
|---|---|
| `GET /v1/events/{event}/reports/summary?by=status\|category\|section\|channel` | Counts |
| `GET /v1/events/{event}/reports/objects?status=&format=json\|csv` | Per-object export |
| `GET /v1/events/{event}/log?after_seq=` | The command log |

**Administration**

| Method and path | Purpose |
|---|---|
| `GET/POST/DELETE /v1/keys` | API keys |
| `GET/POST/PATCH/DELETE /v1/webhooks` | Endpoints |
| `GET /v1/webhooks/{id}/deliveries`, `POST …/deliveries/{id}/replay` | Delivery log |
| `GET/POST /v1/workspaces` | Requires an admin key |
| `POST /v1/embed-tokens` | Short-lived token for the embedded designer or manager |

**Public (publishable key, CORS enabled)**

| Method and path | Purpose |
|---|---|
| `GET /v1/public/events/{event}` | Chart URL, hold lifetime, selection rules |
| `GET /v1/public/events/{event}/state` | Binary snapshot; `?after_seq=n` returns a delta if still buffered |
| `GET /v1/public/events/{event}/stream` | SSE stream |
| `GET /v1/public/rt` | WebSocket |
| `POST /v1/public/hold-tokens` | Create a hold token |
| `POST /v1/public/events/{event}/hold` | Hold with the caller's token |
| `POST /v1/public/events/{event}/release` | Release with the caller's token |
| `POST /v1/public/events/{event}/best-available` | Only if enabled for the event |
| `GET /v1/public/events/{event}/sections` | JSON: floors, sections, categories with availability counts (for the list view without WASM) |
| `GET /v1/public/events/{event}/sections/{section}/rows` | JSON: rows with their available seats, grouped into runs |
| `GET /v1/public/charts/{chart}/versions/{hash}.ccf` | Compiled chart |
| `GET /v1/public/assets/{hash}` | Background images |

**System**

`GET /healthz` (process alive), `GET /readyz` (database reachable, migrations applied), `GET /metrics` (Prometheus), `GET /v1/system/info` (version, licence state).

### 6.3 Examples

Hold:

```http
POST /v1/public/events/hamlet-2027-03-14/hold
Authorization: Bearer pk_live_…

{ "hold_token": "ht_8Zk…", "objects": ["Orchestra-C-12", "Orchestra-C-13"] }
```

```json
{ "seq": 48211, "expires_at": "2027-03-01T19:15:00Z",
  "objects": [ { "label": "Orchestra-C-12", "oid": 4112 }, { "label": "Orchestra-C-13", "oid": 4113 } ] }
```

Refusal (nothing was held):

```json
{ "type": "https://docs.cavea.example/errors/object_not_free",
  "code": "object_not_free", "status": 409,
  "title": "One or more objects are not free",
  "details": { "objects": [ { "label": "Orchestra-C-13", "reason": "not_free" } ] } }
```

Book after payment:

```http
POST /v1/events/hamlet-2027-03-14/book
Authorization: Bearer sk_live_…
Idempotency-Key: order-77120

{ "hold_token": "ht_8Zk…", "order_ref": "77120",
  "ticket_types": { "Orchestra-C-12": "adult", "Orchestra-C-13": "child" } }
```

### 6.4 Error codes

| HTTP | `code` | Meaning |
|---|---|---|
| 400 | `invalid_request` | Malformed body or parameter |
| 401 | `unauthorized` | Missing or invalid key |
| 403 | `forbidden` | Key lacks permission, or origin not allowed |
| 404 | `not_found` | Unknown chart, event, object or token |
| 409 | `object_not_free` | At least one object is not free |
| 409 | `object_not_held_by_token` | Book or release named an object the token does not hold |
| 409 | `version_conflict` | Draft etag mismatch |
| 409 | `upgrade_conflict` | Non-free objects missing from the new chart version |
| 409 | `idempotency_mismatch` | Key reused with a different body |
| 410 | `hold_token_expired` | |
| 422 | `hold_limit_exceeded` | Over the token's object limit |
| 422 | `channel_not_allowed` | |
| 422 | `orphan_rule_violated` | |
| 422 | `not_enough_objects` | Best-available could not satisfy the request |
| 422 | `chart_invalid` | Publish blocked by validation errors |
| 429 | `rate_limited` | |
| 503 | `overloaded` | Actor mailbox full; retry after the given delay |
| 503 | `unavailable` | Storage unreachable |

## 7. Algorithms

### 7.1 Hold

1. Find the hold by token hash. Unknown → `not_found`; expired → `hold_token_expired`.
2. Resolve labels to `idx`. Unknown → `not_found`.
3. For each object: already held by this token → skip. Not free → collect as failed. In a channel the request may not use → collect as failed.
4. If any failed → reply with the list; change nothing.
5. Count: the token's objects plus the new ones must not exceed its limit.
6. If the event's orphan policy is `enforce`, run the orphan check on the token's whole selection (`02-domain-model.md` §12).
7. Mutate memory, add to the pending batch, reply after commit.

Request shape is checked first: an empty object list, more than 1,000 objects, or a lifetime outside 60 to 7,200 seconds → `invalid_request`. Duplicate references in one request count once. Unknown references are all listed together.

**Create hold.** The token's hash must be new to the event. Its expiry is the engine clock at processing time plus the lifetime; its id is the command's `seq`. The caller sets the object limit (none for secret keys); the engine only rejects a limit of zero.

**Release by token.** The token must exist and be unexpired. Every named object must be held by it, otherwise → `object_not_held_by_token` listing each failing object; an empty list releases all of the token's objects. The token stays valid with no objects; only expiry (and booking its last objects) deletes it.

**Extend.** The token must exist and be unexpired: an expired token cannot be revived. The new expiry is processing time plus the requested lifetime; the caller enforces the workspace maximum, the engine the 60 to 7,200 second bounds.

### 7.2 Book

- **With a token:** all named objects (or all the token's objects if none are named) must be held by it. They become `booked` with the order reference. If the token then holds nothing, it is deleted.
- **Direct:** all named objects must be free and channel-permitted (unless `ignore_channels`).
- Areas take `{"label", "quantity"}`.

### 7.3 Expiry

- When the heap's earliest expiry passes, the actor issues an internal `Expire` for that token: all its objects return to free, the hold is deleted, a `hold.expired` outbox row is written.
- Expiry is checked against the server clock at processing time, so a hold used at the last instant either fully succeeds or fully fails.

### 7.4 Best-available

Request: `quantity`, optional `categories`, `sections`, `floor`, `accessible` (count), `prefer_aisle`, `prevent_orphans` (default true), `hold_token`, and channels as for hold.

1. **Eligibility.** A seat is eligible if it is free, matches the filters and is channel-permitted. Accessible seats are eligible only for the accessible part of a request.
2. **Windows.** For every row segment, consider each run of `quantity` consecutive eligible seats.
3. **Cost.** `cost = Σ rank + orphan_penalty + aisle_bonus`, where the orphan penalty applies if the window leaves exactly one eligible seat on either side, and the aisle bonus is negative when `prefer_aisle` and the window touches a segment edge.
4. **Search.** Visit rows in ascending order of their best seat rank. Stop when `quantity ×` the next row's best rank cannot beat the best cost found. Window sums use a running total, so the search is linear in seats visited.
5. **Orphans.** With `prevent_orphans`, a window that creates an orphan is used only if no window anywhere avoids one.
6. **No single run.** If no segment has `quantity` consecutive eligible seats:
   - Try two adjacent rows of the same section with overlapping horizontal extent, splitting as evenly as possible.
   - Otherwise fill greedily with the largest available runs in the best-ranked section, then anywhere.
   - The response sets `contiguous: false` and lists the groups.
7. **Accessible requests.** Choose the accessible seats first, then extend along the segment to companion-flagged or neighbouring seats for the rest of the party.
8. **Hold** the chosen objects in the same command. The choice and the claim are atomic.
9. For areas, best-available returns the area if it has capacity and matches the filters; areas rank after seats unless the request names an area category.

If fewer than `quantity` eligible objects exist, reply `not_enough_objects`.

## 8. Realtime protocol

Binary frames. Integers are LEB128 varints unless stated. The protocol version is 1.

### 8.1 Server to client

| Type | Name | Payload |
|---|---|---|
| `0x01` | HELLO | Protocol version (u8), server time in ms, chart hash (16 bytes), object count |
| `0x02` | SNAPSHOT | `seq`, object count, encoding (u8), state payload, area count, per area (`idx`, availability) |
| `0x03` | DELTA | `from_seq`, `to_seq`, change count, per change (`idx` difference from the previous, code u8), area change count, per area (`idx`, availability) |
| `0x04` | PRIVATE | Hold expiry in ms, object count, `idx` list, per-area quantities |
| `0x05` | RESYNC | The client fell behind; fetch a new snapshot |
| `0x06` | PING | Nonce |
| `0x07` | CLOSE | Reason: event archived, chart upgraded (reload the chart), server shutting down |

Snapshot encodings: 0 = one byte per object; 1 = run-length pairs; 2 = one bit per object (public streams only). The server picks the smallest.

### 8.2 Client to server

| Type | Name | Payload |
|---|---|---|
| `0x10` | SUBSCRIBE | Protocol version, event key, last `seq` seen (0 if none), optional hold token, optional session token |
| `0x11` | PONG | Nonce |

### 8.3 Behaviour

- On SUBSCRIBE the server replies HELLO, then either the DELTAs since `last_seq` (if still in the 60-second ring buffer) or a SNAPSHOT, then PRIVATE if a hold token was given.
- Deltas are coalesced every 100 ms. With more than 20,000 subscribers on an event the interval stretches, up to 500 ms.
- If an object changes twice within an interval, only the final state is sent.
- A subscriber whose send queue exceeds 64 frames is sent RESYNC and its queue is cleared.
- PING every 25 s; a connection silent for 60 s is closed.
- **Public streams** carry public codes only (`02-domain-model.md` §4.2). **Manager streams** (`GET /v1/events/{event}/rt`, secret key or console session) carry domain statuses.
- Viewers with channels get a stream computed for their channel set. Channel sets are cached per event, so cost grows with distinct sets, not with viewers.

### 8.4 Fallbacks

| Transport | Framing |
|---|---|
| WebSocket | Binary messages, one frame each |
| SSE | Each frame base64-encoded in a `data:` line; `Last-Event-ID` carries `seq` |
| Polling | `GET …/state?after_seq=n` every 2 s returns a DELTA or SNAPSHOT frame |

### 8.5 Caching the snapshot

- `GET /v1/public/events/{event}/state` returns a SNAPSHOT frame with `ETag: "<seq>"` and `Cache-Control: public, max-age=1, stale-while-revalidate=5` (configurable).
- With a CDN in front, the opening rush of an on-sale hits the CDN, not the server. Clients then catch up through deltas.
- Channel-specific snapshots are `private` and not cached by intermediaries.

## 9. Webhooks

| Type | Fires when |
|---|---|
| `hold.created`, `hold.extended`, `hold.expired`, `hold.released` | Hold token lifecycle |
| `objects.held`, `objects.released`, `objects.booked`, `objects.status_changed` | Per command, with the object list |
| `event.created`, `event.updated`, `event.upgraded`, `event.archived` | |
| `event.sold_out`, `event.available_again` | Availability crosses zero |
| `chart.published` | |

- Payload: `{ "id", "type", "created_at", "workspace", "event", "seq", "data": { ... } }`.
- Signature header: `Cavea-Signature: t=<unix>,v1=<hex HMAC-SHA256(secret, t + "." + body)>`.
- Delivery is at least once. Consumers de-duplicate on `id` and order on `seq`.
- Retries: 10 attempts over about 24 hours with growing delays. After that the delivery is marked failed and can be replayed from the console.
- Rows are written to the outbox in the same transaction as the state change, so no event is lost.
- A slow endpoint never delays seat operations: dispatch is a separate task with its own concurrency limit per endpoint.

## 10. Reports

| Report | Content |
|---|---|
| Summary by status, category, section or channel | Counts and, for areas, quantities |
| Objects | One row per object: label, section, row, seat, category, status, order reference, ticket type, channel, hold expiry |
| Order | All objects for one order reference |
| Log | Commands in order, filterable by object, order reference, actor |

Live reports read the actor. CSV exports stream from PostgreSQL.

## 11. Authentication and tenancy

| Credential | Holder | Scope |
|---|---|---|
| Admin key | Platform operator | Create and list workspaces |
| Secret key | Customer backend | Everything in one workspace |
| Publishable key | Browser, mobile app | Public endpoints of one workspace; optional origin allow-list |
| Session token | Buyer's browser | A JWT signed by the customer's backend with a secret key (HS256). Claims: `sub`, `exp`, `event`, `max_objects`, `channels` |
| Console session | Staff | Cookie session; role per workspace |
| Embed token | Embedded designer or manager | Short-lived, scoped to one workspace and one chart or event, with a permission list |

- Session tokens are optional. An event can require them (`require_session: true`), which is the strongest defence against anonymous seat hoarding.
- Every query is scoped by workspace id taken from the credential, never from the request.

## 12. Rate limiting and overload

- Public endpoints: token bucket per (publishable key, client IP). Defaults: 120 holds per minute, 3,000 reads per minute. Configurable; can be disabled behind the customer's own gateway.
- Secret-key endpoints: no limit by default.
- Client IP is taken from the socket, or from a configured trusted proxy header.
- Mailbox full → `503 overloaded` with `Retry-After`.
- Database connection pool exhausted → `503 unavailable`.
- Limits are per node in v1.

## 13. Configuration

| Variable | Default | Meaning |
|---|---|---|
| `CAVEA_DATABASE_URL` | — | PostgreSQL connection string (required) |
| `CAVEA_LISTEN` | `0.0.0.0:8080` | |
| `CAVEA_LOG` | `info` | Log filter, in `tracing` directive syntax such as `info,cavea_server=debug` |
| `CAVEA_PUBLIC_URL` | — | External base URL, used in chart URLs and webhooks |
| `CAVEA_LICENCE` | — | Licence file path or contents |
| `CAVEA_ADMIN_KEY` | — | Bootstrap admin key |
| `CAVEA_DB_POOL` | 16 | Connection pool size |
| `CAVEA_HOLD_TTL` | 900 | Default hold lifetime in seconds |
| `CAVEA_DELTA_INTERVAL_MS` | 100 | |
| `CAVEA_SNAPSHOT_MAX_AGE` | 1 | Seconds, for the cache header |
| `CAVEA_TRUSTED_PROXIES` | none | CIDR list |
| `CAVEA_CORS_ORIGINS` | `*` | For public endpoints |
| `CAVEA_LOG_RETENTION_DAYS` | 400 | `event_log` partitions older than this are dropped |
| `CAVEA_OBJECT_STORE_URL` | none | Optional S3-compatible storage for assets |
| `CAVEA_OTLP_ENDPOINT` | none | Trace export |
| `CAVEA_NODE_ID` | hostname | |

## 14. Deployment

- **Artefacts:** a static binary and a distroless Docker image for x86-64 and arm64.
- **Minimum:** 1 vCPU, 512 MB, PostgreSQL 15. **Recommended for on-sales:** 4 vCPU, 8 GB, PostgreSQL on fast storage in the same network.
- **PostgreSQL:** `synchronous_commit = on`. Backups and point-in-time recovery are the customer's responsibility; the docs give a recipe.
- **TLS:** terminate at the customer's proxy or load balancer. WebSocket upgrade must be allowed.
- **Shutdown:** on SIGTERM, stop accepting commands, flush pending batches, send CLOSE to subscribers, release leases, exit within 20 s.
- **Upgrade:** replace the image and restart. Migrations run automatically; a migration that cannot run online is called out in the release notes.
- **Reference files:** `deploy/compose.yaml`, `deploy/helm/`, a systemd unit.

## 15. Observability

Metrics (Prometheus):

| Metric | Type |
|---|---|
| `cavea_commands_total{op, result}` | Counter |
| `cavea_command_duration_seconds{op}` | Histogram |
| `cavea_commit_duration_seconds`, `cavea_commit_batch_size` | Histograms |
| `cavea_actor_mailbox_depth` | Gauge |
| `cavea_actors_loaded` | Gauge |
| `cavea_holds_active` | Gauge |
| `cavea_rt_subscribers{transport}` | Gauge |
| `cavea_rt_resyncs_total` | Counter |
| `cavea_webhook_deliveries_total{result}`, `cavea_webhook_lag_seconds` | Counter, gauge |
| `cavea_http_requests_total{route, status}`, `cavea_http_duration_seconds{route}` | Counter, histogram |
| `cavea_db_pool_in_use` | Gauge |

Tracing: one span per request, with child spans for mailbox wait, apply and commit. Logs are JSON with request id, workspace and event.

## 16. Failure modes

| Failure | Effect | Recovery |
|---|---|---|
| PostgreSQL unreachable | Commands fail with 503. Snapshots keep being served from memory; realtime stays connected | Automatic when the database returns |
| Server crash | In-flight unacknowledged commands are lost (callers got no success). Subscribers reconnect | Restart; actors reload; holds expire on schedule |
| Network partition between two nodes (P2) | The old owner's commits fail the fencing check | The new owner continues |
| Slow webhook endpoint | Deliveries queue for that endpoint only | Retries; replay from the console |
| Subscriber too slow | Receives RESYNC | Client refetches the snapshot |
| Clock jump | Holds may expire early or late by the jump | Run NTP; expiry uses the server clock consistently |
| Corrupt or oversized request | Rejected with 400 | — |
| Disk full on PostgreSQL | Commands fail with 503 | Operator action |

## 17. Scaling path

| Stage | Topology | Ceiling |
|---|---|---|
| 1 (v1) | One node, PostgreSQL | Tens of thousands of commands per second per event; about 50,000 subscribers |
| 2 | Active–passive with lease takeover | Same, with failover in about 20 s |
| 3 | Several nodes; events distributed by ownership; node-to-node forwarding | Subscribers and events scale horizontally |
| 4 | CDN in front of snapshots and chart files | Read storms absorbed at the edge |

Stage 1 is expected to serve most customers indefinitely.
