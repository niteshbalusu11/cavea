// The built-in interface (05 §7), its catalogue (§3.4), prices (§3.1) and
// theme tokens (§8), on the fake view and a fake DOM.
import { afterEach, describe, expect, spyOn, test } from "bun:test";
import { customState, type AdapterContext } from "./adapter.ts";
import { CaveaError, warner } from "./errors.ts";
import type { RendererEvent } from "./events.ts";
import type { FakeNode } from "./fakedom.ts";
import { fakeEnv, flush, loaded, READY, seat } from "./fakes.ts";
import { localeDirection, translator } from "./messages.ts";
import { validateOptions, type SeatMapOptions, type ThemeName } from "./options.ts";
import { checkPricing, prices, type CategoryPricing } from "./pricing.ts";
import { createMap } from "./seatmap.ts";
import { SCHEMES } from "./theme.ts";
import { place } from "./ui.ts";

const env = process.env as Record<string, string | undefined>;
afterEach(() => {
  delete env["NODE_ENV"];
});

const english = translator("en", undefined);
const visible = (n: FakeNode | undefined): boolean => n !== undefined && !n.hidden;
const one = (root: FakeNode, part: string): FakeNode | undefined => root.parts(part)[0];
const hover = (label: string, at: [number, number] = [400, 300], flags = 0): RendererEvent => ({
  type: "objectHover",
  object: { ...seat(label), flags },
  at,
});
const event = (type: string, props: Record<string, unknown> = {}): Event =>
  Object.assign(new Event(type), props);

describe("catalogue", () => {
  test("overrides any subset over English; plurals by Intl.PluralRules; numbers by locale", () => {
    const t = translator("en", {
      zoomIn: "Agrandir",
      max: { one: "Un seul siège.", other: "{count} sièges au plus." },
    });
    expect([t("zoomIn"), t("zoomOut")]).toEqual(["Agrandir", "Zoom out"]);
    expect([t("max", { count: 1 }), t("max", { count: 8 })]).toEqual([
      "Un seul siège.",
      "8 sièges au plus.",
    ]);
    expect(english("max", { count: 1000 })).toBe("You can choose up to 1,000 seats.");
    expect(english("max", { count: 1 })).toBe("You can choose only one seat.");
    // Polish has `few` and `many`; a form a message lacks falls back to `other`.
    const pl = translator("pl", {
      holdLost: { one: "jedno", few: "{count} miejsca", many: "{count} miejsc", other: "inne" },
      max: { other: "do {count}" },
    });
    expect([2, 5, 22, 1].map((count) => pl("holdLost", { count }))).toEqual([
      "2 miejsca",
      "5 miejsc",
      "22 miejsca",
      "jedno",
    ]);
    expect(pl("max", { count: 3 })).toBe("do 3");
    expect(translator("de", undefined)("max", { count: 1000 })).toBe(
      "You can choose up to 1.000 seats.",
    );
    // A placeholder without a value is left as it is.
    expect(english("row")).toBe("Row {row}");
  });

  test("unknown ids warn; a malformed message is an invalid option", () => {
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    validateOptions({ messages: { zoomIn: "In", zoomInn: "?" } }, warner(true));
    expect(warn.mock.calls.map((c) => c[0])).toEqual([
      '[cavea] unknown message "zoomInn" is ignored',
    ]);
    warn.mockRestore();
    for (const messages of [{ zoomIn: 5 }, { max: { one: "x" } }, "nope"]) {
      expect(() => validateOptions({ messages }, () => {})).toThrow(CaveaError);
    }
  });

  test("the locale's script gives the direction", () => {
    expect(
      ["ar", "he-IL", "fa", "ur-PK", "en", "fr-CA", "ku-Arab", "uz-Arab-AF", "az-Latn"].map(
        localeDirection,
      ),
    ).toEqual(["rtl", "rtl", "rtl", "rtl", "ltr", "ltr", "rtl", "rtl", "ltr"]);
  });
});

