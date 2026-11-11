// The screen-reader baseline (11 §5): the map's role and description, the
// proxy's name for each state and flag, and one announcement per event.
import { describe, expect, test } from "bun:test";
import type { RendererEvent } from "./events.ts";
import type { FakeNode } from "./fakedom.ts";
import { loaded, seat } from "./fakes.ts";
import type { SeatMapOptions } from "./options.ts";

const focus = (label: string): RendererEvent => ({
  type: "objectFocus",
  object: seat(label),
  at: [400, 300],
});
const selected = (...labels: string[]): RendererEvent => ({
  type: "selectionChanged",
  expiresAt: null,
  selected: labels.map((l) => seat(l)),
});

/**
 * A loaded map whose seats `A-…` are free unless listed, with the flags
 * given, each 3rd of 12 in its row.
 */
async function map(
  options: SeatMapOptions = {},
  unavailable: string[] = [],
  flags: Record<string, number> = {},
) {
  const m = await loaded({
    pricing: [{ category: "vip", price: 9500 }],
    currency: "GBP",
    ...options,
  });
  m.view.answers["object"] = (label: string) =>
    JSON.stringify({
      ...seat(label, flags[label] ?? 0),
      status: unavailable.includes(label) ? 1 : 0,
      place: [3, 12],
    });
  const canvas = m.root.children[0] as FakeNode;
  const live = m.root.children.find((c) => c.className === "vh") as FakeNode;
  /** The name of the proxy `aria-activedescendant` names, or null. */
  const name = (): string | null => {
    const id = canvas.getAttribute("aria-activedescendant");
    return id === null
      ? null
      : (canvas.children.find((c) => (c as unknown as { id: string }).id === id)?.textContent ??
          "missing");
  };
  return { ...m, canvas, live, name };
}

describe("map element", () => {
  test("one tab stop: an application with a role description and instructions", async () => {
    const { canvas } = await map();
    expect(
      ["role", "tabindex", "aria-roledescription", "aria-label", "aria-describedby"].map((a) =>
        canvas.getAttribute(a),
      ),
    ).toEqual(["application", "0", "seat map", "Seat map", "h"]);
    const help = canvas.children.find((c) => (c as unknown as { id: string }).id === "h");
    expect(help?.textContent).toBe(
      "Use the arrow keys to move between seats, and Enter to choose one.",
    );
  });
});

describe("proxy name", () => {
  test("each state and flag in words, from the tooltip's data; unavailable is short", async () => {
    const { drain, name } = await map({}, ["A-1-4"], { "A-1-2": 8, "A-1-3": 15 });
    expect(name()).toBeNull();
    drain(focus("A-1-2"));
    expect(name()).toBe(
      "A, Row 1, Seat 2. VIP, £95.00. Available. Aisle seat. 3 of 12 in the row.",
    );
    drain(focus("A-1-3"));
    expect(name()).toBe(
      "A, Row 1, Seat 3. VIP, £95.00. Available. Wheelchair accessible. Companion seat. " +
        "Restricted view. Aisle seat. 3 of 12 in the row.",
    );
    drain(focus("A-1-4"));
    expect(name()).toBe("A, Row 1, Seat 4. Unavailable.");
    drain(focus("A-1-5"), selected("A-1-5"), focus("A-1-5"));
    expect(name()).toBe("A, Row 1, Seat 5. VIP, £95.00. Selected. 3 of 12 in the row.");
    // Hidden focus: no active descendant.
    drain({ type: "objectFocus", object: null, at: null });
    expect(name()).toBeNull();
  });

  test("localised through messages, and again on setLocale", async () => {
    const {
      drain,
      name,
      map: m,
      canvas,
    } = await map({
      locale: "fr",
      messages: {
        row: "Rang {row}",
        seat: "Siège {seat}",
        available: "Libre",
        place: "{index} sur {count}",
        mapLabel: "Plan de salle",
      },
    });
    drain(focus("A-1-2"));
    // `Intl` spaces with no-break spaces.
    const plain = (): string | undefined => name()?.replace(/\s/g, " ");
    expect(plain()).toBe("A, Rang 1, Siège 2. VIP, 95,00 £GB. Libre. 3 sur 12.");
    expect(canvas.getAttribute("aria-label")).toBe("Plan de salle");
    m.setLocale("en-GB");
    expect(plain()).toBe("A, Rang 1, Siège 2. VIP, £95.00. Libre. 3 sur 12.");
  });

  test("a move switches the active descendant; the same name again changes nothing", async () => {
    const { drain, canvas } = await map();
    drain(focus("A-1-2"));
    const first = canvas.getAttribute("aria-activedescendant");
    drain(focus("A-1-2"));
    expect(canvas.getAttribute("aria-activedescendant")).toBe(first);
    drain(focus("A-1-3"));
    expect(canvas.getAttribute("aria-activedescendant")).not.toBe(first);
  });
});

describe("announcements", () => {
  test("a selection and a deselection, with the count, in the polite region", async () => {
    const { drain, live } = await map();
    expect(live.getAttribute("role")).toBe("status");
    drain(selected("A-1-2"));
    expect(live.textContent).toBe("A, Row 1, Seat 2 selected. 1 seat selected.");
    drain(selected("A-1-2", "A-1-3"));
    expect(live.textContent).toBe("A, Row 1, Seat 3 selected. 2 seats selected.");
    drain(selected("A-1-3"));
    expect(live.textContent).toBe("A, Row 1, Seat 2 deselected. 1 seat selected.");
    // The same words again are varied, so they are heard again.
    for (let i = 0; i < 2; i += 1) {
      drain({ type: "objectUnavailable", label: "A-1-3" }, selected());
      drain(selected("A-1-3"));
    }
    expect(live.textContent).toBe("A, Row 1, Seat 3 selected. 1 seat selected. ");
  });

  test("readiness (min) is no toast: the removals after it are said", async () => {
    const { drain, live } = await map({ selection: { min: 2 } });
    drain(selected("A-1-2", "A-1-3"));
    drain(selected("A-1-2"), { type: "selectionInvalid", rule: "min", objects: ["A-1-2"] });
    drain(selected());
    expect(live.textContent).toBe("A, Row 1, Seat 2 deselected. 0 seats selected.");
  });

  test("a seat taken, lost or refused is said by its toast only", async () => {
    const { drain, live, root } = await map();
    const toasts = (): number => root.parts("toast").length;
    drain(selected("A-1-2", "A-1-3"));
    const said = live.textContent;
    // Taken: the toast, then the selection without it in the same batch.
    drain({ type: "objectUnavailable", label: "A-1-2" }, selected("A-1-3"));
    expect([live.textContent, toasts()]).toEqual([said, 1]);
    // A refused hold: the refusal now, the revert in a later batch.
    drain(selected("A-1-3", "A-1-5"));
    const added = live.textContent;
    drain({ type: "selectionInvalid", rule: "hold_failed", objects: ["A-1-5"] });
    drain(selected("A-1-3"));
    expect([live.textContent, toasts()]).toEqual([added, 2]);
    // Lost: likewise.
    drain({ type: "holdLost", labels: ["A-1-3"] }, selected());
    expect(live.textContent).toBe(added);
    // Selected again later, it is said.
    drain(selected("A-1-5"));
    expect(live.textContent).toBe("A, Row 1, Seat 5 selected. 1 seat selected.");
  });
});
