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

- **First run.** While no user exists, each start logs one `info` record holding the setup link, `{CAVEA_PUBLIC_URL}/console/setup#token=<64 hex digits>` (the path alone when `CAVEA_PUBLIC_URL` is unset). The token is in the fragment, which never reaches a server or a `Referer`. It works once, for an hour; a restart makes a new one and the old stops working. The console's `/console/setup` page reads the fragment and posts it with an email and a password to `POST /console/api/setup`. The owner becomes **owner of every workspace that exists** (made earlier with the admin key); with none, the request's `workspace` (`{key, name}`, default `main`, `Main`) is created. A workspace the admin key creates after setup has no members: the setup owner is owner of the workspaces that existed at setup, not of later ones, and such a workspace gets its first owner by an invitation the admin key makes (§7; `setup_creates_one_owner_once`). Once a user exists, nothing is logged or stored, and setup answers `401` (`setup_token`) as it does to a wrong token. Of two setups at once, one creates the owner.
- **Passwords:** 15 to 128 characters, no other rule (NIST SP 800-63B rev. 4 for a single factor); argon2id with m = 19 MiB, t = 2, p = 1 (12 §3 T9). Emails are trimmed and lowercased; nothing is sent to them.
- **Sessions:** the cookie `__Host-cavea_console` (`HttpOnly`, `SameSite=Lax`, `Secure`, `Path=/`), ending 12 hours after the last request and 7 days after sign-in in any case (08 §11). Sign-in always starts a new session; a password change ends all of the user's.
- **Roles** are enforced on every `/v1` route and every members route by the permission it declares: one permission per cell of the table above (08 §11, D-54; `x-cavea-permission` and `x-cavea-roles` in the OpenAPI document). `GET /console/api/me` lists each workspace's role and its permissions, for the interface to hide what the role cannot do. The role is read on every request: a change applies to the next one.
- **Members** (§7): owners and admins list the workspace's members, invite people, change a role and remove a member. Only an owner makes, invites or unmakes an owner ("all except ownership transfer"); a workspace always keeps one owner.
- **Not built:** TOTP (the tree has SHA-1 through the WebSocket crate, so it needs no new crate; CON-003), password reset, single sign-on.

## 3. Charts

### 3.1 List

- Grid of cards: thumbnail, name, seat count, published version, last edited, number of events. The thumbnail is the published version's, made by the designer as it publishes (07 §11, DSN-040); a version without one shows a placeholder. The chart page's versions show each version's thumbnail beside its number.
- Search by name or key; filter by archived; sort by name or last edited.
- Card menu: open, duplicate, export, archive.
- Empty state: "Draw your first chart" with the three starting points from `07-designer.md` §16.

### 3.2 Chart detail

- Header: name, key (copyable), published version, draft state ("unpublished changes").
- Tabs: **Versions** (number, date, author, seat count, diff link, "create event from this version"), **Events** using the chart, **Settings** (key, archive).
- Primary action: Open designer.

As built (CON-006, in part): `/charts/{key}`, opened from a chart card's name. The header has the name, the key with a copy button, the published version and its seats, "Unpublished changes" when the draft was saved after the published version was made, and Archived. **Versions** lists every version, newest first (read whole up to 20 pages of 100, and says when it stopped short), with its number, date and seats, the published one marked, and "Create event" opening the event form with the chart and version chosen. **Events** pages `GET /v1/events?chart=` like the events list (oldest first, "load more"), each event linking to its page. **Settings**: the key (it cannot change) and archive, with a confirmation. Not built: Open designer for a server chart (DSN-035), and a version's author and diff link (the API has neither).

## 4. Events

### 4.1 List

- Table: name, key, chart and version, sold and held and free counts with a bar, created, status.
- Filters: chart, date range, archived. Search by key or name.
- Create: key, name, chart, version (default: latest published), hold lifetime, per-token object limit, orphan policy, whether public best-available is enabled, whether a session token is required.

