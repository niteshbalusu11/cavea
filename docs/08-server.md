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
| `engine` (crate `cavea-engine`) | Actor, state machine, hold expiry, best-available; the actor registry (finds or loads the actor for an event, ownership, poison and reload, eviction), kept here behind `SeatStore` so that the simulation harness drives it |
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

Memory: about 7 bytes per object plus holds. A 100,000-seat event is under 1 MB. Booking data (`order_ref`, `ticket_type`, `extra`) lives in a map holding only booked objects: about 106 bytes per booked seat with a ticket type, so a fully booked 100,000-seat event adds about 11 MB, plus the length of any `extra`. The idempotency window (§6.1) adds at most 50,000 records.

### 4.2 Commands

| Command | Effect |
|---|---|
| `CreateHold` | New hold token with expiry and limits |
| `Hold` | Free → held under a token |
| `Release` | Held → free (by token) or booked → free (secret key, no token) |
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
- **Refusals and no-ops.** A refused command, or one that changes nothing (holding only objects the token already holds, releasing a token that holds nothing), takes no `seq` and writes no log row. Its reply carries the `seq` it observed and is released in order with the batch it was applied in, since it may depend on that batch's uncommitted state. A batch of only such commands needs no transaction, unless one carried an idempotency key: its record is written (statement 8) with `seq` unchanged.
- **Expiry.** `next_expiry()` waits on the injected clock, never the wall clock, so a test advances the clock and sees expiries in order. Before any command is applied, every hold due at its processing time is expired first, each as its own `Expire` command with a `seq`; the timer arm only makes that happen when no command arrives. Expiries do not count towards the batch cap: a batch opened after a long stall carries every overdue expiry.
- **Stopping.** When a commit fails, callers whose commands are in that batch, in the next one, or still in the mailbox all get `503 unavailable`, and the mailbox is closed.

### 4.4 Commit

One transaction per batch. The parts, in order:

```sql
-- 1. Fence and advance. Zero rows updated means we lost ownership, or the batch
--    does not continue the stored seq, or extends or deletes a missing hold:
--    nothing is written.
UPDATE events
   SET seq = $new_seq, lease_until = now() + $lease
 WHERE id = $event AND owner_epoch = $epoch AND seq = $new_seq - $commands;

-- 2. New and changed holds.
INSERT INTO holds (...) SELECT ... FROM UNNEST(...);
UPDATE holds SET expires_at = u.expires_at FROM UNNEST(...) u WHERE holds.event_id = $event AND holds.id = u.id;
DELETE FROM holds WHERE event_id = $event AND id = ANY($released_or_expired);

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
-- `detail` holds what replaying the log needs beyond the columns: the status set
-- by change-status, a hold's expiry, a booking's per-object ticket type and extra.
INSERT INTO event_log (event_id, seq, op, actor, hold_id, order_ref, oids, detail)
SELECT ... FROM UNNEST(...);

-- 7. Webhook outbox rows for subscribed types.
INSERT INTO webhook_deliveries (...) SELECT ...;

-- 8. Idempotency records for commands that carried a key, refusals included.
--    A live record is never overwritten: a key another event used is kept.
INSERT INTO idempotency (workspace_id, event_id, key, request_hash, status, response, created_at)
SELECT (SELECT workspace_id FROM events WHERE id = $event), $event, * FROM UNNEST(...)
ON CONFLICT (workspace_id, key) DO UPDATE SET ...
 WHERE idempotency.created_at <= EXCLUDED.created_at - interval '24 hours';
```

All eight parts go in one round trip as one statement: each is a data-modifying CTE fed by `UNNEST` arrays, every write is gated on the fence (`WHERE EXISTS (SELECT FROM fence)`), and the statement returns how many rows the fence updated. A single statement is its own transaction, so it needs no `BEGIN` or `COMMIT`; any error in any part aborts all of it. `sqlx` cannot pipeline separate statements (D-23). When the fence matches nothing, a second query reads the event's epoch and `seq` to report `fenced`, a `seq` mismatch or a missing hold. The fence also renews the lease by the database clock; it checks the epoch only, so a lapsed lease that no other node has taken still commits. A record is replaced once the stored one is 24 hours old or more, matching the engine, for which a record is expired from that instant.

A batch carries the net effect of its commands as of its last one: each object appears at most once, in either the upsert list (with the `seq` of the last command that changed it) or the freed list, and a hold created and extended in the same batch is inserted once with its final expiry; a hold created and deleted in the same batch is not written at all. The statement order above is then correct without the store knowing engine internals.

### 4.5 Reads

- **Public snapshot:** after each commit the actor re-encodes the public snapshot (at most every 50 ms) and swaps it into `public`. HTTP handlers read it without sending a message to the actor.
- **Deltas:** encoded once per coalescing interval and broadcast as shared bytes. No per-subscriber encoding.
- **Object details and reports:** `Inspect` commands for live data; SQL for historical data.

### 4.6 Failure

