# Concepts

The words Cavea uses, what each thing is, and how they fit together. After this page you know which page to read for each part; each of those pages takes a few minutes.

## How the parts fit

A venue's seat map is a **chart**. Staff draw it in the designer and publish it; each publish makes an unchanging **version**. An **event** is one performance on one version: it owns the **status** of every seat for that performance.

A buyer's page shows the event with the seat map. When the buyer clicks a seat, the map **holds** it on the server under a **hold token**, a secret that stands for that buyer's selection. After payment, your backend **books** what the hold token holds. Every open seat map sees seats change through the **realtime stream**.

Your backend authenticates with a **secret key**, the buyer's page with a **publishable key**, and staff sign in to the console, each with a **role** in a **workspace**. A backend that knows its buyers can also sign **session tokens**, so that only buyers it vouches for can hold seats.

```text
workspace
├── charts ── draft ── versions (published, never change)
├── events ── each on one chart version ── seat statuses, holds, bookings
├── API keys (secret, publishable)
└── members (console sign-ins, each with a role)
```

## The pages

| Page | Read it for |
|---|---|
| [Charts and versions](charts.md) | Drafts, publishing, versions, the designer |
| [Events](events.md) | Events, their settings, and the chart version they keep |
| [Objects and statuses](objects.md) | Seats, labels, statuses, and what a buyer can see |
| [Holds and hold tokens](holds.md) | How a seat is reserved while a buyer pays, and for how long |
| [Bookings](bookings.md) | Selling held seats, idempotency, box-office sales |
| [Session tokens](session-tokens.md) | Letting only your signed-in buyers hold seats |
| [Keys](keys.md) | Secret, publishable and admin keys, origins, rotation |
| [Workspaces and roles](workspaces.md) | Tenants, members, roles and permissions |
| [Realtime stream](realtime.md) | How every seat map stays current |

## Glossary

| Term | Meaning |
|---|---|
| Workspace | A tenant: its own charts, events, keys and members. See [Workspaces and roles](workspaces.md). |
| Chart | A venue's seat map, with an editable draft and published versions. See [Charts](charts.md). |
| Chart document | The draft as JSON: what the designer edits and exports. |
| Version | A published, compiled, unchanging snapshot of a chart. |
| Event | One sellable performance, on one chart version. See [Events](events.md). |
| Object | Anything bookable. Today that is a seat. See [Objects](objects.md). |
| Label | An object's name in its chart, such as `Stalls-C-12`. The API names objects by label. |
| Category | A pricing and colour group of seats, such as Standard or VIP. Cavea does not store prices. |
| Status | `free`, `held`, `booked`, `blocked`, or a custom status, per object and event. |
| Hold | A reservation of objects that expires, under a hold token. See [Holds](holds.md). |
| Hold token | The secret that identifies one hold: whoever has it controls the hold. |
| Booking | The sale of objects, with your order reference. See [Bookings](bookings.md). |
| Session token | A short-lived token your backend signs for one buyer and one event. See [Session tokens](session-tokens.md). |
| Secret key, publishable key, admin key | The API's three credentials. See [Keys](keys.md). |
| Sequence number (`seq`) | A per-event counter, one more for every change to its seats. |

## Not yet

These are planned and not in the server yet; the API and these pages will grow when they arrive:

- **Channels**: setting seats aside for a presale, a partner or the box office.
- **Best-available**: the server choosing and holding the best seats for a quantity.
- **General-admission areas** booked by quantity, **tables** and **booths** booked whole.
- **Moving an event to a newer chart version.**
- **Webhooks**, such as one when a hold expires.
