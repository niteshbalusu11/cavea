# Holds and hold tokens

A hold reserves seats for one buyer while they decide and pay, and frees them by itself if they do not. This page explains hold tokens, how long holds last, and who can do what with them.

## Hold tokens

A **hold token** identifies one hold. It is created first, empty, and then used to hold seats: every seat it holds shares its expiry. It looks like `ht_` followed by 64 hexadecimal digits.

A hold token is a secret: whoever has it can add seats to the hold, release them, and, with a secret key, book them. The server stores only its hash. Keep it where the buyer's session is, not in a URL or a log.

| Created by | How | Seat limit | Lifetime |
|---|---|---|---|
| A buyer's page | [`POST /v1/public/hold-tokens`](../api/buyers.md#publicCreateHoldToken) with a publishable key. The seat map does this for you at the buyer's first click. | The event's **Seats per hold token**, 10 unless changed, or less | The event's **Hold lifetime** |
| Your backend | [`POST /v1/hold-tokens`](../api/seats.md#createHoldToken) with a secret key | None unless you set `max_objects` | The event's, or `ttl_seconds` |

With `caveaServer`, the seat map creates a hold token at the buyer's first click and keeps it in the tab's `sessionStorage`, so a reload keeps the buyer's seats. Your page reads it from the selection (`selection.holdToken`) to send to your backend at checkout. If your backend creates the token instead, it passes it to the page, which uses it with `session: "continue"`.

## How long a hold lasts

A hold token expires a fixed time after it is created: the event's hold lifetime, 900 seconds (15 minutes) unless changed, between 60 and 7,200. Adding seats does not move the expiry. At expiry every seat it holds becomes free in one step, and the token is deleted: a later request with it is `404 not_found` with `details.missing` `hold_token`.

Your backend can [extend](../api/seats.md#extendHoldToken) a token, for example while a payment is being confirmed, with a secret key.

A token whose seats were all released stays valid until it expires, so the buyer can choose again with it. A token is deleted when a booking takes its last seats.

## What can be done with a hold token

| Action | Buyer's page (publishable key) | Your backend (secret key) |
|---|---|---|
| Hold more seats | Yes, up to its limit | Yes |
| Release some or all of its seats | Yes | Yes |
| Read what it holds and its expiry | Yes, with the token in `Cavea-Hold-Token` | Yes |
| Extend it | No | Yes |
| Book its seats | No | Yes |

A hold fails as a whole if any seat in it is not free (`409 object_not_free`), or if the token would go over its limit (`422 hold_limit_exceeded`).

## Limiting what one buyer can take

A buyer's page can hold at most the event's seats per hold token under one token, but nothing stops a script from making many tokens. To stop that, your backend signs a [session token](session-tokens.md) for each buyer it knows, and the event requires one: hold tokens are then tied to that buyer, at most two live tokens each by default. Rate limits per client address apply too: see [Limits](../reference/limits.md#rate-limits).

## In the API

- From a buyer's page: [create a hold token, hold, release](../api/buyers.md).
- From your backend: [hold tokens, hold, release, book](../api/seats.md).
