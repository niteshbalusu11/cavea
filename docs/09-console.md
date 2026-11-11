# 09 · Console

**Status:** Draft v0.1 · 2026-11-10

The console is the back-office web application served by `cavea-server`. It contains the chart list, the designer (`07-designer.md`), the event manager, reports, and developer and workspace settings.

It is a SolidJS single-page application embedded in the server binary. It uses only the public REST API plus console-session endpoints, so anything the console does a customer can automate.

## 1. Navigation

```
┌──────────────┬───────────────────────────────────────────────────────────┐
│ ▣ Cavea      │  Charts                                    [ New chart ]  │
│ Acme Tickets▾│  ┌────────┐ ┌────────┐ ┌────────┐ ┌────────┐              │
│              │  │ thumb  │ │ thumb  │ │ thumb  │ │ thumb  │              │
│ Charts       │  │ Royal  │ │ Arena  │ │ Club   │ │ Hall B │              │
│ Events       │  │ 2,148  │ │ 18,400 │ │ 320    │ │ 960    │              │
│ Reports      │  └────────┘ └────────┘ └────────┘ └────────┘              │
│ Developers   │                                                           │
│ Settings     │                                                           │
│              │                                                           │
│ ? Docs       │                                                           │
│ ◐ Theme      │                                                           │
└──────────────┴───────────────────────────────────────────────────────────┘
```

| Section | Screens |
|---|---|
| Charts | List, chart detail, designer |
| Events | List, create, event detail (manager, channels, settings, activity) |
| Reports | Event summary, object export, order lookup |
| Developers | API keys, webhooks, embed tokens, API reference link |
| Settings | Workspace, members, custom statuses, branding, licence, system |

The workspace switcher sits under the logo. Users with one workspace do not see it.

## 2. Authentication and roles

- Sign-in with email and password (argon2id), optional TOTP. Single sign-on (OIDC) is P2.
- The first start prints a one-time setup link in the server log to create the owner account.
- Sessions are HTTP-only, SameSite=Lax cookies, valid 12 hours, renewed on activity.
- All mutating requests carry a CSRF token.

| Role | Charts | Events and seat operations | Developers | Settings and members |
|---|---|---|---|---|
| Owner | Full | Full | Full | Full |
| Admin | Full | Full | Full | All except ownership transfer |
| Editor | Create, edit, publish | View | — | — |
| Operator | View | Block, release, book, assign channels | — | — |
| Viewer | View | View, reports | — | — |

As built (CON-001 to CON-004, SEC-003): the server side below, and the console's screens for it: the setup page, sign-in, sign-out, the account page with the password change, the frame's workspace switcher and user menu, and navigation and actions that follow the role (§9, §10).

- **First run.** While no user exists, each start logs one `info` record holding the setup link, `{CAVEA_PUBLIC_URL}/console/setup#token=<64 hex digits>` (the path alone when `CAVEA_PUBLIC_URL` is unset). The token is in the fragment, which never reaches a server or a `Referer`. It works once, for an hour; a restart makes a new one and the old stops working. The console's `/console/setup` page reads the fragment and posts it with an email and a password to `POST /console/api/setup`. The owner becomes **owner of every workspace that exists** (made earlier with the admin key); with none, the request's `workspace` (`{key, name}`, default `main`, `Main`) is created. A workspace the admin key creates after setup gets its first owner by an invitation the admin key makes (§7). Once a user exists, nothing is logged or stored, and setup answers `401` (`setup_token`) as it does to a wrong token. Of two setups at once, one creates the owner.
- **Passwords:** 15 to 128 characters, no other rule (NIST SP 800-63B rev. 4 for a single factor); argon2id with m = 19 MiB, t = 2, p = 1 (12 §3 T9). Emails are trimmed and lowercased; nothing is sent to them.
- **Sessions:** the cookie `__Host-cavea_console` (`HttpOnly`, `SameSite=Lax`, `Secure`, `Path=/`), ending 12 hours after the last request and 7 days after sign-in in any case (08 §11). Sign-in always starts a new session; a password change ends all of the user's.
- **Roles** are enforced on every `/v1` route and every members route by the permission it declares: one permission per cell of the table above (08 §11, D-54; `x-cavea-permission` and `x-cavea-roles` in the OpenAPI document). `GET /console/api/me` lists each workspace's role and its permissions, for the interface to hide what the role cannot do. The role is read on every request: a change applies to the next one.
- **Members** (§7): owners and admins list the workspace's members, invite people, change a role and remove a member. Only an owner makes, invites or unmakes an owner ("all except ownership transfer"); a workspace always keeps one owner.
- **Not built:** TOTP (the tree has SHA-1 through the WebSocket crate, so it needs no new crate; CON-003), password reset, single sign-on.

