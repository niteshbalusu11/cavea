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
    published: Arc<Feed>,               // §4.5: snapshot, last 60 s of deltas, holds
    tx: broadcast::Sender<Publication>, // pre-encoded delta frames, shared
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
-- by change-status, a hold's expiry and object limit (absent: unlimited), a
-- booking's per-object ticket type and extra. Replaying it rebuilds
-- `object_state` and `holds` except the token hash, which is never logged.
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

The log is sufficient on its own: replaying it from an empty state rebuilds `object_state`, `holds` (but for the token hash) and `events.seq` (INV-6), which the reconciler checks (13 §3.3, `cavea-server verify`). A hold's token hash is not logged (D-37): it is the SHA-256 of a bearer secret, kept in `holds` only while the hold lives, whereas the log is kept for `CAVEA_LOG_RETENTION_DAYS` and exported by reports.

### 4.5 Reads

- **Committed state only (INV-8).** When a batch is sealed, memory is exactly its end state, so the actor records then what it changes: each changed object's final status and the hold of every token it touched. That record is folded into the actor's own copy of the committed statuses only once the batch commits, and discarded if it fails. Everything published is built from that copy, never from memory, which may already hold the next, uncommitted batch. Nothing of a failed batch, or of a batch whose commit result was lost, is ever published by the actor that applied it; after the reload it is part of the loaded state, which is published as the first snapshot.
- **Publication.** At most once per interval (§8.3) the actor publishes everything committed since its last publication: one public DELTA from the last published `seq` to the committed `seq`, the public SNAPSHOT at that same `seq`, and the committed holds of each token touched since. The first commit after a quiet interval is published at once; later ones when the interval ends, so the last commit is never left unpublished. Every snapshot's `seq` is therefore a delta boundary, and a client given any snapshot, including one cached by a CDN, resumes from it through the ring buffer (D-27). A freshly loaded actor publishes its first snapshot before it can be subscribed to, and an actor stopping without failure publishes what it committed before it ends.
- **Readers** share the event's `Feed`: the current snapshot, the ring buffer and the committed holds sit behind one `std::sync::RwLock` that the actor takes for writing only to swap in a publication (an `Arc` swap, a push onto the ring buffer, and one map update per touched token), and readers take only to clone `Arc`s; neither holds it across an await, and no bytes are copied per reader. HTTP handlers read the snapshot without sending a message to the actor. Encoding happens before the lock is taken.
- **Deltas:** encoded once per publication and broadcast as shared bytes on a `tokio::sync::broadcast` channel of 64 publications. No per-subscriber encoding. The actor hands each publication to a relay task of its event on the realtime runtime (§8.3), which broadcasts them in order: waking tens of thousands of subscriber tasks costs that runtime, not the actor (a broadcast from the actor to 25,000 subscribers on another runtime held it for 13 ms, measured locally; from the relay, the actor's share is a queue push).
- **Ring buffer:** the published deltas of the last 60 seconds, at most 4 MiB of frames per event; older frames, then the oldest beyond 4 MiB, are dropped when a delta is published or, while none is, at the actor's idle check. A reload starts with an empty buffer.
- **Object details and reports:** the objects query (§6.2) reads committed rows, as the event's counts do; `Inspect` (§4.2) is not built. Reports are SQL.

### 4.6 Failure

| Failure | Response |
|---|---|
| Commit fails (database down, timeout) | Fail every command in that batch and in later uncommitted batches with `503 unavailable`. Drop the actor. The next request reloads it from PostgreSQL |
| Fencing check updates zero rows, or a heartbeat fails | Same, and for fencing log loudly: another node owns the event |
| Actor panics | The registry catches it, fails pending callers, drops the actor; the next request reloads the event |
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

Eviction: an actor with no subscribers, no outstanding holds and no command for 10 minutes stops: it takes no more commands, commits what it accepted and releases its lease. It is checked once per 10 minutes on the engine clock, so it stops between 10 and 20 minutes after the last command, hold expiry or departing subscriber. A subscriber keeps the actor with a keep-alive guard from the registry. A command after eviction loads the event again.

At startup the server loads every event that has rows in `holds`, archived events included, earliest expiry first and one at a time, in the background once it is listening: readiness is not delayed, and a request for an event not loaded yet loads it itself (sharing any load in progress). A load costs one acquire, the chart (once per version, cached) and one snapshot read, a few milliseconds, so thousands of events take seconds; a hold that falls due meanwhile expires as soon as its event loads, late by at most that time. A load that fails is logged and left to the event's next request.

The chart provider reads a version's `full_ccf`, decodes it on a blocking thread and composes every object's full label as the compiler did (03 §3.1 step 6). Views are cached per version and shared by every event on it; the cache keeps the most recently used versions up to 4,000,000 objects in all (about 400 MB). A stored artefact that does not decode, or whose labels or `oid`s are missing or repeated, fails the load (`503 unavailable` to the caller) and is logged as an error.

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

`event_log` is what the reconciler replays (13 §3.3); its `detail` is described with statement 6 of §4.4.

Migration 4 makes `assets`' key `(workspace_id, hash)`, so that two workspaces may hold the same bytes, indexes `hash`, by which the public asset route finds a file (§6.2), and stores `bytes` out of line without compression (images are compressed already). `size` is in bytes; `content_type` is the type the bytes were sniffed as.

Migration 5 is the console's people (09 §2): `users.email` is stored trimmed and lowercased (a `CHECK` holds it to `lower(email)`), so its `UNIQUE` is case-insensitive; `password_hash` is argon2id in the PHC string form (12 §3 T9); `totp_secret` stays empty until TOTP (CON-003). `memberships` gives a user one of the five roles in a workspace. `console_sessions` has one row per sign-in: `id_hash` is the SHA-256 of the identifier the cookie carries (`cs_` and 64 hex digits, 256 random bits), never the identifier; `expires_at` is the idle end (last use plus 12 hours, written at most every five minutes), `created_at` gives the absolute end (plus 7 days); the CSRF token is derived from the identifier and stored nowhere (§11); an index on `user_id` serves revoking a user's sessions. A new table holds the first-run setup token:

```sql
CREATE TABLE console_setup (
  id         smallint PRIMARY KEY CHECK (id = 1),
  token_hash bytea NOT NULL,                     -- SHA-256 of the token in the logged link
  expires_at timestamptz NOT NULL,               -- one hour after the start that made it
  used_at    timestamptz                         -- set by the setup that used it
);
```

`charts.draft_updated_by` and `chart_versions.created_by` hold the console user who saved or published, and stay empty for an API key.

Migration 6 is invitations and the audit log (09 §7, D-55). `invitations (id, workspace_id, email, role, token_hash UNIQUE, created_by, created_at, expires_at, UNIQUE (workspace_id, email))` holds the SHA-256 of each pending invitation's token, never the token; accepting or revoking deletes the row, and creating one deletes the workspace's expired ones. `audit_log` gains `actor text NOT NULL` (`usr_…`, `key_…`, `admin`, `anonymous`) and `request_id`, and is written from this migration on: one row per action in the action's own transaction (the actions are listed in 09 §7). Two indexes serve the read route, `(workspace_id, id)` and `(user_id, id) WHERE workspace_id IS NULL`; a page is a backward scan of each, a page of the workspace's rows and a page per member of the instance rows, so its cost does not grow with the table (13 §7.4). Rows are kept: there is no retention job.

Migration 3 indexes `holds (event_id, session_sub) WHERE session_sub IS NOT NULL`, for the per-`sub` token cap (§11). `holds.session_sub` holds the SHA-256 of a session token's `sub` as 64 hex digits, never the `sub` itself; the server writes it, the engine never reads it.

The `SeatStore` trait hides all of this from the engine: `acquire`, `load_event`, `commit_batch`, `heartbeat`, `release`. A store speaks for one node. Tests use an in-memory implementation with fault injection, whose leases follow the injected clock.

## 6. REST API

### 6.1 Conventions

- Base path `/v1`. JSON bodies, `snake_case` fields. Times are RFC 3339 UTC.
- Authentication: `Authorization: Bearer sk_…` or `Bearer pk_…` (§11). The workspace is implied by the key. Every route that takes a secret key also takes a console session (the cookie, §11), which names its workspace in `Cavea-Workspace` and needs the role's permission the route declares.
- Mutating secret-key seat requests accept `Idempotency-Key` (1 to 255 printable ASCII characters (U+0020 to U+007E), which is all an HTTP header carries portably; anything else is `400`, `reason: "idempotency_key"`; kept 24 hours). Replaying a key with a different request returns `409 idempotency_mismatch`.
- For seat commands the engine does this atomically with the command. The handler computes a 32-byte fingerprint of the request: SHA-256 over the method and route template, the path parameter (event key or hold token) and the body as parsed and serialised again (object keys sorted, no whitespace), each of the three prefixed with its length. An `extra` object, stored as sent, goes in as sent without insignificant whitespace: nothing in it is decoded or rounded, so key order inside it matters. Two sends of one request that differ only in whitespace, or in key order outside `extra`, are the same request; the same key on another route, another event or token, or with another body is a mismatch. The handler looks the key up in `idempotency` first and, if found, replays or mismatches without reaching the actor; otherwise it submits the command with the key, the fingerprint and the time of that lookup. The actor keeps an in-memory window of its event's records: every keyed command it applies gets a record, refusals included (a refused request replayed later must not succeed), holding the engine's reply exactly as returned, `seq` and per-object failures included. A key in the window replays that reply or fails with `idempotency_mismatch`, changing nothing; a key not in it is applied. The window is the authority for records not yet visible to the handler's lookup (uncommitted, or committed after it) and the store for everything older. Records are written in the commit (§4.4 statement 8) and loaded with the event (§4.7 step 3), so a retry after a lost reply and a reload returns the original reply. A record matches for 24 hours from when its command was applied. If the clock jumps back across a record's 24-hour mark, a record memory has already dropped can match again once the event reloads from the store; nothing else depends on it, and the model-based tests (13 §3.1) do not jump back across that mark. The window holds at most 50,000 records. To make room it drops the oldest only if they were already a minute old at the command's store lookup, never by the time the command is processed: a record the lookup could not see was committed after it (commits time out well within a minute), so it is still in the window however long the command waited in the mailbox. When no record qualifies, the new key is refused `503 overloaded` and the caller looks it up again. `overloaded`, `unavailable`, an invalid key and a mismatch write no record. A key is per workspace, but the engine checks it per event: two events given the same new key at the same moment both apply, and the store keeps the first record. A stored record is replayed with its original status and body (the event it names may since be archived, the token gone). Refusals made before the command reaches the engine (a malformed body, an unknown event or token, an archived event) record nothing. Routes that take a key: `hold`, `release`, `book`, `change-status`, and extending and releasing a hold token. Every other route ignores one: event routes (creating is unique by key, changing and archiving are idempotent), hold-token creation (the token is never stored, so a replay could not return it; a retry costs only an empty token that expires) and every public route (a key is per workspace, and buyers who share a publishable key could otherwise take, or collide with, a key the customer's backend means to use; a buyer's hold and release are safe to repeat with the same token anyway).
- Objects are addressed by label. A request may instead send `{"version": "<hash>", "idx": [..]}`.
- Lists use cursor pagination: `?limit=100&after=<cursor>` (`limit` 1 to 100, default 100). A list response is `{"data": [..], "next": "<cursor>"}`, oldest first; `next` is the id of the last item when another page may follow, else `null`, and is passed back as `after`. Server-generated ids are `ws_` or `key_` and 32 hex digits.
- Request limits (`12-security-licensing.md` T19): a body is at most 256 KiB unless its route sets a larger limit; JSON arrays and objects nest at most 32 deep, checked by a scan of the raw bytes before parsing; a command names at most 1,000 objects (`02-domain-model.md` §14). Name and customer key fields are 1 to 128 characters without U+0000.
- Every response, errors and 404s included, carries `Cavea-Request-Id`. The server generates `req_` and 32 lowercase hex digits. A caller may send its own `Cavea-Request-Id` to correlate with its logs: the server adopts it only if it is 1 to 64 characters from `A-Z a-z 0-9 . _ -`, and otherwise ignores it and generates one. An adopted id is not guaranteed unique.
- Every path that is no route is the router's fallback: a console file or `index.html` (09 §9) for a `GET` outside `/v1`, `/console/api`, `/healthz`, `/readyz` and `/metrics`, else problem+json `404`. The console's files are not part of the API and not in its document.
- The OpenAPI 3.1 document is served at `GET /v1/openapi.json`, without a credential (it describes the API, nothing of any workspace), with `Cache-Control: public, max-age=300` and a strong `ETag` (`304` on `If-None-Match`). It is generated, never written by hand (D-46): every route is registered through the route table (`http::openapi::Routes`), which gives the router its method and path and the document its operation, so a route cannot exist without its entry or the reverse; the component schemas (`http/openapi/schemas.rs`) are held to the code by the router tests, which validate every response they receive and every JSON body the server accepted against them. The document is also committed as `crates/cavea-server/openapi.json` (`cavea-server openapi` prints it; a test fails while the file is stale), so a pull request shows API changes in its diff and clients are generated without running the server. Each operation has an `operationId`, a tag (its resource), its security scheme (`adminKey`, `secretKey`, `publishableKey`, `publishableKeyQuery`, `sessionToken`, `holdToken`), its parameters (`Idempotency-Key`, `If-Match`, the page's `limit` and `after`), its request and response schemas by name, the problem+json responses of each status it can return with their codes, and `Cavea-Request-Id` (and `Retry-After` on `429` and `503`) on every response. `GET /v1/public/rt` is described as an upgrade to the WebSocket protocol of §8, which OpenAPI cannot describe.

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
- **Validate:** `200` with `{draft_etag, valid, findings, stats}`. Each finding is the compiler's documented form (03 §3.2), `{code, severity, node, related, message}`: `code` is an E or W code of 03 §3.2, or `unsupported` for a node the compiler cannot compile yet; `severity` is `error` or `warning`; `node` is the node id, or `null` for findings about settings, categories and floors; `related` is the other node involved, or `null`; `message` is a short English sentence. `valid` is true when no finding is an error. `stats` is `null` when the draft did not compile.
- **Publish:** compiles the draft as it is at the request. With `If-Match`, a draft whose etag differs is `409 version_conflict` before anything is compiled. Findings of error severity refuse it: `422 chart_invalid` with `details.findings` as above. Otherwise, in one transaction with the chart row locked, the server stores a new immutable version (02 INV-12): the next number, the public and full content hashes, the draft as `source`, the public artefact, its brotli form, the full artefact and `stats`; and points the chart's published version at it. A draft saved while the compile ran makes the publish `409 version_conflict`, and an archive while it ran `400` (`reason: "archived"`), creating nothing: a version is always the draft that was current when it was stored. Publishes racing on one chart are serialised by the row lock.
- **Unchanged publish:** versions are identified by the full artefact's hash, which covers every compiled field. A draft that compiles to a version the chart already has publishes that version again: `200` with it, no new row, and the chart's published version points at it (so republishing an older draft goes back to its version). Two versions may share a public hash when they differ only in full-only data (`oid`s, ranks, section priorities, 03 §4.2); they then share one public file.
- **Compile bounds:** validate and publish compile on blocking threads, at most two at once per node; others wait their turn. A request waits at most 60 s for its compile, its turn included, else `503 overloaded` (`reason: "compile_timeout"`); a compile still running keeps its turn until it ends, so compiles never take more than two threads. The work itself is bounded by the draft size limit and by the compiler's limits, checked before expansion allocates (02 §14). Measured locally (AMD Ryzen 9 7900X, release build): compiling 100,000 seats takes 54 ms and brotli 0.5 s; 1,000,000 seats 0.5 s and 4.6 s.
- **Archive:** sets `archived_at`; archiving again returns the chart unchanged. The draft can no longer be saved or published (`400`, `reason: "archived"`); it can still be read and validated. The key stays taken. Versions are untouched and their public files are still served: events pinned to them keep working, and browsers and CDNs cache those files for a year regardless.
- **Who:** `draft_updated_by` and `chart_versions.created_by` are the console user who saved or published (§11), and stay empty for an API key, which is not a user.

`{event}` is the customer's event key. Event rules:

- **Event object:** `{key, name, chart, version, settings, archived_at, created_at}`, plus `counts` from `GET /v1/events/{event}` only. `chart` is the chart's key; `version` is the pinned version object, whose `url` is the public chart file; `name` may be `null`.
- **Creating:** the event pins one immutable version (D-11): the chart's published version, or `version` (a version number of that chart). An unknown chart is `404` (`missing: "chart"`), an unknown version number `404` (`missing: "version"`), a chart with no published version and no `version` `400` (`reason: "unpublished"`), an archived chart `400` (`reason: "archived"`), whatever the version. A key already used in the workspace, by an archived event too, is `409 already_exists`.
- **Settings:** `hold_ttl_seconds` (60 to 7,200, default 900): the lifetime of a hold token created or extended without `ttl_seconds`; `max_objects` (1 to 100, default 10): the per-token object limit for publishable keys (02 §5); `require_session` (boolean, default false): the public hold-token, hold and release routes require a session token (§11); `max_tokens_per_sub` (1 to 10, default 2): the live public hold tokens one session `sub` may have on the event (§11). There is no server-wide default lifetime: an event's effective settings never change with the server's configuration. Unknown fields are refused. The stored settings are what was set; responses show every setting with its effective value. `PATCH` changes what it is given: `name` (`null` clears it) and the whole `settings` object (a setting left out goes back to its default); new settings apply to tokens created or extended afterwards.
- **Counts:** `{free, held, booked, blocked, custom: {"<code>": n}}`, from the committed rows of `object_state`; free is the version's object count less the rest. Committed state is exactly what was acknowledged, and reading it does not load the event's actor (there is no read command yet). A hold past its expiry counts as held until its expiry commits, at once for a loaded event. The list has no counts: they cost a scan per event.
- **Archive:** sets `archived_at`; archiving again returns the event unchanged. An archived event is read-only: seat operations, token creation, extension and release, and `PATCH` are `400` (`reason: "archived"`); reads still work. Outstanding holds are left to expire on schedule (they are at most 2 hours long), bookings stay, and the loaded actor evicts itself once it holds nothing (§4.7). A command checked before the archive committed may still apply. The key stays taken.
**Events**

| Method and path | Purpose |
|---|---|
| `GET /v1/events` | List event objects, archived ones included; the cursor is an event key |
| `POST /v1/events` | Create (`key`, `chart`, optional `version`, `name`, `settings`); `201` with the event object |
| `GET /v1/events/{event}` | The event object with `counts` |
| `PATCH /v1/events/{event}` | Change name or settings |
| `POST /v1/events/{event}/upgrade` | Move to a newer chart version |
| `DELETE /v1/events/{event}` | Archive |
| `GET /v1/events/{event}/rt` | Manager realtime stream (WebSocket) with domain statuses |
| `PUT /v1/events/{event}/channels` | Replace the channel list |
| `POST /v1/events/{event}/channels/assign` | Put objects in a channel, or clear |

**Seat operations**

| Method and path | Purpose |
|---|---|
| `POST /v1/hold-tokens` | Create a hold token (`event`, optional `ttl_seconds`, `max_objects`); `201` |
| `GET /v1/hold-tokens/{token}` | Expiry and held objects |
| `POST /v1/hold-tokens/{token}/extend` | New expiry |
| `DELETE /v1/hold-tokens/{token}` | Release everything it holds |
| `POST /v1/events/{event}/hold` | Hold objects under a token |
| `POST /v1/events/{event}/release` | Release held or booked objects |
| `POST /v1/events/{event}/book` | Book by hold token or directly |
| `POST /v1/events/{event}/change-status` | Block, unblock, set a custom status |
| `POST /v1/events/{event}/best-available` | Choose and hold |
| `GET /v1/events/{event}/objects` | Status of given labels, or a filtered page |

Seat rules (secret-key routes; the public ones follow the public table):

- **Hold tokens:** `ht_` and 64 lowercase hex digits from the operating system's secure random source, returned once by `POST /v1/hold-tokens` as `{hold_token, event, seq, expires_at, max_objects}`. Only the SHA-256 is stored and given to the engine. `ttl_seconds` defaults to the event's `hold_ttl_seconds`; `max_objects` defaults to none (secret-key tokens are unlimited). Creation is not idempotent (§6.1). `GET /v1/hold-tokens/{token}` returns `{event, expires_at, max_objects, objects}` as committed. `POST …/extend` takes `{ttl_seconds}` (optional, the event's default) and returns `{seq, expires_at}`. `DELETE` releases everything the token holds and returns `{seq, objects}`; the token stays valid until it expires.
- **Finding a token's event:** by the token's hash in `holds` (unique), joined to `events` and scoped to the key's workspace: one index probe. A token of another workspace, an expired one and one that never existed are all `404` (`missing: "hold_token"`); a path that is not a token is `404` without a query. The token in the path is never logged: request spans record the route template.
- **Objects:** an array of labels, or `{"version": "<hash>", "idx": [..]}` with the public hash of the event's pinned version; another hash is `409 version_conflict` with the pinned hash in `details.version`. At most 1,000.
- **Requests and replies:** `hold` takes `{hold_token, objects}` and returns `{seq, expires_at, objects}`; `release` takes `{hold_token, objects?}` (held objects, all of the token's when `objects` is left out; an empty list is `invalid_field`) or `{objects}` without a token (booked objects); `book` takes `{hold_token?, objects?, order_ref, ticket_types?, extra?}` (with a token, all of the token's objects when `objects` is left out; an empty list is `invalid_field`), where `ticket_types` and `extra` are maps keyed by label and each `extra` value must be a JSON object (stored as sent); `change-status` takes `{objects, status}`. Each returns `{seq, objects}` but `hold`. `objects` in a reply is `[{label, oid}]` in `idx` order; refusals list failing objects as `{label, oid, reason}`.
- **Status wire form:** `"free"`, `"blocked"`, or a custom status code as a number from 4 to 15 (custom status names per workspace come later). `"held"` and `"booked"` are refused by the engine (`bad_status`), anything else is `invalid_field`.
- **Objects query:** `GET /v1/events/{event}/objects` answers in one of two ways.
  - **By label:** `?label=A&label=B…` (repeated; percent-encoded; at most 1,000, else `too_many_objects`) returns `{data, unknown}`: `data` holds each named object once, in `idx` order; `unknown` lists, once each, the labels the event's chart version does not have. `label` cannot be combined with `status`, `limit` or `after`.
  - **A page:** `?status=&limit=&after=` returns `{data, next}` in `idx` order. `status` is `free`, `held`, `booked`, `blocked` or a custom code (4 to 15); left out, every object. `limit` is 1 to 100 (default 100). `next` is the label of the last object the page looked at while the chart goes on, else `null`; pass it back as `after` (an `after` that is not a label of the chart is `reason: "query"`). A page looks at no more than 10,000 objects after `after`, so a page of a rare status may hold fewer than `limit` objects, or none, with a `next` to continue from; following `next` until it is `null` visits every object of the status exactly once. Each page is one query for the committed rows of the objects it looks at: free objects have no row, so the chart gives the order and the labels.
  - **An object** is `{label, oid, status}`, `status` in the wire form above plus `"held"` and `"booked"`. An object that is not free adds `seq`, the `seq` of the command that last changed it; a held one adds `expires_at` (never its hold token or the token's hash); a booked one adds `order_ref`, `ticket_type` and `extra` (`null` when none was given; `extra` as stored, byte for byte). These go to a secret key, which wrote them, and never to a log.
  - **Freshness:** read from committed rows (`object_state`, `holds`), not the live actor (the engine has no read command, §4.2), and without loading the event. A command acknowledged before the query began is visible; one acknowledged during it may or may not be; a hold past its expiry shows as held until its expiry commits, at once for a loaded event. The pages of one walk are separate reads, so an object that changes between two pages appears as it was when its page was read. Unknown query parameters, repeated ones and a malformed `status` or `limit` are `reason: "query"`.

**Reports and log**

| Method and path | Purpose |
|---|---|
| `GET /v1/events/{event}/reports/summary?by=status\|category\|section\|channel` | Counts |
| `GET /v1/events/{event}/reports/objects?status=&format=json\|csv` | Per-object export |
| `GET /v1/events/{event}/log?after_seq=` | The command log |

**Administration**

| Method and path | Purpose |
|---|---|
| `GET /v1/keys` | List the workspace's API keys, revoked ones included; never the key or its hash. Keys, like every secret-key route, also take a console session of an owner or admin (§11) |
| `POST /v1/keys` | Create (`kind`: `secret` or `publishable`, `name`, optional `allowed_origins` for publishable keys); `201` with the key, shown only here |
| `DELETE /v1/keys/{id}` | Revoke; returns the key. Idempotent. A key may not revoke itself |
| `GET/POST/PATCH/DELETE /v1/webhooks` | Endpoints |
| `GET /v1/webhooks/{id}/deliveries`, `POST …/deliveries/{id}/replay` | Delivery log |
| `GET /v1/workspaces` | List workspaces. Admin key |
| `POST /v1/workspaces` | Create (`key`, `name`); `201` with the workspace and its first secret key, shown only here. Admin key |
| `POST /v1/workspaces/{workspace}/keys` | Mint a secret key (`name`) for an existing workspace; `201` with the key object, the key shown only here. An unknown or malformed `ws_…` id is `404` (`missing: "workspace"`). Admin key |
| `POST /v1/workspaces/{workspace}/invitations` | An owner invitation (`email`) to an existing workspace: `201` with the invitation and its one-time link (09 §7); `409` for a member's email. A new workspace's first member, or the recovery of one whose owners are gone. Admin key |
| `POST /v1/embed-tokens` | Short-lived token for the embedded designer or manager |

**Public (publishable key, CORS enabled)**

| Method and path | Purpose |
|---|---|
| `GET /v1/public/events/{event}` | Chart URL, hold lifetime, selection rules |
| `GET /v1/public/events/{event}/state?key=pk_…` | Binary snapshot (§8.5); `&after_seq=n` returns a delta if still buffered |
| `GET /v1/public/events/{event}/stream?key=pk_…` | SSE stream |
| `GET /v1/public/rt?key=pk_…` | WebSocket (§8.2) |
| `POST /v1/public/hold-tokens` | Create a hold token |
| `GET /v1/public/hold-tokens/current` | The caller's token (in `Cavea-Hold-Token`): expiry and held objects |
| `POST /v1/public/events/{event}/hold` | Hold with the caller's token |
| `POST /v1/public/events/{event}/release` | Release with the caller's token |
| `POST /v1/public/events/{event}/best-available` | Only if enabled for the event |
| `GET /v1/public/events/{event}/sections` | JSON: floors, sections, categories with availability counts (for the list view without WASM) |
| `GET /v1/public/events/{event}/sections/{section}/rows` | JSON: rows with their available seats, grouped into runs |
| `GET /v1/public/charts/{hash}.ccf` | Compiled chart (no credential; 03 §4.6) |
| `GET /v1/public/assets/{hash}` | Background images |

Public rules (publishable key; SRV-015 built the event, hold-token, hold and release routes, RT-003 to RT-010 `state`, `stream` and `rt`; `best-available`, `sections`, `rows` and `assets` arrive with best-available, the list view and asset upload):

- **Event:** `GET /v1/public/events/{event}` returns `{event, chart: {hash, url}, hold_ttl_seconds, max_objects, require_session}`: the pinned version's public hash and file (above), the lifetime of a buyer's token, the object limit of a buyer's token, which is also the maximum-selection rule (02 §12), and whether holding needs a session token, so that the SDK asks the host page for one before the buyer's first tap. Nothing else: no name, counts, settings beyond these, or anything of the backend's. `Cache-Control: private, max-age=30` and `Vary: Authorization`: the buyer's browser keeps it across page loads, per key; a shared cache must not, since the answer depends on the credential (another workspace's key names another event by the same key). An archived event is still described; its seat routes are `400 archived`.
- **Hold tokens:** `POST /v1/public/hold-tokens` takes `{event, max_objects?}` and returns `201` with `{hold_token, event, seq, expires_at, max_objects}`. The lifetime is the event's `hold_ttl_seconds` (no `ttl_seconds`: unknown fields are refused). `max_objects` defaults to the lower of the event's and the session token's (§11) and may lower it further; above it, or 0, is `invalid_field`. With a session token the new token is bound to its `sub`, within the event's `max_tokens_per_sub` (§11). Buyers cannot extend: a stalled checkout is the backend's to extend.
- **Hold and release:** `POST /v1/public/events/{event}/hold` takes `{hold_token, objects}`, `…/release` takes `{hold_token, objects?}`; replies and refusals as on the secret routes. A hold token bound to a `sub` is refused (`403`, `reason: "session"`) without a session token of that `sub` (§11). They are issued with publishable authority, so the engine refuses a booked object (INV-3) and anything but hold and release with a token; a release without `hold_token` is `400`. The token's object limit is the key-configured policy, fixed at creation; a token the backend created and handed to the browser (`session: "continue"`) keeps the limit the backend gave it.
- **The token after a reload:** `GET /v1/public/hold-tokens/current` with the token in `Cavea-Hold-Token` returns `{event, expires_at, max_objects, objects}` as committed (`Cache-Control: no-store`), for `selection.restore` (05 §4). A missing or malformed header, an unknown or expired token and another workspace's are all `404` (`missing: "hold_token"`).
- **Where the buyer's token travels:** in the JSON body or the `Cavea-Hold-Token` header, never in a URL. Public requests cross the open internet and the customer's CDN and proxies, which log URLs; the secret-key routes, called by the customer's backend, keep the token in the path and the server logs only route templates there.
- **Session tokens** travel in the `Cavea-Session` header of hold-token creation, hold and release (§11), never in a URL. An event with `require_session` refuses those three without one (`401`); any event verifies one that is sent. `hold-tokens/current` and the realtime routes take none.
- **Idempotency:** public routes take no `Idempotency-Key` (§6.1).
- **Realtime routes** (`state`, `stream`, `rt`) take the publishable key in the `key` query parameter, not in `Authorization` (D-31): `WebSocket` and `EventSource` cannot set headers, and the snapshot must be cacheable by URL (§8.5). The key is checked as everywhere else: unknown or revoked `401`, another kind `403`, an `Origin` off its allow-list `403`. A secret key in a URL is refused like any wrong kind, and query strings are never logged. The hold token never goes in these URLs either: it travels inside SUBSCRIBE (§8.2).

The compiled chart route takes no credential and ignores any sent. A chart file is named by its content hash alone: the URL must be the same for every buyer so that browsers and CDNs can cache it, a credential in a header would keep CDNs from caching it and force a CORS preflight, and a publishable key in the URL would change it on every key rotation. The hash identifies the bytes, so the route is unambiguous across workspaces (two workspaces that publish identical charts serve one identical file), and it exposes nothing a buyer of the event does not already download: 128 bits cannot be guessed, and the chart's key and workspace are not in the URL. It serves only public artefacts of published versions; the full artefact is never served. Private charts are P2 (03 §4.6).

**Assets** (images: a chart's background and reference images, a version's thumbnail):

- **Upload:** `POST /v1/assets`, secret key. The body is the image's bytes; `Content-Type` and any file name are ignored. At most `CAVEA_ASSET_MAX_BYTES` (§13, 10 MiB by default): more is `400` with `reason: "body_too_large"` and `limit`, from `Content-Length` before anything is read, or while reading, as soon as the body passes it. The type is decided by the bytes, against an allow-list of the formats every renderer host decodes (`createImageBitmap`, ImageIO, `ImageDecoder`): PNG, JPEG and WebP. Anything else, SVG included, is `reason: "asset_type"`. Nothing is decoded: the structure is walked far enough to read the size and to see that the file ends where its format says (PNG: chunks from `IHDR` to an `IEND` that ends the file; JPEG: segments up to the first scan, the size from the frame header, `EOI` last; WebP: a RIFF container that is exactly the file). A truncated file, data after the end (a polyglot) or a header that does not parse is `reason: "asset_malformed"`; a side of 0 or over 8,192 pixels `reason: "asset_dimensions"` with `limit`. A JPEG's entropy-coded data, between its first scan and `EOI`, cannot be checked without decoding, so other bytes can hide there; serving makes them harmless (below). Hashing and the walk run on a blocking thread. **Memory:** the bytes an upload has read count against the node's upload budget, 4 times `CAVEA_ASSET_MAX_BYTES` (40 MiB by default), as they arrive, until the reply; an upload that would pass it is `503 overloaded` (`reason: "asset_memory"`, `Retry-After: 1`) at once rather than waiting, so concurrent uploads cannot grow the process.
- **Reply:** the asset object `{hash, content_type, size, width, height, url}`: `hash` is the first 16 bytes of BLAKE3 of the bytes in 32 lowercase hex digits, as for compiled charts (03 §4.1); `url` is `/v1/public/assets/{hash}`. `201` when the workspace did not hold these bytes, `200` with the same object when it did. Assets belong to the workspace whose key uploaded them; another workspace uploading the same bytes gets its own.
- **Serving:** `GET /v1/public/assets/{hash}` takes no credential and serves the bytes of any workspace's asset with that hash, as the compiled chart route does and for the same reasons (D-49): `Content-Type` the sniffed type, `X-Content-Type-Options: nosniff`, `Content-Security-Policy: default-src 'none'; sandbox` and `Content-Disposition: inline; filename="{hash}.{png|jpg|webp}"`, so that a browser sent to the URL shows an image or nothing and runs nothing; `Cache-Control: public, max-age=31536000, immutable`; `ETag` the hash, quoted, with `304` for `If-None-Match` (an existence check, without reading the bytes); `Access-Control-Allow-Origin: *` whatever `CAVEA_CORS_ORIGINS` says, so the renderer may use it as a WebGL texture from any page. `HEAD` answers as `GET`. Not rate-limited, like the compiled chart files. **Memory:** a response first reads the asset's size, then waits up to 10 s for that many bytes of the node's serving budget (4 times `CAVEA_ASSET_MAX_BYTES`, separate from uploads, so anonymous readers cannot starve the designer), else `503 overloaded` (`reason: "asset_memory"`); the bytes keep them until the last 64 KiB piece written from them is released (the connection takes the next as the client reads), so slow readers count. An upload or a password hash whose request is cancelled keeps its share until its blocking work ends. An image larger than the whole serving budget, which only an upload made before `CAVEA_ASSET_MAX_BYTES` was lowered can be, is never read: `503 overloaded` (`reason: "asset_memory"`), and a warning names the setting to raise. Images in memory are thus at most about twice each budget (the row's bytes and their copy in the driver), whatever the number of requests; a CDN in front takes repeated reads off the node.
- **Lifecycle:** assets are immutable and kept. There is no list or delete endpoint: a draft's image node, a published version and a thumbnail refer to an asset by hash, and none of them may lose it. There is no quota per workspace yet; the size limit bounds each upload. **E007:** validate and publish check that each background image of the compiled chart (read from its public artefact, what a renderer will fetch) names an asset the workspace holds; another workspace's copy does not count. One that does not is a finding `{code: "E007", severity: "error", node, related: null, message}`, `node` the first image node naming the hash (the message names no hash, 03 §3.2): validate answers `valid: false` with it, publish `422 chart_invalid`. It is checked only on a draft that compiles. Reference images are the designer's only and are not checked.
- **The designer:** the image tool (07 §4) uploads the picked file here and writes the reply's `hash` into the image node's `asset`, its `width` and `height` as the node's starting `size`; it shows the image from `url`. Publishing (07 §11) renders the 640 × 480 PNG thumbnail (03 §5) and uploads it here; attaching it to the version, as `chart_versions.thumbnail` holding the asset's 16-byte hash, comes with DSN-040.

**Console** (a console session, 09 §10; the role each `/v1` route needs is in §11)

| Method and path | Purpose |
|---|---|
| `POST /console/api/setup` | First-run owner creation with the one-time token (no session; `201` signed in) |
| `POST /console/api/session` | Sign in (no session; `200` signed in) |
| `DELETE /console/api/session` | Sign out: the session ends on the server, the cookie is cleared; `204` |
| `GET /console/api/me` | The user, their workspaces with role and permissions, the CSRF token, the session's ends |
| `POST /console/api/password` | Change one's password: every session of the user ends, a new one starts |
| `GET /console/api/members` | The workspace's members: `{user: {id, email}, role}`, by user id, paginated. Owner and admin |
| `PATCH /console/api/members/{user}` | Change a member's role (`{role}`); making or unmaking an owner needs `ownership`; the last owner cannot be changed (`400` `last_owner`) |
| `DELETE /console/api/members/{user}` | Remove a member; `204`. The same owner rules |
| `POST /console/api/members` | Invite an email with a role (`{email, role}`): `201` with the invitation and its one-time link; an owner invitation needs `ownership`; `409` for a member's email |
| `GET /console/api/invitations` | Pending invitations, oldest first, paginated; never a link |
| `DELETE /console/api/invitations/{invitation}` | Revoke; `204` |
| `POST /console/api/invitations/accept` | Accept a link's token (`{token, password?}`), signed in as its email or by setting a password for a new account (09 §7); no `settings` |
| `PATCH /console/api/workspace` | Rename (`{name}`) |
| `GET /console/api/audit` | The audit log, newest first, paginated (09 §7) |

The members, invitation, workspace and audit routes take the console session only (no API key), name the workspace in `Cavea-Workspace` like `/v1`, and need `settings`; acceptance takes a token instead, with the session when there is one. Changes to one workspace's members are serialised on its row, so a workspace always keeps an owner whatever runs at once; a change applies to the member's next request and ends nothing else of theirs (sessions, other workspaces). A user removed from every workspace can still sign in and sees none.

**System**

`GET /healthz` (process alive), `GET /readyz` (database reachable, migrations applied, not shutting down), `GET /metrics` (Prometheus, admin key), `GET /v1/openapi.json` (this API, §6.1), `GET /v1/system/info` (version, licence state). §15 has the details.

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
  "details": { "objects": [ { "label": "Orchestra-C-13", "oid": 4113, "reason": "not_free" } ] } }
```

Book after payment:

```http
POST /v1/events/hamlet-2027-03-14/book
Authorization: Bearer sk_live_…
Idempotency-Key: order-77120

{ "hold_token": "ht_8Zk…", "order_ref": "77120",
  "ticket_types": { "Orchestra-C-12": "adult", "Orchestra-C-13": "child" } }
```

```json
{ "seq": 48230, "objects": [ { "label": "Orchestra-C-12", "oid": 4112 }, { "label": "Orchestra-C-13", "oid": 4113 } ] }
```

The same request with the same key returns this again; with another body, `409 idempotency_mismatch`.

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
  "details": { "findings": [ { "code": "E007", "severity": "error", "node": 12, "related": null,
                               "message": "unknown category \"vip\"" } ] },
  "request_id": "req_6ad9b530f0e172e7214232f15266b459" }
```

Objects (fake order reference):

```http
GET /v1/events/hamlet-2027-03-14/objects?label=Orchestra-C-12&label=Orchestra-C-13&label=Orchestra-Z-99
Authorization: Bearer sk_…
```

```json
{ "data": [
    { "label": "Orchestra-C-12", "oid": 4112, "status": "booked", "seq": 48230,
      "order_ref": "77120", "ticket_type": "adult", "extra": null },
    { "label": "Orchestra-C-13", "oid": 4113, "status": "held", "seq": 48211,
      "expires_at": "2027-03-01T19:15:00Z" } ],
  "unknown": [ "Orchestra-Z-99" ] }
```

`GET /v1/events/hamlet-2027-03-14/objects?status=free&limit=2` returns `{"data": [{"label": "Orchestra-A-1", "oid": 4001, "status": "free"}, {"label": "Orchestra-A-2", "oid": 4002, "status": "free"}], "next": "Orchestra-A-2"}`; the next page is `…&after=Orchestra-A-2`.

### 6.4 Error codes

| HTTP | `code` | Meaning |
|---|---|---|
| 400 | `invalid_request` | Malformed body or parameter |
| 401 | `unauthorized` | Missing or invalid key |
| 403 | `forbidden` | Key lacks permission, or origin not allowed |
| 404 | `not_found` | Unknown chart, event, object, token, key or workspace |
| 409 | `already_exists` | A workspace, chart or event with this key already exists |
| 409 | `object_not_free` | At least one object is not free |
| 409 | `object_not_held_by_token` | Book or release named an object the token does not hold |
| 409 | `object_status_conflict` | An object's status does not allow the change: release without a token of an object that is not booked, or change-status to free of a held or booked object |
| 409 | `version_conflict` | Draft etag mismatch (`details.draft_etag` is the current one), or objects named by `idx` in a chart version that is not the event's (`details.version` is the event's) |
| 409 | `upgrade_conflict` | Non-free objects missing from the new chart version |
| 409 | `idempotency_mismatch` | Key reused with a different body |
| 410 | `hold_token_expired` | Not returned by the actor, which deletes an expired token before processing any command at or after its expiry (§7.3); the token is then `not_found` |
| 422 | `hold_limit_exceeded` | Over the token's object limit; or, creating a public hold token, the session's `sub` already has the event's `max_tokens_per_sub` live tokens (§11) |
| 422 | `channel_not_allowed` | |
| 422 | `orphan_rule_violated` | |
| 422 | `not_enough_objects` | Best-available could not satisfy the request |
| 422 | `chart_invalid` | Publish blocked by validation errors; `details.findings` lists them (§6.2) |
| 429 | `rate_limited` | A public rate limit (§12), or the realtime connections one client address may hold (§8.2); `Retry-After` gives the seconds to wait when known |
| 500 | `internal_error` | Unexpected fault, a panic included (§16); the cause is logged under the request id, never returned |
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
- `details` is always an object, empty when there is nothing to add. Engine refusals fill it: `invalid_request` has `reason`; `not_found` has `missing` (`hold_token` or `objects`, with `objects` listing the unresolved references as `{"label"}` or `{"idx"}`); the three object conflicts list `{"idx", "reason"}` per object (the seat routes name each by `{"label", "oid", "reason"}` instead) (`not_free`, `not_held_by_token`, `not_booked`, `not_blocked`); `forbidden` from the engine means the key's kind may not issue the command (`02-domain-model.md` §4.3); `hold_limit_exceeded` has `limit` and `requested`.
- `request_id` equals the `Cavea-Request-Id` header.
- An unknown path, and a known path with the wrong method, are `404 not_found`. A body that is not JSON, lacks `Content-Type: application/json`, does not match the expected shape, is over the size limit or nests too deeply is `400 invalid_request` with `details.reason` one of `json_syntax`, `content_type`, `json_data`, `body_too_large`, `json_depth` (with `limit`), `body_unreadable`. A malformed path parameter is `reason: "path"`, a malformed query parameter or cursor `reason: "query"`, an invalid body field `reason: "invalid_field"` with `field`, and too many objects `reason: "too_many_objects"`. For assets: `asset_type`, `asset_malformed`, `asset_dimensions` (with `limit`), and `body_too_large` with `limit` (§6.2). For charts: a missing or malformed `If-Match` is `reason: "if_match"`, a draft that is not a storable chart document `reason: "document"`, and a change to an archived chart `reason: "archived"`. An identifier in the path that cannot exist, or belongs to another workspace, is `404`.
- An invalid `Idempotency-Key` is `400 invalid_request` with `reason: "idempotency_key"`.
- `401 unauthorized` has `details.reason`: `missing` (no `Authorization`), `malformed` (not `Bearer <token>`), `invalid` (unknown or revoked key, or not the admin key), or `session_token` with `details.session` one of `missing`, `invalid`, `expired`, `event` (§11). `403 forbidden` from authentication has `reason`: `key_kind` (the wrong kind of credential for the route), `origin` (§11), `self_revocation`, or `session` (a hold token bound to another `sub`, or used without a session, §11).
- `hold_limit_exceeded` for the per-`sub` cap has `reason: "tokens_per_sub"` and `limit`.
- `503 overloaded` carries `Retry-After: 1`.

## 7. Algorithms

**Order of checks.** A command is checked in this order, and the first check that fails is the reply: the idempotency key (§6.1: its form, then a replay or `idempotency_mismatch`, then `overloaded` for a full window); the authority (`forbidden`, 02 §4.3); the request's shape (object counts, lifetimes, an object limit of 0, `bad_status`, text fields, booking details' sizes); the hold token (`not_found`, or `token_already_exists` when creating one); the object references (every unknown one, in request order, a booking's objects before its details'); what needs resolved objects (a token with nothing to book, `detail_mismatch`); the per-object conditions (every failing object, ascending `idx`); last the token's object limit. The reference model (13 §3.1) is checked against the engine in exactly this order.

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
| `0x05` | RESYNC | Nothing. The client fell behind and frames were dropped; the current SNAPSHOT (and PRIVATE) follows on the same stream (§8.3) |
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
| `0x10` | SUBSCRIBE | `u8` version · `bytes(512)` event key (UTF-8 of at most 128 characters, `02-domain-model.md` §3) · `v64` last `seq` seen (0 if none) · `opt bytes(256)` hold token · `opt bytes(1024)` session token |
| `0x11` | PONG | `v64` nonce, echoing the PING's |

Tokens are opaque bytes; a present token may be empty.

**Connecting** (`GET /v1/public/rt?key=pk_…`; 12 §3 T11):

- **Upgrade.** The key is checked on the upgrade request (§6.2, §11), so a refusal is an HTTP status: `401` or `403` for the key, `403` (`reason: "origin"`) for an `Origin` that `CAVEA_CORS_ORIGINS` (a list) or the key's allow-list does not admit, `429 rate_limited` past `CAVEA_RT_MAX_PER_IP` open connections from the client's address (§12; an IPv6 client is its /64), or when the key's reads budget for that address is spent (each connection attempt is one read, §12), `503 overloaded` past `CAVEA_RT_MAX_CONNECTIONS` on the node, `400` (`reason: "upgrade"`) for a request that is not a WebSocket upgrade. CORS does not apply to WebSockets, so checking `Origin` here is the only cross-site gate; a request without `Origin` (not a browser) is accepted, as on every public route.
- **SUBSCRIBE first.** The client's first message must be SUBSCRIBE, within 10 s of the upgrade; otherwise the connection is closed without a frame. Before it, PONG, text or a frame that does not decode is CLOSE 6; version 0 is CLOSE 4; unknown frame types are skipped (§8.6).
- **The event.** SUBSCRIBE's event key is looked up in the key's workspace: another workspace's event and an unknown one are both CLOSE 5. An archived event is CLOSE 1.
- **The hold token** in SUBSCRIBE (inside the connection, never in the URL) must be a live hold of that event, in that workspace, with an object limit of 1 to 100: every buyer's token is (the event's `max_objects` is at most 100, §6.2), and the renderer refuses a PRIVATE set of more than 100 objects. Anything else (unknown, expired, another event's or workspace's, or a secret-key token without a limit or above 100) is CLOSE 5, the same frame as an unknown event, so nothing tells which. A backend that hands a browser its own token (`session: "continue"`) creates it with `max_objects`.
- **The session token** (a JWT, §11) is decoded and bounded, and not verified: no public stream depends on a session, so following an event needs none, also with `require_session` (§11). Channel streams (§8.3) will verify it and compute the stream from its `channels`.
- **After SUBSCRIBE** the client sends only PONG. A second SUBSCRIBE, text, a frame that does not decode, more than 10 messages between two PINGs, or a message over 2 KiB (SUBSCRIBE with every field at its limit is 1,812 bytes) is CLOSE 6 and the connection is closed.

