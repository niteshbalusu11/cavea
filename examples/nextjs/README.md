# Cavea example: Next.js checkout

The whole sale in a Next.js app: the buyer chooses seats on the map, checks out, pays (**fake**: clearly marked, standing in for your payment provider) and gets a booking. It follows the [hold, pay, book](https://github.com/niteshbalusu11/cavea/blob/main/apps/docs/content/guides/hold-pay-book.md) guide step by step.

- `app/seat-picker.tsx`, a client component: `<SeatMap>`, the selection, the checkout button (disabled while a seat is still being held), the order and its **Pay** and **Cancel** buttons.
- `app/api/checkout/route.ts`: takes the hold token from the request body, reads what it holds from the Cavea server, prices each seat by its category, extends the hold if the payment could outlast it, and makes an order.
- `app/api/orders/[id]/pay/route.ts`: takes the fake payment and books exactly the seats priced, with the order's idempotency key; if the hold ended during the payment, it refunds (fake) and says so.
- `app/api/orders/[id]/cancel/route.ts`: releases the seats at once.
- `lib/checkout.ts`: those steps, with `@cavea/server-sdk`. `lib/prices.ts`: the price list, by category key.

Orders are kept in memory to keep the example small: keep them in your database, tied to the signed-in buyer.

## What you need

- Node.js 20.9 or later. npm comes with it; bun, pnpm and yarn work too.
- A Cavea server with a published chart and an event: the [quick start](https://github.com/niteshbalusu11/cavea/blob/main/apps/docs/content/quick-start.md) makes both.
- A publishable key and a secret key, from the console's **Developers** page.

## Run it

```sh
npm install
```

Set these environment variables, for example in `.env.local` (never commit it):

| Variable | What it is |
|---|---|
| `NEXT_PUBLIC_CAVEA_URL` | Your Cavea server, such as `http://localhost:8080` |
| `NEXT_PUBLIC_CAVEA_PUBLISHABLE_KEY` | The publishable key, starting `pk_`: built into the page, which is safe |
| `NEXT_PUBLIC_CAVEA_EVENT` | The event's key, such as `opening-night` |
| `CAVEA_SECRET_KEY` | The secret key, starting `sk_`: used only by the route handlers, never sent to the browser |

```sh
npm run dev
```

Open `http://localhost:3000`, choose seats, **Checkout**, then **Pay**. The seats show as booked on every buyer's map, and in the console's event manager. For production, `npm run build` and `npm start`.

The prices in `lib/prices.ts` are for the sample theatre's category keys (`cavea gen theatre`); change them to your chart's.

## Read more

- [Hold, pay, book](https://github.com/niteshbalusu11/cavea/blob/main/apps/docs/content/guides/hold-pay-book.md): every failure this code handles, and why.
- [React](https://github.com/niteshbalusu11/cavea/blob/main/apps/docs/content/guides/react.md): `<SeatMap>` in Next.js.
- [Node backend example](../node-backend): the same checkout without a framework, with session tokens.

## Before the packages are published

The three `@cavea` packages are not on npm yet. Until they are, build them from the Cavea repository and install the tarballs in place of the versions in `package.json`: [Before the first release](https://github.com/niteshbalusu11/cavea/blob/main/apps/docs/content/before-release.md#the-npm-packages).
