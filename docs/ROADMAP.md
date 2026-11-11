# Roadmap

**Status:** Draft v0.1 · 2026-11-10

Ten milestones. Each has a goal, a scope, and exit criteria that must all be met before the next one starts. Tasks are in `TODO.md` under the same milestone names.

## Effort

The estimates below are rough and unvalidated. A "focused week" is about 40 hours of concentrated work.

| Milestone | Estimate (focused weeks) |
|---|---|
| M0 Validation spike | 1 of code, plus outreach over 2–3 calendar weeks |
| M1 Core, format, web renderer | 4–6 |
| M2 Server and live state | 4–5 |
| M3 Designer MVP | 6–8 |
| M4 Console, licensing, docs → private beta | 3–4 |
| M5 Selling features | 5–7 |
| M6 Accessibility, fallbacks, mobile wrappers → public v1 | 5–6 |
| M7 AI import | 5–7 |
| M8 Native mobile renderer | 6–8 |
| M9 Scale and enterprise | Open-ended |

M0 to M4 total roughly 18–24 focused weeks, or 700–950 hours. At 15 hours a week that is about a year to a private beta. AI coding agents will shorten this; by how much is unknown, so do not plan on it.

The order after M4 should follow what pilots ask for. M7 and M8 in particular may swap or drop.

```
M0 ─▶ M1 ─▶ M2 ─▶ M3 ─▶ M4 ══ private beta ══▶ M5 ─▶ M6 ══ public v1 ══▶ M7 / M8 / M9
gate: go/no-go                 gate: paying pilot                          order by demand
```

---

## M0 · Validation spike

**Goal:** Learn whether ticketing platforms want this, and prove the rendering approach on a phone.

**Scope**

- Rendering spike: 100,000 synthetic seats through WebGL2 from Rust, with pan, zoom and tap.
- Two technical spikes: build-time shader translation; measured WASM size.
- Public demo page, landing page, outreach and interviews (`14-go-to-market.md` §1).
- Legal checks: employment contract, name availability.

**Exit criteria**

- [ ] The demo holds 60 fps on the reference Android phone at 100,000 seats.
- [ ] Measured payload of the spike is recorded and consistent with the 150 KB target.
- [ ] 20–30 messages sent, at least 5 conversations held and written up.
- [ ] A written go, reposition or stop decision against `00-product.md` §9.

**If the decision is "stop", everything below is cancelled.**

---

## M1 · Core, format, web renderer

**Goal:** A production-quality buyer renderer that loads a compiled chart and a static state.

**Scope**

- Chart document types, compiler, CCF writer and reader, CLI, synthetic presets.
- Engine, `glow` backend, all five pipelines, level of detail, text, camera, gestures, hit testing, local selection.
- Web SDK: `createSeatMap`, options, callbacks, methods, built-in controls, theming, static and custom state adapters, React wrapper.
- Baseline keyboard navigation and screen-reader proxy.
- Golden-image harness, micro-benchmarks, benchmark page, fuzz targets.

**Exit criteria**

- [ ] Every renderer budget in `01-architecture.md` §5 is met on the reference devices and recorded.
- [ ] Payload size gate is in CI and green.
- [ ] The CCF decoder has run 24 hours of fuzzing without a finding.
- [ ] A developer can render a chart from a JSON document with a custom state adapter using only the docs draft.

---

## M2 · Server and live state

**Goal:** A server that holds and books seats correctly under load, with live updates in the renderer.

**Scope**

- Engine: actor, commands, expiry, idempotency, fencing, load and eviction.
- PostgreSQL store, schema, migrations.
- REST API for charts (publish from a JSON document), events, hold tokens, seat operations; public endpoints.
- Realtime: WebSocket, SSE, polling, cacheable snapshot.
- Renderer: `caveaServer` adapter, hold-on-select, session modes, restore on reload.
- TypeScript server SDK. Docker image and compose file.
- Model-based tests, simulation harness, load generator, reconciler.

**Exit criteria**

- [ ] Simulation runs a million steps with all invariants holding.
- [ ] Load scenarios L1 and L2 pass on the reference hardware with a clean reconciler report.
- [ ] `kill -9` and PostgreSQL-restart chaos experiments pass.
- [ ] Server budgets in `01-architecture.md` §5 are met and recorded.
- [ ] End to end: two browsers see each other's selections become unavailable within 250 ms.

---

## M3 · Designer MVP

**Goal:** A venue manager can draw, number and publish a real theatre.

**Scope**

- Edit core: document, commands, undo and redo, live scene.
- Tools: select, hand, row block, arc block, single row, section, shapes, text, image, focal point, category brush.
- Inspector, numbering panel, categories, layers, snapping and guides, align and distribute.
- Row and seat editing, autosave and recovery, validation panel, preview, publish with diff.
- Templates, JSON import and export.