### 8.3 Behaviour

- On SUBSCRIBE the server replies HELLO, then either the DELTAs since `last_seq` (if still in the 60-second ring buffer) or a SNAPSHOT, then PRIVATE if a hold token was given. Nothing follows HELLO for a `last_seq` equal to the current one; a `last_seq` of 0 always gets the SNAPSHOT. The engine's `Registry::subscribe` returns these frames and a receiver of everything published after them, taken under the same lock, so no delta between the two is lost or repeated.
- Deltas are coalesced over an interval of 100 ms: at most one publication per interval (§4.5). With more than 20,000 subscribers on an event the interval stretches in proportion to the subscriber count, 100 ms × subscribers / 20,000, up to 500 ms at 100,000 subscribers, so the frames sent per second for one event stay at 200,000. The count is read when the publication is due, and a pending publication is rechecked at each 100 ms step, so subscribers leaving shortens it. A publication also waits while the last one has not reached the subscribers yet: while more than 1,000 of them, and more than a tenth, have not taken it from the channel (a connection takes a frame only once it has written the one before), up to 1 s after the last (`MAX_WAIT_MS`). When the cores left for fanning out cannot write every frame within the interval, changes then coalesce over a longer one instead of queueing up behind it until subscribers lag into RESYNC; a few stalled clients, or subscribers that leave, never hold it back.
- If an object changes twice within an interval, only the final state is sent (`wire::coalesce`). A change the public cannot see (held to booked, or a code that went and came back within the interval) is left out of the DELTA. An interval in which `seq` advanced but nothing public changed (an extend, a hold token created, held to booked) still publishes a DELTA, with no changes, so that every subscriber's `seq` follows the committed one. An interval with no commit publishes nothing.
- A subscriber whose send queue exceeds 64 frames is sent RESYNC and its queue is cleared. The engine's broadcast channel keeps the last 64 publications for all subscribers together, so a receiver that falls further behind costs nothing more: it is told it lagged, receives nothing until it takes the current SNAPSHOT (`Subscription::resync`), and continues from there.
- **Two runtimes** (D-45). Request handling, the actors, commits and the database run on one Tokio runtime, at the default scheduling priority; every realtime connection's stream, and each event's relay (§4.5), on a second, `cavea-realtime`, whose threads run under Linux's `SCHED_IDLE` policy. Each has as many worker threads as the cores the process may use (`available_parallelism`: the affinity mask or the container's quota). The kernel runs a request thread the moment it is ready and gives fanning out the cores requests leave idle, so a stampede of subscribers slows delta delivery, never a command, a commit, a health check or a new connection's snapshot. The WebSocket's handshake, the event and token lookups and SUBSCRIBE are handled on the request runtime, where the database is used; only the stream moves. An SSE stream's task runs on the realtime runtime too, but its socket is written by the HTTP connection on the request runtime. Measured locally (13 §7.4): in L1, with 55,000 subscribers on 4 vCPU, hold p99 went from 1.13 to 1.31 s to 18 ms (32 to 107 ms in runs where the machine was busier), the same as with only the 5,000 buyers subscribed, and commit to delta delivered p95 from 0.34 to 0.40 s to 1.1 s. On other systems (no `SCHED_IDLE`) the threads keep the default priority and the separate runtime alone applies.
- **One connection, one task.** It writes and flushes each frame before it takes the next, so its own queue is one frame; the rest of its send queue is its position in the broadcast channel, shared. A client that stops reading fills its socket buffers (bounded by the kernel), then blocks the write; its subscription falls behind, and once the write completes it gets RESYNC, then the current SNAPSHOT and PRIVATE, then DELTAs from there. A write that has not completed in 30 s ends the connection without a CLOSE. RESYNCs are counted for `cavea_rt_resyncs_total`, subscribers for `cavea_rt_subscribers` (§15).
- **PRIVATE.** Each publication carries the committed hold of every token touched since the last one (holds and releases, extends, bookings, expiry, creation). A subscriber that gave a hold token gets a PRIVATE frame right after the DELTA of a publication that touched its token, and none otherwise; it is encoded by that subscriber, so a subscriber whose token did not change does no work beyond a map lookup. A PRIVATE frame is always the token's committed hold at the `seq` of the DELTA it follows (or of the frames it follows on SUBSCRIBE), never ahead of the public state the client has. A token with no hold gets an expiry of 0 and no objects.
- When the event's actor stops, every subscription ends: after a failure (the event reloads; the transport subscribes again with the subscriber's `seq`, which resumes it or sends a SNAPSHOT), or after a clean stop at shutdown (CLOSE 3). A subscriber keeps the actor from being evicted. The connection does the resubscribing itself: the client sees more DELTAs or a SNAPSHOT, never a second HELLO. If the event cannot be had again, it is CLOSE 3.
- **Shutdown** (§14): every connection is sent CLOSE 3 and closed as soon as the signal arrives, before requests drain.
- **Archive and upgrade.** CLOSE 1 is sent to a SUBSCRIBE for an archived event. A subscriber already connected when the event is archived keeps receiving its state (holds still expire) until it reconnects, and keeps its actor loaded until then. CLOSE 2 waits for chart upgrades (EVT-002).
- **Polling** (`after_seq=n`, §8.4): nothing new if `n` is current; otherwise one DELTA from `n`, the buffered deltas merged, if the ring buffer reaches back to `n`, or else the SNAPSHOT.
- PING every 25 s; a connection silent for 60 s is closed. These are the protocol's PING and PONG frames, not WebSocket control frames: a browser's script sees neither control frames nor their timing, so a PING lets the client detect a dead server too. Any message from the client counts as not silent; the client answers each PING with PONG and its nonce. Each connection has one timer, set to the next PING or the silence deadline, whichever is first, so an idle connection wakes once every 25 s. The timer is served before the next frame, so a feed that never pauses does not delay either. No CLOSE is sent to a silent client.
- **Public streams** carry public codes only (`02-domain-model.md` §4.2). **Manager streams** (`GET /v1/events/{event}/rt`, secret key or console session) carry domain statuses.
- Viewers with channels get a stream computed for their channel set. Channel sets are cached per event, so cost grows with distinct sets, not with viewers.

