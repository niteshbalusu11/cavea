# Events

An event is one sellable performance: a concert on one night, one match. It owns the status of every seat for that performance. This page explains what an event keeps and which settings it has.

## An event keeps one chart version

An event is created on one [chart](charts.md) and keeps one version of it: the chart's published version, unless you name another. Publishing the chart again does not change the event. Every seat of that version starts **free** in the event.

An event has a **key** you choose, such as `opening-night` or the identifier your own system uses for the performance: 1 to 128 characters, unique in the workspace, fixed once the event exists. The API and the seat map name events by key. Its optional **name** is for people, such as "Hamlet, 14 March 2027".

## Settings

Each event has four settings, set when it is created and changeable later ([change an event](../api/events.md#updateEvent)). The console shows them in the event form.

| Setting (API field) | Console label | Default | What it does |
|---|---|---|---|
| `hold_ttl_seconds` | Hold lifetime (seconds) | 900 | How long a buyer's hold lasts before its seats are freed: 60 to 7,200 seconds. See [Holds](holds.md). |
| `max_objects` | Seats per hold token (seats) | 10 | The most seats one buyer's hold token may hold from a web page: 1 to 100. |
| `require_session` | Require a session token | off | Buyers' pages must send a [session token](session-tokens.md) your backend signed before they can hold seats. |
| `max_tokens_per_sub` | (API only) | 2 | How many live hold tokens one signed-in buyer may have on the event: 1 to 10. |

The hold lifetime and the seats per hold token are fixed when a hold token is created: a change applies to hold tokens created after it.

## State and counts

Reading an event ([get an event](../api/events.md#getEvent)) returns its version, settings and `counts`: how many seats are free, held, booked, blocked, and in each custom status. It also returns `seq`, the event's **sequence number**, which goes up by one with every change to its seats, so two reads with the same `seq` saw the same seats.

To read seats one by one, [list the event's objects](../api/events.md#listObjects), by label or by status. Staff see the whole event in the console's **event manager**: every seat's status, live counts, and actions to block, unblock, book and release seats.

## Archiving

Archiving an event makes it read-only: it takes no more seat operations, its holds run out, and its bookings stay. Archive events that are over. It cannot be undone in this version.

## In the API

[Events](../api/events.md): create, list, read, change and archive events, list their objects, and read their seat state.
