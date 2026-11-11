// The screen-reader baseline (docs/11-accessibility.md §5, A11Y-002): the
// canvas is the map's one tab stop, `role="application"`; the focused seat is
// a proxy element in the canvas's fallback content, named in words and made
// the canvas's `aria-activedescendant`; selections are said in a polite live
// region. Every string goes in as text (12 §3 T7). Nothing runs per frame:
// the name is rebuilt when the engine reports the focus, which it does only
// when the focus or its seat's state changes.
import type { Translate } from "./messages.ts";
import type { Prices } from "./pricing.ts";
import type { TooltipObject } from "./ui.ts";

export interface A11yHost {
  t: Translate;
  prices: Prices;
  /** What the tooltip shows for `label`, and the seat's place in its row. */
  describe(label: string): (TooltipObject & { place?: [number, number] }) | null;
}

export interface A11y {
  /** The keyboard focus shown, or none. */
  focus(label: string | null): void;
  /** The selection changed: one seat added or removed by the buyer, and the count. */
  selection(added: string[], removed: string[], count: number): void;
  /** Strings or prices changed. */
  refresh(): void;
}

const FLAGS = ["accessible", "companion", "restrictedView", "aisle"] as const;

/** The seat's section, row and seat in words, else its label. */
export function title(o: TooltipObject, t: Translate): string {
  const parts = [
    o.section,
    o.row === undefined ? undefined : t("row", { row: o.row }),
    o.seat === undefined ? undefined : t("seat", { seat: o.seat }),
  ].filter((p) => p !== undefined && p !== "");
  return parts.length > 0 ? parts.join(", ") : o.label;
}

/**
 * The proxy's name (11 §5): "Orchestra, Row C, Seat 12. VIP, £95.00.
 * Available. Aisle seat. 12 of 30 in the row." An unavailable seat is its
 * title and status only. The same data as the tooltip.
 */
export function proxyName(
  o: TooltipObject & { place?: [number, number] },
  t: Translate,
  prices: Prices,
): string {
  const status = t(o.status);
  if (o.status === "unavailable") {
    return `${title(o, t)}. ${status}.`;
  }
  const price = prices.text(o.category.key, o.ticketType);
  const parts = [
    title(o, t),
    price === undefined ? o.category.label : `${o.category.label}, ${price}`,
    status,
    ...FLAGS.filter((flag) => o.flags[flag]).map((flag) => t(flag)),
  ];
  if (o.place !== undefined) {
    parts.push(t("place", { index: o.place[0], count: o.place[1] }));
  }
  return `${parts.join(". ")}.`;
}

export function createA11y(canvas: HTMLElement, root: HTMLElement, host: A11yHost): A11y {
  const doc = canvas.ownerDocument;
  const make = (parent: Element, id: string): HTMLElement => {
    const node = doc.createElement("div");
    node.id = id;
    parent.append(node);
    return node;
  };
  // Two proxies, written alternately, so every change of name also changes
  // `aria-activedescendant`, which is what screen readers announce.
  const proxies = [make(canvas, "p0"), make(canvas, "p1")];
  for (const proxy of proxies) {
    proxy.setAttribute("role", "button");
    proxy.hidden = true;
  }
  // Read through `aria-describedby`, which reaches hidden elements.
  const help = make(canvas, "h");
  help.hidden = true;
  const live = make(root, "");
  live.className = "vh";
  live.setAttribute("role", "status");
  canvas.setAttribute("role", "application");
  canvas.setAttribute("tabindex", "0");
  canvas.setAttribute("aria-describedby", "h");
  let next = 0;
  let focused: string | null = null;
  let said = "";

  function name(): void {
    const t = host.t;
    const object = focused === null ? null : host.describe(focused);
    if (object === null) {
      canvas.removeAttribute("aria-activedescendant");
      for (const proxy of proxies) {
        proxy.hidden = true;
      }
      return;
    }
    const text = proxyName(object, t, host.prices);
    // Reported again unchanged (the camera settled): nothing to say.
    if (
      canvas.getAttribute("aria-activedescendant") !== null &&
      proxies[1 - next]?.textContent === text
    ) {
      return;
    }
    // Only the active proxy is in the tree: a stale name is never found.
    const [proxy, other] = [proxies[next], proxies[1 - next]] as [HTMLElement, HTMLElement];
    proxy.textContent = text;
    proxy.hidden = false;
    canvas.setAttribute("aria-activedescendant", proxy.id);
    other.hidden = true;
    next = 1 - next;
  }

  function strings(): void {
    const t = host.t;
    canvas.setAttribute("aria-label", t("mapLabel"));
    canvas.setAttribute("aria-roledescription", t("mapRole"));
    help.textContent = t("mapHelp");
  }
  strings();

  return {
    focus(label) {
      focused = label;
      name();
    },
    selection(added, removed, count) {
      const t = host.t;
      const one = added.length + removed.length === 1 ? (added[0] ?? removed[0]) : undefined;
      const object = one === undefined ? null : host.describe(one);
      const what =
        object === null
          ? ""
          : `${t(added.length === 1 ? "seatSelected" : "seatDeselected", { seat: title(object, t) })} `;
      let text = what + t("selectedCount", { count });
      // The same words again would not be heard: vary them invisibly.
      if (text === said) {
        text += " ";
      }
      said = text;
      live.textContent = text;
    },
    refresh() {
      strings();
      name();
    },
  };
}
