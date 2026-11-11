# 02 · Domain model

**Status:** Draft v0.1 · 2026-11-10

This document fixes the vocabulary. Every other spec, the code, the API and the docs use these words with these meanings.

## 1. Glossary

| Term | Meaning |
|---|---|
| **Workspace** | An isolated tenant: its own charts, events, keys and settings. A platform typically creates one workspace per organiser |
| **Chart** | A venue layout. Has a mutable draft and zero or more immutable published versions |
| **Chart document** | The editable JSON source of a chart (`03-chart-format.md` §2) |
| **Chart version** | An immutable, compiled snapshot of a chart, identified by a content hash |
| **CCF** | Compiled Chart Format: the binary file the renderer loads |
| **Floor** | A level of a venue (stalls, balcony). Every object belongs to one floor |
| **Section** | A named group of objects with an outline, such as "Orchestra Left". Used for overview display, drill-down and filtering |
| **Row** | An ordered line of seats with a label |
| **Row segment** | A maximal run of physically adjacent seats in a row. Aisles and gaps split a row into segments |
| **Object** | Anything bookable: a seat, a table booked whole, a booth, a general-admission area |
| **Seat** | A single place in a row or at a table |
| **Table** | A shape with seats around it. Booked either seat by seat or as a whole |
| **Booth** | A single bookable object that is not a seat (a box, a stand) |
| **Area** | A general-admission zone with a capacity. Booked by quantity |
| **Category** | A pricing and colour group (VIP, Standard). Every object has one |
| **Ticket type** | A price variant within a category (adult, child). Supplied by the host at render time; Cavea stores only the chosen type's key |
| **Focal point** | The point buyers want to face (the stage). Drives seat quality ranking |
| **Event** | One sellable occurrence. Pinned to one chart version. Owns the seat state |
| **Status** | The state of an object in an event: free, held, booked, blocked, or a custom status |
| **Hold** | A temporary reservation of objects, owned by a hold token, with an expiry |
| **Hold token** | An opaque secret identifying a buyer's hold. Whoever presents it controls that hold |
| **Booking** | The durable sale of objects, tagged with the customer's order reference |
| **Channel** | A named allocation of objects (box office, presale, partner). An object in a channel can only be taken by a request that names that channel |
| **Best-available** | A server operation that chooses and holds the best *n* objects |
| **Orphan** | A single free seat left between unavailable seats or a segment edge |
| **Snapshot** | The full public state of an event at one sequence number |
| **Delta** | The state changes between two sequence numbers |
| **Sequence number (`seq`)** | A per-event counter that increases by one for every committed command |
| **Secret key** | A server-side credential with full power over a workspace |
| **Publishable key** | A browser-safe credential: read public state, create a hold token, hold and release with that token |

## 2. Entity relationships

```
Workspace 1─* Chart 1─1 Draft
                    1─* ChartVersion  (immutable, compiled → CCF)
Workspace 1─* Event *─1 ChartVersion   (pinned)
Event 1─* ObjectState   (only objects that are not in the default state)
Event 1─* Hold 1─* ObjectState
Event 1─* Channel
Event 1─* LogEntry      (append-only, ordered by seq)
ChartVersion 1─* Floor 1─* Section 1─* Row 1─* Seat
ChartVersion 1─* Category
```

## 3. Identifiers

An object has three identifiers. Each has one job.

| Identifier | Type | Scope | Stable across publishes? | Used for |
|---|---|---|---|---|
| **`oid`** | u32 | Chart | Yes. Allocated when the object is created and never reused | Database rows, mapping state when an event moves to a new chart version |
| **`idx`** | u32 | One chart version | No. Dense, 0 to N−1 | Arrays, bitmaps, wire deltas |
| **`label`** | String, ≤ 64 bytes | Chart | Only if nobody renames it | The REST API and humans: `Orchestra-C-12` |

Rules:

- `idx` order is: all seats, grouped by floor, then section, then row, in row order; then tables booked whole, booths, areas.
- A label is composed from a chart-level template, by default `{section}-{row}-{seat}`. Labels must be unique within a chart version; the compiler rejects duplicates.
- The API accepts labels. Responses include both `label` and `oid`. Bookings store both, so a later rename does not lose history.
- Customers choose their own keys for charts and events (`chart_key`, `event_key`): strings of up to 128 characters, unique in a workspace.
- Server-generated identifiers are prefixed: `ws_` workspace, `cv_` chart version, `ht_` hold token, `sk_` secret key, `pk_` publishable key, `key_` API key id (the id is not the key), `wh_` webhook endpoint, `req_` request.

