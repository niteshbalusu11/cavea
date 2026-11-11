# Cavea — spec set

**Status:** Draft v0.1 · 2026-11-10 · pre-validation (nothing is built yet)

"Cavea" is a placeholder codename (the seating bowl of a Roman theatre). Name, npm scope, crate names and domain have **not** been checked for availability or trademarks. Do that before publishing anything (see `14-go-to-market.md` §9).

Cavea is a seat-map toolkit for ticketing platforms: a buyer-facing seat picker (web and mobile), a chart designer, and a self-hosted server that holds per-event seat state. It is sold as a flat-price licence that the customer hosts.

## How to read this

Read the first five in order. The rest are reference documents for the component you are working on.

| # | Document | What it decides |
|---|---|---|
| 00 | [Product brief](00-product.md) | Who buys, what v1 contains, what it does not, pricing hypothesis, kill criteria |
| 01 | [Architecture](01-architecture.md) | Components, repo layout, tech stack, performance budgets |
| 02 | [Domain model](02-domain-model.md) | Vocabulary, identifiers, the seat state machine, invariants |
| — | [Decisions](DECISIONS.md) | The 21 architectural decisions, with alternatives and revisit triggers |
| — | [Roadmap](ROADMAP.md) | Milestones M0–M9 with exit criteria |
| 03 | [Chart format](03-chart-format.md) | Source document schema, compiler, compiled binary format (CCF) |
| 04 | [Renderer](04-renderer.md) | GPU pipelines, level of detail, text, camera, input, hit testing |
| 05 | [Web SDK](05-web-sdk.md) | Public JavaScript API, state adapters, built-in UI, loading |
| 06 | [Mobile SDK](06-mobile-sdk.md) | WebView wrappers first, then the native renderer |
| 07 | [Designer](07-designer.md) | Editor UI, tools, numbering, publish flow, embedding |
| 08 | [Server](08-server.md) | Event actor, SQL schema, REST API, realtime protocol, best-available |
| 09 | [Console](09-console.md) | Back-office screens, roles, event manager |
| 10 | [AI import](10-ai-import.md) | Venue PDF or image to draft chart |
| 11 | [Accessibility](11-accessibility.md) | Keyboard model, screen-reader model, list view, test matrix |
| 12 | [Security and licensing](12-security-licensing.md) | Threat model, keys, abuse controls, licence enforcement |
| 13 | [Testing and benchmarks](13-testing-benchmarks.md) | Test strategy, simulation testing, benchmark methodology |
| 14 | [Go to market](14-go-to-market.md) | Validation plan, pricing tests, launch checklist |
| — | [Orchestration](ORCHESTRATION.md) | How the build is run: one orchestrator, Opus builders, Sol reviewers |
| — | [TODO](TODO.md) | The full task list, grouped by milestone, each item pointing at a spec section |

## The stack on one page

| Layer | Choice |
|---|---|
| Shared core (format, hit testing, selection rules, best-available) | Rust, compiled to WebAssembly, native server, and mobile libraries |
| Web rendering | WebGL2, driven directly from Rust through `glow`; Canvas 2D and list-view fallbacks |
| Native mobile rendering | `wgpu` (Metal on iOS, Vulkan or GLES on Android); WebView wrappers ship first |
| Shaders | One WGSL source; GLSL ES 3.00 generated at build time |
| Text | Baked MSDF atlas for Latin and digits; platform rasteriser for every other script |
| Chart delivery | Compiled binary chart (CCF) on immutable, content-addressed URLs |
| Server | Rust: tokio, axum, sqlx; PostgreSQL is the only required dependency |
| Seat state | One in-memory single-writer actor per event, group-committed to PostgreSQL |
| Realtime | Binary WebSocket frames, SSE fallback, cacheable HTTP snapshot |
| Designer and console UI | SolidJS, TypeScript, Vite; the canvas is the same Rust renderer |
| Mobile bindings | UniFFI for Swift and Kotlin; React Native and Flutter wrappers |

## Rules for these documents

- A number marked **target** is a goal that has not been measured. A number marked **measured** has a benchmark behind it in `13-testing-benchmarks.md`.
- Anything said about a competitor is dated November 2026 and sourced in `00-product.md` §4.
- When a spec and the code disagree, fix one of them in the same change. Record reversals in `DECISIONS.md`.
- Each TODO item names the spec section it implements, for example `(04 §6.2)`.

## Working order

1. Do milestone M0 first. It is a two-to-three-week validation spike, and it decides whether any of the rest gets built.
2. Build in milestone order. Each milestone has exit criteria in `ROADMAP.md`; do not start the next one until they are met.
3. Pick one TODO item at a time, read the section it points at, implement, and tick it.