## 3. Charts

### 3.1 List

- Grid of cards: thumbnail, name, seat count, published version, last edited, number of events.
- Search by name or key; filter by archived; sort by name or last edited.
- Card menu: open, duplicate, export, archive.
- Empty state: "Draw your first chart" with the three starting points from `07-designer.md` §16.

### 3.2 Chart detail

- Header: name, key (copyable), published version, draft state ("unpublished changes").
- Tabs: **Versions** (number, date, author, seat count, diff link, "create event from this version"), **Events** using the chart, **Settings** (key, archive).
- Primary action: Open designer.

## 4. Events

### 4.1 List

- Table: name, key, chart and version, sold and held and free counts with a bar, created, status.
- Filters: chart, date range, archived. Search by key or name.
- Create: key, name, chart, version (default: latest published), hold lifetime, per-token object limit, orphan policy, whether public best-available is enabled, whether a session token is required.

### 4.2 Event detail: Manager

The event manager is the renderer in `manager` mode with an operations panel.

```
┌──────────────────────────────────────────────────────────┬────────────────┐
│ Hamlet · 14 Mar 2027            1,204 sold · 38 held     │ SELECTION (6)  │
│ [Select] [Rows] [Sections] [Box]      Search: C-12 ▸     │ Orchestra C    │
│                                                          │ 12–17          │
│                                                          │                │
│                     MAP (live)                           │ [ Block ]      │
│                                                          │ [ Book… ]      │
│                                                          │ [ Release ]    │
│                                                          │ [ Channel ▾ ]  │
│                                                          │ [ Status ▾ ]   │
│                                                          ├────────────────┤
│                                                          │ DETAILS        │
│ Legend: ● free ● held ● booked ● blocked ● house         │ C-12 booked    │
│                                                          │ Order 77120    │
└──────────────────────────────────────────────────────────┴────────────────┘
```

| Mode | Purpose |
|---|---|
| Inspect | Click an object to see status, order reference, hold expiry, channel, history |
| Select | Click, shift-click, drag a box; then act on the selection |
| Rows | Click selects a whole row |
| Sections | Click selects a whole section |
| Statuses | Paint a status (block, a custom status, free) |
| Channels | Paint channel assignment |
| Book | Box-office sale: select, enter an order reference and ticket types, confirm |
| Static | View only, for a lobby display or a shared screen |

- The map is live: other operators' and buyers' changes appear as they happen.
- Actions are all-or-nothing and show which objects blocked them if they fail.
- Search jumps to an object by label or to all objects of an order reference.
- Destructive actions on booked seats (release) need confirmation. Like every seat command they are recorded with the user (`usr_…`) in the event's command log (08 §11), which is their audit record; the audit log (§7) does not repeat them.
- The manager can be embedded in a customer's back office with an embed token, like the designer (`@cavea/manager-embed`).

### 4.3 Event detail: other tabs

| Tab | Contents |
|---|---|
| Channels | Create, rename, recolour, delete; counts per channel |
| Settings | Hold lifetime, limits, orphan policy, capacity overrides for areas, chart version with an Upgrade action and its conflict report |
| Activity | The command log: time, actor, operation, objects; filter by object, order reference, actor; export |

## 5. Reports

| Screen | Contents |
|---|---|
| Event summary | Totals by status, by category, by section, by channel; switch between table and stacked bars; CSV export |
| Objects | Filterable table of every object with status, order reference, ticket type, channel; CSV export |
| Order lookup | Enter an order reference; see its objects across events and jump to them on the map |

Report charts show one measure each, label values directly, and use the same colour-blind-safe status colours as the map legend.

## 6. Developers

