# Realtime stream

Every open seat map shows seats as others take them, without reloading. This page explains how, what it carries, and what it does not.

## How a seat map stays current

When the seat map starts, `caveaServer` reads the event's public information (its chart version and settings) and then opens a realtime connection to the server for its seat state. The connection first delivers a **snapshot**: the state of every seat at one [sequence number](events.md#state-and-counts). After that it delivers **deltas**: the seats that changed since, gathered over a short interval so that a burst of sales arrives as one update.

`caveaServer` picks the transport by itself, in this order:

| Transport | Route | When |
|---|---|---|
| WebSocket | `GET /v1/public/rt` | By default |
| Server-sent events | `GET /v1/public/events/{event}/stream` | When a WebSocket cannot connect, for example behind a proxy that refuses them |
| Polling | `GET /v1/public/events/{event}/state` | When neither works |

The realtime routes take the publishable key in the `key` query parameter, since browsers cannot send headers on these connections. They never take a session token.

A connection that drops reconnects on its own and catches up: with the changes it missed, or with a new snapshot if it fell too far behind. By default the seat map closes its connection while the page is hidden and reopens it when the page is shown again.

## What it carries

The public stream says only whether each seat is **available** or **unavailable**: a buyer cannot tell held from sold, or learn who holds what. A buyer's own held seats and their expiry come privately, on their own connection, to the hold token's holder.

Staff see real statuses: the console's event manager reads the staff state route ([`GET /v1/events/{event}/state`](../api/events.md#eventState)) with the member's session.

The frames are a compact binary format that the seat map decodes; you do not need to handle it unless you write a renderer of your own. The snapshot route is cacheable for a second by default (`CAVEA_SNAPSHOT_MAX_AGE`), so a CDN in front of the server can absorb a crowd polling at once.

## Limits

One client address may hold 200 realtime connections at once by default, and a server 60,000; connection attempts count against the publishable key's read rate limit. See [Limits](../reference/limits.md#rate-limits).
