# Hold, pay, book

This guide follows one sale from the buyer's click to a booked seat, across the buyer's page and your backend, and says what to do when something goes wrong on the way: the hold expires, the payment fails, the buyer closes the tab, a request is retried. By the end you will have a checkout that never sells a seat twice and never charges for a seat it could not book.

You need a Cavea server with a published chart, an event, a publishable key and a secret key: the [quick start](../quick-start.md) makes all of them. The backend code uses the Node server SDK, `{{server_sdk_package}}` (Node 20.3 or later, or bun); any language works with the [server API](../api/seats.md).

## The sequence

1. The buyer picks seats on the seat map. Each pick is **held** on the Cavea server under the page's [hold token](../concepts/holds.md), so no one else can take it.
2. At checkout, the page sends the hold token and its own order details to your backend. Never prices: your backend works those out.
3. Your backend reads what the hold token holds and how long it has left, and extends it if the payment could outlast it.
4. Your backend takes the payment.
5. Your backend **books** exactly the seats it priced, under the hold token, with your order number and a secret key.

The rule that makes it safe: **hold first, charge second, book third.** A hold that ends before the booking frees its seats and the booking fails, so you refund; you never sell a seat someone else holds.

## 1. The page

Create the map with `caveaServer` and keep the selection. The checkout button sends the hold token to your backend:

```ts check=types
import { caveaServer, createSeatMap, type Selection } from "{{renderer_package}}";
#hide declare const container: HTMLElement;
#hide declare const checkoutButton: HTMLButtonElement;

let selection: Selection | null = null;
let holding = false;
const ready = (): boolean => selection !== null && selection.objects.length > 0 && selection.holdToken !== null;

await createSeatMap(container, {
  state: caveaServer({
    baseUrl: "https://seats.example.com",
    publishableKey: "pk_paste-your-publishable-key-here",
    event: "opening-night",
  }),
  onSelectionChange: (s) => {
    selection = s;
    checkoutButton.disabled = holding || !ready();
  },
  onHoldPending: () => {
    holding = true;
    checkoutButton.disabled = true;
  },
  onHoldSettled: () => {
    holding = false;
    checkoutButton.disabled = !ready();
  },
});

checkoutButton.addEventListener("click", async () => {
  if (selection === null || selection.holdToken === null) return;
  const reply = await fetch("/api/checkout", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ event: "opening-night", holdToken: selection.holdToken }),
  });
  if (reply.ok) location.assign("/checkout/payment");
});
```

Keep the checkout button disabled while a hold is in flight (`onHoldPending` to `onHoldSettled`): a seat the server is still deciding about is not yet the buyer's.

The hold token is a secret: it can release the buyer's seats. Send it in a request body over HTTPS, never in a URL, and keep it in your backend's session for the buyer.

## 2. Your backend: check the hold

Make one client for the process, with the secret key from your environment:

```ts check=types
import { Cavea } from "{{server_sdk_package}}";

const secretKey = process.env.CAVEA_SECRET_KEY;
if (secretKey === undefined) throw new Error("CAVEA_SECRET_KEY is not set");
export const cavea = new Cavea({ baseUrl: "https://seats.example.com", secretKey });
```

At checkout, read the hold token. It says which seats it holds and when it expires; each seat comes with its category, and its section, row and seat apart from its label:

```ts check=types
import { isCaveaError, type Cavea } from "{{server_sdk_package}}";
#hide declare const cavea: Cavea;
#hide declare function priceOf(event: string, category: string): number;

/** What the buyer is about to pay for, or why they cannot. */
export async function basket(event: string, holdToken: string) {
  try {
    const held = await cavea.holdTokens.get(holdToken);
    if (held.event !== event || held.objects.length === 0) return { ok: false as const, why: "empty" };
    const seats = held.objects.map((o) => o.label);
    const total = held.objects.reduce((sum, o) => sum + priceOf(event, o.category.key), 0);
    return { ok: true as const, seats, total, expiresAt: new Date(held.expires_at) };
  } catch (error) {
    // The token has expired, or never existed: its seats are free again.
    if (isCaveaError(error, "not_found")) return { ok: false as const, why: "expired" };
    throw error;
  }
}
```

Store `seats` and `total` with the order: the payment is for these seats, and step 5 books exactly these. Price the seats from what the server says, never from anything the page sent: each object's `category` is the chart's category (`key` and `label`), so your backend needs only a table from category keys to prices. For the ticket, `section`, `row` and `seat` are the parts of the label, so you can print them in your own format; `section` is `null` on a chart without sections, whose labels are the row and seat alone (`D-10`).

If the token has expired, tell the buyer their seats were released and send them back to the map: nothing has been charged.

## 3. Make sure the hold outlasts the payment

A hold lasts the event's hold lifetime from its creation (15 minutes unless changed), and adding seats does not move it. If the payment could still be running when it ends, extend the token first: it then expires that many seconds from now.