| Screen | Contents |
|---|---|
| API keys | List with name, type, prefix, last used; create (the key is shown once); revoke; origin allow-list for publishable keys |
| Webhooks | Endpoints with subscribed types and health; delivery log with request and response; replay; send a test event; rotate secret |
| Embed | Explanation and a tester that mints an embed token and opens the embedded designer or manager |
| Quick start | Copy-paste snippets pre-filled with this workspace's keys and server URL |

## 7. Settings

| Screen | Contents |
|---|---|
| Workspace | Name, key, defaults for new events |
| Members | Invite by email, change role, remove |
| Custom statuses | Up to 12: key, label, colour |
| Validation | Which chart warnings are treated as errors |
| Branding | Accent colour and logo for embedded tools |
| Licence | Plan, expiry, licensed domains; upload a new licence file |
| System (owner) | Server version, database status, loaded events, subscriber count, link to metrics; update check (opt-in) |

As built on the server (CON-012, CON-013; the screens come later; D-55):

- **Invitations.** An owner or admin invites an email with a role; the server answers once with a link, `{CAVEA_PUBLIC_URL}/console/invite#token=<64 hex digits>` (256 random bits, in the fragment). Nothing is sent: whoever invites passes the link on. Only the token's SHA-256 is stored; the link, the token and the email are never logged. It works once (accepting deletes the row, so of two acceptances at once one joins and the other is refused) and for 7 days; a new invitation of the same email to the workspace replaces the old; revoking deletes it. Inviting an owner needs `ownership`; an email that is already a member's is `409`. The person opening the link either is signed in as that email (with the session's CSRF token) and joins, keeping their session (roles are read on every request), or, for an email with no account, sets a password (the sign-in rules: 15 to 128 characters, argon2id two at a time) and is created, joined and signed in with a new session. Signed in as another email is `403` (`invitation_email`) and leaves the invitation; not signed in, for an email that has an account, `409` (`sign_in`). A wrong, expired, revoked or used token is `401` (`invitation`), all alike, after one query and no hashing; only a holder of a live token can learn whether its email has an account. Acceptance draws on the sign-in budget per address (08 §12). Someone who became a member some other way before opening the link keeps their role, and the link is used up.
- **The admin key's owner invitation.** `POST /v1/workspaces/{workspace}/invitations` (admin key, `{email}`) makes an owner invitation: how a workspace the admin key created gets its first member, and the operator's recovery for one whose owners are gone (D-51's gap). The operator can already mint secret keys for any workspace, so this gives them nothing new.
- **Workspace.** `PATCH /console/api/workspace` renames it (`settings`).
- **Audit log** (CON-013). One row per sensitive action, written in the action's own transaction, so an action that rolls back leaves none and one that commits always has its row (a test makes the write fail and sees the action undone). Actions: `console.setup`, `console.sign_in`, `console.sign_in_failed`, `console.sign_out`, `console.password_changed`, `member.invited`, `member.joined`, `member.role_changed`, `member.removed`, `invitation.revoked`, `workspace.created`, `workspace.renamed`, `api_key.created` (the admin key's mint included), `api_key.revoked`, `chart.published`, `chart.archived`, `event.created`, `event.archived`; repeating an idempotent archive or revocation writes nothing. A row holds the actor (`usr_…`, `key_…`, `admin`, or `anonymous` for a failed sign-in), the workspace (none for setup, sign-ins, sign-outs and password changes), the user it concerns, a target (`usr_…`, `key_…`, `inv_…`, `ws_…`, a chart's or event's key), small facts (a role, from and to, a key's kind, a version) and the request id; never a secret, a body, an email or a client address. Seat commands are not repeated: the event log records them with `usr_…` as actor (08 §11), and the hold path writes nothing more. **Failed sign-ins** are written only for an email that is an account's, after the reply is decided and on a task of their own: an unknown email, which an attacker can vary at will, writes nothing; for an account, the back-off (08 §12) holds attempts made one after another to a few dozen an hour, and no attack can write rows faster than the node verifies passwords (two at a time, about 8 ms each), which bounds the attack itself. The reply's timing does not depend on the write. **Retention:** none; rows are kept, since every other action is a person's deliberate act. The read route reads backwards on two indexes, a page at a time, so it does not slow as the log grows (10 million rows measured: 13 §7.4). Owners and admins read it newest first with `GET /console/api/audit`: the workspace's rows and the instance rows of its current members.

## 8. Interface standards

- **Layout:** left navigation, content with a maximum width of 1,280 px for lists and forms; the designer and manager use the full window.
- **Tables:** sticky header, column sorting, keyboard row navigation, cursor pagination with "load more".
- **Forms:** inline validation on blur; errors next to the field; destructive actions in a confirmation dialog that names the object.
- **Feedback:** optimistic updates with rollback on failure; toasts for background results; skeletons while loading.
- **Empty states:** every list explains what it is for and offers the first action.
- **Themes:** light and dark, following the system by default.
- **Keyboard:** every action reachable; `?` shows shortcuts; Ctrl+K opens a command palette that also searches charts and events.
- **Accessibility:** WCAG 2.2 AA for all console screens (`11-accessibility.md` §8).
- **Localisation:** English first; all strings in catalogues; dates and numbers through `Intl`.
- **Responsiveness:** lists, reports and settings work from 768 px; designer and manager need 1,280 px.

## 9. Technology

| Concern | Choice |
|---|---|
| Framework | SolidJS with Solid Router |
| Build | Vite; assets embedded in the server binary with content hashes |
| Styling | Tailwind with design tokens as CSS custom properties |
| Components | Kobalte primitives, styled in-house |
| Server state | Solid signals and a small query helper of our own against the generated TypeScript API client (D-60) |
| Forms | The server's rules (lengths, required fields) checked before sending; its field errors mapped back to the field |
| Canvas | `@cavea/renderer` and the `cavea-edit` WASM module |
| Tests | `bun test` for units; flows proven in headless Chromium over the DevTools protocol against a real server (D-60) |

As built (`apps/console`, CON-001 in part): Solid Router 1.0.1, Tailwind 4.3.3 through its Vite plugin, Kobalte 0.13.14, each pinned. Routes: `/charts`, `/events`, `/reports`, `/developers` and `/settings` in the console frame (left navigation, content at most 1,280 px; every screen but Charts says it is not built yet), and full-window tools outside it, the designer at `/designer/local` for a chart from a file. The design tokens are CSS custom properties in `src/styles.css`, light and dark; Tailwind's own palette and type scale are removed, so a component can use only tokens, and a test checks the text and control pairs for WCAG 2.2 AA contrast in both themes. The theme is the system's unless the user picks one (stored per browser), applied as `data-theme` on the root. Every user-visible string is in one English catalogue, `src/i18n/messages.ts`, by stable id; groups keyed by the edit core's values (node kinds, tools, units, command and import error codes, undo entry keys, inspector fields, the compiler's finding codes) are typed against the generated types, and a test fails when the types or the compiler gain a value without a message. Numbers and times go through `Intl`. The base components are in `src/ui`.

**The API client** (CON-001 to CON-005; D-60). `src/api/generated.ts` is written by `cargo xtask types` from `crates/cavea-server/openapi.json`: a TypeScript type per schema, and `OPERATIONS`, each operation's method, path, `x-cavea-permission` and `x-cavea-roles`, with its input (path and query parameters, headers other than the two below, body) and reply. It is committed and not formatted; `cargo xtask types --check` and `cargo test -p xtask` fail while it is stale, so a change to the server's routes or schemas regenerates the document and then the types. `src/api/client.ts` is the one request layer: `call(operation, input, signal)` sends relative URLs with `credentials: "same-origin"`, `Cavea-Csrf-Token` on every non-safe method (the token is held in memory only), `Cavea-Workspace` on every operation that declares a permission, and rejects with an `ApiError` (`status`, `code`, `details.reason`, the request id, `Retry-After`), or with the caller's abort; an answer that is not a problem (a proxy's page, garbage) is `unexpected`, no answer `network`, 30 s without one `timeout`. A `401` with `reason` `console_session`, or `missing` (no cookie at all), ends the session in one place (`src/session.ts`): the frame sends the person to `/sign-in?next=<where they were>` and sign-in returns there (only to a path of the console). `me` is loaded at start and read again when the page is shown after a minute away, so an expired session or a changed role is noticed; a `403` `role` or `workspace` from the server is shown and reads `me` again. Server state for a screen is `createQuery` (`src/api/query.ts`), keyed on the workspace: switching it cancels what is in flight and starts from loading. The selected workspace is kept per browser. Routes are under Vite's `base` (`import.meta.env.BASE_URL` into the router's `base`), which the server's embedding sets (CON-015); with the root as base, `/console/setup` is the setup page too, as the server logs it.