### 8.4 Fallbacks

| Transport | Framing |
|---|---|
| WebSocket | Binary messages, one frame each |
| SSE | Each frame base64-encoded in a `data:` line; `Last-Event-ID` carries `seq` |
| Polling | `GET …/state?after_seq=n` every 2 s returns a DELTA or SNAPSHOT frame |

SSE and polling have no client-to-server frames: the SUBSCRIBE fields travel as request parameters, the key as `key` (§6.2), the event in the path.

**SSE** (`GET /v1/public/events/{event}/stream?key=pk_…`):

- `200` with `Content-Type: text/event-stream`, `Cache-Control: no-cache` and `X-Accel-Buffering: no` (so that proxies such as nginx do not buffer it), then the frames of a WebSocket subscription, each one event: `data: <base64 of the frame>` (standard alphabet, padded), preceded by `id: <seq>` for SNAPSHOT and DELTA. HELLO comes first, then the SNAPSHOT or the DELTAs since the client's `seq`, then every DELTA; RESYNC and a SNAPSHOT for a client that falls behind; CLOSE (3 at shutdown or when the event cannot be had, 1 for an archived event), after which the server ends the stream.
- **Resuming.** The `seq` comes from `Last-Event-ID` (which `EventSource` sends when it reconnects by itself, with the last `id` it saw), else from `last_seq` in the query, else 0. A `Last-Event-ID` that is not a number is `400` (`reason: "last_event_id"`).
- **Keep-alive.** A comment line (`:`) every 25 s takes PING's place: the client cannot answer, so an SSE stream is never closed for silence. A client that goes away is noticed at the next write, at most 25 s later.
- **Limits and memory.** A stream counts toward the same connection limits as a WebSocket (§8.2), and opening one is one read (§12). The connection's task writes into a channel of one event, which the HTTP connection drains into its write queue (at most 16 chunks) and the socket; a client or proxy that stops reading fills these, the task's write blocks, and after 30 s the task ends without a CLOSE and drops its subscription, as on a WebSocket. The socket belongs to the HTTP connection, on the request runtime (§8.3), which waits for the client to read before it writes the end of the stream, so it stays open until the client reads again or the connection breaks, and keeps its slot until then: a stalled stream still counts toward the connection limits. A stream therefore holds at most 17 events of its own; what it has not sent stays in the engine's shared channel, and RESYNC follows if it falls 64 publications behind. A proxy that buffers the whole response (despite `X-Accel-Buffering`) keeps reading, so the server sees a healthy client and the buffering costs the proxy, not the server.
- **No PRIVATE.** `EventSource` cannot set headers and the hold token never goes in a URL, so an SSE stream carries no hold token and no PRIVATE frames. A buyer on SSE or polling reads their own holds from the hold and release replies and from `GET /v1/public/hold-tokens/current` (on connect, after each SNAPSHOT, and periodically while a hold is live).
- Every refusal comes before the stream starts, as problem+json with the CORS headers: `404` for an unknown event or another workspace's, `401` or `403` for the key, `429` and `503` with `Retry-After`. `EventSource` does not retry an error status, so the client decides. A client calls `close()` on its `EventSource` when it receives any CLOSE: for CLOSE 3 it then reconnects itself with backoff and jitter, passing its `seq` as `last_seq`, as on a WebSocket, rather than let every browser reconnect after the same fixed delay. A dropped connection may be left to `EventSource`, which resumes from `Last-Event-ID`; the renderer closes on every error and reconnects itself the same way, so that its delays are jittered too (`05-web-sdk.md` §4.1).

