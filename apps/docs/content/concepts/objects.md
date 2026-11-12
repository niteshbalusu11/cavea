# Objects and statuses

An object is anything a buyer can book. Today every object is a seat. This page explains how objects are named and which statuses they move through.

## Names: label, oid, idx

Each object has three identifiers, each for one job:

| Identifier | What it is | Use it for |
|---|---|---|
| `label` | Its name in the chart, such as `Stalls-C-12`: section, row and seat joined by `-`, from the chart's label template. At most 64 bytes. | The API and people. Requests name objects by label. |
| `oid` | A number given when the object is created in the designer, never reused, and kept across versions. | Matching an object across versions; replies include it. |
| `idx` | Its position, 0 to n−1, in one compiled version. | Compact formats such as the realtime stream. You rarely need it. |

A request names objects as a list of labels, such as `"objects": ["Stalls-C-12", "Stalls-C-13"]`. Replies name them by `label` and `oid`.

## Statuses

In an event, each object has exactly one status:

| Status | Meaning | Can be sold |
|---|---|---|
| `free` | Available. Every object starts free. | Yes |
| `held` | Reserved for a while under a [hold token](holds.md). | No |
| `booked` | Sold, with your order reference. See [Bookings](bookings.md). | No |
| `blocked` | Taken off sale by staff, such as for a camera position. | No |
| custom, `4` to `15` | Your own status codes, such as house seats or press. | No |

These are the only changes:

| From | To | How | Who |
|---|---|---|---|
| free | held | Hold with a hold token | A buyer's page (publishable key) or your backend (secret key) |
| held | free | Release with the hold token, or the hold expires | The token's holder, or the server at expiry |
| held | booked | Book with the hold token | Your backend |
| free | booked | Book without a hold token, as a box office does | Your backend |
| booked | free | Release a booking (without a token) | Your backend |
| free | blocked or custom | Change status | Your backend, or staff in the console |
| blocked or custom | free | Change status | Your backend, or staff in the console |

A publishable key can only hold and release with its own hold token: it can never book, block, or change a booked seat. A status change never touches a held or booked seat; to move between two custom statuses, go through free.

## All or nothing

Every seat command names one or more objects, up to 1,000, and either applies to all of them or changes nothing. If one object fails, the reply's `details.objects` lists each failing object with its reason, such as `not_free`. See [Error codes](../reference/errors.md#status-409).

Repeating a change that is already done succeeds and changes nothing: holding a seat your token already holds, or blocking a blocked seat.

## What a buyer sees

The public seat state has two values per object: available or unavailable. A buyer cannot tell a held seat from a sold or blocked one. Their own held seats come to their seat map privately, so it shows them as selected. Staff, in the event manager and through the API with a secret key, see the real status of every seat.
