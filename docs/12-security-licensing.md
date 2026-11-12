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
| T7 | Script injection through labels, names or categories, or through uploaded images | All such strings are rendered as text, never as HTML, in the renderer, list view and console (SEC-005: no HTML sink, by lint and test); a strict Content Security Policy on the console, no inline or foreign script (SEC-004, `08-server.md` §6.5, D-67); hosts that return HTML from a custom tooltip own that risk, and the docs say so. An upload's type is decided by its bytes against PNG, JPEG and WebP (never SVG, never the client's header or file name); a truncated file or one with data after its end is refused; it is served with its sniffed type, `nosniff` and `Content-Security-Policy: default-src 'none'; sandbox` (`08-server.md` §6.2) |
| T8 | Server-side request forgery through webhook URLs or image import | Resolve the host and refuse private, loopback and link-local ranges by default; do not follow redirects to such ranges; timeouts and size caps |
| T9 | Console account takeover | argon2id (m = 19 MiB, t = 2, p = 1, OWASP's first set; 8 ms a hash measured locally), on blocking threads two at a time per node, so a burst cannot stall holds; passwords 15 to 128 characters; constant-time comparison; an unknown email and a wrong password answer alike after the same work (a dummy hash). Sign-in rate limited per client address (10 a minute) and backed off per (account, address) and per account, never against an address the member has signed in from (`08-server.md` §12, D-51). Sessions: 256-bit identifiers stored as SHA-256, a `__Host-` cookie that is `HttpOnly`, `Secure`, `SameSite=Lax`; 12 hours idle, 7 days at most; a new identifier at every sign-in; a password change ends every session; sign-out ends it on the server. Every cookie-authenticated non-safe request carries a CSRF token derived from the session and must come from the server's own origin; no credentialed CORS anywhere (`08-server.md` §11, D-50, D-54). Roles checked on every route by the permission it declares, read afresh with each request, so a lowered role or a removal applies to the next one; only an owner makes, invites or unmakes an owner, and a workspace keeps one. Invitations are one-time links of 256 bits, stored as SHA-256, for 7 days, accepted by the invited email's session or by setting a password for a new account, alike for every wrong token, never logged (`09-console.md` §7). An audit log of sign-ins, member, key, chart and event changes, written in each action's transaction; failed sign-ins only for existing accounts, off the reply's path (`09-console.md` §7). An optional second factor, TOTP with one-time recovery codes, and one-time password reset links made by an owner, an admin or the admin key (§3.2, D-89, D-90) |
| T10 | Embed token misuse (embed tokens are not built yet) | Lifetime of at most one hour; scoped to one workspace and one chart or event; bound to the embedding origin; `frame-ancestors` restricted to allowed origins |
| T11 | Denial of service on realtime | Connection limits per IP; handshake and idle timeouts; frame size limits; slow consumers are dropped to resync; snapshots are cacheable at a CDN |
| T12 | Replayed or duplicated booking calls | Idempotency keys; all-or-nothing commands |
| T13 | Forged webhooks at the customer's endpoint | HMAC signature with a timestamp; the docs require a five-minute tolerance check |
| T14 | SQL injection | Parameterised queries only; every query is a `&'static str` (sqlx refuses any other without `AssertSqlSafe`, which only tests use), so no SQL is built from input |
| T15 | Supply-chain compromise | Lockfiles; `cargo-deny` and `cargo-audit`; pinned CI actions; npm provenance; signed images; SBOM per release. As built (D-91): `cargo xtask release` runs `cargo deny check`, `cargo audit` and `bun audit` and stops on a finding (exceptions are dated, reasoned entries in `deny.toml` and `.cargo/audit.toml`); every workflow action pinned by commit SHA, the release refusing one that is not; the image and each architecture's image signed keylessly by the release workflow (cosign, Sigstore's public log), SPDX SBOMs attached as signed attestations and to the GitHub release; npm packages published with provenance from the same workflow, whose publishing job alone holds the identity token. Customers verify by `apps/docs/content/reference/verify-release.md` |
| T16 | Timing attacks on key comparison | Keys are looked up by hash; comparison is constant-time |
| T17 | Split brain between two server nodes | Fencing epoch checked inside every commit (`08-server.md` §4.8) |
| T18 | Abuse of best-available to scan inventory | Rate limited; public use is off by default per event |
| T19 | Oversized or hostile request bodies | Body size limits per route; object count limits; JSON depth limit |
| T20 | Renderer used to attack the host page | No `eval`, no dynamic script loading, no third-party requests; built-in interface isolated in a shadow root; a documented, minimal CSP |