**Polling** (`GET /v1/public/events/{event}/state?key=pk_…&after_seq=n`, every 2 s): `200` with one DELTA from `n` (the buffered deltas merged into one), or the SNAPSHOT if the ring buffer no longer reaches back to `n` (or `n` is 0, or a `seq` the server never published); `204` with an empty body when nothing was published after `n`. A frame carries the snapshot's cache headers (`public, max-age=1, stale-while-revalidate=5`, §8.5) and no `ETag`: a DELTA from `n` stays true (it runs to whatever was current when it was made, and the client then polls from its `to_seq`), as does a SNAPSHOT, so a cached one is only less current, and clients at the same `seq` share one cached URL. A `204` is `Cache-Control: no-store`: it stops being true at the next publication. Each poll is one read (§12): one every 2 s is 30 a minute. The client applies the frame and polls again from its new `seq`.

### 8.5 Caching the snapshot

- `GET /v1/public/events/{event}/state?key=pk_…` returns the current public SNAPSHOT frame (`Content-Type: application/vnd.cavea.frame`, one frame of this section) with `ETag: "<seq>"` and `Cache-Control: public, max-age=1, stale-while-revalidate=5` (the max age is `CAVEA_SNAPSHOT_MAX_AGE`). `If-None-Match` listing the tag (weak or strong) or `*` is `304` with the same headers. It is read from the engine's published snapshot without a message to the actor; an event not loaded is loaded first. Every snapshot's `seq` is a point deltas resume from (D-27), so one cached for a second is as good a start as a fresh one.
- **Scoping and caching (D-31).** Event keys are unique only within a workspace, so the URL names the workspace through the publishable key in `key`: the URL is then the whole cache key, different for every workspace's event of the same key, and a CDN caches it like any public file. A key in `Authorization` would stop shared caches (a cache that ignored it would serve one workspace's state for another's event of the same key) and force a CORS preflight; a request with only `key` is a simple GET and is not preflighted. A publishable key is not a secret: it is in every page that embeds the renderer. Rotating it changes the URL, which costs one cache miss.
- **What a cached answer skips.** A CDN hit is served without the key's checks: a revoked key's URL, or a page whose `Origin` the key's allow-list refuses, may get a cached snapshot for up to `max-age` plus `stale-while-revalidate`. That is accepted: the public snapshot shows only which seats are unavailable (D-12), as any buyer of the event sees, and the allow-list is a hurdle, not a guarantee (12 §3 T3). Errors (`401`, `403`, `404`, `503`) carry `Cache-Control: no-store`. With `CAVEA_CORS_ORIGINS` a list, responses vary by `Origin` (`Vary: Origin`): a CDN must then key on `Origin` or have the setting left at `*`.
- **Events.** An unknown event, and another workspace's, are `404`. An archived event's snapshot is still served: reads keep working (§6.2).
- With a CDN in front, the opening rush of an on-sale hits the CDN, not the server. Clients then catch up through deltas.
- Channel-specific snapshots are `private` and not cached by intermediaries.