| Failure | Response |
|---|---|
| Commit fails (database down, timeout) | Fail every command in that batch and in later uncommitted batches with `503 unavailable`. Drop the actor. The next request reloads it from PostgreSQL |
| Fencing check updates zero rows, or a heartbeat fails | Same, and for fencing log loudly: another node owns the event |
| Actor panics | The registry catches it, fails pending callers, drops the actor |
| Load fails (database down, event leased by another node) | Every request waiting for that load gets `503 unavailable`. For the next second, requests for the event fail at once without touching the database; after that the next request tries again. An outage costs at most one load attempt per event per second. A load that fails after acquiring the event releases its lease |
| Process crash | Nothing acknowledged is lost. On restart, events with outstanding holds are loaded so that expiries still fire |

There is no in-memory rollback path. Reloading from the database is the single recovery mechanism, which keeps it tested. A request that reaches an actor as it stops is not applied: it waits for the actor to end and goes to the reloaded one.

A commit that takes longer than 5 s, from waiting for a connection to the reply, fails as `unavailable`; so does an acquire, heartbeat or release. Whether a timed-out commit was written is then unknown, and the statement may still commit on the server after the actor has stopped. That is safe: if it commits before the reload acquires the event, the reloaded state includes it; if after, it fails its fence, because acquiring raised the epoch (§4.7 step 1), also for a batch of only idempotency records, which leaves `seq` unchanged. Nothing is acknowledged that is not durable (INV-7) and no delta is published for it (INV-8).

### 4.7 Loading and eviction

Load:

1. Acquire ownership: `UPDATE events SET owner_node = $me, owner_epoch = owner_epoch + 1, lease_until = now() + $lease WHERE id = $1 AND (owner_node = $me OR lease_until IS NULL OR lease_until < now()) RETURNING owner_epoch, chart_version_id`. No row: the event is unknown, or another node holds a live lease (`leased`, answered `503 unavailable`). The same node acquiring again (a reload) always succeeds and fences its earlier actor.
2. Load the full CCF for the pinned version (cached per version).
3. Read `object_state`, `holds`, `area_alloc`, `channels`, and the event's `idempotency` records younger than 24 hours by the database clock, all in one `REPEATABLE READ` snapshot; map `oid` to `idx`; fill the arrays. If the snapshot's epoch is not the one just acquired, another node took the event in between and the load fails.
4. Schedule expiries, including any already past: the actor expires those first, as its first commit.

Concurrent requests for an event that is not loaded share one load; a failed load is reported to all of them (§4.6).

Eviction: an actor with no subscribers, no outstanding holds and no command for 10 minutes stops: it takes no more commands, commits what it accepted and releases its lease. It is checked once per 10 minutes on the engine clock, so it stops between 10 and 20 minutes after the last command, hold expiry or departing subscriber. A subscriber keeps the actor with a keep-alive guard from the registry. A command after eviction loads the event again. At startup the server loads every event that has rows in `holds`.

### 4.8 Ownership and failover

- v1 runs one node. The lease and epoch are implemented from the start so that a second node can never corrupt state.
- **Active–passive (P2):** a standby takes over events whose lease has lapsed. Lease 15 s, renewed with every commit and by a 5 s heartbeat when idle (`CAVEA_LEASE_SECS`, `CAVEA_HEARTBEAT_SECS`). Only database time is used. A heartbeat that fails or is fenced stops the actor as a failed commit does. On shutdown and eviction the lease is released (`lease_until = NULL`), so another node or a restart can take the event at once.
- **Multi-node (P2):** a node receiving a command for an event it does not own proxies it to the owner. Realtime subscribers on other nodes are fed by a node-to-node stream.
- A deposed node may serve a stale snapshot for up to one lease period. It cannot commit.

## 5. Storage

PostgreSQL 15 or newer. Migrations are embedded and run at startup under an advisory lock (`sqlx`'s migrator, recording each migration's SHA-384 in `cavea_migrations`). The server refuses to start if the database holds a migration it does not know (a newer server migrated it) or one whose content differs from its own. The schema below is migration 1.

