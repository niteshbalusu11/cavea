# Limits

Every limit of the Cavea server that a customer can reach, with its value, whether it can be changed, and the error it gives. Settings named `CAVEA_…` are environment variables of the server, set by whoever runs it.

## Requests

| Limit | Value | Over it |
|---|---|---|
| Request body | 256 KiB, unless the route says otherwise | `400 invalid_request`, `body_too_large` |
| Chart draft (create a chart, save a draft) | 16 MiB | `400 invalid_request`, `body_too_large` |
| Nesting of arrays and objects in a JSON body | 32 levels | `400 invalid_request`, `json_depth` |
| Objects named in one seat command | 1,000 | `400 invalid_request`, `too_many_objects` |
| Names, and the keys you choose for charts and events | 1 to 128 characters, without U+0000 | `400 invalid_request`, `invalid_field` |
| Items per page of a list (`limit`) | 1 to 100, default 100 | `400 invalid_request`, `query` |
| Event search text (`q`) | 128 characters | `400 invalid_request`, `query` |
| `Idempotency-Key` | 1 to 255 printable ASCII characters, remembered for 24 hours | `400 invalid_request`, `idempotency_key` |

## Holds and bookings

| Limit | Value | Over it |
|---|---|---|
| Hold lifetime | 60 to 7,200 seconds; an event's default is 900 (15 minutes), set per event as **Hold lifetime** | `400 invalid_request`, `ttl_out_of_range` |
| Seats per hold token from a buyer's page | 1 to 100; an event's default is 10, set per event as **Seats per hold token** | `422 hold_limit_exceeded` |
| Seats per hold token from your backend (secret key) | Unlimited unless the token is created with `max_objects` | `422 hold_limit_exceeded` |
| Live hold tokens per buyer (`sub` of a session token) on one event | 1 to 10; default 2 (`max_tokens_per_sub`) | `422 hold_limit_exceeded`, `tokens_per_sub` |
| `order_ref` of a booking | 1 to 128 characters, without U+0000 | `400 invalid_request`, `order_ref_length` |
| `ticket_type` of a booked object | 1 to 128 characters, without U+0000 | `400 invalid_request`, `ticket_type_length` |
| `extra` data of a booked object | 2,048 bytes | `400 invalid_request`, `extra_too_large` |
| Session token | 1,024 bytes; `sub` and each claim string 1 to 128 characters | `401 unauthorized`, `session_token` |

## Rate limits

A buyer's page is limited per publishable key and per client address. When the server is behind a proxy, load balancer or CDN, the operator lists it in `CAVEA_TRUSTED_PROXIES` so that each buyer counts by their own address; otherwise every buyer shares the proxy's. Your backend's secret-key requests are not limited.

| Limit | Default | Setting | Over it |
|---|---|---|---|
| Hold-token creations, holds and releases from buyers' pages | 120 a minute | `CAVEA_RATE_LIMIT_HOLDS` (0 turns it off) | `429 rate_limited` with `Retry-After` |
| Other requests from buyers' pages: event, state, realtime connection attempts | 3,000 a minute | `CAVEA_RATE_LIMIT_READS` (0 turns it off) | `429 rate_limited` with `Retry-After` |
| Open realtime connections from one client address | 200 | `CAVEA_RT_MAX_PER_IP` | `429 rate_limited` |
| Open realtime connections on one server | 60,000 | `CAVEA_RT_MAX_CONNECTIONS` | `503 overloaded` |
| Console sign-in, setup and password-change attempts from one address | 10 a minute | `CAVEA_RATE_LIMIT_SIGN_IN` (0 turns it off) | `429 rate_limited`, `sign_in` |
| Failed sign-ins in a row for one account | After 5 from one address, each attempt waits longer: 1 second, then 2, 4, up to 15 minutes | — | `429 rate_limited`, `backoff` |

A bucket refills continuously: a client at rest may send a minute's worth at once, then one request every 60 seconds divided by the rate. Each server counts on its own, so two servers behind one load balancer admit twice the rate.

## Charts

The designer and the server's compiler check these when a chart is validated or published; the designer's Validation panel names anything over.

| Limit | Value |
|---|---|
| Bookable objects per chart | 1,000,000 |
| Seats per row | 4,095 |
| Label, and each part of a label | 64 bytes of UTF-8 |
| Categories per chart | 4,095 |
| Floors per chart | 255 |
| Sections per chart | 65,535 |
| Background image | PNG, JPEG or WebP; each side at most 8,192 pixels; 10 MiB by default (`CAVEA_ASSET_MAX_BYTES`, at most 256 MiB) |

## Keys and the console

| Limit | Value | Over it |
|---|---|---|
| Allowed origins of a publishable key | 1 to 50 | `400 invalid_request`, `invalid_field` |
| Console password | 15 to 128 characters | `400 invalid_request`, `invalid_field` |
| Setup link | Valid for one hour; a restart of a server without users makes a new one | `401 unauthorized`, `setup_token` |
| Console session | Ends 12 hours after its last use, and 7 days after sign-in | `401 unauthorized`, `console_session` |
