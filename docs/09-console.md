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

As built (server side; CON-002 to CON-004, SEC-003): the screens come later.

- **First run.** While no user exists, each start logs one `info` record holding the setup link, `{CAVEA_PUBLIC_URL}/console/setup#token=<64 hex digits>` (the path alone when `CAVEA_PUBLIC_URL` is unset). The token is in the fragment, which never reaches a server or a `Referer`. It works once, for an hour; a restart makes a new one and the old stops working. The console's `/console/setup` page reads the fragment and posts it with an email and a password to `POST /console/api/setup`. The owner becomes **owner of every workspace that exists** (made earlier with the admin key); with none, the request's `workspace` (`{key, name}`, default `main`, `Main`) is created. Workspaces the admin key creates after setup have no members until members management exists (CON-012). Once a user exists, nothing is logged or stored, and setup answers `401` (`setup_token`) as it does to a wrong token. Of two setups at once, one creates the owner.
- **Passwords:** 15 to 128 characters, no other rule (NIST SP 800-63B rev. 4 for a single factor); argon2id with m = 19 MiB, t = 2, p = 1 (12 §3 T9). Emails are trimmed and lowercased; nothing is sent to them.
- **Sessions:** the cookie `__Host-cavea_console` (`HttpOnly`, `SameSite=Lax`, `Secure`, `Path=/`), ending 12 hours after the last request and 7 days after sign-in in any case (08 §11). Sign-in always starts a new session; a password change ends all of the user's.
- **Roles** are stored per workspace and `GET /console/api/me` lists them; the console session on `/v1`, with the role table above checked on every route, is CON-004.
- **Not built:** TOTP (the tree has SHA-1 through the WebSocket crate, so it needs no new crate; CON-003), members and invitations (CON-012), password reset, single sign-on.

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
- Destructive actions on booked seats (release) need confirmation and are written to the audit log with the user.
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
| Server state | TanStack Query (Solid) against the generated TypeScript API client |
| Forms | Schema validation shared with the API types |
| Canvas | `@cavea/renderer` and the `cavea-edit` WASM module |
| Tests | `bun test` for units, Playwright for flows |

As built (`apps/console`, CON-001 in part): Solid Router 1.0.1, Tailwind 4.3.3 through its Vite plugin, Kobalte 0.13.14, each pinned. TanStack Query, the API client, form validation and Playwright arrive with the first screen that talks to the server. Routes: `/charts`, `/events`, `/reports`, `/developers` and `/settings` in the console frame (left navigation, content at most 1,280 px; every screen but Charts says it is not built yet), and full-window tools outside it, the designer at `/designer/local` for a chart from a file. The design tokens are CSS custom properties in `src/styles.css`, light and dark; Tailwind's own palette and type scale are removed, so a component can use only tokens, and a test checks the text and control pairs for WCAG 2.2 AA contrast in both themes. The theme is the system's unless the user picks one (stored per browser), applied as `data-theme` on the root. Every user-visible string is in one English catalogue, `src/i18n/messages.ts`, by stable id; groups keyed by the edit core's values (node kinds, tools, units, command and import error codes, undo entry keys, inspector fields, the compiler's finding codes) are typed against the generated types, and a test fails when the types or the compiler gain a value without a message. Numbers and times go through `Intl`. The base components are in `src/ui`.

**Talking to the server in development.** The console is one origin with the server in production (CON-015 embeds it). In development the Vite dev server must proxy `/console/api` and `/v1` to `cavea-server` (Vite's `server.proxy`, with `changeOrigin` left `false` so the server sees the dev server's `Host`), and the console calls relative URLs. The browser then sees one origin: the session cookie is first-party, `Sec-Fetch-Site` is `same-origin`, and the server needs no CORS for credentials, which it never gives (08 §11). `http://localhost` counts as secure in browsers, so the default `Secure`, `__Host-` cookie works there; only a dev server reached over plain HTTP by another host name needs `CAVEA_CONSOLE_COOKIE_SECURE=false`. Do not call the server's port directly from the page: the cookie would be cross-site and every console request refused. (`/v1` takes the console session once CON-004 is in; until then the console signs in and reads `me` only.)

## 10. Console-only endpoints

Under `/console/api`, cookie-authenticated:

| Endpoint | Purpose |
|---|---|
| `POST /console/api/session` , `DELETE` | Sign in, sign out |
| `GET /console/api/me` | User, workspaces, roles |
| `POST /console/api/setup` | First-run owner creation with the one-time token |
| `GET/POST/PATCH/DELETE /console/api/members` | Membership management (not built: CON-012) |
| `POST /console/api/totp` | Enrol and verify (not built: CON-003) |
| `POST /console/api/password` | Change one's password; every session of the user ends and a new one starts |

Everything else goes through `/v1` with the console session accepted as a credential.

As built, for the console's screens (the shapes are `ConsoleSetup`, `ConsoleSignIn`, `ConsolePassword` and `ConsoleMe` in the OpenAPI document, tag `console`):

- `POST /console/api/setup` `{token, email, password, workspace?: {key, name}}` → `201` `ConsoleMe` and the cookie. `401` `setup_token`; `400` `invalid_field` (`email`, `password`, `key`, `name`); `409` for a taken workspace key.
- `POST /console/api/session` `{email, password}` → `200` `ConsoleMe` and the cookie. `401` `credentials` (a wrong email and a wrong password alike); `429` `backoff` or `sign_in` with `Retry-After`; `503` `password_hashing`.
- `DELETE /console/api/session` → `204`, cookie cleared.
- `GET /console/api/me` → `ConsoleMe`: `{user: {id: "usr_…", email}, workspaces: [{id: "ws_…", key, name, role}], csrf_token, session: {expires_at, absolute_expires_at}}`. A `401` with `reason: "console_session"` anywhere means: sign in again.
- `POST /console/api/password` `{current_password, new_password}` → `200` `ConsoleMe` and a new cookie; `403` `credentials` for a wrong current password.
- **Every non-safe request** with the cookie sends `Cavea-Csrf-Token: <csrf_token>` (from `me` or the sign-in reply); `403` `csrf` without it, `403` `origin` from another site.