### 3.1 As built (SEC-001, 2026-11-11)

Each row checked against the code at this date: where the mitigation is, what proves it, and what is missing with the task that owns it. Tests named without a path are in `crates/cavea-server/src/http/tests` (the database ones under `api/`); "the matrix" is `authorisation_matrix` (13 §10).

| # | As built | Where | Proof | Gap → task |
|---|---|---|---|---|
| T1 | Per-token object limit (`max_objects`, default 10, at most 100); per (key, address) rate limits for holds and reads; hold lifetime 60 s to 2 h, default 15 min; `require_session`, `max_tokens_per_sub` and the session token's `max_objects`; `cavea_holds_active` and per-command metrics | `http/events.rs` `Settings`, `limiter.rs`, `http/session.rs`, `metrics.rs` | `public_calls_are_limited_per_key_and_address`, `session_tokens`, `event_rules`, `public_flow` | No alert or webhook for unusual hold volume → **SRV-042**; no documented gateway-challenge pattern → **DOC-011** |
| T2 | 256-bit token, SHA-256 stored only; lifetime as T1; bound to the session's `sub`; booking needs a secret key | `seats.rs` `token_hash`, `http/session.rs`, the route table | `session_tokens`, the matrix (a publishable key cannot book) | None |
| T3 | Optional per-key `allowed_origins` (403 `origin`); rate limits; publishable keys reach only the buyer's routes | `http/auth.rs` `check_origin` | `origins_and_unset_admin_key`, the matrix | None (a hurdle by design) |
| T4 | Keys stored as SHA-256 with a prefix, shown once, `last_used_at` (hourly), any number of live secret keys, so rotation overlaps | `http/keys.rs`, `http/auth.rs` | `key_lifecycle`, `admin_mints_a_secret_key` | Restricted keys (P2) → SRV-052; `last_used_at` shown on the keys screen → CON-010 |
| T5 | The workspace from the credential only; every query bound to it; chart and image files public by content hash | `http/auth.rs`, `http/staff.rs`, every handler | the matrix (every route × 19 credentials, foreign identifiers `404`), `identical_keys_never_cross`, `every_route_has_a_row` | None |
| T6 | Decoders checked against hostile bytes and fuzzed; body and object limits; the renderer loads the chart the event names, from the server it was given | `cavea-format`, `cavea-engine` wire, `fuzz/` (`ccf_decode`, `wire_decode`, `snapshot_apply`, `document_parse`) | the fuzz targets (13 §5), `cavea-format`'s hostile-input tests | None |
| T7 | Renderer: text only, `innerHTML` and its kin throw in the test DOM; console: no HTML sink in any source, lint `no-restricted-properties`; the console's policy (08 §6.5); uploads sniffed, `nosniff`, `sandbox` | `packages/renderer/src/ui.ts`, `a11y.ts`; `apps/console`; `.oxlintrc.json`; `http/headers.rs`; `assets.rs` | `ui.test.ts` "strings from the chart and the host are text", `apps/console/src/text.test.ts`, `every_route_carries_its_familys_headers`, `assets_are_sniffed_stored_and_served`; the browser proof (08 §6.5) | List view not built (M6, its own text test then) |
| T8 | Nothing makes an outbound request: the server's only connections are PostgreSQL and its own health probe to 127.0.0.1; no image import, no webhooks yet | — | review of `crates/cavea-server` and `cavea-engine` (no HTTP client in the tree) | The guard itself comes with webhooks → SEC-002 (open) |
| T9 | As the row says: argon2id, the sign-in budget and back-offs, `__Host-` cookie, CSRF and origin checks, roles per request, one-time invitations, audit log | `console/`, `http/console.rs`, `http/staff.rs`, `http/members.rs`, `audit.rs` | `sign_in_is_uniform_rotates_and_signs_out`, `sign_in_backs_off_without_locking_out`, `sessions_slide_and_end`, `a_password_change_ends_every_session`, `role_changes_apply_to_the_next_request`, `invitations_work_once_for_their_email`, the matrix (CSRF, origin, roles) | TOTP and password reset → CON-003 |
| T10 | Not built | — | — | SRV-038 |
| T11 | Connections per node and per address, handshake and idle timeouts, a 2 KiB cap on client messages, slow subscribers resynced, snapshot `max-age` for a CDN | `realtime.rs`, `http/realtime.rs` | `websocket_refuses_what_breaks_the_rules`, `a_stalled_sse_stream_keeps_its_slot`, `snapshot_and_polling` | None |
| T12 | `Idempotency-Key` with fingerprint and mismatch; commands all-or-nothing | `http/seats.rs`, `cavea-engine` | `idempotency`, `cavea-sim`'s model tests | None |
| T13 | Not built | — | — | SRV-032, SRV-033 |
| T14 | As the row says | `sqlx` without `macros`; `&'static str` queries | review: `AssertSqlSafe` appears only in tests | None |
| T15 | Lockfiles, `--locked` and `--frozen-lockfile` in CI | `Cargo.lock`, `bun.lock`, `.github/workflows/ci.yml` | CI | CI actions pinned by tag, not commit; `cargo-deny`, `cargo-audit` and a JavaScript audit run nowhere → **SEC-010**; signed images, SBOM, npm provenance → SEC-008 |
| T16 | Keys found by their hash; the admin key, CSRF token, setup and invitation tokens and session-token signatures compared in constant time | `http/auth.rs`, `http/staff.rs`, `console/setup.rs`, `http/members.rs`, `http/session.rs` (`subtle`) | review | None |
| T17 | Fencing epoch checked in every commit, renewal and release | `store/mod.rs`, `cavea-engine` actor | `cavea-sim` (`sim`, counted fenced commits), the soak runs (13 §3.1) | None |
| T18 | Not built | — | — | SRV-030 |
| T19 | 256 KiB default body limit, larger per route; object count and JSON depth limits | `http/extract.rs`, `http/mod.rs` | `body_size_and_json_depth_limits` | None |
| T20 | As the row says; also `no-eval` and `no-new-func` in the lint | `packages/renderer`, `.oxlintrc.json` | `bun fixtures/run.ts csp-*` with Trusted Types (05 §10, 13 §8) | None |