Times cross the boundary as Unix milliseconds converted in SQL, event ids as hex text cast to `uuid`, JSON as text.

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
```

Migration 2 adds `chart_versions.full_hash bytea NOT NULL` (16 bytes, the full artefact's content hash), replaces `UNIQUE (chart_id, hash)` with `UNIQUE (chart_id, full_hash)`, and indexes `hash`, by which the public chart route finds a file (§6.2).

The `SeatStore` trait hides all of this from the engine: `acquire`, `load_event`, `commit_batch`, `heartbeat`, `release`. A store speaks for one node. Tests use an in-memory implementation with fault injection, whose leases follow the injected clock.

## 6. REST API

### 6.1 Conventions

- Base path `/v1`. JSON bodies, `snake_case` fields. Times are RFC 3339 UTC.
- Authentication: `Authorization: Bearer sk_…` or `Bearer pk_…` (§11). The workspace is implied by the key.
- Mutating requests accept `Idempotency-Key` (1 to 255 characters, none of them U+0000; kept 24 hours). Replaying a key with a different body returns `409 idempotency_mismatch`.
- For seat commands the engine does this atomically with the command. The handler computes a 32-byte fingerprint of the request (method, path, body), looks the key up in `idempotency` first and, if found, replays or mismatches without reaching the actor; otherwise it submits the command with the key, the fingerprint and the time of that lookup. The actor keeps an in-memory window of its event's records: every keyed command it applies gets a record, refusals included (a refused request replayed later must not succeed), holding the engine's reply exactly as returned, `seq` and per-object failures included. A key in the window replays that reply or fails with `idempotency_mismatch`, changing nothing; a key not in it is applied. The window is the authority for records not yet visible to the handler's lookup (uncommitted, or committed after it) and the store for everything older. Records are written in the commit (§4.4 statement 8) and loaded with the event (§4.7 step 3), so a retry after a lost reply and a reload returns the original reply. A record matches for 24 hours from when its command was applied. The window holds at most 50,000 records. To make room it drops the oldest only if they were already a minute old at the command's store lookup, never by the time the command is processed: a record the lookup could not see was committed after it (commits time out well within a minute), so it is still in the window however long the command waited in the mailbox. When no record qualifies, the new key is refused `503 overloaded` and the caller looks it up again. `overloaded`, `unavailable`, an invalid key and a mismatch write no record. A key is per workspace, but the engine checks it per event: two events given the same new key at the same moment both apply, and the store keeps the first record.
- Objects are addressed by label. A request may instead send `{"version": "<hash>", "idx": [..]}`.
- Lists use cursor pagination: `?limit=100&after=<cursor>` (`limit` 1 to 100, default 100). A list response is `{"data": [..], "next": "<cursor>"}`, oldest first; `next` is the id of the last item when another page may follow, else `null`, and is passed back as `after`. Server-generated ids are `ws_` or `key_` and 32 hex digits.
- Request limits (`12-security-licensing.md` T19): a body is at most 256 KiB unless its route sets a larger limit; JSON arrays and objects nest at most 32 deep, checked by a scan of the raw bytes before parsing; a command names at most 1,000 objects (`02-domain-model.md` §14). Name and customer key fields are 1 to 128 characters without U+0000.
- Every response, errors and 404s included, carries `Cavea-Request-Id`. The server generates `req_` and 32 lowercase hex digits. A caller may send its own `Cavea-Request-Id` to correlate with its logs: the server adopts it only if it is 1 to 64 characters from `A-Z a-z 0-9 . _ -`, and otherwise ignores it and generates one. An adopted id is not guaranteed unique.
- The OpenAPI 3.1 document is served at `/v1/openapi.json`.

### 6.2 Endpoints

Secret key unless marked.

**Charts**

| Method and path | Purpose |
|---|---|
| `GET /v1/charts` | List chart objects (never drafts), archived ones included; the cursor is a chart key |
| `POST /v1/charts` | Create (`key`, `name`, optional `document`); `201` with the chart object |
| `GET /v1/charts/{chart}` | The chart object: metadata, draft etag, published version |
| `GET /v1/charts/{chart}/draft` | The draft document, with its etag in `ETag` |
| `PUT /v1/charts/{chart}/draft` | Replace the draft (the body is the document); requires `If-Match`; returns the chart object |
| `POST /v1/charts/{chart}/validate` | Compile the draft and return its findings, without publishing |
| `POST /v1/charts/{chart}/publish` | Compile and publish; optional `If-Match`; `201` with the new version, or `200` with the existing one |
| `GET /v1/charts/{chart}/versions` | List version objects, oldest first; the cursor is a version id |
| `GET /v1/charts/{chart}/versions/{number}/diff` | Objects added, removed and relabelled against the previous version |
| `POST /v1/charts/{chart}/copy` | Duplicate |
| `DELETE /v1/charts/{chart}` | Archive; returns the chart object. Idempotent |
| `POST /v1/assets` | Upload an image; returns its hash |

`{chart}` is the customer's chart key (02 §3). Chart rules:

- **Chart object:** `{key, name, draft_etag, draft_updated_at, published_version, archived_at, created_at}`. `published_version` is a version object or `null`.
- **Version object:** `{id, number, hash, url, stats, created_at}`. `id` is `cv_` and 32 hex digits; `number` counts from 1 per chart; `hash` is the public artefact's content hash, 32 lowercase hex digits (03 §4.1); `url` is where it is served, relative to the server's base URL (`/v1/public/charts/{hash}.ccf`); `stats` is `{seats, rows, segments, sections, floors, public_bytes, public_br_bytes, full_bytes}`.
- **Draft etag:** an opaque string such as `"3"` (the quotes are part of it, as in an HTTP entity tag), which changes on every save. Pass it back verbatim in `If-Match`. `If-Match: *` matches any draft. Any other form, or a weak tag, is `400` (`reason: "if_match"`).
- **Saving a draft:** a missing `If-Match` is `400` (`reason: "if_match"`): a caller that means to overwrite whatever is there says so with `*`. A stale etag is `409 version_conflict` with the current `draft_etag` in `details`. The body is at most 16 MiB (a create body 16 MiB and 4 KiB): 27 times a parametric document at the design limit of 1,000,000 objects (02 §14; 600 KB measured), or about 16 bytes of per-seat data per object. A draft must parse as a chart document of a schema the server reads (unknown fields refused, 03 §2) and may not hold `\u0000`, which PostgreSQL `jsonb` cannot store; otherwise `400` with `reason: "document"` and the parser's message. It need not validate: the designer saves work in progress. Parsing runs on a blocking thread. The draft is stored as uploaded, as `jsonb` (03 §2.8), and read back in `jsonb`'s canonical form.
- **Creating:** without `document`, the draft is `{"schema": 1, "name": <name>, "nextOid": 1}`. A key already used in the workspace, by an archived chart too, is `409 already_exists`.
- **Validate:** `200` with `{draft_etag, valid, findings, stats}`. Each finding is `{code, severity, node, message}`: `code` is an E or W code of 03 §3.2, or `unsupported` for a node the compiler cannot compile yet; `severity` is `error` or `warning`; `node` is the node id, or `null` for findings about settings, categories and floors. `valid` is true when no finding is an error. `stats` is `null` when the draft did not compile.
- **Publish:** compiles the draft as it is at the request. With `If-Match`, a draft whose etag differs is `409 version_conflict` before anything is compiled. Findings of error severity refuse it: `422 chart_invalid` with `details.findings` as above. Otherwise, in one transaction with the chart row locked, the server stores a new immutable version (02 INV-12): the next number, the public and full content hashes, the draft as `source`, the public artefact, its brotli form, the full artefact and `stats`; and points the chart's published version at it. A draft saved while the compile ran makes the publish `409 version_conflict`, and an archive while it ran `400` (`reason: "archived"`), creating nothing: a version is always the draft that was current when it was stored. Publishes racing on one chart are serialised by the row lock.
- **Unchanged publish:** versions are identified by the full artefact's hash, which covers every compiled field. A draft that compiles to a version the chart already has publishes that version again: `200` with it, no new row, and the chart's published version points at it (so republishing an older draft goes back to its version). Two versions may share a public hash when they differ only in full-only data (`oid`s, ranks, section priorities, 03 §4.2); they then share one public file.
- **Compile bounds:** validate and publish compile on blocking threads, at most two at once per node; others wait their turn. A request waits at most 60 s for its compile, its turn included, else `503 overloaded` (`reason: "compile_timeout"`); a compile still running keeps its turn until it ends, so compiles never take more than two threads. The work itself is bounded by the draft size limit and by the compiler's limits, checked before expansion allocates (02 §14). Measured locally (AMD Ryzen 9 7900X, release build): compiling 100,000 seats takes 54 ms and brotli 0.5 s; 1,000,000 seats 0.5 s and 4.6 s.
- **Archive:** sets `archived_at`; archiving again returns the chart unchanged. The draft can no longer be saved or published (`400`, `reason: "archived"`); it can still be read and validated. The key stays taken. Versions are untouched and their public files are still served: events pinned to them keep working, and browsers and CDNs cache those files for a year regardless.
- **Who:** `draft_updated_by` and `chart_versions.created_by` are left empty until console users exist (09 §2): an API key is not a user.

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
| `GET /v1/keys` | List the workspace's API keys, revoked ones included; never the key or its hash |
| `POST /v1/keys` | Create (`kind`: `secret` or `publishable`, `name`, optional `allowed_origins` for publishable keys); `201` with the key, shown only here |
| `DELETE /v1/keys/{id}` | Revoke; returns the key. Idempotent. A key may not revoke itself |
| `GET/POST/PATCH/DELETE /v1/webhooks` | Endpoints |
| `GET /v1/webhooks/{id}/deliveries`, `POST …/deliveries/{id}/replay` | Delivery log |
| `GET /v1/workspaces` | List workspaces. Admin key |
| `POST /v1/workspaces` | Create (`key`, `name`); `201` with the workspace and its first secret key, shown only here. Admin key |
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
| `GET /v1/public/charts/{hash}.ccf` | Compiled chart (no credential; 03 §4.6) |
| `GET /v1/public/assets/{hash}` | Background images |

The compiled chart route takes no credential and ignores any sent. A chart file is named by its content hash alone: the URL must be the same for every buyer so that browsers and CDNs can cache it, a credential in a header would keep CDNs from caching it and force a CORS preflight, and a publishable key in the URL would change it on every key rotation. The hash identifies the bytes, so the route is unambiguous across workspaces (two workspaces that publish identical charts serve one identical file), and it exposes nothing a buyer of the event does not already download: 128 bits cannot be guessed, and the chart's key and workspace are not in the URL. It serves only public artefacts of published versions; the full artefact is never served. Private charts are P2 (03 §4.6).

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

Keys and workspaces (the full key below is a fake):

```http
POST /v1/workspaces
Authorization: Bearer <CAVEA_ADMIN_KEY>

