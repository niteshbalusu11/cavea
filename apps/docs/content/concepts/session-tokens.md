# Session tokens

A session token is a short-lived token your backend signs for one buyer and one event. With it, an event can refuse holds from anyone your platform has not vouched for. This page explains what a session token holds, how to sign one, and what it changes.

## Why

A publishable key is public: anyone can read it from your page and call the server with it. Without more, a script could create hold tokens and hold seats it never means to buy. A session token closes that: your backend signs one only for a buyer it knows (signed in, past a queue, past a challenge), and an event that sets **Require a session token** (`require_session`) takes holds only with one.

Not to be confused with the console's session, which is a staff member's sign-in.

## What it holds

A session token is a JSON Web Token (JWT) signed with HMAC-SHA-256 (`HS256`) with one of the workspace's secret keys. Its header names that key by its id (`kid`, `key_…`, not the key itself). Its claims:

| Claim | Required | What it is |
|---|---|---|
| `sub` | Yes | Your identifier for the buyer, 1 to 128 characters. Cavea never stores or logs it as given. |
| `event` | Yes | The event's key, exactly. |
| `exp` | Yes | When it expires, in seconds since 1970 (a whole number). The server allows 30 seconds of clock difference. |
| `max_objects` | No | The most seats a hold token created with it may hold: the lower of this and the event's limit. |
| `channels` | No | Channel keys. Checked for shape; no effect until channels exist. |

The whole token is at most 1,024 bytes. Other claims, such as `iat`, are ignored.

## Signing one

With the Node server SDK, `{{server_sdk_package}}`, given the secret key and its id:

```ts check=types
import { Cavea } from "{{server_sdk_package}}";
#hide declare const buyer: { id: string };

const cavea = new Cavea({
  baseUrl: "https://seats.example.com",
  secretKey: process.env.CAVEA_SECRET_KEY,
  keyId: process.env.CAVEA_SECRET_KEY_ID, // key_…, from the key's creation or the keys list
});

// When a signed-in buyer opens the seat picker:
const sessionToken = await cavea.signSessionToken({ sub: buyer.id, event: "opening-night" });
```

In another language, any JWT library does it: sign `{"sub", "event", "exp"}` with `HS256`, the secret key's text as the HMAC secret, and the key's id as the header's `kid`.

The page passes it to the seat map, which sends it in the `Cavea-Session` header with hold-token creation, hold and release:

```js check=types
#hide import * as Cavea from "{{renderer_package}}";
#hide declare const baseUrl: string, publishableKey: string, sessionToken: string;
Cavea.caveaServer({ baseUrl, publishableKey, event: "opening-night", sessionToken });
```

`sessionToken` may also be a function returning a promise of a token: the map calls it at the first hold, and again once if the token has expired.

## What it changes

- **On an event with `require_session`**, creating a hold token, holding and releasing from a buyer's page need a valid session token. Without one: `401 unauthorized`, `reason` `session_token`, with `details.session` saying `missing`, `invalid`, `expired`, or `event` (signed for another event).
- **On any event**, a session token that is sent is checked: an invalid one is refused, never ignored.
- **A hold token created with a session token is tied to its `sub`.** Holding or releasing with it then needs a session token of the same buyer (`403 forbidden`, `reason` `session`), so a stolen hold token is useless alone.
- **One buyer may have at most `max_tokens_per_sub` live hold tokens on an event**, 2 by default (`422 hold_limit_exceeded`, `reason` `tokens_per_sub`). Each token has its own seat limit, so a buyer can hold up to that many times the event's seats per hold token: 20 with the defaults. The default 2 lets a buyer keep a second tab; set it to 1 where a buyer must never hold more than one token's worth.

Realtime connections never need a session token: the public seat state shows only which seats are unavailable.

Revoking the secret key that signed a token stops it at once. To rotate, sign with the new key's id.