Found and fixed in this review (SEC-004, -005, -006, -009): no security headers but `nosniff` on the console (now 08 §6.5, with HSTS when the public URL is HTTPS); no proof that logs carry no secret beyond the matrix's own credentials (`secrets_never_reach_the_logs`), and typed bodies that would print a password with `Debug` (now `Secret`); the startup warning covered the public URL and the admin key only (now §4's list); the threat table claimed compile-time checked SQL (corrected above).

### 3.2 The second factor and reset links (CON-003, 2026-11-12)

What each choice answers (`09-console.md` §2, §10; D-89, D-90):

- **A guessed, reused or phished password** is no longer enough for a user who turns on the second factor: signing in takes a current code from their authenticator as well. TOTP (RFC 6238) is what every authenticator app speaks; it needs no message sent, which the server cannot do.
- **The second step cannot skip or replace the password.** Only a verified password makes a challenge (256 bits, shown once, stored as SHA-256, five minutes), bound to the user and to the password hash it was verified against: a password changed or reset meanwhile kills it. It is deleted when used, so it cannot be replayed, and dies after five wrong codes.
- **Sign-in is not an oracle.** Up to the password's verification, the reply, its status and the work are the same whether the account exists or has a second factor (the same dummy hash as before); only a right password learns that a code is needed.
- **Code guessing.** A code is six digits and three are accepted at a time (a step either way, for clock drift), so each wrong code counts in the sign-in back-off exactly as a wrong password (per address and per account, D-51), the password's success does not clear the address's failures until the code is right too, and every attempt draws on the per-address budget. With the back-off's doubling to 15 minutes, guessing is a few hundred tries a day per account at most: about 1 in 1,000 a day.
- **Replay of a seen code.** The last step whose code was accepted is stored, and a code of that step or an earlier one is refused (RFC 6238 §5.2); the statement that accepts a code records its step, so two requests with one code cannot both pass. A recovery code is deleted when used. Comparisons of codes are constant-time; recovery codes and reset tokens are looked up by SHA-256, like API keys.
- **A lost phone** is recovered by a recovery code (ten, about 50 bits each, shown once, stored as SHA-256), or by an owner or admin resetting the factor (audited); **a lost password** by a reset link an owner or admin makes (nothing is sent to emails, so there is no self-service reset), single use, an hour, stored as SHA-256, ending every session of that person. The two are separate: a reset link clears the factor only when its maker chose so, since otherwise whoever obtains a link could also skip the second factor. An instance whose only owner lost both recovers with the admin key (an owner's link with `clear_second_factor`).
- **One workspace's admin weakening another's account.** An account is the instance's, not a workspace's, so a reset needs the caller to be able to manage the person in every workspace the person belongs to (owner or admin there, owner for an owner): otherwise `other_workspaces`. Nobody resets their own account through these routes; turning one's own factor off needs the password and a code.
- **The secret at rest is not hashed, and cannot be:** the server computes the codes from it. What protects it is what protects the database: its access control, the operator's handling of backups and of the database's credentials. Whoever can read `users.totp_secret` can make codes for that user, but still needs the password, whose argon2id hash that same reader cannot reverse cheaply; a read of the database is therefore not a sign-in. The server does not encrypt the secret with a key of its own: a key on the same host as the database's credentials would add little against a host compromise and would be one more secret to lose. The secret is never logged, never returned after enrolment, and sent once only in a `Secret` body (`secrets_never_reach_the_logs` enrols, signs in with a challenge and a recovery code, turns the factor off and uses a reset link, and finds none of them in the logs).

## 4. Secure defaults

- Public best-available: off.
- Exact general-admission counts in public state: off.
- Private network destinations for webhooks: refused.
- Console sign-up: disabled. The first account is the owner, made with the one-time setup link of a server that has no user (`09-console.md` §2); every other comes by an invitation of an owner or admin, or of the admin key for a workspace's owner (`09-console.md` §7).
- CORS for secret-key routes: none.
- Logs: no hold tokens, no order references unless enabled, no request bodies; no passwords or their hashes, console session identifiers, CSRF tokens, invitation or reset tokens or links, sign-in challenges, TOTP secrets, codes or recovery codes, or emails (a console user appears as `usr_…`). The one secret ever logged is the first-run setup link, on purpose: one `info` record, only while no user exists, valid once for an hour (`09-console.md` §2). As built (SEC-009): the request span records the route template, never the path, query or headers; secrets in typed bodies print as `Secret(..)`; `secrets_never_reach_the_logs` drives every route with recognisable secrets in every place one can go and finds none in everything logged at `trace` (`08-server.md` §15). Order references are never logged; there is no setting to enable them yet.
- TLS: required in production; the server warns when `CAVEA_PUBLIC_URL` is not HTTPS (unless on `localhost`), and sends `Strict-Transport-Security` when it is (`08-server.md` §6.5).
- Insecure settings: one `warn` record each at start, naming the variable and never its value (`08-server.md` §13, SEC-006). The server refuses to start only on a value it cannot use, such as an admin key under 32 characters.
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

- Signed with Ed25519. The public keys are embedded in the server and the renderer; more than one key is accepted so that keys can be rotated. As built: §7.6.
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

The rule behind the table: **a licence problem never stops a sale** (`DECISIONS.md` D-20). The server's column as built, and exactly what "read-only" refuses: §7.6.

### 7.4 Renderer licence check

- For customers using the server, the public event endpoint includes the licence state; the renderer shows or hides the mark accordingly. As built: `licence: {status, domains}` on the server (§7.6), the renderer's decision in §7.7.
- For renderer-only customers, the licence string is passed in the options and verified in the browser with the platform's Ed25519 support. Where that is unavailable, the licence is treated as valid.
- The check runs after the first interactive frame and never delays it.

### 7.5 Piracy

A determined customer can remove a watermark. We accept that. The paid value is updates, support, the embeddable tools and the right to use the software commercially. No anti-tamper measures that could break a checkout will be added.

### 7.6 As built (LIC-001, LIC-002, LIC-005, 2026-11-11)

**The file** (`crates/cavea-licence`, D-80). One JSON object, at most 16 KiB, UTF-8:

```json
{"cavea_licence":1,"key_id":"<16 hex digits>","payload":"<base64>","signature":"<base64>"}
```

- `payload` is the licence document of §7.2, its exact bytes as issued, in standard base64 with padding. `signature` is the Ed25519 signature (64 bytes, base64) of the ASCII bytes `cavea-licence-v1`, one zero byte, and the payload's bytes. `key_id` is the first 8 bytes of the SHA-256 of the signing key's 32-byte public key, in lowercase hex: derived from the key, so it cannot name another.
- What is signed is the payload's bytes; nothing is ever re-serialised. A verifier checks the envelope, finds the trusted key the id names, checks the signature, and only then reads the payload as JSON.
- Strict about signatures, lenient about the rest. Unknown fields of the envelope and of the payload are ignored, so a licence issued with a field a later version adds still verifies on an older server; an unknown `plan` or feature name is carried as text. Required in the payload: `licence_id`, `customer`, `plan`, `issued_at`, `expires_at`; `features` and `domains` default to empty, `max_workspaces` to `null` (unlimited), `grace_days` to 0. `domains` are lowercased.
- **Invalid**, with a reason for the log and the owner's screen (`cavea_licence::Invalid`): `too_large` (over 16 KiB), `not_utf8`, `malformed` (not the envelope: not JSON, nested deeper than 16, a field missing, duplicated or of the wrong type, base64 that does not decode), `version` (an envelope other than 1), `unknown_key`, `signature`, `payload` (signed, but not a licence document), `dates` (a date that does not exist, or expiry before issue); the server adds `unreadable` (the file named cannot be read). Nesting is checked by a scan before parsing; the size cap bounds the rest. None panics. Proof: `cavea-licence`'s tests (every byte of an issued file changed several ways is invalid; the issued bytes verify; hostile files; signed nonsense; newer fields).
- **Time.** Dates are `YYYY-MM-DD` in UTC and must exist. A licence is valid from 00:00 UTC on `issued_at` through 23:59:59.999 UTC on `expires_at`; in grace for the `grace_days` whole days after; expired from 00:00 UTC on the day after the last day of grace. A clock before `issued_at` (a server whose clock is wrong, or a licence issued ahead of its term) counts as valid and is logged (`before_issue`): only expiry, never a clock alone, takes a licence away. The state is computed from the server's clock on every read, never cached, so a server running across a boundary changes state without a restart (`the_state_moves_with_the_clock`, on the engine's injected clock).