**Served by the server** (CON-015, D-56). `cargo xtask build-server` builds the WebAssembly modules and the console (`apps/console/dist`), writes each file and its brotli form (quality 11, kept when at most nine tenths of the file) under `target/console/` with a table of `include_bytes!`, and builds `cavea-server` in release with `CAVEA_CONSOLE_EMBED` naming the table; `crates/cavea-server/build.rs` compiles it in. Without that variable (a plain `cargo build`, CI) the table is empty. The console is served at the root of the server's origin, as Vite builds it (`base` `/`, files in `/assets/`), from the router's fallback, so no API route can be shadowed:

- A request path is looked up exactly in the table: nothing is read from disk and no path can name anything else.
- `/assets/…` (content-hashed names): `Cache-Control: public, max-age=31536000, immutable`. `index.html`: `no-cache`. Each has a strong `ETag` (`304` for `If-None-Match`), `X-Content-Type-Options: nosniff` and its type (`application/wasm` for WebAssembly, so it compiles while streaming). `HEAD` answers as `GET`.
- To a request accepting `br` the brotli form, `Content-Encoding: br`, with its own `ETag`; files that have one say `Vary: Accept-Encoding`. No gzip: browsers send `br` over HTTPS, and gzip would need a new crate.
- A `GET` for another path answers `index.html` only to a navigation (`Accept` naming `text/html`), and `/` always: the console's routes (`/charts/…`, `/designer/local`, `/console/setup`, `/console/invite`) work when opened or reloaded. Under `/assets/`, or without `text/html`, it is problem+json `404`, so a missing script or image is never HTML. `/v1`, `/console/api` and their subpaths, `/healthz`, `/readyz` and `/metrics` are never the console's.
- The files take no credential and are the same for everyone; the console cookie's `no-store` (08 §11) does not apply to them.
- A server built without the console answers its navigations `404` with a short text saying so and how to build it.
- The built console references only the designer's WebAssembly (`/assets/cavea_designer_bg-….wasm`, 2.46 MB, 567 KB brotli); the renderer's is not loaded by any screen yet, and `build-server` builds it anyway so a screen that starts using it is served. Five files, 3.49 MB with their brotli forms; Content Security Policy and the other headers of SEC-004 can be added in one place (`http/web.rs`), and the built `index.html` has no inline script.

