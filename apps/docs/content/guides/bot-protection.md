# Bot protection

A popular on-sale attracts scripts that hold seats they never buy, to deny them to real buyers or to resell them. This guide says what Cavea limits by itself, what it leaves to you, and how to tie every hold to a buyer your platform knows. By the end you will have the settings and the code for an on-sale where a script holds no more than one buyer's share.

## What the server does by itself

| Limit | Default | Where it is set |
|---|---|---|
| Seats per hold token from a web page | 10 (1 to 100) | The event's `max_objects`, **Seats per hold token (seats)** in the console |
| How long a hold lasts | 15 minutes (1 minute to 2 hours) | The event's `hold_ttl_seconds`, **Hold lifetime (seconds)** in the console |
| Hold-token creations, holds and releases per minute, per publishable key and client address | 120 | `CAVEA_RATE_LIMIT_HOLDS` on the server |
| Other public requests per minute, per publishable key and client address (the seat state, live connection attempts) | 3,000 | `CAVEA_RATE_LIMIT_READS` |
| Open live connections per client address | 200 | `CAVEA_RT_MAX_PER_IP` |

A request over a rate limit is `429 rate_limited` with `Retry-After`. The seat map waits and retries once if the wait is short, and otherwise tells the buyer the seat could not be held. An IPv6 client counts by its /64, since one household often has a whole /64.

These limits slow a script down; they do not stop one that makes a new hold token every few seconds from many addresses. That needs to know who the buyer is, which only your platform does.

> **Warning** Behind a load balancer, CDN or proxy, every buyer comes from the proxy's address unless you list it in `CAVEA_TRUSTED_PROXIES`. Then the whole audience shares one client's budget of 120 holds a minute, and an on-sale fails in its first minute. See [reverse proxy](../deployment/proxy.md#client-addresses-and-rate-limits).

## What the server does not do

Cavea has no CAPTCHA, no waiting room or queue, and no bot detection, and it does not alert on an unusual number of holds yet. It never sees who the buyer is: only a publishable key, a client address and, if you send one, a session token you signed. The server's metrics count holds (`cavea_holds_active`, and holds per result in `cavea_commands_total`), which you can watch and alert on yourself ([Monitoring](../deployment/monitoring.md)).

## Tie holds to buyers with session tokens

A [session token](../concepts/session-tokens.md) is a short-lived signed statement from your backend: "this page belongs to buyer `sub`, for this event". When the event **requires** one:

- a page cannot create a hold token, hold or release without a valid session token for that event (`401`);
- each hold token is tied to the buyer who created it: another buyer's session cannot use it;
- one buyer may have at most `max_tokens_per_sub` live hold tokens on the event, 2 by default, so at most twice the seats per hold token.

So a script needs a buyer account per share of seats, and making accounts is what your sign-up flow, your payment checks and your challenge already control.

### 1. Require it on the event

Set **Require a session token** on the event in the console, or with the API:

```ts check=types
import type { Cavea } from "{{server_sdk_package}}";
#hide declare const cavea: Cavea;

await cavea.events.update("opening-night", {
  settings: { require_session: true, max_tokens_per_sub: 1, max_objects: 6 },
});
```

`max_tokens_per_sub` (**Hold tokens per session (tokens)** in the console) at 1 means one hold token per buyer: a second tab cannot double their share. The default 2 lets a buyer keep a second tab open.

### 2. Sign a token for each buyer

Your backend signs one when a signed-in buyer opens the seat map, after whatever checks you make. The secret key's id (`key_…`) is needed to sign:

```ts check=types
import { Cavea } from "{{server_sdk_package}}";
#hide declare const buyer: { id: string };

const cavea = new Cavea({
  baseUrl: "https://seats.example.com",
  secretKey: process.env.CAVEA_SECRET_KEY ?? "",
  keyId: process.env.CAVEA_SECRET_KEY_ID ?? "",
});

// In the route your page calls for a token, once the buyer is signed in and has passed your checks:
const token = await cavea.signSessionToken({ sub: buyer.id, event: "opening-night", maxObjects: 6 });
```

The [Node backend example](../examples/index.md) has such a route, `POST /api/seat-session`.

`sub` is your id for the buyer: any stable string, never their email. The token expires (in 15 minutes unless you say otherwise); the map asks again when it does.

### 3. Give it to the map

Pass a function as `sessionToken`, so the map asks your backend at the first hold and again when the token expires:

```ts check=types
import { caveaServer } from "{{renderer_package}}";

const state = caveaServer({
  baseUrl: "https://seats.example.com",
  publishableKey: "pk_paste-your-publishable-key-here",
  event: "opening-night",
  sessionToken: async () => {
    const reply = await fetch("/api/seat-session", { method: "POST", credentials: "same-origin" });
    if (!reply.ok) throw new Error(`seat session: ${reply.status}`);
    return ((await reply.json()) as { token: string }).token;
  },
});
```

Watching the seats needs no session token: the public seat state says only which seats are unavailable.

## Where a challenge goes

A challenge (a CAPTCHA, a proof of work, a virtual waiting room) belongs in front of the step that signs the session token: in your own route, or at your gateway or CDN on that route. A buyer who has not passed it gets no session token, and without one they can hold nothing. Your page shows the challenge before it creates the seat map, or when the token route refuses.

```text
Buyer's page ──▶ your gateway (challenge, queue) ──▶ your backend: is this buyer signed in and allowed?
                                                         │ yes: sign a session token (secret key)
Buyer's page ◀── session token ──────────────────────────┘
Buyer's page ──▶ Cavea server: hold, with the session token ──▶ checked, tied to the buyer
```

Do not put the challenge in front of the Cavea server's public routes themselves: the seat map makes many small requests (the state, the live connection, each hold), and a challenge on those breaks it. The session token is what carries the result of the challenge to the server.

## A checklist for an on-sale

1. The event requires a session token, with `max_tokens_per_sub` and `max_objects` set for this sale.
2. Your token route signs only for signed-in buyers who passed your challenge, and is itself rate limited by your gateway.
3. `CAVEA_TRUSTED_PROXIES` lists every proxy in front of the server, so rate limits count buyers, not the proxy.
4. The publishable key lists your site's origins (**Allowed origins**), so other sites' pages cannot use it from a browser. A script outside a browser ignores this; the session token is what stops it.
5. Someone watches `cavea_holds_active` and `429` rates during the on-sale ([Monitoring](../deployment/monitoring.md)).