**Exit criteria**

- [ ] A person who has never seen the tool draws a given 2,000-seat theatre from a reference image in under 60 minutes. Test with three people.
- [ ] The designer budgets in `01-architecture.md` §5 are met.
- [ ] Undo property tests pass.
- [ ] The full flow works end to end: draw, publish, create an event, sell a seat.

---

## M4 · Console, licensing, docs → private beta

**Goal:** Something a pilot customer can install, operate and pay for.

**Scope**

- Console: sign-in, charts, events, basic event manager, API keys, members.
- Licence files and enforcement that never blocks sales.
- Security baseline: threat-model review, SSRF guard, login protection, signed releases.
- Documentation: quick start, references, deployment guide, examples.
- Release pipeline, upgrade test, demo instance.
- Pilot agreement, licence text, pricing page.

**Exit criteria**

- [ ] A stranger follows the quick start from an empty machine to a seat held in a browser in under 30 minutes.
- [ ] The release checklist in `13-testing-benchmarks.md` §13 passes.
- [ ] The licence agreement has been reviewed by a lawyer.
- [ ] At least one pilot has agreed to install it.

**Gate:** eight weeks after the beta starts, check the criteria in `00-product.md` §9.

---

## M5 · Selling features

**Goal:** Close the feature gaps that block real on-sales.

**Scope**

- Best-available; orphan, consecutive and minimum rules on client and server; group selection.
- General-admission areas, tables, booths, multiple floors.
- Channels; custom statuses; event upgrade with conflict report.
- Webhooks; reports and CSV; activity log.
- Renderer: filters, section drill-down, static, spotlight and manager modes.
- Console: full event manager, channels, reports, webhooks.
- Designer: tables, booths, areas, floors, safe mode, embedding, CSV and seats.io import.
- Server SDKs in five more languages; Vue and Svelte wrappers; locale packs.

Prioritise within this list by what pilots need first.

**Exit criteria**

- [ ] Every P1 row in `00-product.md` §6 for server, console and designer is done.
- [ ] A pilot customer has run a real on-sale.
- [ ] Load scenarios L3 to L5 pass.

---

## M6 · Accessibility, fallbacks, mobile wrappers → public v1

**Goal:** A product that can be launched publicly and recommended without caveats.

**Scope**

- Full keyboard model, live regions, list view (both implementations), non-colour cues, palettes, forced colours, timing dialog.
- Canvas 2D fallback and the capability ladder; dynamic resolution.
- WebView wrappers for iOS, Android, React Native and Flutter, with example apps.
- Migration guide, comparison page, public benchmark page, accessibility statement.

**Exit criteria**

- [ ] All eight journeys in `11-accessibility.md` §9.3 pass in the full screen-reader matrix.
- [ ] Sessions with at least three screen-reader users completed; blocking findings fixed.
- [ ] The renderer works with WebGL2 disabled and with WASM blocked.
- [ ] All four mobile packages are published and their UI tests pass.
- [ ] The public launch checklist in `14-go-to-market.md` §10 is complete.

---

## M7 · AI import

**Goal:** A venue's PDF becomes a mostly correct draft chart in about a minute.

**Scope:** `10-ai-import.md`, phases I1 to I4.

**Exit criteria**

- [ ] The evaluation targets for clean vector and clean raster inputs are met on the corpus.
- [ ] Correction time is at least 60% lower than drawing from scratch, measured with three people.
- [ ] Cost per import is measured and credits are priced.

---

## M8 · Native mobile renderer

**Goal:** Replace the WebView on iOS and Android with a native view, with no API change.

**Scope:** `06-mobile-sdk.md` §3.

**Exit criteria**

- [ ] Mobile budgets in `06-mobile-sdk.md` §3.6 are met on the reference phones.
- [ ] Backend parity golden tests pass.
- [ ] VoiceOver and TalkBack journeys pass.
- [ ] A customer's app ships with it.

---

## M9 · Scale and enterprise

**Goal:** Remove the remaining objections of larger customers.

**Scope, in likely order**

1. Active–passive failover.
2. Single sign-on for the console; restricted API keys.
3. Seasons and season tickets.
4. Multi-node operation.
5. Worker-mode rendering.
6. View-from-seat images, minimap, print mode.
7. SVG and DXF import, SVG export.
8. Private charts, object storage for assets, Helm chart.
9. External accessibility audit and conformance report.
10. External security review.

**Exit criteria:** defined per item when it is scheduled.

---

## Always

These apply to every milestone:

- Budgets are checked on the reference devices before a milestone closes.
- A spec that no longer matches the code is corrected in the same change.
- A decision that is reversed gets a new entry in `DECISIONS.md`.
- Nothing is added to `cavea-core`, `cavea-format` or `cavea-web` without checking its effect on payload size.
