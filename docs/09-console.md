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

## 10. Console-only endpoints

Under `/console/api`, cookie-authenticated:

| Endpoint | Purpose |
|---|---|
| `POST /console/api/session` , `DELETE` | Sign in, sign out |
| `GET /console/api/me` | User, workspaces, roles |
| `POST /console/api/setup` | First-run owner creation with the one-time token |
| `GET/POST/PATCH/DELETE /console/api/members` | Membership management |
| `POST /console/api/totp` | Enrol and verify |

Everything else goes through `/v1` with the console session accepted as a credential.