As built (CON-007, in part): the table pages as `GET /v1/events` does, 25 to a page with "load more". Search, the chart filter, which events are shown (active, the default; archived; both) and the order (oldest first, the default; newest first) are the server's (`q`, `chart`, `archived`, `order`, 08 §6.2): they cover every event, not only the pages loaded. Typing is sent 300 ms after it pauses, at most 128 characters, every character literal (`50%_` finds `50%_`); a new search, filter or order cancels the request for the one before. They are kept in the page's URL (`?q=&chart=&shown=&order=`, defaults left out), so a reload or a shared link shows the same list; the URL holds what was typed and chosen, nothing else. Each row's counts (booked, held, free, with a bar) come with the list (`counts`), with no request per row. An empty result says so and offers to clear the search and filters. The form sends only the settings that differ from the server's defaults (an event keeps what was set and takes later defaults for the rest), shows each with its unit and limits (hold lifetime 60 to 7,200 seconds, seats per hold token 1 to 100, hold tokens per session 1 to 10, offered with a required session token), and puts the server's refusals on their fields (a taken key, `invalid_field`, a chart that is missing, archived or unpublished, a version that is not). Archive asks first. An event's name opens its page (§4.2). Not built, for want of server support: a date-range filter (the list takes no date), and the orphan policy and public best-available on the form (no such settings).

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

As built (CON-008, the basic manager; D-68, D-82). The event's page is `/events/{key}` (the manager) and `/events/{key}/settings`, a full-window tool outside the console frame: a bar with the event's name, key, chart and version, its counts (booked, held, blocked, free, custom), and the two tabs. It needs a session and the events section's permission like the frame's pages; a workspace switch remounts it. The renderer and the page are a separate chunk, loaded only when an event is opened.