**Trusted keys** (D-80). The public keys are compiled in: `TRUSTED` in `crates/cavea-licence/src/keys.rs`, a list of base64 keys. **It is empty**: no private key exists yet, so every build treats every licence as `invalid` (`unknown_key`), which is the "missing or invalid" state, and sells normally. No setting, file or environment variable adds a key: whoever runs the server controls those, and a key they could add would let them sign their own licences. Tests trust their own key, made at run time, through `Verifier::new`, which only code can call; a by-hand test build may add one key at compile time with the `trust-test-key` feature (below). Neither is in any release.

**What the owner does before the first licence** (offline, on a machine that is not this one):

1. Check out the repository on an offline machine with the flake (`nix develop`), and make a directory on encrypted storage that is backed up.
2. `cargo xtask licence keygen --out <that directory>/2027a`. It writes `signing-key` (mode 0600, refused if it exists) and `public-key`, and prints the key id and a base64 line.
3. Open a pull request that adds that one line to `TRUSTED` with a comment of its key id and the date, e.g. `"<base64>", // <key id>, added 2027-01-05`. Nothing else changes; every release after it accepts licences signed by the key.
4. Issue: `cargo xtask licence issue --key <dir>/2027a/signing-key --id lic_2027_0001 --customer "<name>" --plan growth --expires 2028-01-10 --domain tickets.example.com --domain '*.example.com' --feature embed --dry-run`, read what it prints, then run it again with `--out lic_2027_0001.json` in place of `--dry-run`. `--issued` defaults to today, `--grace-days` to 30, `--max-workspaces` to unlimited; names are checked (lowercase), domains are checked, dates must exist, expiry must be after issue and within three years. The private key is read from the file only, never from an argument, and refused if others may read it; the tool writes no key material anywhere else and never overwrites a file.
5. Check: `cargo xtask licence verify lic_2027_0001.json --public-key <dir>/2027a/public-key` (and, once a build trusts the key, without `--public-key`). `inspect` shows what any licence file says without checking it.
6. Send the customer the `.json` file. They set `CAVEA_LICENCE` to its path or its contents, or upload it in the console.