**Talking to the server in development.** The console is one origin with the server in production (CON-015 embeds it). In development the Vite dev server must proxy `/console/api` and `/v1` to `cavea-server` (Vite's `server.proxy`, with `changeOrigin` left `false` so the server sees the dev server's `Host`), and the console calls relative URLs. The browser then sees one origin: the session cookie is first-party, `Sec-Fetch-Site` is `same-origin`, and the server needs no CORS for credentials, which it never gives (08 §11). `http://localhost` counts as secure in browsers, so the default `Secure`, `__Host-` cookie works there; only a dev server reached over plain HTTP by another host name needs `CAVEA_CONSOLE_COOKIE_SECURE=false`. Do not call the server's port directly from the page: the cookie would be cross-site and every console request refused. As built: `CAVEA_SERVER=http://127.0.0.1:<port> bun run dev --port <port>` in `apps/console` (the server's default `http://127.0.0.1:8080` when unset).

## 10. Console-only endpoints

Under `/console/api`, cookie-authenticated:

| Endpoint | Purpose |
|---|---|
| `POST /console/api/session` , `DELETE` | Sign in, sign out |
| `GET /console/api/me` | User, workspaces, roles |
| `POST /console/api/setup` | First-run owner creation with the one-time token |
| `GET/POST/PATCH/DELETE /console/api/members` | Membership management: list, invite, change a role, remove |
| `GET /console/api/invitations`, `DELETE /console/api/invitations/{id}` | Pending invitations; revoke one |
| `POST /console/api/invitations/accept` | Accept an invitation link |
| `PATCH /console/api/workspace` | Rename the workspace |
| `GET /console/api/audit` | The audit log, newest first |
| `POST /console/api/totp` | Enrol and verify (not built: CON-003) |
| `POST /console/api/password` | Change one's password; every session of the user ends and a new one starts |

