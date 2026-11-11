# 12 · Security and licensing

**Status:** Draft v0.1 · 2026-11-10

## 1. What we protect

| Asset | Why it matters |
|---|---|
| Seat state integrity | A double booking or a lost booking is the worst failure the product can have |
| Seat availability | Inventory that bots hold cannot be sold |
| Secret keys and console accounts | They control every seat in a workspace |
| Tenant isolation | A platform's organisers must never see or change each other's data |
| The customer's checkout page | Our renderer runs inside it; a flaw in our code is a flaw in their page |

Cavea stores no payment data and, by design, no personal data about buyers.

## 2. Trust boundaries

```
Buyer's browser (untrusted) ──publishable key, hold token, session token──▶ cavea-server
Customer backend (trusted by its workspace) ──secret key──▶ cavea-server
Staff browser ──console session / embed token──▶ cavea-server
cavea-server ──▶ PostgreSQL (trusted)        cavea-server ──▶ webhook endpoints (untrusted destination)
```

Everything arriving from a browser is untrusted, including chart files and realtime frames on the client side.

## 3. Threats and mitigations

| # | Threat | Mitigation |
|---|---|---|
| T1 | **Seat hoarding:** a bot holds large parts of the house to deny sales or to resell | Per-token object limit; rate limit per key and IP; short hold lifetime; optional mandatory session tokens signed by the customer's backend (`require_session`), with a cap on each `sub`'s live hold tokens per event (`max_tokens_per_sub`, default 2) and on each token's objects (`max_objects` in the session token) (`08-server.md` §11); metrics and a webhook for unusual hold volume; documented pattern for a challenge at the customer's gateway |
| T2 | Stolen hold token | Bearer secret with short life; only a hash is stored; bound to the session `sub` when created with a session token, and refused with any other `sub` or none (`08-server.md` §11); cannot book (booking needs a secret key) |
| T3 | Publishable key used from another site | Origin allow-list (a hurdle, not a guarantee); rate limits; the key can only do what an anonymous buyer can |
| T4 | Leaked secret key | Hashed at rest; shown once; last-used time visible; rotation with two valid keys during the overlap; restricted keys (P2) |
| T5 | Cross-tenant access | Workspace comes from the credential only; every query filters by workspace; an automated test calls every endpoint with every kind of credential and with a foreign workspace's identifiers, and expects 404 for those (13 §10). The exceptions are compiled chart files and uploaded images: public by nature, served without a credential and named only by their 128-bit content hash (`08-server.md` §6.2, D-26, D-49) |
| T6 | Malicious compiled chart or realtime frame | Hardened, fuzzed decoders; strict limits; the renderer loads charts only from the configured origin |
| T7 | Script injection through labels, names or categories, or through uploaded images | All such strings are rendered as text, never as HTML, in the renderer, list view and console; a strict Content Security Policy on the console (SEC-004; the server serves the console's files from one place, `http/web.rs`, with `nosniff` already, and its built `index.html` has no inline script); hosts that return HTML from a custom tooltip own that risk, and the docs say so. An upload's type is decided by its bytes against PNG, JPEG and WebP (never SVG, never the client's header or file name); a truncated file or one with data after its end is refused; it is served with its sniffed type, `nosniff` and `Content-Security-Policy: default-src 'none'; sandbox` (`08-server.md` §6.2) |
| T8 | Server-side request forgery through webhook URLs or image import | Resolve the host and refuse private, loopback and link-local ranges by default; do not follow redirects to such ranges; timeouts and size caps |
| T9 | Console account takeover | argon2id (m = 19 MiB, t = 2, p = 1, OWASP's first set; 8 ms a hash measured locally), on blocking threads two at a time per node, so a burst cannot stall holds; passwords 15 to 128 characters; constant-time comparison; an unknown email and a wrong password answer alike after the same work (a dummy hash). Sign-in rate limited per client address (10 a minute) and backed off per (account, address) and per account, never against an address the member has signed in from (`08-server.md` §12, D-51). Sessions: 256-bit identifiers stored as SHA-256, a `__Host-` cookie that is `HttpOnly`, `Secure`, `SameSite=Lax`; 12 hours idle, 7 days at most; a new identifier at every sign-in; a password change ends every session; sign-out ends it on the server. Every cookie-authenticated non-safe request carries a CSRF token derived from the session and must come from the server's own origin; no credentialed CORS anywhere (`08-server.md` §11, D-50, D-54). Roles checked on every route by the permission it declares, read afresh with each request, so a lowered role or a removal applies to the next one; only an owner makes, invites or unmakes an owner, and a workspace keeps one. Invitations are one-time links of 256 bits, stored as SHA-256, for 7 days, accepted by the invited email's session or by setting a password for a new account, alike for every wrong token, never logged (`09-console.md` §7). An audit log of sign-ins, member, key, chart and event changes, written in each action's transaction; failed sign-ins only for existing accounts, off the reply's path (`09-console.md` §7). Not built yet: TOTP |
| T10 | Embed token misuse (embed tokens are not built yet) | Lifetime of at most one hour; scoped to one workspace and one chart or event; bound to the embedding origin; `frame-ancestors` restricted to allowed origins |
| T11 | Denial of service on realtime | Connection limits per IP; handshake and idle timeouts; frame size limits; slow consumers are dropped to resync; snapshots are cacheable at a CDN |
| T12 | Replayed or duplicated booking calls | Idempotency keys; all-or-nothing commands |
| T13 | Forged webhooks at the customer's endpoint | HMAC signature with a timestamp; the docs require a five-minute tolerance check |
| T14 | SQL injection | Parameterised queries only, checked at compile time; no SQL is built from input |
| T15 | Supply-chain compromise | Lockfiles; `cargo-deny` and `cargo-audit`; pinned CI actions; npm provenance; signed images; SBOM per release |
| T16 | Timing attacks on key comparison | Keys are looked up by hash; comparison is constant-time |
| T17 | Split brain between two server nodes | Fencing epoch checked inside every commit (`08-server.md` §4.8) |
| T18 | Abuse of best-available to scan inventory | Rate limited; public use is off by default per event |
| T19 | Oversized or hostile request bodies | Body size limits per route; object count limits; JSON depth limit |
| T20 | Renderer used to attack the host page | No `eval`, no dynamic script loading, no third-party requests; built-in interface isolated in a shadow root; a documented, minimal CSP |

## 4. Secure defaults

- Public best-available: off.
- Exact general-admission counts in public state: off.
- Private network destinations for webhooks: refused.
- Console sign-up: disabled. The first account is the owner, made with the one-time setup link of a server that has no user (`09-console.md` §2); every other comes by an invitation of an owner or admin, or of the admin key for a workspace's owner (`09-console.md` §7).
- CORS for secret-key routes: none.
- Logs: no hold tokens, no order references unless enabled, no request bodies; no passwords or their hashes, console session identifiers, CSRF tokens, invitation tokens or links, or emails (a console user appears as `usr_…`). The one secret ever logged is the first-run setup link, on purpose: one `info` record, only while no user exists, valid once for an hour (`09-console.md` §2).
- TLS: required in production; the server warns when `CAVEA_PUBLIC_URL` is not HTTPS.
- Admin key: there is no default; it must be set explicitly (`CAVEA_ADMIN_KEY`, at least 32 characters, or the server will not start). Unset, the admin endpoints refuse every request (`DECISIONS.md` D-24).

## 5. Data and privacy

| Data | Stored | Notes |
|---|---|---|
| Charts, events, seat state | Yes | Not personal data |
| Order references | Yes | The customer's opaque identifier. The docs tell customers not to put personal data in it |
| `extra` per object | Yes | Free-form; the customer controls its content |
| Session `sub` | Its SHA-256 only, on the hold it created, until the hold is deleted (expiry, or booking its last objects); never logged, never in the event log | Should be an opaque identifier |
| Buyer IP addresses | In memory for rate limiting; in logs only if the customer enables it | |
| Console users | Email, password hash; an invited email until the invitation is accepted, revoked or replaced | The only personal data we require. The audit log holds identifiers, never an email or an address |

- The software sends nothing to us. Update checks and usage statistics are opt-in.
- Encryption at rest and backups are the customer's PostgreSQL configuration.
- For the hosted AI import service only, we process uploaded files; they are deleted after the job (`10-ai-import.md` §6).

## 6. Security process

- `SECURITY.md` with a contact address and a disclosure policy.
- Fix targets: critical within 72 hours, high within 14 days.
- Security advisories are published with each fix; customers on a support plan are notified directly.
- Supported versions: the latest two minor releases.
- Before 1.0: an external review of the engine's hold and booking paths and of the authentication code, as budget allows.

## 7. Licensing

### 7.1 Model

- Commercial licence, sold per customer organisation, annual.
- Plans and prices are in `00-product.md` §8.
- Development, staging and evaluation use are free and show a watermark.
- **Open question:** whether to publish the renderer's source under a source-available licence to build trust and adoption. Decide after the first customer conversations (`14-go-to-market.md` §11). The server and designer stay closed either way; the Scale plan includes source access.

### 7.2 Licence file

A signed JSON document:

```json
{
  "licence_id": "lic_2027_0142",
  "customer": "Acme Tickets Ltd",
  "plan": "growth",
  "features": ["embed", "white_label", "webhooks"],
  "domains": ["tickets.acme.example", "*.acme.example"],
  "max_workspaces": null,
  "issued_at": "2027-01-10",
  "expires_at": "2028-01-10",
  "grace_days": 30
}
```

- Signed with Ed25519. The public keys are embedded in the server and the renderer; more than one key is accepted so that keys can be rotated.
- Verified offline. No network call is ever made to check a licence.
- Issued by a private command-line tool. The signing key is kept offline.

### 7.3 Behaviour by state

| State | Server | Console | Renderer |
|---|---|---|---|
| Valid | Normal | Normal | No watermark on licensed domains |
| Grace (expired, within `grace_days`) | Normal | Banner for owners and admins | No watermark |
| Expired | **Seat operations continue** | Read-only except seat operations in the event manager; banner | Small "Powered by Cavea" mark |
| Missing or invalid | Seat operations continue | "Development" banner | "Development licence" mark |
| Unlicensed domain | — | — | Watermark |

The rule behind the table: **a licence problem never stops a sale** (`DECISIONS.md` D-20).

### 7.4 Renderer licence check

- For customers using the server, the public event endpoint includes the licence state; the renderer shows or hides the mark accordingly.
- For renderer-only customers, the licence string is passed in the options and verified in the browser with the platform's Ed25519 support. Where that is unavailable, the licence is treated as valid.
- The check runs after the first interactive frame and never delays it.

### 7.5 Piracy

A determined customer can remove a watermark. We accept that. The paid value is updates, support, the embeddable tools and the right to use the software commercially. No anti-tamper measures that could break a checkout will be added.

## 8. Legal documents needed

| Document | For |
|---|---|
| Licence agreement | All customers |
| Support policy | Paid plans |
| Data processing agreement | The hosted import service |
| Privacy policy | Website and hosted service |
| Terms for the hosted import service | Users of AI import |
| Contributor terms | Only if any component becomes source-available |

Have a lawyer review the licence agreement before the first sale.
