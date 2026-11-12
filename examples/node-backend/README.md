# Cavea example: a Node checkout backend

A small backend for the Cavea seat map, in one file, `server.ts`: Node's own HTTP server and `@cavea/server-sdk`, no framework. It does what a ticketing backend does around the map:

- gives a buyer a hold token, kept in their session, for the page to hold seats under;
- signs a buyer's session token, for events that require one;
- prices what a hold token holds, from the server's own answer (each seat's category), never from the page;
- takes a payment (**fake**, clearly marked) and books exactly the seats it priced, with an idempotency key, so a retry never books twice;
- refunds when the hold ended during the payment, and releases the seats when the buyer cancels.

Sessions and orders are kept in memory to keep it short. Keep them in your database.

## What you need

- Node.js 22.18 or later (it runs `server.ts` as it is). npm comes with it; bun, pnpm and yarn work too.
- A Cavea server with a published chart and an event: the [quick start](https://github.com/niteshbalusu11/cavea/blob/main/apps/docs/content/quick-start.md) makes both.
- A secret key and its id, from the console's **Developers** page (**New API key**, **Secret key**). The id starts `key_`.
- Prices for your chart's categories: change `PRICES` in `server.ts` to your chart's category keys. The keys here are those of the sample theatre (`cavea gen theatre`).

## Run it

```sh
npm install
```

Set these environment variables in your shell (never in a file you commit):

| Variable | What it is |
|---|---|
| `CAVEA_URL` | Your Cavea server, such as `http://localhost:8080` |
| `CAVEA_EVENT` | The event's key, such as `opening-night` |
| `CAVEA_SECRET_KEY` | The secret key, starting `sk_` |
| `CAVEA_SECRET_KEY_ID` | The secret key's id, starting `key_` |
| `PORT` | Optional; 3000 by default |

```sh
npm start
```

It prints `listening on http://localhost:3000`. `npm run typecheck` checks the types.

## The routes

All take and return JSON, and identify the buyer by an HttpOnly cookie (`sid`).

| Route | Does |
|---|---|
| `POST /api/hold-token` | The buyer's hold token, made once and kept in their session: `{ holdToken, expiresAt }` |
| `POST /api/seat-session` | A session token for the buyer and the event: `{ token }` |
| `POST /api/checkout` | Prices the hold token's seats and makes an order: `{ orderId, seats, total }`, the total in pence. Takes `{ holdToken }`, or uses the session's. `409 { error: "hold_expired" }` when the hold has ended |
| `POST /api/orders/{id}/pay` | Takes the fake payment and books the order's seats: `{ status: "booked", seats }`. Calling it again answers the same. `409 { error: "hold_expired" }` after a refund when the hold ended first |
| `POST /api/orders/{id}/cancel` | Releases the order's seats: `{ status: "cancelled" }` |

## The page

The buyer's page holds seats on the map under the backend's hold token:

```js
const { holdToken } = await (await fetch("/api/hold-token", { method: "POST" })).json();
Cavea.createSeatMap(document.getElementById("map"), {
  state: Cavea.caveaServer({
    baseUrl: "http://localhost:8080",
    publishableKey: "pk_paste-your-publishable-key-here",
    event: "opening-night",
    session: "continue",
    holdToken,
  }),
});
```

Or the map makes its own hold token and proves who the buyer is with a session token: pass `sessionToken: async () => (await (await fetch("/api/seat-session", { method: "POST" })).json()).token` instead of `session` and `holdToken`, and send `selection.holdToken` to `/api/checkout`.

At checkout the page posts to `/api/checkout`, shows the seats and the total from the answer, and then posts to `/api/orders/{id}/pay`. The [Next.js example](../nextjs) is the whole page.

## Read more

- [Hold, pay, book](https://github.com/niteshbalusu11/cavea/blob/main/apps/docs/content/guides/hold-pay-book.md): the sequence, and what to do when each step fails.
- [Bot protection](https://github.com/niteshbalusu11/cavea/blob/main/apps/docs/content/guides/bot-protection.md): requiring session tokens.

## Before the packages are published

`@cavea/server-sdk` is not on npm yet. Until it is, build it from the Cavea repository and install the tarball in place of the version in `package.json`: [Before the first release](https://github.com/niteshbalusu11/cavea/blob/main/apps/docs/content/before-release.md#the-npm-packages).