## 4. Statuses

### 4.1 Domain statuses

| Code | Status | Meaning | Sellable |
|---|---|---|---|
| 0 | `free` | Default. No row is stored | Yes |
| 1 | `held` | Temporarily reserved under a hold token | No |
| 2 | `booked` | Sold. Carries an order reference | No |
| 3 | `blocked` | Taken off sale by staff | No |
| 4–15 | Custom | Workspace-defined (for example `house`, `press`) | No |

### 4.2 What the public sees

The public snapshot reduces every status to one of two codes, so that a buyer cannot tell a held seat from a sold one:

| Public code | Meaning |
|---|---|
| 0 | Available to this viewer |
| 1 | Unavailable |

An object in a channel is "available" only to a viewer whose session names that channel. A buyer's own holds are delivered privately on their connection.

Manager connections (authenticated staff) receive the domain statuses, hold expiry and order references.

### 4.3 Transitions

| From | To | Command | Who may issue it | Conditions |
|---|---|---|---|---|
| free | held | `hold` | Publishable or secret key | Valid, unexpired hold token; under the token's object limit; channel allows; orphan policy allows |
| held | free | `release` | Holder of the token, or secret key | — |
| held | free | (expiry) | Server | Token expired |
| held | booked | `book` with hold token | Secret key | Token valid; all named objects held by it |
| free | booked | `book` without token | Secret key | Channel allows, unless overridden |
| booked | free | `release` | Secret key | — |
| free | blocked or custom | `change-status` | Secret key or console user | — |
| blocked or custom | free | `change-status` | Secret key or console user | — |
| held | held (new expiry) | `extend` | Secret key | New expiry within the workspace maximum |

Every command is **all-or-nothing**: if one object fails its condition, nothing changes and the response lists the failing objects with reasons.

Only the listed transitions exist. In particular blocked → a different custom status (or between two custom statuses) goes through free, and `change-status` never touches a held or booked object: setting blocked or custom requires each object to be free (`not_free`), setting free requires it to be blocked or custom (`not_blocked`). An object already in the requested status is a successful no-op. A release without a token (secret key) applies to booked objects only (`not_booked`); held objects are released with their token or by expiry.

The engine enforces the "Who may issue it" column itself: each command carries the caller's authority (publishable, or secret for a secret key or console user), and `extend`, `book`, release without a token and `change-status` from a publishable key are refused with `forbidden`. That makes INV-3 hold whatever caller is wired up. Everything that depends on how a key is configured stays with the caller: the per-token object limit, the workspace's longest hold, channels from session tokens, allowed origins and which custom codes the workspace defines.

## 5. Holds

- A hold token is created first, then used to hold objects. All objects under one token share its expiry.
- **Default lifetime:** 900 seconds from token creation. Configurable per workspace and per event between 60 and 7,200 seconds.
- **Object limit:** 10 per token for publishable keys by default, configurable up to 100. Secret keys are not limited.
- Holding an object you already hold is a successful no-op.
- When a token expires, all its objects return to `free` in one command, and a `hold.expired` webhook fires.
- A token can hold objects in more than one event only if the events belong to the same season (P2).
- The token is a bearer secret. The server stores only its SHA-256 hash.
- A token is expired from its expiry instant onwards (`now >= expires_at`, checked when the command is processed). Expiry is applied before any command processed at or after that instant: the objects return to `free` and the token is deleted, so a later request with it gets `not_found`. A token is also deleted when booking takes its last objects. A token whose objects were all released stays valid until it expires.

## 6. Bookings

