# Examples

Four small projects that run against a Cavea server, each complete and short enough to read in one sitting. Copy one, set its environment variables, and run it; then change it into your own.

| Example | What it shows | Run with |
|---|---|---|
| [Plain page]({{repository}}/tree/main/examples/plain-page) | One HTML file and a script tag: the map, the chosen seats with prices from the page's own table, and a **Continue** button that sends the hold token to your backend | Any static web server; `npm start` serves it |
| [React]({{repository}}/tree/main/examples/react) | A Vite app with `<SeatMap>`: the selection lifted into the app's state, prices, and the map in your brand's colours with a dark switch | `npm run dev` |
| [Next.js]({{repository}}/tree/main/examples/nextjs) | The whole sale: the map in a client component, and route handlers that price the hold from the server's answer, take a fake payment and book with an idempotency key, refund when the hold ended, and release on cancel | `npm run dev` |
| [Node backend]({{repository}}/tree/main/examples/node-backend) | The same checkout with no framework, in one file, plus a hold token kept in the buyer's session and a signed session token | `npm start` |

Each has its own `package.json`, with every dependency at an exact version and a lockfile, and a README that says what it needs and how to run it. The commands are npm's; bun, pnpm and yarn work the same way. The payments are fake, and marked so in the code: that is where your payment provider goes.

All four are in the repository's `examples/` directory. Copy a directory out as it is: nothing in it depends on the rest of the repository.

## How they fit together

The pages hold seats with a [publishable key](../concepts/keys.md#publishable-keys); a backend books them with a secret key, which never reaches a browser. The Next.js and Node examples are the [hold, pay, book](../guides/hold-pay-book.md) guide as running code. The checkout route of the Next.js example, for instance:

```ts check=file:examples/nextjs/app/api/checkout/route.ts
import { answer, checkout } from "../../../lib/checkout";

// The page sends its hold token in the body (never in a URL): it can release the seats.
export async function POST(request: Request): Promise<Response> {
  const body: unknown = await request.json().catch(() => null);
  const holdToken = (body as { holdToken?: unknown } | null)?.holdToken;
  if (typeof holdToken !== "string") {
    return Response.json({ error: "no_hold_token" }, { status: 400 });
  }
  return answer(() => checkout(holdToken));
}
```

Prices come from the backend's own table, by the category keys the [chart's designer](../designer/categories.md#keys) gave, applied to the categories the server reports for the seats actually held, never from anything the page sends. The examples' tables are for the sample theatre the quick start's API path makes (`premium`, `standard`, `value`, `access`): change them to your chart's.

## They are tested

Each example is run against a real Cavea server before a change to Cavea is merged: installed with npm from its own lockfile, started, and driven as a buyer would (a seat clicked on the map in a browser, checkout, payment), and the server is then asked whether the seat was held, booked or released.

> **Not yet** The packages are not on npm yet, so `npm install` cannot fetch them until the first release. Until then, install them from tarballs built from the repository: [Before the first release](../before-release.md#the-npm-packages).
