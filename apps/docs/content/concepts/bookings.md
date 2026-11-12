# Bookings

A booking sells seats: it makes them `booked` for good and records your order reference. This page explains the sequence that keeps a sale safe, and the ways to book and undo a booking.

## Hold, pay, book

Cavea does not know prices, buyers or payments: your platform does. The safe sequence is:

1. The buyer picks seats on the seat map, which **holds** them under a [hold token](holds.md).
2. At checkout, the page sends the hold token to your backend (`selection.holdToken` on the seat map).
3. Your backend takes payment.
4. Your backend **books** the hold token's seats with a secret key, with your order's reference.

```sh
curl -X POST https://seats.example.com/v1/events/opening-night/book \
  -H "Authorization: Bearer $CAVEA_SECRET_KEY" \
  -H 'Content-Type: application/json' \
  -H 'Idempotency-Key: order-10042' \
  -d '{"hold_token": "ht_…", "order_ref": "order-10042"}'
```

The reply lists the booked seats and the event's new `seq`. If the hold expired before you booked, the booking fails (`409 object_not_held_by_token`, or `404 not_found` for a token already gone) and nothing is booked: refund or retry the payment, never the other way round. Never charge first and hold second.

## What a booking records

| Field | Required | What it is |
|---|---|---|
| `order_ref` | Yes | Your order's identifier, 1 to 128 characters. It is shown in the event manager and with each booked seat. |
| `hold_token` | No | Book what this token holds: all of it, or the `objects` you name. |
| `objects` | No | The seats to book, by label. Without a hold token, these are booked directly. |
| `ticket_types` | No | A ticket type per label, such as `adult` or `child`, 1 to 128 characters each. |
| `extra` | No | A JSON object per label, up to 2,048 bytes each, stored as sent. |

Booking the token's last seats deletes the token.

## Booking without a hold

A box office sells over the counter without a buyer's hold: book free seats directly by naming `objects` and no `hold_token`. The seats must be free. Staff do the same in the event manager with **Book…**.

## Retrying safely

A network failure can lose the reply of a booking that succeeded. Send each booking with an `Idempotency-Key` header, such as your order's id: sending the same request again with the same key, within 24 hours, returns the first reply instead of booking again. The same key with a different request is `409 idempotency_mismatch`. Holds, releases and status changes from your backend take the header too.

## Undoing a booking

[Release](../api/seats.md#release) without a hold token frees booked seats, for a refund or an exchange. Only a secret key, or staff in the console, can do it; a buyer's page never can.

## In the API

[Holds and bookings](../api/seats.md): book, release, change status, and manage hold tokens.