{ "key": "acme", "name": "Acme Tickets" }
```

```json
{ "id": "ws_4f0c1d9e8b7a4e2f9c3d5a6b7c8d9e0f", "key": "acme", "name": "Acme Tickets",
  "created_at": "2027-01-10T09:00:00Z",
  "secret_key": { "id": "key_0a1b2c3d4e5f40718293a4b5c6d7e8f9", "kind": "secret",
    "name": "Initial secret key", "prefix": "sk_00000000", "allowed_origins": null,
    "created_at": "2027-01-10T09:00:00Z", "last_used_at": null, "revoked_at": null,
    "key": "sk_0000000000000000000000000000000000000000000000000000000000000000" } }
```

```http
POST /v1/keys
Authorization: Bearer sk_…

{ "kind": "publishable", "name": "Box office web", "allowed_origins": ["https://tickets.acme.example"] }
```

`201` with a key object as in `secret_key` above. `GET /v1/keys` returns key objects without `key`; `DELETE /v1/keys/{id}` returns the key object with `revoked_at` set.

Charts:

```http
POST /v1/charts
Authorization: Bearer sk_…

{ "key": "royal", "name": "Royal Theatre" }
```

```json
{ "key": "royal", "name": "Royal Theatre", "draft_etag": "\"1\"",
  "draft_updated_at": "2027-01-10T09:00:00Z", "published_version": null,
  "archived_at": null, "created_at": "2027-01-10T09:00:00Z" }
