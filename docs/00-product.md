# 00 · Product brief

**Status:** Draft v0.1 · 2026-11-10

## 1. What we are building

A seat-map toolkit that a ticketing platform installs on its own infrastructure.

| Component | Who uses it | What it does |
|---|---|---|
| **Renderer** | Ticket buyers | Interactive "pick your seat" map inside the customer's checkout, on web and in mobile apps |
| **Designer** | Venue staff, platform staff | Browser editor for drawing seating charts |
| **Server** | The customer's backend | Holds per-event seat state: holds, bookings, blocks; pushes live updates |
| **Console** | Platform staff, box office | Back office: charts, events, manual seat operations, reports, API keys |

The customer keeps payments, orders, customers and pricing. Cavea never touches money.

## 2. Customer

**Ideal customer:** a small or mid-sized ticketing platform (5–100 people) with its own checkout, an engineering team of 2–20, selling roughly 10,000 to 2,000,000 reserved seats a year.

**Secondary:** theatre and cinema groups with their own apps; agencies that build ticketing for venues.

**Later, not now:** bus, ferry and rail seat booking; exhibition floor plans; desk booking.

| Persona | Needs |
|---|---|
| Integrating developer (the buyer's champion) | Fast integration, typed SDK, no iframe, control over styling, predictable cost |
| CTO (the economic buyer) | Flat price, no dependency on a vendor's uptime, data stays in-house |
| Venue manager | Draw a 2,000-seat theatre in under an hour without training |
| Box-office operator | Block, release and manually book seats quickly |
| Ticket buyer | The map opens instantly on a phone, and a tap selects the seat they meant |

## 3. Problem

- Building a seat map in-house is a 6–12 month project by one vendor's own estimate, so most platforms buy.
- The market leader bills per seat used. Cost grows with the customer's success.
- Every vendor's mobile SDK is a WebView around the web renderer.
- Self-hosting is available from one vendor only, at €20,000 a year.
- No vendor documents keyboard or screen-reader support for buyers, while customers selling in the EU now have accessibility obligations for their checkout.

## 4. Competition (verified November 2026)

| | seats.io | Seatmap Pro | SeatLayer |
|---|---|---|---|
| Hosting | Vendor cloud only | Cloud, or on-premise | Vendor cloud |
| Price | €600/month for 5,000 seats, then €0.12 per seat | From €100/month; on-premise €20,000/year | $0.10 down to $0.05 per sold seat |
| Web embedding | iframe | Mounts in host DOM | Mounts in host DOM; iframe optional |
| 2D rendering | Not stated | WebGL with Canvas 2D fallback | "Canvas", technology not named |
| Mobile SDKs | iOS, Android, React Native, Flutter — all WebView | None documented | iOS, Android, React Native, Flutter — all WebView |
| Performance claim | "80,000 seat stadiums" on a phone | 10,000+ seats at 60 fps | 200,000 seats chart-ready in 1.95 s on desktop |
| Best-available API | Yes | Not found | Yes |
| Channels, seasons | Yes | Series only | Yes |
| Booking webhooks | No (configuration changes only) | No | Yes |
| Buyer accessibility claims | None found | None found | Checklist item only |
| 3D view | Yes | Partly | Yes |
| API rate limit | 100 concurrent requests per account | Unspecified | 120 holds per minute per IP |

Notes:

- SeatLayer is newer than the other two but broad: it documents multi-floor charts, SVG and DXF import, seasons, channels, 43 languages and a seats.io migration guide. Treat it as a serious competitor.
- All performance figures are the vendors' own.
- Sources are listed at the end of this document.

## 5. Where we win

| Wedge | Why it is open |
|---|---|
| **Self-hosted at a flat price** | Nothing exists between per-seat cloud pricing and €20,000 a year |
| **Fastest renderer, including native mobile** | All mobile SDKs are WebViews; no vendor publishes independent benchmarks |
| **Works with any backend** | The renderer accepts seat state from the customer's own inventory system; the server is optional |
| **Accessible seat picking** | No vendor documents keyboard or screen-reader support |
| **No vendor rate limit** | The customer runs the server; the leader caps accounts at 100 concurrent requests |
| **Chart from a PDF or image** | The two established vendors only let you trace over a background |

Lead with the first two. The others support them.

## 6. Scope

Priority: **P0** is needed for the private beta, **P1** for the public v1, **P2** comes after.

### Designer

| Feature | Priority |
|---|---|
| Rows, row blocks, curved blocks (arcs), sections | P0 |
| Categories, row and seat numbering schemes | P0 |
| Shapes, text, background reference image, focal point | P0 |
| Undo and redo, snapping, align and distribute, duplicate | P0 |
| Draft autosave, validation, immutable published versions | P0 |
| Tables, booths, general-admission areas | P1 |
| Multiple floors | P1 |
| Publish diff and safe mode for charts with bookings | P1 |
| Embeddable in the customer's back office (white-label) | P1 |
| seats.io chart importer | P1 |
| SVG and DXF import | P2 |
| AI import from PDF or image | P2 |

### Renderer

| Feature | Priority |
|---|---|
| Pan, zoom, tap to select, level of detail, live updates | P0 |
| Pricing per category with ticket types | P0 |
| Maximum selection, hold-on-select, tooltips | P0 |
| Themes, basic keyboard support and ARIA | P0 |
| Bring-your-own state adapter | P0 |
| Orphan-seat, consecutive-seat and minimum-selection validators | P1 |
| One-click group selection | P1 |
| Category, price and accessibility filters | P1 |
| Section drill-down, multi-floor selector | P1 |
| Accessible list view, full screen-reader support | P1 |
| Localisation, right-to-left layouts | P1 |
| Static, spotlight and manager modes | P1 |
| WebView wrappers for iOS, Android, React Native, Flutter | P1 |
| Native mobile renderer | P2 |
| View-from-seat images, minimap, print mode | P2 |
| Rendering in a web worker | P2 |

### Server and console

| Feature | Priority |
|---|---|
| Events, hold tokens, book, release, block | P0 |
| Realtime snapshot and deltas | P0 |
| Secret and publishable keys, workspaces, idempotency | P0 |
| Console: charts, events, basic event manager, keys | P0 |
| Docker image, TypeScript server SDK, React wrapper, docs | P0 |
| Best-available | P1 |
| Channels, general admission, tables, booths | P1 |
| Webhooks for seat changes | P1 |
| Reports and CSV export | P1 |
| Server SDKs for PHP, Python, Java, C#, Go | P1 |
| Custom statuses | P2 |
| Seasons and season tickets | P2 |
| Active–passive failover, then multi-node | P2 |
| Single sign-on for the console | P2 |

## 7. Non-goals

- **3D venue views.** Two competitors have them. Our position is 2D speed; revisit only if paying customers ask.
- **Payments, orders, ticket issuance, customer accounts.** These stay in the customer's system.
- **Pricing logic.** The renderer displays prices the host supplies.
- **A managed cloud.** Not before the self-hosted product has paying customers.
- **Consumer seating planners** for weddings and similar events.

## 8. Pricing hypothesis

Untested. The purpose of milestone M0 is to test it.

| Plan | Annual price | Contents |
|---|---|---|
| Starter | €1,900 | One production workspace; renderer, designer, server; community support |
| Growth | €4,900 | Unlimited workspaces; embeddable white-label designer and manager; email support |
| Scale | €12,000 | Failover and multi-node; single sign-on; priority support; source access |
| AI import | Per-import credits | Optional hosted service |

Rules:

- No per-seat fees on any plan.
- Development and staging use are free, with a watermark.
- These prices sit between the leader's entry cloud plan (€7,200 a year plus overage) and the only self-hosted offer (€20,000 a year).

## 9. Success metrics and kill criteria

| Checkpoint | Continue if | Stop or reposition if |
|---|---|---|
| End of M0 (validation) | At least 3 real conversations and 1 "I would pilot this" from 20–30 outreach messages | Fewer than 3 replies, or nobody is unhappy with their current vendor |
| 8 weeks after the beta (M4) | 1 paying pilot | No pilot despite 50+ qualified conversations |
| 12 months after M4 | €3,000 monthly recurring revenue | Under €1,000 with no pipeline |

Product metrics to track from the first pilot:

- Time from signup to first chart rendered in the customer's staging checkout (target: under one hour).
- Chart-ready time and frame time on the reference phones (see `01-architecture.md` §5).
- Double bookings: zero, ever.

## 10. Risks

| Risk | Mitigation |
|---|---|
| Switching cost is high; platforms rarely change seat-map vendor | Target new builds and platforms with a painful per-seat bill; ship a seats.io importer and an API mapping guide |
| SeatLayer is moving fast | Compete on what a hosted vendor cannot copy cheaply: self-hosting, flat price, native mobile |
| The designer is a very large piece of work | Strict milestone gating; ship row blocks and arcs first; defer tables, floors and imports |
| A solo developer supporting sales-critical software | Self-hosting keeps the customer's uptime in their hands; publish a realistic support policy; invest in simulation testing |
| A double booking destroys trust | Single-writer state engine, model-based and simulation tests, a reconciliation tool (`13-testing-benchmarks.md`) |
| Demand cannot be measured from outside | M0 exists to measure it before the build |
| Employment contract | Check the IP assignment clause before writing code |

## Sources

- seats.io pricing: https://www.seats.io/pricing
- seats.io docs: https://docs.seats.io (renderer, embedded designer, API, event manager, webhooks, rate limit)
- seats.io mobile SDK sources: https://github.com/seatsio/seatsio-ios , https://github.com/seatsio/seatsio-android , https://github.com/seatsio/seatsio-react-native , https://github.com/seatsio/seatsio-flutter
- Seatmap Pro knowledge base: https://seatmap.pro/knowledge-base/
- Seatmap Pro comparison articles: https://seatmap.pro/blog/seats-io-alternatives/ , https://seatmap.pro/blog/seatmap-pro-vs-seats-io/
- SeatLayer docs: https://docs.seatlayer.io
- SeatLayer SDK repository: https://github.com/seatlayer/seatlayer-sdk