### 8.6 Framing, validation and versioning

- **One frame per message.** A WebSocket binary message, an SSE `data:` line once base64-decoded, and an HTTP response body (the snapshot endpoint, polling) each carry exactly one frame.
- **Unknown frame types** are ignored: the receiver skips the message and carries on. One frame per message is what makes skipping possible on every transport.
- **Trailing bytes** after a frame's last field are an error, except in a SUBSCRIBE whose version is above the server's (versioning, below).
- **Lists.** Each `idx` is strictly above the previous one: a difference of 0 after the first item is an error. Every `idx` is below 1,000,000 (`02-domain-model.md` §14) when decoded, and below the receiver's object count before anything is applied; a frame naming an `idx` at or past the receiver's count is refused whole. A SNAPSHOT's areas are checked against its own object count when decoded.
- **Counts and lengths.** An object or list count above 1,000,000 is an error; so is a list count the remaining bytes could not hold (two bytes per item with a code or value, one without), so a hostile count never drives a large allocation. A length past the end of the frame is an error.
- **Object count.** A SNAPSHOT's object count must equal the receiver's (from its chart); otherwise it is refused. A client that receives the snapshot before its chart keeps the bytes and checks them when the chart is decoded (`04-renderer.md` §4).
- **Sequence numbers.** A DELTA has `to_seq` > `from_seq`; it moves the receiver from `from_seq` to `to_seq` and is applied only by a receiver whose `seq` is `from_seq`. Anything else (a gap, a repeat) is the adapter's cue to resync; the decoder never guesses. A DELTA may carry no changes: commands that change nothing a public stream shows (held to booked) still advance `seq`. A server's DELTAs on one stream are contiguous, and every SNAPSHOT it serves has a `seq` that some DELTA starts from, so a receiver that applies every frame in order never sees a gap.
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
| 5 | No such event for this credential, or a hold token that may not be followed on it (§8.2) | Stop and report; without the token, subscribe again |
| 6 | The server could not decode the client's frame, or did not expect it (§8.2) | Reconnect with backoff |

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
| Session token | Buyer's browser | A JWT the customer's backend signs with a secret key (HS256, `kid` the key's id) for one buyer and one event. Claims: `sub`, `exp`, `event`, `max_objects`, `channels` (below) |
| Console session | Staff | Cookie session (below); on `/v1`, one workspace per request, by role (09 §2) |
| Embed token | Embedded designer or manager | Short-lived, scoped to one workspace and one chart or event, with a permission list |

- Session tokens are optional. An event can require them (`require_session: true`), which is the strongest defence against anonymous seat hoarding (12 §3 T1).
- Every query is scoped by workspace id taken from the credential, never from the request alone. In the server the workspace id is obtainable only from an authenticated key, or from a console session whose user is a member of the workspace the request names (`http::Auth`). The one exception is the compiled chart route, which takes no credential and finds a public file by its content hash alone (§6.2).

API keys:

- A key is `sk_` or `pk_` and 64 lowercase hex digits: 256 bits from the operating system's secure random source. It is returned once, at creation. Only its SHA-256 (`api_keys.hash`) and its first 11 characters (`prefix`, for display) are stored. A key's id (`key_…`) is not the key.
- A presented key is found by its SHA-256, so no stored secret is compared byte by byte (12 T16). Unknown and revoked keys are both `401`; a valid key of the wrong kind for the route is `403`. Secret keys are refused on public endpoints and publishable keys everywhere else.
- `last_used_at` is updated by the lookup itself when it is unset or more than an hour old: accurate to an hour, and at most one write per key per hour.
- Rotation: create a second key, move the backend to it, revoke the first with it. Both are valid in between.
- A key may not revoke itself (`403`, `reason: "self_revocation"`). Every revocation by a key is made by another unrevoked secret key, serialised per workspace, so keys alone always leave one. An owner or admin in the console may revoke any key, the last secret key included: the console can mint another.
- Origin allow-list (publishable keys, 12 T3): up to 50 origins such as `https://tickets.example.com`, compared without regard to case. A request whose `Origin` is not on the list, `null` included, is `403` (`reason: "origin"`). A request without `Origin` is accepted: native apps send none, and a non-browser client can send any `Origin` it likes, so refusing it would stop no attacker. A key without a list accepts every origin.

Console sessions (09 §2, §10; `DECISIONS.md` D-50). Not the buyer's session token below: a staff member's sign-in.

- **Cookie.** `__Host-cavea_console=cs_…; Path=/; Max-Age=604800; HttpOnly; SameSite=Lax; Secure`, set by sign-in, setup and a password change; `Max-Age=0` clears it at sign-out. `CAVEA_CONSOLE_COOKIE_SECURE=false` (development over plain HTTP from a host other than `localhost`) drops `Secure` and the `__Host-` prefix, which requires it: the name is then `cavea_console`. `__Host-` pins the cookie to this host and path `/`, so no other subdomain can set or read it.
- **Lifetimes.** A session ends 12 hours after its last use ("renewed on activity": a request with it moves the end, at most every five minutes, so a busy console writes once per session per five minutes) and 7 days after sign-in however used. Both are checked by the lookup, on the database's clock. A disabled user's sessions stop at once.
- **Rotation and revocation.** Sign-in always starts a new session and ends one presented with it; setup starts the owner's first. A password change ends every session of the user and starts a new one. Sign-out ends the session on the server. A role change takes effect on the next request, since the role is read with each; there is no elevated mode to rotate into yet (TOTP).
- **CSRF.** Every non-safe method (anything but `GET`, `HEAD`, `OPTIONS`) authenticated by the cookie, on `/console/api` and on `/v1`, must send `Cavea-Csrf-Token`: the session's token, which `GET /console/api/me` and the sign-in reply give, the SHA-256 of a fixed label and the session identifier (nothing stored; another site can neither read the cookie nor derive it). Without it, or wrong: `403`, `reason: "csrf"`. Such a request must also come from the server's own origin: `Sec-Fetch-Site` `same-origin` (or `none`), or without it an `Origin` whose host is the request's `Host`; else `403`, `reason: "origin"`. Sign-in and setup, which have no session yet, take the origin check too, and like every JSON route need `Content-Type: application/json`, which another site cannot send without a preflight nobody answers. No `GET` changes anything. A request with `Authorization` is read as an API key and never for a cookie, so neither check applies to keys. Every response to a request carrying the console cookie and no `Authorization` is `Cache-Control: no-store`, whatever the route: a cookie, unlike `Authorization`, does not stop a shared cache from storing the answer (RFC 9111 §3.5), and one workspace's answer must never serve another's request to the same URL.
- **On `/v1`.** Every route that takes a secret key takes a console session instead when `Authorization` is absent. The request names its workspace in `Cavea-Workspace` (`ws_…`, from `me`): missing, `400` `reason: "workspace"`; a workspace the user is not a member of, existing or not, `403` `reason: "workspace"`. The workspace is then the credential's, as a key's would be: every query is scoped by it, and another workspace's identifiers are `404`. The member's role must grant the permission the route declares (`403` `reason: "role"`, with `details.permission`). The engine sees secret-key authority, and the event log's `actor` is `usr_` and the user's id (never the email); drafts and versions record the user. Admin-key routes (`/v1/workspaces`, `/metrics`) and the public routes, realtime included, take no console session: they read only their own credential, so a cookie sent along changes nothing. The role is read with the session on every request, in the session lookup's own query, so a role lowered or a membership removed applies to the member's next request; nothing caches it. Measured locally (`measure_session_lookup`: 2,000 sequential `GET /v1/events/{event}` through the router, release build; AMD Ryzen 9 7900X shared with two other builders, load average 3.4 to 4, the shared PostgreSQL in Docker; four runs): a session costs 0.06 ms more than a secret key at the median (0.43 to 0.44 ms against 0.37 to 0.38 ms). Its p99 ranged from 0.54 to 1.39 ms against the key's 0.49 to 0.59 ms: the lookup is one heavier statement (a join, the membership subquery and the conditional renewal, a data-modifying CTE) where the key's is a primary-key probe, so it is more exposed to a busy database; for a console sending a few requests a second it does not matter. The hold path with a key does not change (`measure_hold_with_a_key`, 1,600 holds: p50 1.91 to 2.00 ms on this branch, 1.87 to 2.19 ms before it, same conditions).
- **Permissions** (09 §2, D-54). Each route in the route table declares one (`openapi::Needs::Staff` for the `/v1` routes, `Needs::Member` for the console's workspace routes), shown in the OpenAPI document as `x-cavea-permission`, with `x-cavea-roles`. One permission per cell of 09 §2's table, so each route maps to exactly one; those without a route yet keep room for theirs, and `me` lists them all:

  | Permission | Routes | Roles |
  |---|---|---|
  | `charts.read` | `GET` charts, drafts, versions | all five |
  | `charts.write` | create, save the draft, validate, publish; upload images | owner, admin, editor |
  | `charts.archive` | archive a chart | owner, admin |
  | `events.read` | `GET` events, objects, hold tokens | all five |
  | `events.write` | create, change, archive events (and their channel lists, when built) | owner, admin |
  | `seats.hold` | create and extend hold tokens, hold (and best-available, when built) | owner, admin |
  | `seats.write` | change status (block), release (a token's too), book | owner, admin, operator |
  | `channels.assign` | put objects in a channel (not built) | owner, admin, operator |
  | `reports.read` | reports and exports (not built) | owner, admin, viewer |
  | `developers` | API keys (webhooks, embed tokens when built) | owner, admin |
  | `settings` | members (`/console/api/members`); workspace settings when built | owner, admin |
  | `ownership` | making or unmaking an owner, checked by the members routes besides `settings` | owner |

  Read cell by cell: the editor's charts are "create, edit, publish", so archiving is not theirs; the operator's seats are "block, release, book, assign channels", so holding is not theirs (a box-office sale books directly, 09 §4.2), while viewing the events they act on is; the viewer's "View, reports" gives reports to the viewer and not, as written, to the editor or the operator. Validate is `charts.write`: it compiles, which only those who can publish need. A test lists every route's permission by hand (`every_route_declares_its_permission`) and another the role table bit by bit (`the_role_table_is_09s`).
- **Cross-origin.** No route answers another origin with credentials: CORS is only on `/v1/public/`, never with `Access-Control-Allow-Credentials`, and those routes take no cookie. In development the console's dev server proxies `/console/api` and `/v1` to the server (09 §9), so everything is one origin.

Session tokens (`DECISIONS.md` D-34):

- **What it is.** A compact JWS (RFC 7515): `base64url(header) "." base64url(payload) "." base64url(signature)`, base64url without padding, header and payload JSON. The customer's backend mints one when its buyer opens the seat picker, and the browser sends it with every public hold call.
- **Header.** `alg` exactly `"HS256"`; `kid` the id (`key_…`) of a secret key of the workspace; `typ` and other fields are ignored, except `crit`, which is refused (no extension is understood).
- **Signing key.** HMAC-SHA-256 keyed with the secret key, as every JWT library does with an HS256 secret: `HMAC(sk_…, header "." payload)`. A secret key is 67 bytes, longer than SHA-256's 64-byte block, so HMAC hashes it first (RFC 2104 §2): signing with the key text is exactly signing with its 32-byte SHA-256, which is what the server stores (`api_keys.hash`) and verifies with. A backend may hold either. Signing with the SHA-256 as 64 hex characters is a different key and fails. The key must be live: a revoked key's tokens stop verifying at once; rotation is signing with the new key's id.
- **Claims.** `sub` (required): the customer's identifier for the buyer, 1 to 128 characters, opaque to Cavea. `exp` (required): seconds since the Unix epoch, a whole number; accepted until 30 s after it by the server clock, for clock skew. `event` (required): the event key, exactly. `max_objects` (optional): a positive integer; a hold token created with the session holds at most the lower of it and the event's `max_objects`. `channels` (optional): a list of channel keys, each 1 to 128 characters; it is checked for shape and has no effect until channels exist (02 §7), when it will be the only source of a buyer's channels. Unknown claims (`iat`, `nbf`, `jti`, …) are ignored, a repeated claim is refused. The whole token is at most 1,024 bytes.
- **Verification.** The token is split and decoded; the header must name `HS256` and a `kid`; the key is looked up among the live secret keys of the publishable key's workspace (never a workspace the token names); one HMAC-SHA-256 is computed (with a dummy key if there is none, so the work is the same) and compared in constant time; then the claims are checked. `none` and every other algorithm fail by construction: no other code path exists.
- **Refusals** are `401 unauthorized`, `reason: "session_token"`, with `details.session`: `missing` (the event requires one and none was sent), `invalid` (anything wrong up to and including the signature: structure, base64, JSON, algorithm, an unknown, revoked, publishable or other workspace's `kid`, a bad signature; or, once signed, claims of the wrong shape), `expired` and `event`. `expired` and `event` are told only for a correctly signed token, so they help the integrator, who signed it; nobody else learns whether a `kid` exists, or anything about a token they could not sign.
- **Carriers.** The `Cavea-Session` request header on `POST /v1/public/hold-tokens`, `POST /v1/public/events/{event}/hold` and `…/release`, and SUBSCRIBE's session field on the WebSocket (§8.2). Never a URL: SSE, the snapshot and polling take none.
- **`require_session`.** With it, those three routes refuse a request without a valid session token (`401`). Without it, a session token sent is still verified (an invalid one is `401`, not ignored) and still binds the hold token it creates. Secret-key routes never take one.
- **What realtime requires.** Nothing: no transport (WebSocket, SSE, snapshot, polling) requires or reads a session token. A public stream reveals only which seats are unavailable (D-12), the snapshot is cached by URL at a CDN (D-31) and cannot depend on a header, and the purpose of session tokens is to stop anonymous holding, which the hold routes enforce. PRIVATE frames follow the hold token alone, as `hold-tokens/current` does.
- **The `sub` binding** (12 §3 T2). A hold token created with a session is bound to the session's `sub`: hold and release with it are refused (`403`, `reason: "session"`) without a session token of the same `sub`. A token created without one (by the backend, or on an event without sessions) is bound to nothing and may be used with any valid session. The engine's `CreateHold` does not carry the `sub`; the server writes it: after the actor has committed the hold and before the token is returned, it sets `holds.session_sub` in its own transaction, and it never changes. Nobody can use the token in between, because only the server has seen it. If the binding fails, the token is never returned; the hold holds nothing and expires. The binding is read before each public hold or release, without a race, since it is written before the token exists for anyone and never changes.
- **A cap per `sub`** (12 §3 T1). One `sub` may have at most the event's `max_tokens_per_sub` live (unexpired) public hold tokens on an event, default 2, so that a buyer cannot multiply `max_objects` by minting tokens; 2 lets a buyer keep a second tab. Over it, creation is `422 hold_limit_exceeded` with `reason: "tokens_per_sub"` and `limit`. The binding enforces it: it takes a PostgreSQL advisory transaction lock on (event, `sub`), counts the `sub`'s live tokens in the next statement, and binds only below the cap, so of two creations at once the second waits and counts the first. A count before the hold is created refuses a buyer already at the cap without making the actor create a hold for nothing; the binding decides.
- **Personal data** (12 §5). The `sub` is never logged and never stored: `holds.session_sub` keeps its SHA-256, until the hold row is deleted (when the hold expires, or when booking takes its last objects). It is not in the event log, webhooks or any response.

A worked example (fake key): the secret key `sk_abab…ab` (`sk_` and 32 times `ab`) has the id `key_0123456789abcdef0123456789abcdef`. The backend builds

```
header  {"alg":"HS256","typ":"JWT","kid":"key_0123456789abcdef0123456789abcdef"}
payload {"sub":"buyer-42","event":"hamlet-2027-03-14","exp":4102444800,"max_objects":4}
```

encodes each as base64url without padding, joins them with `.`, and appends `.` and the base64url HMAC-SHA-256 of that text keyed with the secret key (or its SHA-256):

```
eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCIsImtpZCI6ImtleV8wMTIzNDU2Nzg5YWJjZGVmMDEyMzQ1Njc4OWFiY2RlZiJ9
.eyJzdWIiOiJidXllci00MiIsImV2ZW50IjoiaGFtbGV0LTIwMjctMDMtMTQiLCJleHAiOjQxMDI0NDQ4MDAsIm1heF9vYmplY3RzIjo0fQ
.S1xq35OM5lNCnx_kOyT1bTEFtIPFnwM1i0V0Eg1Bxks
```

(one line, without the breaks). Any JWT library does the same given the key as an HS256 secret and the `kid` header; in Python's standard library it is `hmac.new(secret_key.encode(), signing_input.encode(), hashlib.sha256)`. The server's test suite verifies this exact token. The browser then sends it as `Cavea-Session: eyJ…` with hold-token creation, hold and release.

CORS (public routes):

- Every path under `/v1/public/` answers browsers across origins; the compiled chart files and assets keep their own open policy (`Access-Control-Allow-Origin: *` whatever the setting, so one file is one CDN entry). Secret-key, admin and every other route carry no CORS header at all (12 §4), and `OPTIONS` on them is `404`.
- `CAVEA_CORS_ORIGINS` is the outer gate, enforced by the browser: `*` (default) answers `Access-Control-Allow-Origin: *`; a list answers with the request's `Origin` if it is on the list (compared without regard to case), and with no CORS headers otherwise, plus `Vary: Origin` on every public response.
- Preflight (`OPTIONS`) carries no credential, so it is answered from the setting alone: `204` with `Access-Control-Allow-Methods: GET, POST`, `Access-Control-Allow-Headers: Authorization, Content-Type, Cavea-Hold-Token, Cavea-Request-Id, Cavea-Session` and `Access-Control-Max-Age: 7200`. Other responses, errors included, carry `Access-Control-Expose-Headers: Cavea-Request-Id, Retry-After`, so the renderer can read every problem+json and its request id.
- The key's allow-list is the inner gate, checked on the request itself: an origin the setting admits but the key does not is `403` (`reason: "origin"`), with the CORS headers, so the page sees why. An origin must pass both. With a list in the setting, the key's list can only narrow it.
- Credentials mode is off: no `Access-Control-Allow-Credentials`, no cookies; the key goes in `Authorization`, which is why every request with it is preflighted, except on the realtime routes, which take it in the `key` query parameter (§6.2, D-31) and are simple requests.
- WebSockets are not subject to CORS: the upgrade checks `Origin` against both `CAVEA_CORS_ORIGINS` (a list) and the key's allow-list itself, and refuses with `403` (§8.2).

Admin key and workspaces:

- `CAVEA_ADMIN_KEY` has no default. Set, it must be 32 to 512 printable ASCII characters or the server will not start; unset, the server starts with a warning and the admin endpoints refuse every request (`401`, or `403` for a valid API key, as on any admin route) (`DECISIONS.md` D-24). The presented token is hashed and the digests are compared in constant time. The admin key is checked before API keys; it is never logged.
- `POST /v1/workspaces` creates the workspace and its first secret key in one statement and returns the key once. From there the workspace manages its own keys.
- `POST /v1/workspaces/{workspace}/keys` mints another secret key for an existing workspace and returns it once: the operator's recovery for a workspace whose keys are all lost.

## 12. Rate limiting and overload

- **Public endpoints:** a token bucket per (publishable key, client address, budget), checked once the key is found and its origin allowed, before the handler runs. Two budgets: **holds**, every public `POST` (hold-token creation, hold, release; best-available when it exists), default 120 a minute (`CAVEA_RATE_LIMIT_HOLDS`); **reads**, every other request with a publishable key (event info, the snapshot and polling, `hold-tokens/current`, and each WebSocket or SSE connection attempt), default 3,000 a minute (`CAVEA_RATE_LIMIT_READS`). A bucket at rest admits a minute's worth at once, then one every 60 s divided by the rate (GCRA). Over the limit: `429 rate_limited`, problem+json with the CORS headers and `Retry-After` in whole seconds (at least 1) until a request would be admitted; a refused request takes nothing. 0 turns a budget off. The compiled chart files take no key and are not limited; an open WebSocket or SSE stream is not counted again (its frames are bounded by §8.3, open connections per address by `CAVEA_RT_MAX_PER_IP`).
- **Secret-key and admin endpoints:** not limited.
- **Console sign-in** (12 §3 T9, D-51). Sign-in, setup and password-change attempts draw on a third budget per client address alone, **sign-in**, default 10 a minute (`CAVEA_RATE_LIMIT_SIGN_IN`; `429`, `reason: "sign_in"`). Behind it, a back-off in memory (`console::guard`): after 5 failures in a row for an (account, address) pair, the next attempt from there waits 1 s, then 2, 4, … up to 15 minutes; after 20 failures on an account from anywhere, addresses that have not signed in to it within 30 days wait the same way. A waiting attempt is `429`, `reason: "backoff"`, with `Retry-After`, and checks nothing. Failures are forgotten an hour after the last; a success clears its pair. Every layer is keyed alike whether or not the account exists. Nothing locks a member out for longer than an attacker keeps going, and never from an address they have signed in from. Per node, like every limit here.
- **Memory and cost.** Buckets are in memory, in 16 maps under their own locks, never held across an await. Each bucket is one time (40 bytes with its key); a map holds at most 14,336, in 16,384 slots, so the node's buckets take about 11 MiB at most. A bucket that has refilled carries nothing: when a map is full, its refilled buckets are dropped (at most once a second), and if it is still full, a request it has no room to count is admitted. Under a flood of addresses the limiter fails open rather than refuse buyers it cannot count; the connection limits and the actor's mailbox (`503 overloaded`) still hold. A check costs about 60 ns on one thread, and 8 threads together make 22 million a second (measured locally, AMD Ryzen 9 7900X, release build); it allocates only when a new bucket is added.
- **Client address.** The socket's peer, unless the peer is in `CAVEA_TRUSTED_PROXIES` (addresses or CIDR ranges, IPv4 or IPv6; none by default). Behind a trusted peer, `X-Forwarded-For` (all its lines in order, as one list) is read from the right, skipping trusted entries: the first untrusted address is the client. A malformed entry ends the walk at the nearest trusted hop, which is then counted as the client, so a broken chain falls on the proxy rather than being believed; if every entry is trusted, the leftmost is the client. Entries may be bare addresses, `[v6]`, `[v6]:port` or `v4:port`. From any other peer the header is ignored, so a client cannot choose its address. `Forwarded` (RFC 7239) is not read. An IPv4-mapped IPv6 address is the IPv4 address. An IPv6 client is counted by its /64, since one subscriber is routinely given a whole /64, an IPv4 client by its address. The same address counts realtime connections (§8.2).
- **Behind a proxy or CDN.** A load balancer, ingress or CDN in front of the server must be listed in `CAVEA_TRUSTED_PROXIES` and must append the address it saw to `X-Forwarded-For`, as the common ones do. If it is not listed, every buyer behind it shares the proxy's buckets: 120 holds a minute for the whole audience, and `429`s from the first minute of an on-sale. That is how a missing setting shows itself; the server does not try to detect it. A CDN that serves the snapshot and polling (§8.5) forwards its misses from its own addresses: list its published ranges, or set `CAVEA_RATE_LIMIT_READS=0` and leave reads to the CDN. A deployment behind the customer's own rate-limiting gateway may set both budgets to 0.
- Mailbox full → `503 overloaded` with `Retry-After`.
- Database connection pool exhausted → `503 unavailable`.
- Limits are per node in v1: N nodes behind a balancer admit up to N times the rate.

## 13. Configuration

| Variable | Default | Meaning |
|---|---|---|
| `CAVEA_DATABASE_URL` | — | PostgreSQL connection string (required) |
| `CAVEA_LISTEN` | `0.0.0.0:8080` | |
| `CAVEA_LOG` | `info` | Log filter, in `tracing` directive syntax such as `info,cavea_server=debug`. Records go to standard output through a queue of 16,384 written by a thread of its own; a record the queue cannot take or the output cannot write is dropped and counted (§15), never retried or reported on standard error |
| `CAVEA_LOG_FORMAT` | `json` | `json` (one object per line, for production) or `text` (readable, for development) |
| `CAVEA_PUBLIC_URL` | — | External base URL such as `https://seats.example.com` (`http://` or `https://`, kept without a trailing slash). The first-run setup link starts with it (09 §2); without it the link is a path. The server warns at start when it is not HTTPS. Later: webhooks |
| `CAVEA_LICENCE` | — | Licence file path or contents |
| `CAVEA_ADMIN_KEY` | none | The operator's key for `/v1/workspaces`: 32 to 512 printable ASCII characters. Unset, those endpoints refuse every request (§11) |
| `CAVEA_DB_POOL` | 16 | Connections to PostgreSQL, all opened at start, before the server listens, and kept open, so the first burst after a start or a quiet night opens none (13 §7.4). The first must open or the start fails; any the database refuses beyond it are opened later, with a warning. Every 10 minutes the pool retires idle connections older than 10 minutes and opens back up to this number in the background; a request that finds none idle opens one. Size it within the database's `max_connections`, counting a second process during a rolling restart: a 1 vCPU node beside a small managed PostgreSQL does well with 4 |
| `CAVEA_DELTA_INTERVAL_MS` | 100 | Not read yet: the engine's interval is fixed at 100 ms (§8.3) |
| `CAVEA_SNAPSHOT_MAX_AGE` | 1 | Seconds a shared cache may keep the public snapshot (`max-age`, §8.5) |
| `CAVEA_RT_MAX_CONNECTIONS` | 60000 | Realtime connections the node holds open at most; beyond, the upgrade is `503 overloaded` (§8.2) |
| `CAVEA_RT_MAX_PER_IP` | 200 | Realtime connections one client address (§12) may hold open; beyond, `429 rate_limited`. High enough for buyers behind one carrier or office NAT |
| `CAVEA_TRUSTED_PROXIES` | none | Comma-separated addresses and CIDR ranges, such as `10.0.0.0/8,2001:db8::/32`, whose `X-Forwarded-For` names the client (§12) |
| `CAVEA_RATE_LIMIT_HOLDS` | 120 | Public hold-token creations, holds and releases per minute per publishable key and client address; 0 is unlimited (§12) |
| `CAVEA_RATE_LIMIT_READS` | 3000 | Other public requests per minute per publishable key and client address, realtime connection attempts included; 0 is unlimited (§12) |
| `CAVEA_RATE_LIMIT_SIGN_IN` | 10 | Console sign-in, setup and password-change attempts per minute per client address; 0 is unlimited (§12) |
| `CAVEA_CONSOLE_COOKIE_SECURE` | `true` | `false` drops `Secure` and the `__Host-` prefix from the console session cookie (§11), for a development server over plain HTTP reached by a host name; browsers treat `http://localhost` as secure, so local development keeps the default |
| `CAVEA_CORS_ORIGINS` | `*` | Origins the public endpoints answer across: `*`, or a comma-separated list such as `https://tickets.example.com,http://localhost:5173` (§11) |
| `CAVEA_LOG_RETENTION_DAYS` | 400 | `event_log` partitions older than this are dropped |
| `CAVEA_ASSET_MAX_BYTES` | 10485760 | The largest image `POST /v1/assets` takes, in bytes (10 MiB); at most 256 MiB, since an upload is held in memory whole before it is stored. Four times it is the node's budget of image bytes in memory for uploads, and again for serving (§6.2) |
| `CAVEA_OBJECT_STORE_URL` | none | Not read yet (SRV-054): optional S3-compatible storage for assets, which are stored in PostgreSQL |
| `CAVEA_OTLP_ENDPOINT` | none | Trace export |
| `CAVEA_NODE_ID` | hostname | This node's name in `events.owner_node`; nodes on one database must differ |
| `CAVEA_LEASE_SECS` | 15 | How long ownership of an event lasts unless renewed |
| `CAVEA_HEARTBEAT_SECS` | 5 | How long an idle actor waits before renewing its lease; less than the lease |

## 14. Deployment

- **Artefacts:** a binary for Linux (x86-64 and arm64, glibc 2.34 or newer) and a distroless Docker image for both (D-57). As built (SRV-023): `deploy/Dockerfile` builds everything from the repository at its pinned versions (Rust from `rust-toolchain.toml`, bun from `package.json`, binaryen 132 as CI) on the build machine's platform and cross-compiles the server for the target's with Debian's cross GCC (either way: an arm64 build machine makes the x86-64 image the same way), so neither needs emulation; the image is `gcr.io/distroless/cc-debian12:nonroot` with the binary (console included), about 46 MB, running as `nonroot` (65532) with no shell, `CAVEA_LISTEN=0.0.0.0:8080` inside the container, `EXPOSE 8080`, `STOPSIGNAL SIGTERM` and a `HEALTHCHECK` running `cavea-server probe` (`GET /readyz` on the container's own port). It writes nothing to disk, so it runs with a read-only root filesystem. Docker's default stop timeout is 10 s: give 30 s (`docker stop -t 30`, compose `stop_grace_period`). Not a static (musl) binary: ring's C code would need musl cross compilers the build image does not have, and musl's allocator, slow under contention, would need a replacement allocator crate on the hold path.
- **Minimum:** 1 vCPU, 512 MB, PostgreSQL 15. **Recommended for on-sales:** 4 vCPU, 8 GB, PostgreSQL on fast storage in the same network.
- **PostgreSQL:** `synchronous_commit = on`. Backups and point-in-time recovery are the customer's responsibility; the docs give a recipe.
- **TLS:** terminate at the customer's proxy or load balancer. WebSocket upgrade must be allowed.
- **Shutdown:** on SIGTERM, stop accepting commands, flush pending batches, send CLOSE to subscribers, release leases, exit within 20 s. First every realtime connection is sent CLOSE 3 and closed (a CLOSE that cannot be written within 1 s is dropped; a connection whose write is blocked by a client that does not read, or that has not sent SUBSCRIBE yet, ends at once), so that long-lived connections never hold the drain open or eat into the actors' time; clients reconnect with backoff to another node or the restarted one and resume from their `seq`. The server stops accepting connections and gives requests in flight up to 10 s to finish (their commands reach their actors); then the registry stops every actor, each committing what it accepted and releasing its lease, within what is left of the 20 s. Only after that does the server wait, until the deadline, for realtime connections still open: an SSE response whose client has stopped reading keeps its socket, and its slot, until the client reads (§8.4), and must not take the actors' time. `GET /readyz` answers `503` from the signal on. At exit the log output gets 250 ms more to write what is queued.
- **Upgrade:** replace the image and restart. Migrations run automatically; a migration that cannot run online is called out in the release notes.
- **Reference files:** `deploy/compose.server.yaml` (the image, its own PostgreSQL with `synchronous_commit = on` and a named volume, health checks, the server after the database is healthy, a read-only root filesystem, a 30 s stop grace period, the port on 127.0.0.1, `POSTGRES_PASSWORD` required, of URL-safe characters since it goes into `CAVEA_DATABASE_URL` as it is, and no admin key unless one is set); `deploy/smoke.ts`, the image's end-to-end check (13 §13); `deploy/compose.yaml` is only the development database. Still to do: `deploy/helm/` and a systemd unit.

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

More metrics, for what the table does not cover: `cavea_actor_mailbox_wait_seconds` (histogram: from a command's submission until its actor takes it), `cavea_rt_connections` (gauge: open realtime connections, subscribed or not), `cavea_rt_frames_total`, `cavea_rt_publications_total`, `cavea_rt_fanout_seconds` (histogram: from a publication until nine in ten of the subscribers it went to have taken it), `cavea_rate_limited_total{budget}` (`holds`, `reads`, `sign_in`), `cavea_console_sign_ins_total{result}` (`ok`, `credentials`: a wrong email or password, `backoff`: refused unchecked), `cavea_db_pool_connections` (gauge: open, in use or idle), `cavea_log_records_dropped_total`. The webhook metrics wait for webhooks (SRV-032).

- **Labels** are bounded: `op` is one of the engine's seven, `result` `ok` or one of its eleven error codes (§6.4), `route` the matched template or `<unmatched>`, `status` the HTTP status, `transport` `websocket` or `sse`, `budget` `holds`, `reads` or `sign_in`, `result` of a sign-in one of three. No key, token, workspace, event, client address or raw path is ever a label.
- **Histograms** have fixed buckets: durations at 0.5, 1, 2.5, 5, 10, 15, 25, 50, 100, 250 and 500 ms and 1, 2.5, 5 and 10 s (15 ms is the hold budget, 01 §5); `cavea_commit_batch_size` at 1, 2, 5, 10, 20, 50, 100, 200, 500, 1,000 and 2,000 commands. `cavea_command_duration_seconds{op}` runs from submission to the reply, after the commit; with the mailbox wait and the commit histograms it gives a command's stages (the apply itself is microseconds). `cavea_http_duration_seconds` ends when the response starts, so a stream's lifetime is not in it.
- **Cost.** Recording is a few relaxed atomic additions and at most three clock readings per command, and never locks, allocates or fails. Against PostgreSQL the actor's throughput was the same with and without it (45,700 and 46,700 commands a second); on the in-memory store, where every command is its own batch, it adds about 0.1 to 0.2 µs to the 0.93 µs a command took (measured locally, 13 §7.4).
- **Who may read them.** `GET /metrics` takes the admin key (`Authorization: Bearer`, as `/v1/workspaces`; `401`, `403` otherwise; unset, it refuses every request): its figures describe every workspace on the node. Prometheus sends it with `authorization: {credentials: …}` in its scrape configuration. `GET /healthz` and `GET /readyz` take no credential: probes cannot send one, and they say nothing about any workspace. Answers are `text/plain; version=0.0.4` and `Cache-Control: no-store`.

Health:

- `GET /healthz` answers `ok` while the process serves HTTP: liveness.
- `GET /readyz` answers `ready`, or `503` with `database unreachable` or `shutting down`: readiness. The database is probed by one `SELECT 1` through the pool every second (failing after 2 s); `/readyz` reads that result and the shutdown flag, so probing it costs no query and never reaches an actor. Migrations run before the server listens, so a server that answers has applied them; preloading does not delay readiness (§4.7). A pool so busy that the probe waits 2 s for a connection also reads as not ready.

Tracing: one span per request, with child spans for mailbox wait, apply and commit. Logs are JSON with request id, workspace and event.

```
request {request_id, method, route, status, duration_ms, workspace, event, event_id, commit_seq}
├─ mailbox_wait {op}     queued in the actor's mailbox
├─ apply {op}            on the actor's task, parented to the request explicitly
└─ db {statement}        a database call made by the handler
commit {event_id, seq, commands}   one per batch; follows_from each apply span in it
actor {event_id}                   one per loaded event: its load, actor and lease release
└─ heartbeat                       an idle lease renewal
realtime {request_id, transport, workspace, event}   one per realtime connection
```

- `route` is the matched template (`/v1/events/{event_key}`), or `<unmatched>`; the raw path is never logged because it carries customer keys. Query strings, headers and bodies are never logged.
- Each request logs one `request completed` record when it finishes: `info`, or `debug` for a successful `/healthz`, `/readyz` or `/metrics`.
- A batch serves many requests, so `commit` is not a child of any one of them. Its `follows_from` links lead to each `apply` span and through it to the request id, for trace export. In logs, the engine records the batch's `event_id` and `seq` (as `commit_seq`) on the span that submitted each command (the request span, when the handler submits from it), so a record inside `commit {event_id: E, seq: 42}` and the `request completed` records with `event_id: E, commit_seq: 42` belong together. `seq` alone is per event and does not identify a commit. `event_id` is the event's UUID as 32 hex digits; `event` is the customer's event key.
- A JSON record lists every enclosing span with its fields under `spans`, outermost first, so a record inside `apply` carries the request id.
- A realtime connection outlives its upgrade request, whose `request completed` record (`101`) is logged when the connection opens. Its own `realtime` span carries that request's id and logs two records at `info`: `realtime subscribed` (`last_seq`, `private`) and `realtime connection closed` (`duration_ms`, `frames`, `resyncs`, `reason`: `client_closed`, `idle`, `stalled`, `bad_frame`, `shutdown`, `unavailable`, `handshake_timeout`, `unsupported_version`, `not_found`, `archived`). Nothing is logged per frame at `info`, and no frame content, key or token ever.

## 16. Failure modes

| Failure | Effect | Recovery |
|---|---|---|
| PostgreSQL unreachable at start | The migrations' connection fails and the process exits with an error before it listens, so nothing is half up (the pool, opened after them, waits up to 5 s for its first connection, then does the same). A container platform restarts it (compose: `restart: unless-stopped`, after the database is healthy) | Automatic once the database answers |
| PostgreSQL unreachable | Commands fail with 503. An actor stops at its next failed commit or heartbeat (within 5 s, §4.8), ending its subscriptions; its event cannot be reloaded, so its snapshot and realtime are unavailable until the database returns. `/readyz` answers `503` within about a second | Automatic when the database returns |
| Log output fails or stalls (disk full, closed pipe, a stopped collector) | Records are dropped and counted in `cavea_log_records_dropped_total`; requests, actors and shutdown carry on. Standard input, output or error closed at start are held open on `/dev/null`, so the server's sockets never take their descriptors | Fix the output; logging resumes by itself |
| Fanning out saturates the cores (tens of thousands of subscribers) | Delta delivery slows and publications coalesce over up to 1 s (§8.3); commands, commits and snapshots are not delayed | None needed; fewer subscribers per node, or more cores |
| A handler, extractor or layer panics (a bug) | That request is answered `500 internal_error` with its request id; the panic is logged once, at `error`, in the request's span (`panic`, with its source location; its message only when it is a string literal, since a formatted one may quote the request), and counted in `cavea_http_requests_total{route, status="500"}`. The connection stays open and every other request is served: the locks requests share recover from poisoning | Fix the bug |
| An event actor panics | Its callers are answered `503 unavailable`; the panic is logged in the actor's span; the actor is dropped without releasing its lease | The next command for the event reloads it on this node (§4.7) |
| A burst of console sign-ins | Passwords are hashed on blocking threads, two at a time per node (12 §3 T9); a request waits up to 5 s for its turn, then `503 overloaded` (`reason: "password_hashing"`). Holds and other requests are not delayed (13 §7.4) | None needed |
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