```

```http
PUT /v1/charts/royal/draft
Authorization: Bearer sk_…
If-Match: "1"
Content-Type: application/json

{ "schema": 1, "name": "Royal Theatre", "nextOid": 20431, "categories": [ … ], "nodes": [ … ] }
```

`200` with the chart object, `draft_etag` now `"2"`, also in `ETag`.

```http
POST /v1/charts/royal/publish
Authorization: Bearer sk_…
If-Match: "2"
```

```json
{ "id": "cv_7d1e0c4b9a8f4e3d2c1b0a9f8e7d6c5b", "number": 1,
  "hash": "9f2c4e1a7b3d5f6e8a0c2b4d6f8e0a1c",
  "url": "/v1/public/charts/9f2c4e1a7b3d5f6e8a0c2b4d6f8e0a1c.ccf",
  "stats": { "seats": 2000, "rows": 144, "segments": 144, "sections": 24, "floors": 1,
             "public_bytes": 28209, "public_br_bytes": 9752, "full_bytes": 32653 },
  "created_at": "2027-01-10T09:05:00Z" }
```

Refused (nothing was published):

```json
{ "code": "chart_invalid", "status": 422, "message": "The chart has validation errors.",
  "details": { "findings": [ { "code": "E007", "severity": "error", "node": 12,
                               "message": "E007 node 12: unknown category \"vip\"" } ] },
  "request_id": "req_6ad9b530f0e172e7214232f15266b459" }
