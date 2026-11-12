# Bring your own backend

This guide connects the seat map to seat state your own system keeps, without a Cavea server. By the end you will have a `customState` adapter that shows which seats are taken, keeps them current, and holds seats in your system when the buyer selects them.

Use this when your ticketing platform already decides who has which seat and you want only the map. Your backend then owns everything the Cavea server would do: holds, their expiry, bookings, and making sure two buyers cannot take one seat.

## What you need

- The compiled chart of your venue, a `.ccf` file, served from a URL your page can fetch. Draw the venue in the Cavea designer and publish it; the compiled file is what the server serves as the event's chart, or `cavea compile` makes it from the chart's document.
- The seats' **labels**, as the chart names them (`Stalls-C-12`). The adapter speaks to the map in labels, so your system must store seats by the same labels, or map its own ids to them.
- Three endpoints in your backend, which this guide assumes:

| Endpoint | Answers |
|---|---|
| `GET /api/events/{event}/seats` | `{ "unavailable": [labels], "mine": [labels], "expiresAt": ms or null }`: the seats the buyer cannot have, and the ones this buyer holds |
| `GET /api/events/{event}/changes` | Server-Sent Events, each `data` a JSON `{ "unavailable": [labels], "available": [labels] }` |
| `POST /api/events/{event}/holds`, `DELETE /api/events/{event}/holds` | Hold or release `{ "labels": [...] }` for this buyer's session; a refusal is `409` with `{ "failed": [{ "label", "reason" }] }` |

The shapes are yours to choose; these are the ones the adapter below reads.

## The adapter

```ts check=types
import { createSeatMap, customState, type HoldRefusal } from "{{renderer_package}}";
#hide declare const container: HTMLElement;

const event = "opening-night";
let changes: EventSource | null = null;

const state = customState({
  async connect(ctx) {
    const reply = await fetch(`/api/events/${event}/seats`, { signal: ctx.signal });
    if (!reply.ok) throw new Error(`seats: ${reply.status}`);
    const seats = (await reply.json()) as { unavailable: string[]; mine: string[]; expiresAt: number | null };

    // Every change after this snapshot, as label lists.
    changes = new EventSource(`/api/events/${event}/changes`);
    changes.onmessage = (message) => ctx.applyDelta(JSON.parse(message.data));
    changes.onerror = () => ctx.reportError(new Error("live seat updates interrupted"));

    return {
      kind: "map",
      unavailable: seats.unavailable,
      mine: seats.mine,
      ...(seats.expiresAt === null ? {} : { expiresAt: seats.expiresAt }),
    };
  },

  async hold(labels) {
    const reply = await fetch(`/api/events/${event}/holds`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ labels }),
    });
    if (reply.ok) return { ok: true };
    if (reply.status === 409) {
      const body = (await reply.json()) as { failed: HoldRefusal[] };
      return { ok: false, failed: body.failed };
    }
    return { ok: false, code: `http_${reply.status}` };
  },

  async release(labels) {
    const reply = await fetch(`/api/events/${event}/holds`, {
      method: "DELETE",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ labels }),
    });
    if (!reply.ok) throw new Error(`release: ${reply.status}`);
  },

  disconnect() {
    changes?.close();
  },
});

await createSeatMap(container, { chart: { url: `/charts/grand-hall.ccf` }, state });
```

What each part does:

1. **`connect`** fetches the first state and resolves with it. The map is ready once it is applied. Pass `ctx.signal` to `fetch`, so a map destroyed while loading cancels the request. If `connect` rejects, `createSeatMap` fails with `state_connect_failed` (after up to three retries for a failure that may pass), so it must leave nothing open when it fails: here the `EventSource` opens only after the snapshot has arrived.
2. **`ctx.applyDelta`** applies each change as it arrives. A label in both lists ends unavailable. A seat taken while the buyer has it selected leaves their selection (`onObjectUnavailable`).
3. **`hold`** is called when the buyer selects a seat, one call at a time, with the labels to hold. Answer `{ ok: true }` when all are held. A refusal is all or nothing: name the seats at fault in `failed` with a reason; seats you do not name are asked for again with the next hold. Use the reason `not_free` for a seat someone else has: the map then says it was taken. A `hold` that rejects or takes more than 20 s fails, and the seat is dropped.
4. **`release`** is called when the buyer deselects.
5. **`disconnect`** is called once, when the map is destroyed.

Without `hold` and `release`, the map runs in a manual session: taps select at once, nothing is held, and your page holds at checkout from `map.getSelection()`.

## The buyer's own holds and their expiry

`mine` in the snapshot is the seats this buyer holds: they stay selected, and are selected again after a reload. `expiresAt` is when those holds end, in milliseconds since 1970 on your server's clock.

When your backend expires the buyer's holds, tell the map: call `ctx.setMine([])`, and the seats leave the selection with `onHoldLost` and then `onHoldExpired`. When the holds are extended, call `ctx.setMine(labels, newExpiresAt)`. If your server's clock and the buyer's may differ, send your server's time with the snapshot and call `ctx.setClockOffset(serverNow - Date.now())`, so that `onHoldExpiring` fires on time.

## Keeping it consistent

The map trusts what the adapter says. Two buyers can only be kept from one seat by your backend: a hold must check and take the seat in one step (a database transaction or a conditional update), and a booking must check that the buyer's hold is still there. If your change stream can miss messages (a dropped `EventSource` reconnects without the changes it missed), send a fresh snapshot with `ctx.applySnapshot` when it reconnects. Implement `resync()` too: the map calls it when your page calls `map.resync()`.

The [state adapter reference](../renderer/adapters.md#customState) has every method and the context's calls.

## Licence

Using the seat map without a Cavea server is licensed on its own. This version of the map has no licence option and shows no licence mark without a server; ask for the terms before going live.
