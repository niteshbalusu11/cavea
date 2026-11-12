# 14 · Go to market

**Status:** Draft v0.1 · 2026-11-10

The engineering plan is large. This document exists to make sure it is only executed if people will pay for the result, and to get the first customers while the product is still small.

## 1. Milestone M0: validate before building

**Timebox:** two to three weeks. **Outcome:** a go or no-go decision against the criteria in `00-product.md` §9.

### 1.1 What to produce

| Item | Detail |
|---|---|
| Demo | A public page with a synthetic 100,000-seat stadium and a 2,000-seat theatre, rendered by the M0 spike. Shows chart-ready time and a frame-time meter. Must be convincing on a phone |
| Landing page | One sentence of positioning, three wedges (self-hosted and flat price; fastest on mobile; accessible), the pricing hypothesis, an early-access form asking for company, role, current solution and yearly seat volume |
| Short video | A 30-second screen recording of the demo on a mid-range phone |
| Prospect list | 40 ticketing platforms with: company, country, size, current seat-map vendor if detectable, a named technical contact, source |
| Outreach | 20–30 personal messages in two variants |
| Interviews | At least five conversations, each recorded in the same template |

### 1.2 Finding prospects

- Ticketing platforms that visibly use a hosted seat-map vendor (their checkout loads that vendor's script). These have the bill we are attacking.
- Customer logos and case studies on the competitors' sites.
- Software directories' ticketing categories; regional ticketing companies outside the largest markets, where euro-priced per-seat fees hurt most.
- Open-source ticketing projects and the agencies around them.
- Recent job posts mentioning seat maps or reserved seating.

### 1.3 Outreach message

Principles: short, specific to them, a question instead of a pitch, a link to the demo, no attachment.

> Subject: your seat map bill
>
> Hi {name} — I noticed {company}'s checkout uses {vendor} for reserved seating. I'm building a self-hosted alternative with a flat yearly price and a renderer aimed at being the fastest on phones (demo with 100,000 seats: {link}).
>
> Before I go further I'm trying to learn whether this is a real problem. Would per-seat pricing or the hosted dependency be worth 15 minutes of complaining to me? No sales pitch; there is nothing to sell yet.

The second variant leads with mobile performance and accessibility instead of price. Track which one gets replies.

### 1.4 Interview script

1. How do you handle reserved seating today? Since when?
2. What do you pay per year, roughly, and how does that scale with your sales?
3. What annoys you most about it?
4. How is it on mobile, in your app and on the mobile web?
5. Who draws the charts, and how long does one take?
6. Have accessibility requirements for your checkout come up?
7. Would running it yourselves be attractive or a burden? Who would operate it?
8. What would have to be true for you to switch? What would block it?
9. Reaction to a flat €1,900 / €4,900 / €12,000 a year: too cheap to trust, fair, or too much?
10. If a pilot were ready in a few months, would you try it on one venue? What would the pilot need to include?

Record answers verbatim. Do not explain the product until question 9.

### 1.5 Decision

Hold a written review at the end of the timebox:

| Signal | Count |
|---|---|
| Messages sent | |
| Replies | |
| Conversations held | |
| "This is a real problem for us" | |
| "I would pilot this" | |
| Price reactions: too cheap / fair / too much | |
| Most-requested capability | |
| Most-cited blocker | |

Go, reposition, or stop, according to `00-product.md` §9. Write down the reasoning.

## 2. Positioning

**One line:** The seat map you own: self-hosted, flat-priced, and the fastest on a phone.

| Audience | Message |
|---|---|
| CTO | Stop paying per seat. Run it on your own infrastructure, with your data and your uptime in your hands |
| Developer | A typed SDK that mounts in your DOM, no iframe, native mobile views, and a renderer that works with your own inventory backend |
| Product owner | Seat selection that opens instantly on mobile and that every buyer can use, including with a keyboard or a screen reader |
| Venue manager | Draw a theatre in under an hour; start from your existing PDF |

A comparison page states honestly what competitors have that we do not: 3D views, a managed cloud, years of production use.

## 3. Pricing tests

- Show alternating price sets to alternating prospects and record reactions.
- Ask the four price-sensitivity questions: at what yearly price is it too cheap to trust, a bargain, getting expensive, too expensive.
- Test whether a renderer-only licence (no server) is wanted, and at what price.
- Test appetite for paid integration help.
- Do not discount the first pilots to zero. A paid pilot, even small, is the only reliable signal.

## 4. The first pilots

- Offer two or three design partners a reduced first-year price in exchange for a reference and weekly feedback.
- **Concierge chart drawing:** draw their venues' charts for them during the pilot. This removes the dependency on a polished designer and teaches what the designer must do well.
- Start with one venue and one event series, in their staging environment, then one real on-sale.
- Agree in writing what "success" means before starting: for example, a named on-sale completed with no seat-map incident and a measured chart-ready time.

## 5. Channels

| Channel | Action |
|---|---|
| Search | Pages for "seats.io alternative", "self-hosted seating chart", "seat map SDK for React", "React Native seat map", "accessible seat picker" |
| Documentation | Public, complete, with runnable examples; the docs are the main sales material for developers |
| Public benchmark | The benchmark page with method and scripts (`13-testing-benchmarks.md` §6.2) |
| Open components | Publish the framework wrappers, example apps and the benchmark harness under a permissive licence |
| Technical writing | "Rendering 100,000 seats on a phone", "A seat engine that cannot double-book: how we test it", "What we learned drawing 40 venues" |
| Open-source ticketing projects | An integration or plugin for the most active ones; their users are exactly the self-hosting audience |
| Agencies | A partner page and a referral fee for agencies that build ticketing systems |
| Industry | Ticketing trade associations and their conferences, once there is a reference customer |
| Direct | Continue the M0 outreach in small weekly batches |

## 6. Migration from an incumbent

Switching cost is the main obstacle, so reduce it:

- **Chart importer** for seats.io chart JSON (`07-designer.md` §12).
- **Concept and API mapping guide:** their terms and calls next to ours (events, hold tokens, booking, channels, best-available, renderer callbacks).
- **State import:** a script that reads booked objects from the incumbent's reports and books them in Cavea by label.
- **Parallel run recipe:** how to move one venue at a time.

## 7. Documentation site

| Section | Contents |
|---|---|
| Quick start | From Docker to a rendered chart in ten minutes |
| Concepts | Charts and versions, events, holds and bookings, channels, keys |
| Renderer | Options, callbacks, methods, theming, adapters, performance, accessibility |
| Mobile | iOS, Android, React Native, Flutter |
| Designer | User guide for venue staff; embedding guide for developers |
| Server | API reference (generated), webhooks, deployment, sizing, backups, upgrades, monitoring |
| Guides | Hold, pay, book; bring your own backend; best-available; channels; migrating from seats.io |
| Reference | Error codes, limits, changelog, security |

The site's sources are `apps/docs/` (D-83): Markdown pages, a build to static files, the API reference generated from the OpenAPI document, and a check that runs the quick start end to end. As built: the quick start, concepts, the server API reference, error codes, limits and the changelog; the renderer reference (generated from the package's declarations where it can be, every sample type-checked), the guides (hold, pay, book; your own backend; React; bot protection), the deployment guide (run by its own check, with TLS, a live WebSocket change through the proxy, and backup and restore) and the designer's keyboard shortcuts (generated). Still to come: the designer's guide for venue staff and the examples (DOC-008, DOC-009); mobile, migration and best-available are later milestones.