```ts check=types
import type { Cavea } from "{{server_sdk_package}}";
#hide declare const cavea: Cavea;

export async function keepForPayment(holdToken: string, expiresAt: Date): Promise<Date> {
  const tenMinutes = 10 * 60 * 1000;
  if (expiresAt.getTime() - Date.now() > tenMinutes) return expiresAt;
  const extended = await cavea.holdTokens.extend(holdToken, { ttl_seconds: 600 });
  return new Date(extended.expires_at);
}
```

Only a secret key can extend a hold; a buyer's page cannot. The seat map may go on showing the old expiry (`expiresAt`, `onHoldExpiring`) until it next reads the hold; the server's expiry is the one that counts.

## 4. Take the payment

Use your payment provider as you do today. If it supports it, authorise the amount now and capture it after the booking: then a booking that fails needs a release of the authorisation, not a refund.

## 5. Book

Book the seats you priced in step 2, by label, under the hold token, with your order number in the order reference and the idempotency key. Name them: a booking without `objects` takes whatever the token holds at that moment, and the buyer's page can still add or release seats while the payment runs. Named, the booking takes exactly what was paid for, and fails if one of those seats is no longer held.

```ts check=types
import { isCaveaError, type Cavea } from "{{server_sdk_package}}";
#hide declare const cavea: Cavea;

export type Booked = { ok: true; seats: string[] } | { ok: false; why: "hold_ended" };

/** `seats`: the labels priced and paid for, stored with the order at checkout. */
export async function book(
  event: string,
  holdToken: string,
  orderId: string,
  seats: string[],
): Promise<Booked> {
  try {
    const reply = await cavea.events.book(
      event,
      { hold_token: holdToken, objects: seats, order_ref: orderId },
      { idempotencyKey: `book-${orderId}` },
    );
    return { ok: true, seats: reply.objects.map((o) => o.label) };
  } catch (error) {
    // The hold ended before the booking, or a paid-for seat was released meanwhile.
    if (isCaveaError(error, "not_found") || isCaveaError(error, "object_not_held_by_token")) {
      return { ok: false, why: "hold_ended" };
    }
    throw error;
  }
}
```

A booking is all or nothing: either every seat named is booked, or none is. The seats show as booked on every buyer's map within a moment. Booking the token's last seats deletes the token; if the buyer added seats during the payment, the token still holds those, unpaid: release them with `cavea.holdTokens.release(holdToken)` (the booked ones stay booked), or let them expire.

## When something goes wrong

| What happened | What the server has | What to do |
|---|---|---|
| **The hold expired before checkout** | The seats are free; the token is gone (`holdTokens.get` is `not_found`) | Charge nothing. Tell the buyer, and send them back to the map, which has already dropped the seats (`onHoldLost`, `onHoldExpired`). |
| **The hold expired during the payment** | The seats are free and may be someone else's; `book` fails with `not_found` or `object_not_held_by_token` | Refund or void the payment. Extending the hold before charging (step 3) is what prevents this. |
| **The buyer released a paid-for seat during the payment** (another tab, a deselect) | That seat is free; the others are held | `book` fails with `object_not_held_by_token` and books nothing: refund, or ask the buyer to choose again. |
| **The payment failed or was declined** | The seats are still held, until the hold expires | Let the buyer try again: the hold still covers them. If they give up, release the seats at once with `cavea.holdTokens.release(holdToken)` rather than waiting for expiry. |
| **The buyer closed the tab** | The seats stay held until the hold expires, then become free by themselves | Nothing. If the buyer reloads the same tab before the hold ends, the map brings their held seats back. A new tab starts a new selection. |
| **The booking's reply was lost** (a timeout, a dropped connection, your process restarted) | The seats may or may not be booked | Send the same booking again with the same idempotency key. Within 24 hours the server answers with the first reply instead of booking again, so the order is booked exactly once. |
| **The booking got `503 unavailable` or `500 internal_error`** | Unknown: the change may have been stored | The same: retry with the same key, backing off between tries. |
| **The same key, another request** | Nothing new | `409 idempotency_mismatch`: a bug in how you make keys. Use one key per order and send the same body with it. |
| **The order was booked, and the booking is sent again** | Booked | With the same key, the first reply comes back. Without a key, the token is gone (booking its last seats deleted it) and the retry fails with `not_found`, though the order is booked. Always send the key. |

The SDK already retries a call whose answer was lost, with the same key, a few times (`maxRetries`, default 2). A retry after your own process restarted needs a key you can make again: derive it from the order, as `book-${orderId}` above, never from a random number.

## Releasing and refunds

To give back seats before booking, release the hold token (`cavea.holdTokens.release(holdToken)`) or some of its seats (`cavea.events.release(event, { hold_token, objects })`). To undo a booking, for a refund or an exchange, release the booked seats without a hold token: `cavea.events.release(event, { objects: ["Stalls-C-12"] })`. Only a secret key or staff in the console can do that; a buyer's page never can.

## Next

- The [Next.js example](../examples/index.md) and the [Node backend example](../examples/index.md) are this guide as running code, each checked against a real server.
- [Bot protection](bot-protection.md): tie holds to signed-in buyers so a script cannot hold the house.
- [Bookings](../concepts/bookings.md) and [holds](../concepts/holds.md), the concepts behind this guide.
- [Holds and bookings](../api/seats.md) in the API reference.