describe("prices", () => {
  const pricing: CategoryPricing[] = [
    {
      category: "vip",
      price: 9500,
      ticketTypes: [
        { key: "adult", price: 9500 },
        { key: "child", label: "Child", price: 6000 },
      ],
    },
    { category: "std", price: 4500 },
  ];
  const book = (
    locale: string,
    currency?: string,
    format?: (m: number) => string,
    fail = () => {},
  ) => prices(pricing, format, currency, locale, english, fail);

  test("Intl in the currency's own minor unit, or two decimals without a currency", () => {
    expect(book("en-GB", "GBP").text("std")).toBe("£45.00");
    expect(book("de-DE", "EUR").text("std")?.replace(/\s/g, " ")).toBe("45,00 €");
    // Zero and three decimal currencies: minor units are yen and fils.
    expect(book("ja-JP", "JPY").text("std")).toMatch(/^[￥¥]4,500$/);
    expect(book("en", "KWD").text("std")?.replace(/\s/g, " ")).toBe("KWD 4.500");
    expect(book("en").text("std")).toBe("45.00");
  });

  test("ranges over ticket types, one type's price, nothing without prices", () => {
    const b = book("en-GB", "GBP");
    expect(b.text("vip")).toBe("£60.00 – £95.00");
    expect(b.text("vip", "child")).toBe("£60.00");
    expect([b.price("vip"), b.price("vip", "child"), b.price("std", "child")]).toEqual([
      9500, 6000, 4500,
    ]);
    expect([b.text("none"), b.price("none")]).toEqual([undefined, undefined]);
    expect(b.types("vip").map((t) => t.key)).toEqual(["adult", "child"]);
  });

  test("formatPrice is the host's; one that throws is reported and Intl is used", () => {
    const calls: unknown[] = [];
    const failures: unknown[] = [];
    const custom = prices(
      pricing,
      (minor, ctx) => (calls.push(ctx), `${minor}p`),
      "GBP",
      "en",
      english,
      () => {},
    );
    expect(custom.text("vip", "child")).toBe("6000p");
    expect(calls).toEqual([
      { category: "vip", ticketType: "child", currency: "GBP", locale: "en" },
    ]);
    const broken = prices(
      pricing,
      () => {
        throw new Error("no");
      },
      "GBP",
      "en-GB",
      english,
      (e) => failures.push(e),
    );
    expect(broken.text("std")).toBe("£45.00");
    expect(failures).toHaveLength(1);
  });

  test("malformed entries are left out with a warning; the rest are kept", () => {
    const warnings: string[] = [];
    const kept = checkPricing(
      [
        { category: "a", price: 100 },
        { category: "b" },
        { category: "c", price: 1.5 },
        { category: "d", price: -1 },
        { category: "e", ticketTypes: [{ key: "x", price: "1" }] },
        { category: "f", ticketTypes: [{ key: "x", price: 1 }] },
        null,
      ],
      (m) => warnings.push(m),
    );
    expect(kept.map((p) => p.category)).toEqual(["a", "f"]);
    expect(warnings).toHaveLength(5);
  });
});