- A booking sets objects to `booked` and stores `order_ref` (the customer's order identifier, 1 to 128 characters), an optional `ticket_type` key per object (1 to 128 characters; neither may contain U+0000, which PostgreSQL text cannot hold) and an optional `extra` JSON object (up to 2 KB per object).
- With a hold token, the booking takes the named objects, or all the token's objects if none are named; per-object details may name only objects being booked, each at most once.
- The recommended sequence is the one the incumbents document: hold, take payment, book with the hold token. If booking fails, refund or retry; never charge first and hold second.
- Booking is idempotent under an `Idempotency-Key` header. Replaying the same request returns the original result.
- Cavea does not know prices, buyers or payments.

## 7. Channels

- An event has zero or more channels, each with a key, a name and a colour.
- An object belongs to at most one channel per event.
- A hold or book request names the channel keys it may use. An object in a channel not named is unavailable to that request.
- Secret-key requests may pass `ignore_channels: true`.
- For publishable-key requests, allowed channels come from the signed session token, never from the request body.

## 8. General-admission areas

- An area has a capacity `C` in the chart, overridable per event.
- State is two counters: `booked_qty` and `held_qty`. Available is `C − booked_qty − held_qty`.
- Requests address an area by label and quantity: `{"label": "Standing", "quantity": 4}`.
- A hold token's quantity in an area counts towards its object limit.
- The public sees a bucketed availability by default (`available`, `limited`, `sold_out`). Exact counts are opt-in per event.

## 9. Tables

- `book_as: "seats"`: the seats are ordinary objects; the table shape is decoration.
- `book_as: "table"`: the table is one object; its seats are drawn but not individually bookable.
- Variable occupancy is not in v1.

## 10. Chart versions and events

- Publishing a draft creates a new immutable chart version. Nothing ever changes a published version.
- An event is pinned to exactly one chart version. Its state arrays are indexed by that version's `idx`.
- **Upgrading** an event to a newer version is an explicit command. The server maps state by `oid`. If a non-free object no longer exists in the new version, the upgrade is refused and the conflicts are listed.
- **Safe mode** in the designer (default when a chart has events with non-free objects) warns before any change that deletes or relabels such an object.

## 11. Seasons (P2)

- A season groups events that share a chart version.
- Booking an object in the season books it in every event of the season, atomically.
- The engine design in `08-server.md` §4 keeps this possible: a season is a second kind of actor that owns the lock order over its events.

## 12. Selection rules

These run in the renderer for instant feedback and again on the server as the authority. Both use the same code in `cavea-core` (`cavea_core::rules`: plain functions over counts; the orphan and consecutive rules will take the row segments and seat state). Hosts see a broken rule by its code: `max`, `min`.

| Rule | Definition |
|---|---|
| **Maximum selection** | At most *n* objects per hold token, optionally per ticket type |
| **Minimum selection** | At least *n* objects before checkout is allowed (renderer only) |
| **Consecutive seats** | All selected seats in a row form one unbroken run within one segment |
| **No orphans (strict)** | The selection must not create a new orphan |
| **No orphans (lenient)** | A new orphan is allowed only where it is unavoidable: the free run it sits in had exactly one more seat than the buyer took from it |

Orphan, precisely: after applying the selection, a free seat is an orphan if each neighbour within its row segment is either unavailable, selected, or absent (the segment edge). Whether a segment edge counts as a neighbour is a setting (`edge_counts`, default true). Seats that were already orphans before the selection are ignored.

## 13. Invariants

The engine must uphold these at every sequence number. Tests in `13-testing-benchmarks.md` §3 check all of them.

| ID | Invariant |
|---|---|
| INV-1 | An object has exactly one status |
| INV-2 | An object is `held` if and only if it is listed under exactly one unexpired hold |
| INV-3 | A booked object is never changed by a publishable key |
| INV-4 | For an area, `booked_qty + held_qty ≤ capacity` |
| INV-5 | `seq` increases by exactly one per committed command; no gaps, no repeats |
| INV-6 | Replaying the log from an empty state reproduces the current state |
| INV-7 | A command acknowledged to the caller is durable in PostgreSQL |
| INV-8 | A delta is never published for a command that did not commit |
| INV-9 | The public snapshot never distinguishes held from booked |
| INV-10 | Two different successful `book` commands never contain the same object unless a release sits between them in the log |
| INV-11 | At most one node writes an event's state at any time (enforced by a fencing epoch) |
| INV-12 | Published chart versions are immutable |

## 14. Limits

| Limit | Value |
|---|---|
| Bookable objects per chart | 1,000,000 (design limit; tested to 500,000) |
| Categories per chart | 4,095 |
| Floors per chart | 255 |
| Sections per chart | 65,535 |
| Seats per row | 4,095 |
| Label length | 64 bytes UTF-8 |
| Points per explicit section outline | 4,096 |
| Points in all explicit section outlines of a chart | 262,144 |
| `MESH` vertices per chart | 262,144 |
| `MESH` indices per chart | 2,097,152 |
| Shapes per chart | 65,535 |
| Points per shape, once flattened | 4,096 |
| Points in all shapes of a chart, once flattened | 262,144 |
| Objects per command | 1,000 (publishable: the token limit) |
| Custom statuses per workspace | 12 |
| Channels per event | 4,095 |
| `extra` per object | 2 KB |