Everything else goes through `/v1` with the console session accepted as a credential.

As built, for the console's screens (the shapes are `ConsoleSetup`, `ConsoleSignIn`, `ConsolePassword` and `ConsoleMe` in the OpenAPI document, tag `console`):

- `POST /console/api/setup` `{token, email, password, workspace?: {key, name}}` → `201` `ConsoleMe` and the cookie. `401` `setup_token`; `400` `invalid_field` (`email`, `password`, `key`, `name`); `409` for a taken workspace key.
- `POST /console/api/session` `{email, password}` → `200` `ConsoleMe` and the cookie. `401` `credentials` (a wrong email and a wrong password alike); `429` `backoff` or `sign_in` with `Retry-After`; `503` `password_hashing`.
- `DELETE /console/api/session` → `204`, cookie cleared.
- `GET /console/api/me` → `ConsoleMe`: `{user: {id: "usr_…", email}, workspaces: [{id: "ws_…", key, name, role, permissions: ["charts.read", …]}], csrf_token, session: {expires_at, absolute_expires_at}}`. A `401` with `reason: "console_session"` anywhere means: sign in again; so does `reason: "missing"`, which is the answer when the request carried no cookie at all (a cookie deleted or expired in the browser).
- `POST /console/api/password` `{current_password, new_password}` → `200` `ConsoleMe` and a new cookie; `403` `credentials` for a wrong current password.
- **Every non-safe request** with the cookie sends `Cavea-Csrf-Token: <csrf_token>` (from `me` or the sign-in reply); `403` `csrf` without it, `403` `origin` from another site.
- **Every `/v1` request** with the cookie sends `Cavea-Workspace: ws_…` (one of `me.workspaces`); `400` `workspace` without it, `403` `workspace` for one the user is not in, `403` `role` (with `details.permission`) for what the role does not allow. Each operation's `x-cavea-permission` and `x-cavea-roles` in the document say who may call it; compare with the workspace's `permissions` in `me`.
- **Members** (`WorkspaceMember`, `MemberList`, `ChangeRole`), with the cookie, `Cavea-Workspace` and, when not `GET`, `Cavea-Csrf-Token`; owner and admin (`settings`): `GET /console/api/members?limit=&after=` → `{data: [{user: {id: "usr_…", email}, role}], next}`; `PATCH /console/api/members/{usr_…}` `{role}` → `200` the member; `DELETE /console/api/members/{usr_…}` → `204`. `403` `role` with `details.permission: "ownership"` when an admin makes or unmakes an owner; `400` `last_owner`; `404` (`missing: "member"`) for someone who is not a member of the workspace.
- **Invitations** (`InviteMember`, `InvitationCreated`, `InvitationList`, `AcceptInvitation`), owner and admin (`settings`) but acceptance: `POST /console/api/members` `{email, role}` → `201` `{invitation: {id: "inv_…", email, role, created_at, expires_at}, link}`, the link shown only here (`403` `role` with `permission: "ownership"` for an admin inviting an owner; `409` for a member's email). `GET /console/api/invitations?limit=&after=` → `{data: [invitation], next}`, unexpired, oldest first, never a link. `DELETE /console/api/invitations/{inv_…}` → `204`. The link opens `/console/invite#token=…`; the screen reads the fragment and calls `POST /console/api/invitations/accept` `{token, password?}` with `Content-Type: application/json` (and, when signed in, the cookie and `Cavea-Csrf-Token`) → `200` `ConsoleMe` (with a new cookie when the user was created). Errors: `401` `invitation` (wrong, expired, used, revoked), `403` `invitation_email` (signed in as another email: offer to sign out), `409` `sign_in` (the email has an account: sign in, then open the link again), `400` `invalid_field` `password` (missing when not signed in, or not 15 to 128 characters), `429`, `503` `password_hashing`.
- **Workspace:** `PATCH /console/api/workspace` `{name}` → `200` `{id, key, name}`.
- **Audit log** (`AuditList`): `GET /console/api/audit?limit=&after=` → `{data: [{id: "aud_…", at, actor, action, target, detail, request_id}], next}`, newest first; `after` is an entry's id.