/** WCAG 2 relative luminance of `#rrggbb`. */
function luminance(hex: string): number {
  const [r = 0, g = 0, b = 0] = [1, 3, 5].map((i) => {
    const c = Number.parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}
/** WCAG 2 contrast ratio of two `#rrggbb` colours. */
function contrast(a: string, b: string): number {
  const [x, y] = [luminance(a), luminance(b)];
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}

test("the built-in schemes meet WCAG 2.2 AA for the interface's pairs", () => {
  for (const name of Object.keys(SCHEMES) as ThemeName[]) {
    const c = SCHEMES[name];
    const pair = (fg: string, bg: string): number => contrast(c[fg] ?? "", c[bg] ?? "");
    // Text: on panels, toasts (inverted), the skeleton and pressed buttons: 4.5:1.
    for (const [fg, bg] of [
      ["fg", "bg"],
      ["fg-muted", "bg"],
      ["fg-muted", "seat-unavailable-fill"],
    ]) {
      expect([name, fg, bg, pair(fg as string, bg as string) >= 4.5]).toEqual([name, fg, bg, true]);
    }
    // Boundaries, the focus ring (on its background halo) and the legend's state shapes: 3:1.
    for (const [fg, bg] of [
      ["fg-muted", "bg"],
      ["ring-focus", "bg"],
      ["seat-selected-fill", "bg"],
      ["seat-unavailable-stroke", "bg"],
      ["seat-selected-ink", "seat-selected-fill"],
    ]) {
      expect([name, fg, bg, pair(fg as string, bg as string) >= 3]).toEqual([name, fg, bg, true]);
    }
  }
});

test("the tooltip goes above the point, below it at the top edge, and stays inside", () => {
  const box: [number, number] = [800, 600];
  const size: [number, number] = [200, 80];
  // Centred above, 14 px clear of the point.
  expect(place([400, 300], size, box)).toEqual([300, 206]);
  // At the top: below the point.
  expect(place([400, 40], size, box)).toEqual([300, 54]);
  // At the left and right edges: 8 px in.
  expect(place([10, 300], size, box)).toEqual([8, 206]);
  expect(place([795, 300], size, box)).toEqual([592, 206]);
  // Bottom-right corner of a box too small for either side: inside, from the top.
  expect(place([799, 599], size, [210, 90])).toEqual([8, 8]);
  // A 320 px map with a point near the top: below, inside on the right.
  expect(place([310, 20], size, [320, 400])).toEqual([112, 34]);
});

describe("elements", () => {
  test("each is on by default with its part names, and off with false", async () => {
    const on = await loaded();
    for (const part of [
      "zoom-controls",
      "zoom-in",
      "zoom-out",
      "zoom-fit",
      "legend",
      "legend-summary",
      "tooltip",
      "toasts",
      "loading",
      "error",
      "retry",
    ]) {
      expect([part, on.root.parts(part).length]).toEqual([part, 1]);
    }
    expect(on.root.parts("button").length).toBe(5);
    const off = await loaded({
      ui: { zoomControls: false, legend: false, tooltip: false, toasts: false, loading: false },
    });
    // The canvas and the selections' live region (11 §5), which is not an option.
    expect(off.root.children.map((c) => c.tagName)).toEqual(["CANVAS", "DIV"]);
    off.drain(hover("A-1-1"), { type: "selectionInvalid", rule: "max", objects: ["A-1-2"] });
    expect(off.root.children).toHaveLength(2);
  });

  test("options of the interface are checked; later ones warn", () => {
    for (const [option, options] of [
      ["ui.legend", { ui: { legend: "yes" } }],
      ["ui.tooltip", { ui: { tooltip: 1 } }],
      ["ui", { ui: true }],
      ["locale", { locale: "not a locale!" }],
      ["currency", { currency: "EURO" }],
      ["dir", { dir: "up" }],
      ["pricing", { pricing: {} }],
      ["formatPrice", { formatPrice: "£" }],
    ] as const) {
      expect(() => validateOptions(options, () => {})).toThrow(
        expect.objectContaining({ code: "invalid_option", option }) as unknown as Error,
      );
    }
    const warnings: string[] = [];
    validateOptions({ ui: { holdTimer: true, tooltipp: true } }, (m) => warnings.push(m));
    expect(warnings).toEqual([
      'option "ui.holdTimer" is not supported yet and is ignored',
      'unknown option "ui.tooltipp" is ignored',
    ]);
  });

  test("zoom controls fly the camera and are disabled at the limits, keeping focus", async () => {
    const { root, view, drain } = await loaded();
    drain({ type: "cameraChanged", centre: [100, 50], zoom: 2, range: [0.5, 2], lod: "seats" });
    const [zoomIn, zoomOut, fit] = ["zoom-in", "zoom-out", "zoom-fit"].map((p) => one(root, p));
    expect([
      zoomIn?.getAttribute("aria-label"),
      zoomIn?.getAttribute("aria-disabled"),
      zoomOut?.getAttribute("aria-disabled"),
    ]).toEqual(["Zoom in", "true", "false"]);
    view.calls.length = 0;
    // Zoom in is at its limit: the click does nothing.
    zoomIn?.click();
    zoomOut?.click();
    fit?.click();
    expect(view.calls.filter((c) => c[0] === "flyTo").map((c) => c.slice(2))).toEqual([
      [undefined, false, false],
      [undefined, false, false],
    ]);
    drain({ type: "cameraChanged", centre: [100, 50], zoom: 0.5, range: [0.5, 2], lod: "seats" });
    expect([zoomIn?.getAttribute("aria-disabled"), zoomOut?.getAttribute("aria-disabled")]).toEqual(
      ["false", "true"],
    );
  });

  test("the legend lists categories with colour token, name and price, then the states", async () => {
    const { root } = await loaded({
      locale: "en-GB",
      currency: "GBP",
      pricing: [
        {
          category: "vip",
          ticketTypes: [
            { key: "a", price: 9500 },
            { key: "c", price: 6000 },
          ],
        },
      ],
    });
    const [categories, states] = [one(root, "legend-categories"), one(root, "legend-states")];
    expect(categories?.children.map((li) => li.textContent)).toEqual(["VIP£60.00 – £95.00"]);
    expect(categories?.children[0]?.children[0]?.style.getPropertyValue("background")).toBe(
      "var(--cavea-category-vip,#010203ff)",
    );
    expect(states?.children.map((li) => li.textContent)).toEqual([
      "Available",
      "Selected",
      "Unavailable",
    ]);
    expect(one(root, "legend-summary")?.textContent).toBe("Key");
    // 800 px wide: open; the buyer's choice is kept after that.
    expect(one(root, "legend")?.open).toBe(true);
  });

  test("the tooltip follows hover; Escape, a pan and leaving dismiss it", async () => {
    const { root, doc, drain, view } = await loaded({
      locale: "en-GB",
      currency: "GBP",
      pricing: [{ category: "vip", price: 9500 }],
    });
    const tip = one(root, "tooltip");
    // The tooltip shows the view's details of the object, as `findObject` does.
    view.answers["object"] = (label: string) =>
      JSON.stringify({ ...seat(label, 1 | 4), status: 1 });
    drain(hover("A-1-1", [400, 300], 1 | 4));
    expect(visible(tip)).toBe(true);
    expect(tip?.children.map((line) => line.children.map((c) => c.textContent))).toEqual([
      ["A", "Row 1", "Seat 1"],
      ["", "VIP", "£95.00"],
      ["Wheelchair accessible", "Restricted view"],
      ["Unavailable"],
    ]);
    expect([tip?.style.getPropertyValue("left"), tip?.style.getPropertyValue("top")]).toEqual([
      "300px",
      "206px",
    ]);
    expect(doc.listeners.size).toBe(1);
    doc.dispatchEvent(event("keydown", { key: "Escape" }));
    expect([visible(tip), doc.listeners.size]).toEqual([false, 0]);
    // Dismissed stays dismissed until the hover moves on.
    drain(hover("A-1-1"));
    expect(visible(tip)).toBe(false);
    drain(hover("A-1-2"));
    expect(visible(tip)).toBe(true);
    drain({ type: "cameraChanged", centre: [0, 0], zoom: 1, range: [0.5, 2], lod: "seats" });
    expect(visible(tip)).toBe(false);
    drain(hover("A-1-2"), { type: "objectHover", object: null, at: null });
    expect(visible(tip)).toBe(false);
  });

  test("a host's tooltip replaces the content; null shows none; a throw is reported", async () => {
    const errors: string[] = [];
    let mode = "text";
    const { root, drain } = await loaded({
      onError: (e) => errors.push(e.code),
      ui: {
        tooltip: (o) => {
          if (mode === "throw") {
            throw new Error("host");
          }
          return mode === "null" ? null : `${o.label} ${o.status} ${o.category.label}`;
        },
      },
    });
    const tip = one(root, "tooltip");
    drain(hover("A-1-1"));
    expect([visible(tip), tip?.textContent, tip?.childElementCount]).toEqual([
      true,
      "A-1-1 unavailable VIP",
      0,
    ]);
    mode = "null";
    drain(hover("A-1-2"));
    expect(visible(tip)).toBe(false);
    mode = "throw";
    drain(hover("A-1-1"));
    expect([visible(tip), errors]).toEqual([false, ["callback_failed"]]);
  });

  test("a tap that selects a seat with ticket types pins the choice; choosing reports it", async () => {
    const selections: Array<Array<[string, string | undefined, number | undefined]>> = [];
    const { root, canvas, doc, drain, view, map } = await loaded({
      locale: "en-US",
      currency: "USD",
      pricing: [
        {
          category: "vip",
          ticketTypes: [
            { key: "adult", price: 9500 },
            { key: "child", label: "Child", price: 6000 },
          ],
        },
      ],
      onSelectionChange: (s) =>
        selections.push(s.objects.map((o) => [o.label, o.ticketType, o.price])),
    });
    view.answers["object"] = (label: string) => JSON.stringify({ ...seat(label), status: 0 });
    canvas.dispatchEvent(
      event("pointerdown", { clientX: 120, clientY: 200, pointerType: "touch" }),
    );
    drain({ type: "selectionChanged", expiresAt: null, selected: [seat("A-1-1")] });
    const tip = one(root, "tooltip");
    expect([visible(tip), tip?.classList.contains("pin")]).toEqual([true, true]);
    const types = root.parts("ticket-type");
    expect(types.map((b) => [b.textContent, b.getAttribute("aria-pressed")])).toEqual([
      ["adult$95.00", "true"],
      ["Child$60.00", "false"],
    ]);
    // A pinned tooltip ignores hover elsewhere.
    drain(hover("A-1-2"));
    // A refresh while a type button has focus (a hold confirmed) keeps it on its successor.
    root.parts("ticket-type")[1]?.focus();
    drain({ type: "selectionChanged", expiresAt: 1, selected: [seat("A-1-1")] });
    expect(doc.activeElement === types[1]).toBe(false);
    expect(doc.activeElement).toBe(root.parts("ticket-type")[1] ?? null);
    root.parts("ticket-type")[1]?.click();
    // The buttons are rendered again; focus moves to the new "Child".
    expect(doc.activeElement).toBe(root.parts("ticket-type")[1] ?? null);
    expect(selections).toEqual([
      [["A-1-1", "adult", 9500]],
      [["A-1-1", "adult", 9500]],
      [["A-1-1", "child", 6000]],
    ]);
    expect(map.getSelection().objects[0]).toMatchObject({ ticketType: "child", price: 6000 });
    expect(root.parts("ticket-type").map((b) => b.getAttribute("aria-pressed"))).toEqual([
      "false",
      "true",
    ]);
    // The next press on the map unpins it.
    canvas.dispatchEvent(event("pointerdown", { clientX: 0, clientY: 0, pointerType: "mouse" }));
    expect(visible(tip)).toBe(false);
  });

  test("toasts: refusals and losses in words, the same one once, at most three; off, callbacks still fire", async () => {
    const { root, drain } = await loaded({ selection: { max: 4 } });
    const texts = (): string[] => root.parts("toast").map((t) => t.firstChild?.textContent ?? "");
    drain({ type: "selectionInvalid", rule: "max", objects: ["A-1-2"] });
    drain({ type: "selectionInvalid", rule: "max", objects: ["A-1-3"] });
    expect(texts()).toEqual(["You can choose up to 4 seats."]);
    drain(
      { type: "selectionInvalid", rule: "min", objects: [] },
      { type: "objectUnavailable", label: "A-1-2" },
      { type: "selectionInvalid", rule: "not_free", objects: ["A-1-3"] },
      { type: "selectionInvalid", rule: "rate_limited", objects: ["A-1-3"] },
    );
    expect(texts()).toEqual([
      "A-1-2 was just taken by someone else.",
      "That seat is not available.",
      "That seat could not be held. Please try again.",
    ]);
    one(root, "toast-dismiss")?.click();
    expect(texts()).toHaveLength(2);
    expect(one(root, "toasts")?.getAttribute("aria-live")).toBe("polite");

    const refused: string[] = [];
    const off = await loaded({
      ui: { toasts: false },
      onSelectionInvalid: (rule) => refused.push(rule),
    });
    off.drain({ type: "selectionInvalid", rule: "max", objects: ["A-1-2"] });
    expect([refused, off.root.parts("toast")]).toEqual([["max"], []]);
  });

  test("strings from the chart and the host are text, never markup (12 §3 T7)", async () => {
    const mark = '<img src=x onerror="alert(1)">';
    const { root, view, drain } = await loaded({
      selection: { max: 1 },
      messages: { zoomIn: mark, legend: mark, max: { other: mark }, mapHelp: mark, mapLabel: mark },
      pricing: [
        {
          category: mark,
          price: 1,
          ticketTypes: [
            { key: "a", label: mark, price: 1 },
            { key: "b", label: mark, price: 2 },
          ],
        },
      ],
      formatPrice: () => mark,
    });
    view.answers["object"] = (label: string) =>
      JSON.stringify({
        label,
        category: mark,
        section: mark,
        row: mark,
        seat: mark,
        flags: 1,
        status: 0,
      });
    drain({
      ...READY,
      categories: [{ key: mark, label: mark, color: [1, 2, 3, 255], objects: 1 }],
    } as RendererEvent);
    drain(
      {
        type: "objectHover",
        object: { label: mark, category: mark, section: mark, row: mark, seat: mark, flags: 1 },
        at: [1, 1],
      },
      { type: "objectUnavailable", label: mark },
      { type: "selectionInvalid", rule: "max", objects: [mark] },
      // The proxy's name and the live region (11 §5).
      {
        type: "objectFocus",
        object: { label: mark, category: mark, section: mark, row: mark, seat: mark, flags: 1 },
        at: [1, 1],
      },
      {
        type: "selectionChanged",
        expiresAt: null,
        selected: [{ label: mark, category: mark, section: mark, row: mark, seat: mark, flags: 1 }],
      },
    );
    const tags = new Set([...root.walk()].map((n) => n.tagName));
    expect(
      [...tags].filter(
        (t) =>
          ![
            "CANVAS",
            "DIV",
            "SPAN",
            "STRONG",
            "BUTTON",
            "SVG",
            "PATH",
            "DETAILS",
            "SUMMARY",
            "UL",
            "LI",
            "P",
            "#TEXT",
          ].includes(t),
      ),
    ).toEqual([]);
    // The markup arrived, as text.
    expect(
      [...root.walk()].filter((n) => n.tagName === "#TEXT" && n.data.includes(mark)).length,
    ).toBeGreaterThan(10);
    const canvas = root.children[0];
    expect([canvas?.getAttribute("aria-label"), canvas?.children[2]?.textContent]).toEqual([
      mark,
      mark,
    ]);
    expect(one(root, "zoom-in")?.getAttribute("aria-label")).toBe(mark);
    // A category key that is not an identifier is not spliced into CSS.
    expect(
      one(root, "legend-categories")?.children[0]?.children[0]?.style.getPropertyValue(
        "background",
      ),
    ).toBe("#010203ff");
    // A host's string tooltip is text too.
    const host = await loaded({ ui: { tooltip: () => mark } });
    host.drain(hover("A-1-1"));
    expect([
      one(host.root, "tooltip")?.textContent,
      one(host.root, "tooltip")?.childElementCount,
    ]).toEqual([mark, 0]);
  });

  test("destroy leaves no node, listener, observer or timer; a hundred maps leak nothing", async () => {
    const live = new Set<unknown>();
    // The late report is warned about, as for a map without `onError`.
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    const realSet = globalThis.setTimeout;
    const realClear = globalThis.clearTimeout;
    globalThis.setTimeout = ((fn: () => void, ms?: number) => {
      const id = realSet(() => {
        live.delete(id);
        fn();
      }, ms);
      live.add(id);
      return id;
    }) as typeof setTimeout;
    globalThis.clearTimeout = ((id?: ReturnType<typeof setTimeout>) => {
      live.delete(id);
      realClear(id);
    }) as typeof clearTimeout;
    try {
      const left = { listeners: 0, observers: 0, schemes: 0, attached: 0 };
      for (let i = 0; i < 100; i++) {
        let context: AdapterContext | undefined;
        const f = await loaded({
          pricing: [{ category: "vip", price: 1 }],
          state: customState({
            connect(ctx) {
              context = ctx;
              return Promise.resolve({ kind: "map", unavailable: [] });
            },
            disconnect() {},
          }),
        });
        live.clear();
        f.drain(hover("A-1-1"), { type: "selectionInvalid", rule: "max", objects: ["A-1-2"] });
        expect(live.size).toBe(1);
        f.map.destroy();
        // A late report from the adapter shows nothing and starts no timer.
        context?.reportError(new CaveaError("state_disconnected", "late"));
        left.listeners += f.doc.listeners.size + f.canvas.listeners.size;
        left.observers += f.state.watchers.size;
        left.schemes += f.state.schemeListeners;
        left.attached += f.root.parentNode === null ? 0 : 1;
      }
      expect([left, live.size]).toEqual([
        { listeners: 0, observers: 0, schemes: 0, attached: 0 },
        0,
      ]);
    } finally {
      globalThis.setTimeout = realSet;
      globalThis.clearTimeout = realClear;
      warn.mockRestore();
    }
  });
});

describe("theme tokens", () => {
  test("the page's custom properties reach the canvas when they change, and only then", async () => {
    const { map, view, root, restyle, setDark } = await loaded({
      theme: { base: "light", tokens: { "--cavea-ring-focus": "#123456" } },
    });
    const themes = (): unknown[] =>
      view.calls.filter((c) => c[0] === "setTheme").map((c) => c.slice(1));
    // `theme.tokens` are set on the root, so the interface's CSS sees them too.
    expect([
      root.getAttribute("data-s"),
      root.style.getPropertyValue("--cavea-ring-focus"),
    ]).toEqual(["light", "#123456"]);
    expect(themes().at(-1)).toEqual(["light", ["ring-focus", "#123456"]]);
    const before = themes().length;
    restyle({ bg: "#101010", "category-vip": "#ff0000" });
    expect(themes().at(-1)).toEqual([
      "light",
      ["bg", "#101010", "ring-focus", "#123456", "category-vip", "#ff0000"],
    ]);
    restyle({ bg: "#101010", "category-vip": "#ff0000" });
    setDark(true);
    expect(themes().length).toBe(before + 1);
    // `theme.tokens` win over the page's.
    restyle({ "ring-focus": "#000000" });
    expect(themes().at(-1)).toEqual(["light", ["ring-focus", "#123456"]]);
    // `auto` follows the system: the root's scheme and the canvas together.
    map.setTheme("auto");
    expect([
      root.getAttribute("data-s"),
      root.style.getPropertyValue("--cavea-ring-focus"),
    ]).toEqual(["dark", ""]);
    expect(themes().at(-1)).toEqual(["dark", ["ring-focus", "#000000"]]);
    // A change the map cannot see is read with refreshTheme.
    const n = themes().length;
    map.refreshTheme();
    expect(themes().length).toBe(n);
  });

  test("high contrast is a scheme for the interface too", async () => {
    const { root } = await loaded({ theme: "high-contrast" });
    expect(root.getAttribute("data-s")).toBe("high-contrast");
  });
});

describe("loading and error states", () => {
  test("a chart that cannot be fetched shows the error; retry fetches it again", async () => {
    const f = fakeEnv();
    const errors: CaveaError[] = [];
    let attempts = 0;
    f.env.fetchBytes = () =>
      attempts++ === 0
        ? Promise.reject(new Error("HTTP 503"))
        : Promise.resolve(new Uint8Array([1]));
    let done = false;
    const promise = createMap(f.env, {} as HTMLElement, {
      chart: { url: "c.ccf" },
      onError: (e) => errors.push(e),
    });
    void promise.then(() => (done = true));
    expect(visible(one(f.root, "loading"))).toBe(true);
    f.wasm.resolve(() => f.view.engine);
    await flush();
    expect([visible(one(f.root, "loading")), visible(one(f.root, "error")), done]).toEqual([
      false,
      true,
      false,
    ]);
    expect(errors.map((e) => [e.code, e.fatal])).toEqual([["chart_fetch_failed", false]]);
    expect(one(f.root, "error")?.textContent).toBe("The seat map could not be loaded.Try again");
    one(f.root, "retry")?.click();
    const map = await promise;
    expect([attempts, visible(one(f.root, "error")), visible(one(f.root, "loading"))]).toEqual([
      2,
      false,
      true,
    ]);
    // The first frame drawn with the chart ends the skeleton.
    f.attached()?.onFrame?.(0, 0);
    expect(visible(one(f.root, "loading"))).toBe(false);
    map.destroy();
  });
});

describe("locale and pricing at run time", () => {
  test("setLocale reformats and redirects; setPricing replaces prices; soldOut reaches the view", async () => {
    const { map, root, view } = await loaded({
      locale: "en-GB",
      currency: "GBP",
      messages: { soldOut: "Complet" },
      pricing: [{ category: "vip", price: 9500 }],
    } as SeatMapOptions);
    const price = (): string | undefined =>
      one(root, "legend-categories")?.children[0]?.children[2]?.textContent;
    expect([price(), root.getAttribute("dir"), root.getAttribute("lang")]).toEqual([
      "£95.00",
      "ltr",
      "en-GB",
    ]);
    expect(view.calls).toContainEqual(["setMessage", "soldOut", "Complet"]);
    map.setLocale("ar-EG");
    expect(root.getAttribute("dir")).toBe("rtl");
    expect(price()).not.toBe("£95.00");
    map.setPricing([{ category: "vip", price: 100 }]);
    map.setLocale("en-GB");
    expect(price()).toBe("£1.00");
    map.setPricing([]);
    expect(price()).toBeUndefined();
    const errors: string[] = [];
    const other = await loaded({ onError: (e) => errors.push(e.code) });
    other.map.setLocale("not a locale!");
    other.map.setPricing({} as CategoryPricing[]);
    expect(errors).toEqual(["invalid_input", "invalid_input"]);
    // Without a locale of the host's, the page's direction is inherited.
    expect(other.root.getAttribute("dir")).toBeNull();
  });
});