**Rotation.** Generate a second key the same way, add its line beside the first, ship a release, and sign new licences with it. A licence verifies on any server whose build has its key, so retire an old key (remove its line) only once every licence it signed has expired or been replaced, and only in a release made after that. A key that leaks is removed at once, and its customers are sent new licences with the release.

**The server** (`crate::licence`, D-81).

- **Source.** `CAVEA_LICENCE`: the file's path, or the file's contents when the value starts with `{` (empty is no licence; no value of it can stop the server starting); only a regular file is read; and a licence uploaded through the console, stored as uploaded in the one-row table `licence` (migration 8), so every node reads the same one. When both exist the better counts: valid, then grace, then expired, then invalid, then missing; then the later `expires_at`; then the upload. An upload can therefore never make things worse than the setting, and a stale file cannot hide a renewal.
- **Reading.** At start (after migrating; serving does not wait more than 5 s for it), and every 30 s on each node, the file named and the stored upload are read again; the node that takes an upload uses it at once. Every node reaches the same licence within 30 s of an upload or of a file replaced on disk, and the public event's `max-age=30` adds at most 30 s in a browser. A database that cannot be read keeps the upload last read. The state itself is computed per request from memory: a read lock and a clone of a few small fields.
- **Logs.** One record at start and one whenever the state or the licence's id changes (the clock crossing a boundary included, within 30 s): `licence` with `state`, `licence` (the id), `source` and, when invalid, `reason`; `info` while valid or in grace, `warn` otherwise with "seat operations are unaffected". Never the file, the payload or the signature. An upload logs `licence uploaded` with the user and the id, and writes the audit row `licence.uploaded` (target the licence's id, detail its expiry) in its transaction.