```

### 6.4 Error codes

| HTTP | `code` | Meaning |
|---|---|---|
| 400 | `invalid_request` | Malformed body or parameter |
| 401 | `unauthorized` | Missing or invalid key |
| 403 | `forbidden` | Key lacks permission, or origin not allowed |
| 404 | `not_found` | Unknown chart, event, object, token, key or workspace |
| 409 | `already_exists` | A workspace or chart (later: event) with this key already exists |
| 409 | `object_not_free` | At least one object is not free |
| 409 | `object_not_held_by_token` | Book or release named an object the token does not hold |
| 409 | `object_status_conflict` | An object's status does not allow the change: release without a token of an object that is not booked, or change-status to free of a held or booked object |
| 409 | `version_conflict` | Draft etag mismatch; `details.draft_etag` is the current one |
| 409 | `upgrade_conflict` | Non-free objects missing from the new chart version |
| 409 | `idempotency_mismatch` | Key reused with a different body |
| 410 | `hold_token_expired` | Not returned by the actor, which deletes an expired token before processing any command at or after its expiry (§7.3); the token is then `not_found` |
| 422 | `hold_limit_exceeded` | Over the token's object limit |
| 422 | `channel_not_allowed` | |
| 422 | `orphan_rule_violated` | |
| 422 | `not_enough_objects` | Best-available could not satisfy the request |
| 422 | `chart_invalid` | Publish blocked by validation errors; `details.findings` lists them (§6.2) |
| 429 | `rate_limited` | |
| 500 | `internal_error` | Unexpected fault; the cause is logged under the request id, never returned |
| 503 | `overloaded` | Actor mailbox full, or a chart compile did not finish in time (`reason: "compile_timeout"`); retry after the given delay |
| 503 | `unavailable` | Storage unreachable |

Every error is `application/problem+json` with exactly these members:

```json
{
  "code": "object_not_free",
  "status": 409,
  "message": "At least one object is not free.",
  "details": { "objects": [{ "idx": 3, "reason": "not_free" }] },
  "request_id": "req_6ad9b530f0e172e7214232f15266b459"
}
```

- `code` and `status` always pair as in the table. `message` is for humans and may change; match on `code`.
- `details` is always an object, empty when there is nothing to add. Engine refusals fill it: `invalid_request` has `reason`; `not_found` has `missing` (`hold_token` or `objects`, with `objects` listing the unresolved references as `{"label"}` or `{"idx"}`); the three object conflicts list `{"idx", "reason"}` per object (`not_free`, `not_held_by_token`, `not_booked`, `not_blocked`); `forbidden` from the engine means the key's kind may not issue the command (`02-domain-model.md` §4.3); `hold_limit_exceeded` has `limit` and `requested`.
- `request_id` equals the `Cavea-Request-Id` header.
- An unknown path, and a known path with the wrong method, are `404 not_found`. A body that is not JSON, lacks `Content-Type: application/json`, does not match the expected shape, is over the size limit or nests too deeply is `400 invalid_request` with `details.reason` one of `json_syntax`, `content_type`, `json_data`, `body_too_large`, `json_depth` (with `limit`), `body_unreadable`. A malformed path parameter is `reason: "path"`, a malformed query parameter or cursor `reason: "query"`, an invalid body field `reason: "invalid_field"` with `field`, and too many objects `reason: "too_many_objects"`. For charts: a missing or malformed `If-Match` is `reason: "if_match"`, a draft that is not a storable chart document `reason: "document"`, and a change to an archived chart `reason: "archived"`. An identifier in the path that cannot exist, or belongs to another workspace, is `404`.
- `401 unauthorized` has `details.reason`: `missing` (no `Authorization`), `malformed` (not `Bearer <token>`), `invalid` (unknown or revoked key, or not the admin key). `403 forbidden` from authentication has `reason`: `key_kind` (the wrong kind of credential for the route), `origin` (§11), or `self_revocation`.
- `503 overloaded` carries `Retry-After: 1`.

## 7. Algorithms

### 7.1 Hold

1. Find the hold by token hash. Unknown → `not_found`. An expired token has already been deleted (§7.3), so it is unknown too.
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

- **With a token:** all named objects (or all the token's objects if none are named) must be held by it. They become `booked` with the order reference. If the token then holds nothing, it is deleted. A token holding nothing books nothing: `invalid_request` (`no_objects`).
- **Direct:** all named objects must be free and channel-permitted (unless `ignore_channels`).
- Ticket types and `extra` come as per-object details; a detail naming an object not being booked, or one object twice, is `invalid_request` (`detail_mismatch`). The engine bounds `extra` to 2 KB and stores it as given; the handler checks it is a JSON object.
- **Release without a token** (secret key): every named object must be booked (`object_status_conflict`, `not_booked`); it becomes free and its booking data is dropped.
- **Change status:** to blocked or a custom code, every object must be free or already have that status (`object_not_free`); to free, every object must be blocked, custom or already free (`object_status_conflict`, `not_blocked`). Held and booked are not settable (`invalid_request`, `bad_status`).
- Areas take `{"label", "quantity"}`.

### 7.3 Expiry

- When the heap's earliest expiry passes, the actor issues an internal `Expire` for that token: all its objects return to free, the hold is deleted, a `hold.expired` outbox row is written. `Expire` takes a `seq` and is logged with actor `system`, even for a hold with no objects. Holds due at the same instant expire in hold id order.
- Extending a hold pushes a new heap entry; the old one, and the entry of a hold deleted early, are skipped when they surface, so they never fire.
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

Binary frames, encoded and decoded by `cavea_format::wire`, which the server and every client share. The protocol version is 1.

Field types used below:

| Notation | Bytes |
|---|---|
| `u8` | One byte |
| `v64` | Unsigned LEB128, 1 to 10 bytes, least significant group first. The tenth byte, if reached, is 0 or 1. Redundant `0x80` continuation bytes are accepted |
| `v32` | A `v64` whose value is at most 2³² − 1 |
| `hash` | 16 bytes |
| `bytes(n)` | A `v32` length of at most `n`, then that many bytes |
| `opt bytes(n)` | A presence byte: 0 (absent; nothing follows) or 1 (followed by `bytes(n)`) |
| `list(F)` | A `v32` count, then per item: its `idx` as a `v32` difference from the previous item's `idx` (the first item's from 0, so its own `idx`), followed by `F` |

A frame is a type byte followed by its fields in the order listed, with nothing between them. The rules every decoder applies, and version negotiation, are in §8.6.

### 8.1 Server to client

| Type | Name | Layout |
|---|---|---|
| `0x01` | HELLO | `u8` version · `v64` server time (Unix ms) · `hash` chart hash · `v32` object count |
| `0x02` | SNAPSHOT | `v64` `seq` · `v32` object count · `u8` encoding · `v32` state length · state (that many bytes) · `list(v32 value)` areas |
| `0x03` | DELTA | `v64` `from_seq` · `v64` `to_seq` · `list(u8 code)` changes · `list(v32 value)` area changes |
| `0x04` | PRIVATE | `v64` hold expiry (Unix ms) · `list()` held objects · `list(v32 quantity)` held area quantities |
| `0x05` | RESYNC | Nothing. The client fell behind; fetch a new snapshot |
| `0x06` | PING | `v64` nonce |
| `0x07` | CLOSE | `u8` reason |

**Snapshot encodings.** The state covers exactly the object count, in `idx` order:

| Encoding | State |
|---|---|
| 0 | One code byte per object. Length = object count |
| 1 | (`u8` code, `v32` run) pairs. Every run is at least 1; the runs add up to exactly the object count; the state ends with the last pair |
| 2 | One bit per object, bit `i % 8` of byte `i / 8` (least significant first). Length = ⌈object count / 8⌉; the padding bits of the last byte are 0. Public streams only |

The server picks the encoding with the smallest state, ties going to the lower number, and never picks 2 on a manager stream or for a code above 1. A manager-stream decoder refuses encoding 2. An empty event (object count 0) has an empty state in encoding 0.

**Codes.** On a public stream every code is 0 (available) or 1 (unavailable) (`02-domain-model.md` §4.2); on a manager stream it is a domain status, 0 to 15 (§4.1). A code outside the stream's range is an error. The only way the server builds a public frame is by reducing domain statuses to public codes inside the encoder (any status other than 0 becomes 1), so held and booked cannot be told apart (INV-9). Channel streams (§8.3) will reduce against the viewer's channel set the same way.

**Areas.** Area fields belong to a later milestone (`02-domain-model.md` §8). Servers send area counts of 0 today. Their layout is fixed now, so that adding them needs no new version: a version-1 receiver parses and checks area entries like any other list, and may ignore them.

### 8.2 Client to server

| Type | Name | Layout |
|---|---|---|
| `0x10` | SUBSCRIBE | `u8` version · `bytes(512)` event key (UTF-8, the 128-character key of `02-domain-model.md` §3) · `v64` last `seq` seen (0 if none) · `opt bytes(256)` hold token · `opt bytes(256)` session token |
| `0x11` | PONG | `v64` nonce, echoing the PING's |

Tokens are opaque bytes; a present token may be empty.

### 8.3 Behaviour

- On SUBSCRIBE the server replies HELLO, then either the DELTAs since `last_seq` (if still in the 60-second ring buffer) or a SNAPSHOT, then PRIVATE if a hold token was given.
- Deltas are coalesced every 100 ms. With more than 20,000 subscribers on an event the interval stretches, up to 500 ms.
- If an object changes twice within an interval, only the final state is sent (`wire::coalesce`).
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

SSE and polling have no client-to-server frames: the SUBSCRIBE fields travel as request parameters (RT-007, RT-008).

### 8.5 Caching the snapshot

- `GET /v1/public/events/{event}/state` returns a SNAPSHOT frame with `ETag: "<seq>"` and `Cache-Control: public, max-age=1, stale-while-revalidate=5` (configurable).
- With a CDN in front, the opening rush of an on-sale hits the CDN, not the server. Clients then catch up through deltas.
- Channel-specific snapshots are `private` and not cached by intermediaries.

### 8.6 Framing, validation and versioning

- **One frame per message.** A WebSocket binary message, an SSE `data:` line once base64-decoded, and an HTTP response body (the snapshot endpoint, polling) each carry exactly one frame.
- **Unknown frame types** are ignored: the receiver skips the message and carries on. One frame per message is what makes skipping possible on every transport.
- **Trailing bytes** after a frame's last field are an error, except in a SUBSCRIBE whose version is above the server's (versioning, below).
- **Lists.** Each `idx` is strictly above the previous one: a difference of 0 after the first item is an error. Every `idx` is below 1,000,000 (`02-domain-model.md` §14) when decoded, and below the receiver's object count before anything is applied; a frame naming an `idx` at or past the receiver's count is refused whole. A SNAPSHOT's areas are checked against its own object count when decoded.
- **Counts and lengths.** An object or list count above 1,000,000 is an error; so is a list count the remaining bytes could not hold (two bytes per item with a code or value, one without), so a hostile count never drives a large allocation. A length past the end of the frame is an error.
- **Object count.** A SNAPSHOT's object count must equal the receiver's (from its chart); otherwise it is refused. A client that receives the snapshot before its chart keeps the bytes and checks them when the chart is decoded (`04-renderer.md` §4).
- **Sequence numbers.** A DELTA has `to_seq` > `from_seq`; it moves the receiver from `from_seq` to `to_seq` and is applied only by a receiver whose `seq` is `from_seq`. Anything else (a gap, a repeat) is the adapter's cue to resync; the decoder never guesses. A DELTA may carry no changes: commands that change nothing a public stream shows (held to booked) still advance `seq`.
- **PRIVATE** carries the full set of the subscriber's own holds, replacing the previous one; an empty list means none.
- **Errors.** A frame that fails any rule is not applied. A client then resyncs (reconnects with its last `seq`); a server sends CLOSE 6 and closes.

**Versioning.** HELLO and SUBSCRIBE carry the version at offset 1, and CLOSE's layout never changes. SUBSCRIBE carries the highest version the client speaks (0 is invalid). The server answers with HELLO in the lower of that and its own highest, and every later frame on the stream uses that version; if the client's version is below the oldest the server supports, the server sends CLOSE 4 and closes. A later version may only append fields to SUBSCRIBE, so a server reading a SUBSCRIBE from a newer client reads the fields it knows and ignores the rest. A client refuses a HELLO whose version it does not speak. Frames served over plain HTTP (the snapshot endpoint, polling) are version 1; a later version will be requested explicitly.

**CLOSE reasons.** A client treats an unknown reason like 3.

| Code | Reason | Client |
|---|---|---|
| 1 | Event archived | Stop |
| 2 | Chart upgraded | Reload the chart, then resubscribe |
| 3 | Server shutting down | Reconnect with backoff |
| 4 | Unsupported version | Stop and report |
| 5 | No such event for this credential | Stop and report |
| 6 | The server could not decode the client's frame | Reconnect with backoff |

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
- Every query is scoped by workspace id taken from the credential, never from the request. In the server the workspace id is obtainable only from an authenticated key (`http::Auth`). The one exception is the compiled chart route, which takes no credential and finds a public file by its content hash alone (§6.2).

API keys:

- A key is `sk_` or `pk_` and 64 lowercase hex digits: 256 bits from the operating system's secure random source. It is returned once, at creation. Only its SHA-256 (`api_keys.hash`) and its first 11 characters (`prefix`, for display) are stored. A key's id (`key_…`) is not the key.
- A presented key is found by its SHA-256, so no stored secret is compared byte by byte (12 T16). Unknown and revoked keys are both `401`; a valid key of the wrong kind for the route is `403`. Secret keys are refused on public endpoints and publishable keys everywhere else.
- `last_used_at` is updated by the lookup itself when it is unset or more than an hour old: accurate to an hour, and at most one write per key per hour.
- Rotation: create a second key, move the backend to it, revoke the first with it. Both are valid in between.
- A key may not revoke itself (`403`, `reason: "self_revocation"`). Every revocation is made by another unrevoked secret key, serialised per workspace, so a workspace always keeps at least one secret key.
- Origin allow-list (publishable keys, 12 T3): up to 50 origins such as `https://tickets.example.com`, compared without regard to case. A request whose `Origin` is not on the list, `null` included, is `403` (`reason: "origin"`). A request without `Origin` is accepted: native apps send none, and a non-browser client can send any `Origin` it likes, so refusing it would stop no attacker. A key without a list accepts every origin.