- **The map** is `@cavea/renderer` in `picker` mode (the only mode the SDK has, 05 §3) with a `customState` adapter of the console's own. Free seats are drawn available; held, booked, blocked and custom seats are drawn unavailable, alike, since the picker has two states. The map's legend says so ("Free", "Held, booked or blocked", "Selected"), its tooltip names each seat's true status, and a section with no free seat says "None free". An operator cannot see at a glance which unavailable seats are held, booked or blocked: the counts in the bar, the tooltip and the details say it.
- **Selecting any seat.** The picker refuses a seat it draws unavailable; the adapter draws a seat available while it is selected, whatever its status. A click on an unavailable seat (the picker's `not_free` refusal) draws that seat available and selects it; deselected, it is drawn unavailable again. Keyboard: the renderer's own (arrow keys and Enter, D-52). At most 100 seats at a time (the picker's largest selection). The selection lists each seat with its status as now read, and "was …" when it changed since it was selected.
- **Inspect.** The details show the seat last clicked or focused: label, section, row and seat, category, status, and the order reference and ticket type of a booked seat or the hold's expiry of a held one.
- **Actions** (owner, admin, operator: `seats.write`; others see "Your role … can see the seats but not change them"): Block (free seats, `change-status` `blocked`), Unblock (blocked or custom seats, `change-status` `free`), Book… (free seats, `book` without a token, with an order reference of 1 to 128 characters that the server requires), Release… (booked seats, `release` without a token). Each takes the whole selection and is offered only when every selected seat is in the status it takes from, else disabled with how many are not; each sends an `Idempotency-Key`. A held seat cannot be acted on: it is the buyer's until booked with their token or expired (02 §4.3). Release asks first, naming the seats and their number and saying that bookings and order references are removed and that it cannot be undone; blocking takes free seats only, so it never touches a hold. After an action the selection clears; a refusal leaves it as it was, lists the seats that stopped it with the engine's reason and their status read again at once, and a `403` re-reads `me`, so a lowered role's actions disappear. An answer that never arrived says the outcome is unknown and re-reads the seats.
- **Live, from the staff state route** (D-82, replacing D-68's polling; D-72). The console fetches the event's chart once, hands the bytes to the map and reads every object's full label by `idx` from them (`manager/labels.ts`, the rules of `cavea_format::full_label`), and fetches beside it the manager SNAPSHOT (`GET /v1/events/{event}/state`, `application/vnd.cavea.frame`, decoded in the manager view by `manager/frames.ts`: domain statuses 0 to 15, encodings 0 and 1). Every 2 s it asks for what came after its `seq`: `204` changes nothing; a DELTA from its `seq` is applied and its `to_seq` kept; a SNAPSHOT (the server keeps about 60 s of deltas, and none from before a restart) replaces everything. A DELTA from another `seq`, or a frame that breaks a rule of 08 §8.6 or does not fit the chart (another object count, an `idx` past it), is not applied: the map keeps its last good state, the status line says what is shown is not current and why, and the next poll asks for a snapshot (`after_seq=0`). The map is told only the seats whose status changed (`applyDelta`), so it draws nothing while nothing changes. The counts in the bar are the state's own. Order references, ticket types and hold expiries are not in the frames: the selected and inspected seats are read by label (`GET …/objects?label=`) when they change, when the state changes one of them, and when the state moves while one of them is held (an extended hold); a detail is shown only while the seat is still in the status it was read in. The operator's own action shows from its reply until the state reaches the reply's `seq` (the state trails the committed `seq` by at most one publication). Nothing runs while the page is hidden (the renderer's `pause`; shown again, it asks at once); leaving the page or switching workspace cancels the requests and destroys the map. In a dev build `window.caveaManager.live.polls()` lists the last 50 polls (what was asked, the answer's kind and bytes, the time).
- **Measured locally** (AMD Ryzen 9 7900X shared with other builders; headless Chromium 155, WebGL2 on SwiftShader; dev build through the Vite proxy; release server and the shared PostgreSQL; `stadium-100k` with 20,000 booked, 4,500 blocked, 500 custom and 500 held). Before (D-68): 282 requests and 1.5 s per whole read, live in 1.7 s, a hold shown in 1.8 s, the map up to 15 s behind on a busy event. After: the first load is 3 requests, the event (1 KB), the chart (181 KB, which the map fetched before too) and the snapshot (74 KB), fetched together; the page was live 0.46 s after opening (the map interactive at 0.51 s); each poll while nothing changed was a `204` of 0 bytes in 4 to 14 ms; no frame was drawn in 10 s idle; a hold, a booking and a block made through the API showed 0.8, 2.0 and 2.0 s after their commands (at most one poll interval plus a publication). A frame of garbage was reported and followed by a snapshot 2 s later, the map unchanged meanwhile; with the server stopped the status line said it could not be reached, and after a restart the page had missed a change for, `after_seq=39` was answered with a snapshot and the change shown. Reading the labels of 100,000 seats takes about 40 ms.
- **What the server should still offer** for the full manager: the manager stream (SRV-039), so changes are pushed rather than polled.
- Not built (CON-020, CON-021, CON-028): the manager's modes (rows, sections, box, statuses, channels, static), true statuses drawn on the map, search by label or order, channels and custom statuses as actions, ticket types when booking, the embedded manager.

### 4.3 Event detail: other tabs

| Tab | Contents |
|---|---|
| Channels | Create, rename, recolour, delete; counts per channel |
| Settings | Hold lifetime, limits, orphan policy, capacity overrides for areas, chart version with an Upgrade action and its conflict report |
| Activity | The command log: time, actor, operation, objects; filter by object, order reference, actor; export |

As built (CON-009): the **Settings** tab edits the event's name and everything `PATCH /v1/events/{event}` takes: hold lifetime (60 to 7,200 seconds), seats per hold token (1 to 100), whether a session token is required, and hold tokens per session (1 to 10, with a session token), each with its unit and limits, checked on blur and before sending, the server's refusals on their fields. Only the settings that differ from the server's defaults are sent, as the create form does: one left out goes back to its default, the same value. Archive asks first and says what it does. A role without `events.write`, and an archived event, see the settings read-only with the reason. Not built: the chart-version upgrade with its conflict report (CON-024), capacity overrides and channels (CON-022), the orphan policy (no such setting on the server).

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

As built (CON-010, CON-011): **API keys** lists name, type, prefix, origins, created, last used and revoked keys on request, a page at a time. A new key is shown once, in a dialog that says so, with a copy button; it is held only by that dialog (never in a URL, storage or a log) and gone when it closes; the server keeps its hash only. A publishable key's origins are set when it is made (one per line, at most 50, none for every origin) and changed later with **Edit origins…** (`PATCH /v1/keys/{key_…}`): "only pages on the origins listed" (1 to 50, checked on blur as the server checks them: `http://` or `https://`, then a host and port only, at most 255 characters) or "pages on every origin", which shows a warning that any website could then use the key and turns the button into "Allow every origin". The server's refusals land on the field (`invalid_field` `allowed_origins`) or say the key has been revoked meanwhile (`reason: "revoked"`, and the list is read again); the key's next request follows the change. Revoking asks first and names the key and its prefix. **Quick start** fills three snippets (create an event, show the seat map with `caveaServer`, book at checkout) with the page's own origin (the server's, once embedded) and the workspace. A secret key appears only as `$CAVEA_SECRET_KEY`. A publishable key fills the page snippet in its creation dialog, or when pasted on the quick start (kept in the page only; a pasted secret key is refused with a warning). Text from the workspace stays on one line, so a name cannot become a command in a pasted shell snippet.

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

As built in the console (CON-012): Settings has three tabs. **Workspace**: the name (the key cannot change). **Members**: the members a page at a time, each role a choice and Remove, both confirmed; an admin is not offered owner and cannot change or remove an owner, and the last owner (when the whole list is loaded) cannot be demoted or removed, each with the reason; the server's refusal (`last_owner`, `ownership`, a member gone) is shown and the list read again; a change to one's own role or membership reads `me` again. **Invite** takes an email and a role and shows the link once, as a whole URL (the server gives a path alone without `CAVEA_PUBLIC_URL`), with a copy button; pending invitations list with Revoke. **Audit log**: newest first, a page at a time, members' emails in place of their identifiers. The **invitation page** at `/console/invite` takes the token from the fragment at start, before the setup page's reader, and removes it from the address bar; it keeps it in memory across a detour to sign-in. Signed in, the person accepts as themselves and the joined workspace is selected; signed out, they set a password and land signed in; an email with an account is sent to sign in and back; another email signed in is offered sign-out; a spent link says so. Not built: defaults for new events (no server setting yet).

As built on the server (CON-012, CON-013; D-61):

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

As built (CON-014, in part), checked in headless Chromium at 1,440 × 900 and 768 × 900, light and dark, keyboard only: each screen Tabbed through from its top (every control reached, a visible focus ring on each stop, no trap), every dialog and menu opened with Enter (focus moves in and stays in, Escape closes, focus returns to the control that opened it), forms submitted empty, route changes followed. Shared fixes, so every screen gains them: a dialog is a `dialog` (Kobalte assigns `Dialog`'s and `AlertDialog`'s parts onto one root, so with both imported every form dialog was an `alertdialog`; `ui/overlays.tsx` now takes the parts by name); focus returns to a dialog's opener, or, when its row was drawn again or removed, to the same control in the new row (`data-focus-id`) or the page's heading, never to the body (`ui/focus.ts`), and a dialog opened from a menu returns it to the menu's trigger; when the focused element leaves the page later (its row archived and the list read again) and focus falls to the body, the same rule moves it; a gated action (`Action`) is one element whether available or not, so focus stays on it when it becomes unavailable; after a route change that removed the focused element, the new page's heading takes focus; a form whose own checks refuse a submit focuses its first invalid field; a section's screen is mounted as a component, so a screen that reads its URL is not made again (and focus lost) when its parameters change; tables keep their header in view in a scroll box of their own that also scrolls sideways. ✓ met, ✗ not met, — not applicable.

| Screen | Layout | Tables | Forms | Feedback | Empty state | Themes | Keyboard | 768 px |
|---|---|---|---|---|---|---|---|---|
| Setup | ✓ | — | ✓ | ✓ | — | ✓ | ✓ | ✓ |
| Sign-in | ✓ | — | ✓ | ✓ | — | ✓ | ✓ | ✓ |
| Invitation | ✓ | — | ✓ | ✓ | — | ✓ | ✓ | ✓ |
| Account | ✓ | — | ✓ | ✓ | — | ✓ | ✓ | ✓ |
| Charts list | ✓ | — (cards; sort by name or last edited) | ✓ | ✓ | ✓ | ✓ | ✗ palette, `?` | ✓ |
| Chart detail | ✓ | ✗ sorting, row keys | ✓ | ✓ | ✓ | ✓ | ✗ palette, `?` | ✓ |
| Events list and create | ✓ | ✗ sorting (order by creation only), row keys | ✓ | ✓ | ✓ | ✓ | ✗ palette, `?` | ✓ |
| Event manager | ✓ (full window) | — | ✓ (Book) | ✓ | ✓ | ✓ | ✓ (map by arrows and Enter, D-52) | — (1,280 px) |
| Event settings | ✓ | — | ✓ | ✓ | — | ✓ | ✓ | ✓ |
| API keys, quick start | ✓ | ✗ sorting, row keys | ✓ | ✓ | ✓ | ✓ | ✗ palette, `?` | ✓ |
| Settings: workspace, members, invitations, audit | ✓ | ✗ sorting, row keys | ✓ | ✓ | ✓ | ✓ | ✗ palette, `?` | ✓ |

What the table marks ✗, and what is met only in part:

- **Tables:** sticky headers and "load more" everywhere; no column sorting (the charts list sorts by a choice, the events list orders by creation, as the API does) and no keyboard row navigation (rows are reached by Tab through their controls).
- **Keyboard:** every action is reachable and operable by keyboard, with no trap; the console frame has neither `?` for shortcuts nor a Ctrl+K palette (the designer has both, 07).
- **Feedback:** results and refusals are toasts or alerts and loading lists show skeletons, but actions wait for the server's answer instead of updating optimistically with a rollback (the manager shows its own action from the reply until the state catches up, D-82).
- **Accessibility:** what the walk checks (keyboard, focus, roles and names, contrast of the tokens by `tokens.test.ts`) holds on every screen above; not proven: a screen reader, a second browser engine, zoom to 400 % and reflow at 320 px, and the designer (07).

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

As built (`apps/console`, CON-001 in part): Solid Router 1.0.1, Tailwind 4.3.3 through its Vite plugin, Kobalte 0.13.14, each pinned. Routes: `/charts`, `/events`, `/reports`, `/developers` and `/settings` in the console frame (left navigation, content at most 1,280 px; every screen but Charts says it is not built yet), and full-window tools outside it, the designer at `/designer/local` for a chart from a file and at `/designer/<workspace key>/<chart key>` for a chart's server draft (07 §9, DSN-035), opened from the charts list or a chart's page. The design tokens are CSS custom properties in `src/styles.css`, light and dark; Tailwind's own palette and type scale are removed, so a component can use only tokens, and a test checks the text and control pairs for WCAG 2.2 AA contrast in both themes. The theme is the system's unless the user picks one (stored per browser), applied as `data-theme` on the root. Every user-visible string is in one English catalogue, `src/i18n/messages.ts`, by stable id; groups keyed by the edit core's values (node kinds, tools, units, command and import error codes, undo entry keys, inspector fields, the compiler's finding codes) are typed against the generated types, and a test fails when the types or the compiler gain a value without a message. Numbers and times go through `Intl`. The base components are in `src/ui`.

**The API client** (CON-001 to CON-005; D-60). `src/api/generated.ts` is written by `cargo xtask types` from `crates/cavea-server/openapi.json`: a TypeScript type per schema, and `OPERATIONS`, each operation's method, path, `x-cavea-permission` and `x-cavea-roles`, with its input (path and query parameters, headers other than the two below, body) and reply. It is committed and not formatted; `cargo xtask types --check` and `cargo test -p xtask` fail while it is stale, so a change to the server's routes or schemas regenerates the document and then the types. `src/api/client.ts` is the one request layer: `call(operation, input, signal)` sends relative URLs with `credentials: "same-origin"`, `Cavea-Csrf-Token` on every non-safe method (the token is held in memory only), `Cavea-Workspace` on every operation that declares a permission, and rejects with an `ApiError` (`status`, `code`, `details.reason`, the request id, `Retry-After`), or with the caller's abort; an answer that is not a problem (a proxy's page, garbage) is `unexpected`, no answer `network`, 30 s without one `timeout`. A `401` with `reason` `console_session`, or `missing` (no cookie at all), ends the session in one place (`src/session.ts`): the frame sends the person to `/sign-in?next=<where they were>` and sign-in returns there (only to a path of the console). `me` is loaded at start and read again when the page is shown after a minute away, so an expired session or a changed role is noticed; a `403` `role` or `workspace` from the server is shown and reads `me` again. Server state for a screen is `createQuery` (`src/api/query.ts`), keyed on the workspace: switching it cancels what is in flight and starts from loading. The selected workspace is kept per browser. Routes are under Vite's `base` (`import.meta.env.BASE_URL` into the router's `base`), which the server's embedding sets (CON-015); with the root as base, `/console/setup` is the setup page too, as the server logs it.

**Served by the server** (CON-015, D-62). `cargo xtask build-server` builds the WebAssembly modules and the console (`apps/console/dist`), writes each file and its brotli form (quality 11, kept when at most nine tenths of the file) under `target/console/` with a table of `include_bytes!`, and builds `cavea-server` in release with `CAVEA_CONSOLE_EMBED` naming the table; `crates/cavea-server/build.rs` compiles it in. Without that variable (a plain `cargo build`, CI) the table is empty. The console is served at the root of the server's origin, as Vite builds it (`base` `/`, files in `/assets/`), from the router's fallback, so no API route can be shadowed:

- A request path is looked up exactly in the table: nothing is read from disk and no path can name anything else.
- `/assets/…` (content-hashed names): `Cache-Control: public, max-age=31536000, immutable`. `index.html`: `no-cache`. Each has a strong `ETag` (`304` for `If-None-Match`), `X-Content-Type-Options: nosniff` and its type (`application/wasm` for WebAssembly, so it compiles while streaming). `HEAD` answers as `GET`.
- To a request accepting `br` the brotli form, `Content-Encoding: br`, with its own `ETag`; files that have one say `Vary: Accept-Encoding`. No gzip: browsers send `br` over HTTPS, and gzip would need a new crate.
- A `GET` for another path answers `index.html` only to a navigation (`Accept` naming `text/html`), and `/` always: the console's routes (`/charts/…`, `/designer/local`, `/console/setup`, `/console/invite`) work when opened or reloaded. Under `/assets/`, or without `text/html`, it is problem+json `404`, so a missing script or image is never HTML. `/v1`, `/console/api` and their subpaths, `/healthz`, `/readyz` and `/metrics` are never the console's.
- The files take no credential and are the same for everyone; the console cookie's `no-store` (08 §11) does not apply to them.
- A server built without the console answers its navigations `404` with a short text saying so and how to build it.
- The built console loads the designer's WebAssembly (`/assets/cavea_designer_bg-….wasm`) and the seat labels' glyph atlas (`/assets/glyphs-….png`, since the designer's canvas, #63); the event manager and the designer's preview (when first opened, 07 §11; the chart handed over as bytes, no `blob:` URL or worker) load the renderer's (`/assets/cavea_web_bg-….wasm`) and the same atlas, and the compiled chart from `/v1/public/charts/`, all from the console's own origin. Six files, 4.15 MB with their brotli forms. The Content Security Policy and the other security headers are set by path for every response (08 §6.5, SEC-004); the built `index.html` has no inline script or style element.

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
- **Licence** (`ConsoleLicence`, `LicenceUpload`; 12 §7.6, D-81; the screen and banners are LIC-004). `me.licence` (also in every sign-in, setup and password reply): `{state: "valid" | "grace" | "expired" | "missing" | "invalid", read_only, grace_until, manage, source, reason, licence}`. Everyone gets `state`, `read_only` (true while expired: disable chart and event writes, keep the event manager's seat operations), `grace_until` (the last day of grace, `YYYY-MM-DD`, while in grace, else `null`) and `manage`. `manage` is true for a user who owns every workspace on the server; only then are `source` (`setting` or `upload`), `reason` (why invalid: `unreadable`, `too_large`, `not_utf8`, `malformed`, `version`, `unknown_key`, `signature`, `payload`, `dates`) and `licence` (`{id, customer, plan, features, domains, max_workspaces, issued_at, expires_at, grace_days, key_id}`) filled, else `null`. Banners: grace for owners and admins (the role in `workspaces`), "expired" with read-only for everyone, "Development" while missing or invalid. Upload (when `manage`): `PUT /console/api/licence` `{licence: "<the file's text>"}` with `Cavea-Csrf-Token` → `200` `ConsoleLicence`; `400` `invalid_field` with `details.field: "licence"` and `details.problem` (one of the reasons above) for a file that does not verify, nothing stored; `403` `instance_owner` for anyone else. A refused write while expired is `403` with `reason: "licence_expired"` on the chart and event routes (`charts.write`, `charts.archive`, `events.write`) only.

What `/v1` offers the console's screens beyond the above (08 §6.2; shapes in the OpenAPI document; the cookie, `Cavea-Workspace` and, when not `GET`, `Cavea-Csrf-Token` as everywhere):

- **An event's state for the manager** (events read; D-72): `GET /v1/events/{event}/state` → `200` the manager SNAPSHOT frame (`application/vnd.cavea.frame`, 08 §8.1, decoded with `View::Manager`: domain statuses 0 to 15, encodings 0 and 1). Then poll `…/state?after_seq=<the seq you have>` every 1 to 3 s: `204` nothing new; `200` one DELTA from your `seq` (apply it; its `to_seq` is your new `seq`) or a SNAPSHOT (replace everything) when the server no longer keeps the deltas from your `seq` (about 60 s) or restarted. A frame you cannot apply (a DELTA whose `from_seq` is not yours) means: poll with `after_seq=0`. Every event object has `seq` (committed) and `counts`, so `GET /v1/events/{event}` alone says whether anything changed; the state's `seq` follows the committed one within one publication interval (100 ms, longer with very many subscribers). Order references and hold expiries come from `GET /v1/events/{event}/objects?label=…` for the seats a screen shows (up to 1,000 labels a request). There is no stream for staff yet (SRV-039).
- **Events list** (events read): `GET /v1/events?limit=&after=&chart=&q=&archived=&order=`: `q` searches key and name, ignoring case, every character literal (at most 128); `archived=true|false`; `order=oldest|newest` (default `oldest`); each row has `counts` (`{free, held, booked, blocked, custom}`) and `seq`; `after` is the last row's key and stays valid with any filters and order.
- **A publishable key's origins** (developers): `PATCH /v1/keys/{key_…}` `{allowed_origins: ["https://…"] | null}` → `200` the key; the field is required (`null` allows every origin), 1 to 50 origins; `400 invalid_field` (`allowed_origins`) for a secret key or a malformed origin, `400` `reason: "revoked"` for a revoked key. Audited (`api_key.origins_changed`).
- **The designer's editing lock** (read: charts read; take and release: charts write): `GET`, `POST` (`{}` or `{take_over: true}`) and `DELETE /v1/charts/{chart}/lock`; `409 chart_locked` with `details.lock` `{holder, email, since, expires_at}`; the life cycle is in 07 §9.
- **Version thumbnails and authors** (charts write; reads: charts read): `PUT /v1/charts/{chart}/versions/{number}/thumbnail` `{asset: "<hash from POST /v1/assets>"}` → `200` the version. Every version object has `thumbnail` (`{hash, url}` or `null`) and `created_by` (`usr_…`, `key_…` or `null`), so the charts list's `published_version` carries the card's thumbnail; the versions list adds `created_by_email`, filled for owners and admins (`settings`) while the publisher is a member, else `null`.