**Behaviour by state, as built.**

| State | Server | Refused |
|---|---|---|
| Valid, grace | Normal | Nothing |
| Expired | Seat operations, the public routes and every secret-key call as always | With a **console session only**, the routes whose permission is `charts.write`, `charts.archive` or `events.write`: `POST /v1/charts`, `PUT /v1/charts/{chart}/draft`, `POST /v1/charts/{chart}/validate`, `POST /v1/charts/{chart}/publish`, `PUT /v1/charts/{chart}/versions/{number}/thumbnail`, `POST` and `DELETE /v1/charts/{chart}/lock`, `DELETE /v1/charts/{chart}`, `POST /v1/assets`, `POST /v1/events`, `PATCH /v1/events/{event}`, `DELETE /v1/events/{event}`: `403` with `reason: "licence_expired"`, after the role check |
| Missing or invalid | Normal | Nothing |

Never refused in any state: every seat operation (holds, releases, bookings, status changes, hold tokens) by any credential, the console's event manager included; the public routes, the snapshot, polling, the stream and the WebSocket; every call with a secret key or the admin key (a backend creating tomorrow's event is selling); reads; API keys (a leaked key must be revocable); members, invitations, the workspace's name, the audit log; setup, sign-in, sign-out, a password change; and the licence upload. Structure: the refusal is one check in the console-member extractor (`http::staff`), reached only when the route's permission is one of the three (`Permission::closed_while_expired`); no seat route's permission is, so no seat route reads the licence, and no seat route can fail on it. `Licences::status` and `public` cannot fail.

**What the public event endpoint discloses** (`GET /v1/public/events/{event}`, `PublicLicence`): `"licence": {"status": "licensed" | "expired" | "unlicensed", "domains": [..]}`. `licensed` is valid or in grace (grace is not disclosed); `unlicensed` is missing or invalid; `domains` are the licence's domain patterns while licensed, else empty. Nothing else: not the customer, plan, id, features, dates or key. A verdict from `Origin` was rejected: the server sees no `Origin` on a same-origin `GET`, and the renderer knows its own host for certain. Older renderers ignore the field (they read only the fields they know); the snapshot and the stream are unchanged.