Admin key and workspaces:

- `CAVEA_ADMIN_KEY` has no default. Set, it must be 32 to 512 printable ASCII characters or the server will not start; unset, the server starts with a warning and the admin endpoints refuse every request (`401`, or `403` for a valid API key, as on any admin route) (`DECISIONS.md` D-24). The presented token is hashed and the digests are compared in constant time. The admin key is checked before API keys; it is never logged.
- `POST /v1/workspaces` creates the workspace and its first secret key in one statement and returns the key once. From there the workspace manages its own keys.

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
| `CAVEA_LOG_FORMAT` | `json` | `json` (one object per line, for production) or `text` (readable, for development) |
| `CAVEA_PUBLIC_URL` | — | External base URL, used in chart URLs and webhooks |
| `CAVEA_LICENCE` | — | Licence file path or contents |
| `CAVEA_ADMIN_KEY` | none | The operator's key for `/v1/workspaces`: 32 to 512 printable ASCII characters. Unset, those endpoints refuse every request (§11) |
| `CAVEA_DB_POOL` | 16 | Connection pool size |
| `CAVEA_HOLD_TTL` | 900 | Default hold lifetime in seconds |
| `CAVEA_DELTA_INTERVAL_MS` | 100 | |
| `CAVEA_SNAPSHOT_MAX_AGE` | 1 | Seconds, for the cache header |
| `CAVEA_TRUSTED_PROXIES` | none | CIDR list |
| `CAVEA_CORS_ORIGINS` | `*` | For public endpoints |
| `CAVEA_LOG_RETENTION_DAYS` | 400 | `event_log` partitions older than this are dropped |
| `CAVEA_OBJECT_STORE_URL` | none | Optional S3-compatible storage for assets |
| `CAVEA_OTLP_ENDPOINT` | none | Trace export |
| `CAVEA_NODE_ID` | hostname | This node's name in `events.owner_node`; nodes on one database must differ |
| `CAVEA_LEASE_SECS` | 15 | How long ownership of an event lasts unless renewed |
| `CAVEA_HEARTBEAT_SECS` | 5 | How long an idle actor waits before renewing its lease; less than the lease |

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