## 8. Support policy

Realistic for one person:

| Plan | Channel | First response |
|---|---|---|
| Starter | Public issue tracker | Best effort |
| Growth | Email | Two working days |
| Scale | Email and a shared channel | One working day; critical production issues the same day in working hours |

- No round-the-clock support. Self-hosting means the customer controls their own uptime; say this plainly in sales conversations.
- Paid integration help by the day.
- A public status page for known issues and a clear upgrade policy.

## 9. Legal and administrative checklist

- [ ] Read the employment contract's intellectual-property and outside-work clauses. Get written confirmation if anything is unclear.
- [ ] Choose a final name. Search trademarks in the main markets, and check the domain, npm scope, crate names, GitHub organisation and app-store package identifiers.
- [ ] Decide the legal entity and where it is registered.
- [ ] Licence agreement reviewed by a lawyer.
- [ ] Privacy policy and, for the hosted import service, terms and a data processing agreement.
- [ ] Payment and invoicing: consider a merchant of record to handle cross-border sales tax.
- [ ] Read competitors' terms before publishing any benchmark of their demos.
- [ ] Font and icon licences for anything bundled.

## 10. Launch checklists

**Private beta (after M4)**

- [ ] Two reference devices' benchmarks published.
- [ ] Docs: quick start, renderer reference, API reference, deployment guide.
- [ ] Demo checkout with hold, fake payment and booking.
- [ ] Licence agreement and pilot terms ready.
- [ ] A way to deliver builds and licence files.
- [ ] Issue tracker and a feedback log.

**Public v1 (after M6)**

- [ ] At least one reference customer willing to be named.
- [ ] Pricing page with the tested prices.
- [ ] Comparison page and migration guide.
- [ ] Accessibility statement.
- [ ] Mobile wrappers published.
- [ ] Security policy and a disclosure address.
- [ ] Announcement posts and the launch-day support plan.

## 11. Open questions

| Question | How it gets answered |
|---|---|
| Is the renderer's source published under a source-available licence? | Ask prospects whether it affects trust; decide before the beta |
| Is there demand for a renderer-only licence? | M0 interviews |
| Is native mobile a buying reason or a nice-to-have? | M0 interviews; decides the priority of M8 |
| Do prospects want a managed cloud after all? | M0 interviews; revisit `DECISIONS.md` D-01 if most do |
| Which regions respond best? | Outreach tracking |
| What do customers expect to pay for AI import? | Pilot conversations |
| Is 3D ever a blocker? | Track how often it is asked for |

## 12. Metrics

| Stage | Metric |
|---|---|
| Interest | Outreach reply rate; early-access sign-ups; demo visits |
| Evaluation | Trials started; time to first rendered chart in a customer's staging |
| Conversion | Pilots started; pilots converted to paid |
| Revenue | Monthly recurring revenue; average contract value |
| Retention | Renewals; support hours per customer |
| Product | Chart-ready time and frame time reported by customers' `onMetrics`, if they choose to share |
