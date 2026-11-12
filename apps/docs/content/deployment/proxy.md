# The reverse proxy and rate limits

The server speaks plain HTTP and expects a proxy or load balancer in front of it to terminate TLS. This page says what that proxy must do, how the server finds each buyer's address behind it, and how the rate limits count. By the end you will be able to put Cavea behind Caddy, nginx, a cloud load balancer or a CDN without breaking live updates or sharing one rate limit among every buyer.

[Running Cavea](index.md#a-production-set-up-with-docker-compose) has a complete set-up with Caddy. It is run by the documentation's own check (TLS with a certificate made at run time, a buyer's page receiving a live change over a WebSocket through the proxy, the console's setup link and session cookie), so you can start from it as it is.

## What any proxy must do

| Requirement | Why |
|---|---|
| Terminate TLS, and set `CAVEA_PUBLIC_URL` to the `https://` address | The console's session cookie is only sent over HTTPS, and the setup and invitation links start with the public URL |
| Serve the server at the root of its own host name, such as `https://seats.example.com/` | The console and its cookie live at `/` |
| Pass WebSocket upgrades on `/v1/public/rt` | Buyers' live seat updates. Without it the seat map falls back to Server-Sent Events after two failures, which costs a little start-up time and works |
| Not buffer `text/event-stream` answers | Server-Sent Events, the first fallback. The server sends `X-Accel-Buffering: no`, which nginx obeys |
| Keep idle connections open at least 60 s | The server sends a ping every 25 s; a proxy that cuts a quiet connection sooner makes buyers reconnect |
| Pass the `Host` header and the request body as they are | The console compares `Host` with the page's origin; uploads of background images go up to `CAVEA_ASSET_MAX_BYTES` (10 MiB by default) |
| Append the client's address to `X-Forwarded-For` | So rate limits count buyers, not the proxy (below) |

The server sets its own security headers (Content Security Policy, `X-Content-Type-Options`, `Referrer-Policy`, `Strict-Transport-Security` when the public URL is HTTPS, and CORS on the public routes). Do not remove them or add a second set at the proxy.

Caddy, as configured in [Running Cavea](index.md), does all of this by default. For nginx, the equivalents are `proxy_http_version 1.1` with the `Upgrade` and `Connection` headers passed, `proxy_read_timeout` above 60 s, `client_max_body_size` at least the asset limit, and `proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for`; this site has no tested nginx configuration yet.

## Client addresses and rate limits

The server counts rate limits per publishable key and **client address**. Without help it takes the address the connection came from, which behind a proxy is the proxy's: every buyer then shares one budget of 120 holds a minute, and an on-sale fails with `429 rate_limited` in its first minute. The server does not detect this; it shows only as those `429`s.

So list every proxy in front of the server in `CAVEA_TRUSTED_PROXIES`: addresses or CIDR ranges, IPv4 or IPv6. From a listed address, the server reads `X-Forwarded-For` from the right, skipping trusted entries, and takes the first untrusted address as the client. From any other address the header is ignored, so a buyer cannot choose their own address. A malformed entry ends the walk at the nearest trusted proxy, which then counts as the client. `Forwarded` (RFC 7239) is not read.

| What the server counts | Budget | Setting |
|---|---|---|
| Hold-token creations, holds and releases from buyers' pages | 120 a minute per key and address | `CAVEA_RATE_LIMIT_HOLDS` |
| Other requests with a publishable key: the seat state, live connection attempts | 3,000 a minute per key and address | `CAVEA_RATE_LIMIT_READS` |
| Console sign-ins, setup, password changes | 10 a minute per address | `CAVEA_RATE_LIMIT_SIGN_IN` |
| Open live connections | 200 per address, 60,000 in all | `CAVEA_RT_MAX_PER_IP`, `CAVEA_RT_MAX_CONNECTIONS` |

A budget at rest admits a minute's worth at once, then refills evenly. Over it, the answer is `429 rate_limited` with `Retry-After`. An IPv6 client counts by its /64. Requests with a secret key or the admin key are not limited. The limits are kept per server, in memory: a restart resets them.

**Behind a CDN** that caches the public seat state (it may, for `CAVEA_SNAPSHOT_MAX_AGE` seconds), its misses come from its own addresses: list its published ranges, or set `CAVEA_RATE_LIMIT_READS=0` and let the CDN limit reads. **Behind a gateway that rate-limits by itself**, both public budgets may be `0`. Either way the server warns at start about a budget at `0`.

## Origins

Buyers' pages call the public routes from their own origin, so those routes answer with CORS. `CAVEA_CORS_ORIGINS` is `*` by default: the public routes can do only what an anonymous buyer can. To limit which sites may use a publishable key from a browser, set the key's **Allowed origins** in the console; a page from another origin is refused. A script outside a browser ignores CORS: against scripts, see [bot protection](../guides/bot-protection.md).

The console and the secret-key API do not answer other origins. The console also checks that every request that changes something comes from its own pages (the browser's `Sec-Fetch-Site`, else the `Origin` against the `Host` header), which is why the proxy must pass `Host` unchanged.