```
request {request_id, method, route, status, duration_ms, workspace, event, event_id, commit_seq}
├─ mailbox_wait {op}     queued in the actor's mailbox
├─ apply {op}            on the actor's task, parented to the request explicitly
└─ db {statement}        a database call made by the handler
commit {event_id, seq, commands}   one per batch; follows_from each apply span in it
actor {event_id}                   one per loaded event: its load, actor and lease release
└─ heartbeat                       an idle lease renewal
```

- `route` is the matched template (`/v1/events/{event_key}`), or `<unmatched>`; the raw path is never logged because it carries customer keys. Query strings, headers and bodies are never logged.
- Each request logs one `request completed` record when it finishes: `info`, or `debug` for a successful `/healthz`.
- A batch serves many requests, so `commit` is not a child of any one of them. Its `follows_from` links lead to each `apply` span and through it to the request id, for trace export. In logs, the engine records the batch's `event_id` and `seq` (as `commit_seq`) on the span that submitted each command (the request span, when the handler submits from it), so a record inside `commit {event_id: E, seq: 42}` and the `request completed` records with `event_id: E, commit_seq: 42` belong together. `seq` alone is per event and does not identify a commit. `event_id` is the event's UUID as 32 hex digits; `event` is the customer's event key.
- A JSON record lists every enclosing span with its fields under `spans`, outermost first, so a record inside `apply` carries the request id.

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