**What the console reads** (`ConsoleMe.licence`, `ConsoleLicence`): for every signed-in user `state`, `read_only` (expired), `grace_until` (the last day of grace, while in grace) and `manage`; for a user who owns every workspace on the server (`manage: true`) also `source`, `reason` (while invalid) and `licence` (id, customer, plan, features, domains, max workspaces, dates, grace days, key id); `null` for others. `PUT /console/api/licence` `{licence: "<the file's text>"}` takes a console session of such a user (`403` `instance_owner` otherwise, so that on a server shared by several organisations no workspace's owner can change what all run under); a file that does not verify is `400` `invalid_field` (`field: "licence"`, `problem`: the reason) and nothing is stored; a verified one, even expired, is stored and answered as `ConsoleLicence`. A server shared by organisations whose owners are not the operator keeps its licence in `CAVEA_LICENCE`.

**`max_workspaces` and `features`** are reported to managers and enforce nothing yet: the features they name are not built. Later: `max_workspaces` at workspace creation (`POST /v1/workspaces`, a refusal the admin key sees, never a seat path), `embed` when embed tokens exist (SRV-038), `white_label` when the embeddable designer and manager can be branded, `webhooks` when they exist (SRV-032), each refusing the feature's own configuration and never a sale.

**Proof** (LIC-005, `http/tests/api/licence.rs`): `seat_operations_ignore_the_licence` runs 25 seat operations (public hold token, its read, public hold and release; secret-key hold token, read, extend, hold, release, book by token, book, release booked, change status, delete a token; a console member's hold token, hold, book, change status, release; the objects list; the public event; the snapshot, a poll, the stream and a WebSocket's first snapshot) on a server of its own in each of ten states: valid, grace, expired years ago, missing, malformed, signed by another key, issued in the future, for another domain, a valid file replaced by garbage while the server runs, garbage stored as the upload. Every answer, normalised for tokens, times and the chart's hash, equals the valid run's. `expired_console_is_read_only_but_for_seats` checks the refusals above and what is not refused, with the upload restoring writes at once; `what_each_one_is_told` the public disclosure in each state, `me` for a manager and not, the upload's refusals, its audit row, and a second node reading it; `the_state_moves_with_the_clock` the boundaries on an injected clock. The authorisation matrix has the upload's row.

**Trying it by hand.** A build that trusts a test key: `CAVEA_TEST_LICENCE_KEY=<base64 public key> cargo build -p cavea-server --features trust-test-key` (the key is compiled in; the build fails without the variable, and with `--release`). A normal build given the same variable at run time trusts nothing.

### 7.7 As built (LIC-003, LIC-004, 2026-11-12)

**The renderer's mark** (`packages/renderer/src/licence.ts`, D-84). Decided once, after the first frame drawn with the first state (the `interactive` mark), on a timer of its own: the first frame never waits for it, and nothing re-checks it.

| What the map has | On a licensed host | On another host | On loopback or an opaque origin |
|---|---|---|---|
| `caveaServer`, `licensed` (valid or grace) | none | "Development licence" | none |
| `caveaServer`, `expired` | "Powered by Cavea" | "Powered by Cavea" | "Powered by Cavea" |
| `caveaServer`, `unlicensed` (missing or invalid) | "Development licence" | "Development licence" | "Development licence" |
| `caveaServer`, no `licence` field (a server older than licensing), or one this version cannot read (not an object, an unknown `status`, `domains` not a list or longer than a 16 KiB licence can hold) | none | none | none |
| A custom adapter, `licence` option verified: valid or grace | none | "Development licence" | none |
| …verified and expired | "Powered by Cavea" | "Powered by Cavea" | "Powered by Cavea" |
| …not a licence, or one that does not verify (today: every licence, `unknown_key`, since no key is trusted) | "Development licence" | "Development licence" | "Development licence" |
| …well formed and from a trusted key, but the browser cannot check Ed25519 (no WebCrypto Ed25519, or not a secure context) | none | none | none |
| A custom adapter without `licence` (staticState included) | "Development licence" | "Development licence" | "Development licence" |
| Anything in the path throwing (a hostile option, WebCrypto throwing, a broken adapter report) | none | none | none |

- **Hosts.** The page's host is its origin's host name (ASCII, lowercase, no port). A pattern `x.com` covers `x.com`; `*.x.com` covers every name under `x.com` at any depth and not `x.com`; nothing else matches (no suffix tricks, a final dot ignored, a pattern with anything but letters, digits, dots and hyphens matches nothing, Unicode patterns compared in their ASCII form, a wildcard never covers an IP address). Loopback (`localhost`, `*.localhost`, `127.0.0.0/8`, `[::1]`) is covered by any licence, so a developer sees there what their production page shows; an opaque origin (a file, a sandboxed frame) cannot be known and counts as covered. Development builds show the same marks; they add one console warning saying which mark and why.
- **Renderer-only verification.** The `licence` option is the file's text. It is checked as the server checks it (§7.6): at most 16 KiB, the envelope's types, version 1, canonical standard base64, a trusted key by id, a 64-byte signature verified with `crypto.subtle` (`Ed25519`) over `cavea-licence-v1`, a zero byte and the payload, then the payload's fields and dates, against the page's clock. The trusted keys are `packages/renderer/src/generated/licence-keys.ts`, generated from `TRUSTED` by `cargo xtask types` (`--check` and `cargo test -p xtask` fail while it is stale); the shipped map reads them from there only. Tests reach the verification through its `keys` argument with a key made at run time; no option, global or export of the published package can add one. Proof: `licence.test.ts` (licences issued by `cargo xtask licence` with a run-time key verify and move through valid, grace and expired with the clock; every single-byte mutation of the file, with the replacements the server's test uses, is invalid; hostile inputs; no Ed25519 means `unknown`; today's empty key list means `unknown_key`).
- **The mark.** Text in the map's shadow root (`.w`), bottom inline-end, 12 px or `.75rem`, the theme's muted text on its background (contrast as the controls', 05 §8), `pointer-events: none` (a press on it reaches the map), not focusable, not a live region (a screen reader reaches it as text, it never interrupts), not a link (the owner has chosen no destination), with no `part`. It hides with the controls while the loading or error state shows; the legend keeps clear of the room it may take; a keyboard-focused seat is kept clear of it as of the controls. No request, no font, no image, no inline style a Content Security Policy would refuse. Strings: `licenceDevelopment`, `licencePoweredBy` (05 §3.4).
- **The console's own maps** (the event manager and the designer's preview) carry no mark: `consoleMap` from `@cavea/renderer/licence`, a workspace-only entry that the published package does not export.
- **Payload.** About 1.5 KB brotli (the mark and its decision about 0.9 KB, the verification about 0.7 KB); WebCrypto does the cryptography. **Timing** (AMD Ryzen 9 7900X, Linux, headless Chromium 155 with SwiftShader WebGL2, a debug server on loopback, 25 interleaved runs, medians): chart-ready and the interactive frame unchanged within the run-to-run spread, with `caveaServer` and with a custom adapter carrying a licence string; the mark appears about 3 ms after the interactive frame.

**The console** (`console/Licence.tsx`, `console/licence.ts`, `api/permissions.ts`).

- **Banners**, above every console screen and on the event page: valid, none; grace, for whoever manages the server and members whose role opens Settings (owners and admins), with the last day of grace; expired, for whoever manages it and every role that writes charts or events (owner, admin, editor), saying charts and events are read-only and selling carries on; missing or invalid, one quiet line for everyone ("Development licence…"), which can be hidden for the tab and comes back from a small control in the navigation, never hidden for good. Whoever manages the server gets a link to the licence screen; others are told to ask an owner of the server.
- **Read-only.** One rule, `refusal(workspace, operation, readOnly)`: the role first, then, while `me.licence.read_only`, the three permissions the server closes (`charts.write`, `charts.archive`, `events.write`) and no other. Every screen's `allows` goes through it, so a control the server would refuse is disabled with the licence's reason ("New chart", "Import chart file", duplicate and archive in the charts list, a chart's archive and its "New event", the events list's "New event" and archive, an event's settings and archive), and the designer opens read-only through its read-only mode, saying why. Seat actions in the event manager, keys, members and settings are untouched. A write attempted anyway (a page read before expiry) is refused by the server, said in words (`licence_expired`), and `me` is read again. A test reads the server's `closed_while_expired` from its source and checks that exactly the twelve routes above close.
- **Settings → Licence**: the state in words; for whoever manages the server, the customer, plan, licence id, issue, expiry and grace dates (UTC days, in the person's locale), licensed domains (a wildcard explained), features, workspaces, signing key and where it came from, with a note that features and workspaces are not enforced; the upload (choose a file or paste its text; a file over 16 KiB refused before it is sent; a refusal shown on the field in words, every `reason` of §7.6 having a message, which a test checks against the crate and the server's source; the new state shown at once). Others see the state and whom to ask.
- **Proof** (browser, 2026-11-12): a debug server trusting a test key (`trust-test-key`), on a database of its own, restarted in each of valid, grace, expired, invalid and missing; the buyer's map (`caveaServer`) on a licensed host, another host and localhost, light and dark, 1,000 px and a 375 px phone, with the mark expected in each, never over a control, a press on it reaching the map, and selecting and holding a seat working; the banner each of owner, admin, editor, operator and viewer sees; the licence screen; the designer read-only and the event settings closed while expired, a console seat status change succeeding, and a write from a page read before expiry refused with the licence's words; a tampered upload refused on its field and a valid one applied at once; a custom-adapter page with no licence, a valid one (through the test seam) and a tampered one on each host.

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
