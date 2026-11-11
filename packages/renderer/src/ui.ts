// The built-in interface (docs/05-web-sdk.md §7): zoom controls, legend,
// tooltip, toasts, and the loading and error states, as plain DOM over the
// canvas in the map's shadow root. Every string from the chart or the host
// goes in as text or an attribute value, never as markup (12 §3 T7). Nothing
// here draws the canvas or runs while idle: the only timers are toasts'.
import type { Translate } from "./messages.ts";
import type { SeatObject } from "./options.ts";
import type { Prices } from "./pricing.ts";
import type { ObjectStatus } from "./seatmap.ts";

/** What the tooltip shows, and what a host's tooltip function is given. */
export type TooltipObject = SeatObject & { status: ObjectStatus };
export type TooltipRenderer = (object: TooltipObject) => HTMLElement | string | null | undefined;

export interface UiOptions {
  zoomControls?: boolean;
  legend?: boolean;
  /** `false`, or a function that renders the content itself. */
  tooltip?: boolean | TooltipRenderer;
  toasts?: boolean;
  /** The loading skeleton and the error state with its retry. */
  loading?: boolean;
}

export interface UiHost {
  t: Translate;
  prices: Prices;
  options: UiOptions;
  /** The map's CSS size. */
  size(): [number, number];
  zoom(how: "in" | "out" | "fit"): void;
  /** An object's details and status, now; null for an unknown label. */
  describe(label: string): TooltipObject | null;
  /** A category's colour in the chart, `#rrggbbaa`. */
  color(key: string): string | undefined;
  chooseTicketType(label: string, key: string): void;
  /** A host tooltip function threw. */
  failed(error: unknown): void;
}

export interface Ui {
  /** Strings or prices changed: everything shown is rendered again. */
  refresh(): void;
  loading(on: boolean): void;
  /** Shows the error state; resolves when the buyer asks to retry. */
  failed(): Promise<void>;
  legend(categories: Array<{ key: string; label: string; color: string; objects: number }>): void;
  camera(zoom: number, range: [number, number]): void;
  /** The hovered or focused object, with its centre in CSS pixels. */
  hover(label: string | null, at: [number, number] | null): void;
  /** The selection changed; `label` was just selected alone (the tooltip may pin to it). */
  selected(label: string | null): void;
  toast(text: string): void;
  destroy(): void;
}

const SVG = "http://www.w3.org/2000/svg";
const ICONS = {
  in: "M12 5v14M5 12h14",
  out: "M5 12h14",
  fit: "M4 9V4h5m6 0h5v5m0 6v5h-5m-6 0H4v-5",
};
const TOAST_MS = 5000;
const TOASTS = 3;
/** The tooltip's distance from the object's centre, and from the map's edges. */
const GAP = 14;
const EDGE = 8;
/** A tap selects within this long of the press. */
const TAP_MS = 1500;

/**
 * Where the tooltip goes: centred above the point, else below it, inside the
 * map with an `EDGE` margin (the left or top edge when it does not fit).
 */
export function place(
  [x, y]: [number, number],
  [w, h]: [number, number],
  [width, height]: [number, number],
): [number, number] {
  const inside = (v: number, room: number): number => Math.max(EDGE, Math.min(v, room - EDGE));
  const above = y - GAP - h;
  return [inside(x - w / 2, width - w), inside(above >= EDGE ? above : y + GAP, height - h)];
}

const R = "var(--cavea-radius,8px)";
const PANEL = "var(--cavea-shadow,0 1px 4px rgb(0 0 0/.25))";
/** The interface's stylesheet; the schemes' defaults come from `schemeCss`. */
export const UI_CSS =
  ".r{position:absolute;inset:0;overflow:hidden;color:var(--_fg);font-family:var(--cavea-font);" +
  "font-size:max(14px,.875rem);line-height:1.4;--_t:none;--_k:none}" +
  "@media (prefers-reduced-motion:no-preference){.r{--_t:i;--_k:k}}" +
  ".r[data-m=reduce]{--_t:none;--_k:none}.r[data-m=full]{--_t:i;--_k:k}" +
  ".r [hidden],.r[data-l] .z,.r[data-l] .l{display:none!important}" +
  ".b{all:unset;box-sizing:border-box;display:inline-flex;align-items:center;justify-content:center;gap:8px;" +
  `min-width:44px;min-height:44px;padding:0 12px;cursor:pointer;color:inherit;border-radius:${R};font:inherit}` +
  ".b:hover{background-image:linear-gradient(color-mix(in srgb,currentColor 12%,#0000) 0 0)}" +
  ".b:focus-visible{outline:2px solid var(--_ring-focus);outline-offset:1px;box-shadow:0 0 0 4px var(--_bg)}" +
  ".b[aria-disabled=true]{opacity:.4;cursor:default;background-image:none}" +
  ".b[aria-pressed=true]{background:var(--_fg);color:var(--_bg)}" +
  // After `.b`, whose `all:unset` would otherwise clear a button's panel.
  `.p{background:var(--_bg);border:1px solid var(--_fg-muted);border-radius:${R};box-shadow:${PANEL};box-sizing:border-box}` +
  ".z{position:absolute;top:12px;inset-inline-end:12px;display:flex;flex-direction:column;gap:8px}" +
  ".z .b{padding:0}" +
  ".l{position:absolute;bottom:12px;inset-inline-start:12px;max-width:calc(100% - 24px);" +
  "max-height:calc(100% - 24px);overflow:auto}" +
  ".l summary{list-style:none;display:flex;justify-content:space-between}.l summary::-webkit-details-marker{display:none}" +
  ".l summary::after{content:'';width:6px;height:6px;border:solid;border-width:0 2px 2px 0;" +
  "transform:translateY(-2px) rotate(45deg)}.l[open] summary::after{transform:translateY(2px) rotate(-135deg)}" +
  ".l ul{list-style:none;margin:0;padding:4px 12px 10px;display:grid;gap:6px}" +
  ".l ul+ul{padding-top:10px;border-top:1px solid var(--_fg-muted)}" +
  ".l li,.c{display:flex;align-items:center;gap:8px}" +
  ".pr{margin-inline-start:auto;padding-inline-start:16px;white-space:nowrap}" +
  ".sw{flex:none;width:14px;height:14px;border-radius:50%;box-sizing:border-box;position:relative;" +
  "box-shadow:inset 0 0 0 1px rgb(0 0 0/.25)}" +
  ".sa{background:var(--_fg-muted)}.ss{background:var(--_seat-selected-fill)}" +
  ".ss::after{content:'';position:absolute;left:5px;top:2px;width:3px;height:6px;" +
  "border:solid var(--_seat-selected-ink);border-width:0 2px 2px 0;transform:rotate(45deg)}" +
  ".su{border:2px solid var(--_seat-unavailable-stroke);background:" +
  "linear-gradient(45deg,#0000 42%,var(--_seat-unavailable-stroke) 42% 58%,#0000 58%)," +
  "linear-gradient(-45deg,#0000 42%,var(--_seat-unavailable-stroke) 42% 58%,#0000 58%),var(--_seat-unavailable-fill)}" +
  "@media (forced-colors:active){.sw{forced-color-adjust:none}}" +
  ".tt{position:absolute;left:0;top:0;z-index:2;max-width:min(300px,calc(100% - 16px));padding:8px 12px;" +
  "pointer-events:none;display:grid;gap:2px}.tt.pin{pointer-events:auto}" +
  ".tt strong{font-weight:600}.f{color:var(--_fg-muted)}" +
  ".ty{display:flex;flex-wrap:wrap;gap:6px;margin-top:6px}.ty .b{border:1px solid var(--_fg-muted)}" +
  ".ts{position:absolute;top:12px;inset-inline:12px;z-index:3;display:flex;flex-direction:column;" +
  "align-items:center;gap:8px;pointer-events:none}.zc .ts{inset-inline-end:68px}" +
  `.to{pointer-events:auto;display:flex;align-items:center;max-width:100%;padding-inline-start:12px;` +
  `background:var(--_fg);color:var(--_bg);border-radius:${R};box-shadow:${PANEL};animation:var(--_t) .2s ease-out}` +
  ".s,.e{position:absolute;inset:0;z-index:1;display:grid;place-content:center;justify-items:center;gap:12px;" +
  "padding:24px;text-align:center;background:var(--_bg)}" +
  `.k{position:absolute;inset:24px;border-radius:${R};background:var(--_seat-unavailable-fill);` +
  "animation:var(--_k) 1.2s ease-in-out infinite alternate}" +
  ".s p,.e p{position:relative;margin:0;color:var(--_fg-muted)}.e p{color:var(--_fg)}.e .b{border:1px solid var(--_fg-muted)}" +
  "@keyframes i{from{opacity:0;transform:translateY(-4px)}}@keyframes k{to{opacity:.55}}";

export function createUi(root: HTMLElement, canvas: HTMLElement, host: UiHost): Ui {
  const doc = root.ownerDocument;
  const { options } = host;
  const t: Translate = (id, values) => host.t(id, values);
  /** After `destroy` nothing is shown again (a late adapter report, say). */
  let dead = false;
  /** Undone by `destroy`: listeners and the toasts' timers. */
  const cleanup: Array<() => void> = [];
  /** Re-renders of what is shown, for `refresh`. */
  const renders: Array<() => void> = [];

  function make<K extends keyof HTMLElementTagNameMap>(
    tag: K,
    className = "",
    part = "",
    parent?: Element,
  ): HTMLElementTagNameMap[K] {
    const node = doc.createElement(tag);
    if (className !== "") {
      node.className = className;
    }
    if (part !== "") {
      node.setAttribute("part", part);
    }
    parent?.append(node);
    return node;
  }
  function on(target: EventTarget, type: string, listener: (event: Event) => void): void {
    target.addEventListener(type, listener);
    cleanup.push(() => target.removeEventListener(type, listener));
  }
  /** A button named by `aria-label`, or by its text; a function name follows the catalogue. */
  function button(
    parent: Element,
    part: string,
    name: string | (() => string),
    icon?: string,
    text = false,
  ): HTMLButtonElement {
    const b = make("button", "b", `button ${part}`, parent);
    b.type = "button";
    if (icon !== undefined) {
      const svg = doc.createElementNS(SVG, "svg");
      const path = doc.createElementNS(SVG, "path");
      for (const [k, v] of [
        ["viewBox", "0 0 24 24"],
        ["width", "20"],
        ["height", "20"],
        ["aria-hidden", "true"],
        ["fill", "none"],
        ["stroke", "currentColor"],
        ["stroke-width", "2"],
        ["stroke-linecap", "round"],
      ]) {
        svg.setAttribute(k as string, v as string);
      }
      path.setAttribute("d", icon);
      svg.append(path);
      b.append(svg);
    }
    const label = (): void => {
      const value = typeof name === "string" ? name : name();
      if (text) {
        b.textContent = value;
      } else {
        b.setAttribute("aria-label", value);
      }
    };
    label();
    if (typeof name !== "string") {
      renders.push(label);
    }
    return b;
  }
  const swatch = (parent: Element, kind: string): HTMLSpanElement =>
    make("span", `sw ${kind}`, "", parent);
  /** A category's swatch: the `--cavea-category-<key>` token, else the chart's colour. */
  function paint(parent: Element, key: string, color: string | undefined): void {
    const sw = swatch(parent, "");
    if (color !== undefined) {
      // A key that is not a plain identifier keeps the chart's colour.
      sw.style.setProperty(
        "background",
        /^[\w-]+$/.test(key) ? `var(--cavea-category-${key},${color})` : color,
      );
    }
  }

  // Zoom controls: disabled at the limits with `aria-disabled`, so focus stays.
  let zoomIn: HTMLButtonElement | undefined;
  let zoomOut: HTMLButtonElement | undefined;
  if (options.zoomControls !== false) {
    root.classList.add("zc");
    const group = make("div", "z", "zoom-controls", root);
    group.setAttribute("role", "group");
    const zoomButton = (how: "in" | "out" | "fit", id: "zoomIn" | "zoomOut" | "zoomFit") => {
      const b = button(group, `zoom-${how}`, () => t(id), ICONS[how]);
      b.classList.add("p");
      on(b, "click", () => {
        if (b.getAttribute("aria-disabled") !== "true") {
          host.zoom(how);
        }
      });
      return b;
    };
    zoomIn = zoomButton("in", "zoomIn");
    zoomOut = zoomButton("out", "zoomOut");
    zoomButton("fit", "zoomFit");
  }

  // The legend: categories with colour, name and price; then the states,
  // with the shapes the map draws (solid, tick, hollow and crossed).
  let legend: Ui["legend"] | undefined;
  if (options.legend !== false) {
    const details = make("details", "l p", "legend", root);
    details.open = host.size()[0] >= 600;
    const summary = make("summary", "b", "button legend-summary", details);
    const categories = make("ul", "", "legend-categories", details);
    const states = make("ul", "", "legend-states", details);
    let shown: Parameters<Ui["legend"]>[0] = [];
    const render = (): void => {
      summary.textContent = t("legend");
      categories.replaceChildren();
      for (const c of shown.filter((category) => category.objects > 0)) {
        const item = make("li", "", "", categories);
        paint(item, c.key, c.color);
        make("span", "", "", item).textContent = c.label;
        const price = host.prices.text(c.key);
        if (price !== undefined) {
          make("span", "pr", "", item).textContent = price;
        }
      }
      categories.hidden = categories.childElementCount === 0;
      states.replaceChildren();
      for (const [kind, id] of [
        ["sa", "available"],
        ["ss", "selected"],
        ["su", "unavailable"],
      ] as const) {
        const item = make("li", "", "", states);
        swatch(item, kind);
        make("span", "", "", item).textContent = t(id);
      }
    };
    legend = (list) => {
      shown = list;
      render();
    };
    render();
    renders.push(render);
  }

  // Loading and error states.
  let status: HTMLElement | undefined;
  let error: HTMLElement | undefined;
  let retry: (() => void) | undefined;
  if (options.loading !== false) {
    // While loading or failed, the controls over the map are hidden.
    root.setAttribute("data-l", "");
    status = make("div", "s", "loading", root);
    status.setAttribute("role", "status");
    make("div", "k", "", status);
    const text = make("p", "", "", status);
    error = make("div", "e", "error", root);
    error.setAttribute("role", "alert");
    error.hidden = true;
    const message = make("p", "", "", error);
    const again = button(error, "retry", () => t("retry"), undefined, true);
    on(again, "click", () => retry?.());
    const render = (): void => {
      text.textContent = t("loading");
      message.textContent = t("loadFailed");
    };
    render();
    renders.push(render);
  }

  // The tooltip, for the hovered or focused object; pinned (interactive)
  // after a tap selects a seat on touch, or one with ticket types to choose.
  const tip = options.tooltip === false ? undefined : make("div", "tt p", "tooltip", root);
  let shownLabel: string | null = null;
  let shownAt: [number, number] = [0, 0];
  let pinned = false;
  /** Dismissed with Escape: not shown again until the hover moves on. */
  let dismissed: string | null = null;
  let press: { at: [number, number]; touch: boolean; time: number } | undefined;
  let onKey: ((event: Event) => void) | undefined;
  if (tip !== undefined) {
    tip.hidden = true;
    // A press on the map unpins; its point anchors a tooltip the tap pins.
    on(canvas, "pointerdown", (e) => {
      const event = e as PointerEvent;
      const rect = canvas.getBoundingClientRect();
      press = {
        at: [event.clientX - rect.left, event.clientY - rect.top],
        touch: event.pointerType !== "mouse",
        time: performance.now(),
      };
      if (pinned) {
        hide();
      }
    });
  }

  function hide(): void {
    if (tip === undefined) {
      return;
    }
    tip.hidden = true;
    tip.classList.remove("pin");
    pinned = false;
    shownLabel = null;
    if (onKey !== undefined) {
      doc.removeEventListener("keydown", onKey);
      onKey = undefined;
    }
  }

  function content(object: TooltipObject): Node[] {
    /** A line of parts, spaced by the stylesheet rather than by punctuation. */
    const line = (className: string, parts: Array<string | undefined>): HTMLElement => {
      const node = make("span", className);
      for (const part of parts) {
        if (part !== undefined) {
          make("span", "", "", node).textContent = part;
        }
      }
      return node;
    };
    const { row, seat } = object;
    const title = line("c h", [
      object.section,
      row === undefined ? undefined : t("row", { row }),
      seat === undefined ? undefined : t("seat", { seat }),
    ]);
    if (title.childElementCount === 0) {
      make("span", "", "", title).textContent = object.label;
    }
    const category = line("c", []);
    paint(category, object.category.key, host.color(object.category.key));
    make("span", "", "", category).textContent = object.category.label;
    const price = host.prices.text(object.category.key, object.ticketType);
    if (price !== undefined) {
      make("span", "pr", "", category).textContent = price;
    }
    const lines: Node[] = [title, category];
    const flags = (["accessible", "companion", "restrictedView", "aisle"] as const).filter(
      (flag) => object.flags[flag],
    );
    if (flags.length > 0) {
      lines.push(
        line(
          "c f",
          flags.map((flag) => t(flag)),
        ),
      );
    }
    lines.push(line("f", [t(object.status)]));
    const types = host.prices.types(object.category.key);
    if (pinned && object.status === "selected" && types.length > 1) {
      const group = make("div", "ty", "ticket-types");
      group.setAttribute("role", "group");
      group.setAttribute("aria-label", t("ticketType"));
      for (const type of types) {
        const b = button(group, "ticket-type", type.label ?? type.key, undefined, true);
        b.setAttribute("aria-pressed", String(type.key === object.ticketType));
        const typePrice = host.prices.text(object.category.key, type.key);
        if (typePrice !== undefined) {
          make("span", "pr", "", b).textContent = typePrice;
        }
        b.addEventListener("click", () => host.chooseTicketType(object.label, type.key));
      }
      lines.push(group);
    }
    return lines;
  }

  /** Renders the tooltip for `label` at `at`; nothing for an unknown object. */
  function show(label: string, at: [number, number], pin: boolean): void {
    const object = tip && host.describe(label);
    if (tip === undefined || object === null || object === undefined) {
      hide();
      return;
    }
    pinned = pin;
    shownLabel = label;
    shownAt = at;
    if (typeof options.tooltip === "function") {
      let custom: ReturnType<TooltipRenderer>;
      try {
        custom = options.tooltip(object);
      } catch (thrown) {
        host.failed(thrown);
        custom = null;
      }
      if (custom === null || custom === undefined || custom === "") {
        hide();
        return;
      }
      tip.replaceChildren(custom);
    } else {
      tip.replaceChildren(...content(object));
    }
    tip.classList.toggle("pin", pin);
    tip.hidden = false;
    const [left, top] = place(at, [tip.offsetWidth, tip.offsetHeight], host.size());
    tip.style.setProperty("left", `${left}px`);
    tip.style.setProperty("top", `${top}px`);
    if (onKey === undefined) {
      onKey = (event) => {
        if ((event as KeyboardEvent).key === "Escape") {
          dismissed = shownLabel;
          hide();
        }
      };
      doc.addEventListener("keydown", onKey);
    }
  }
  renders.push(() => {
    if (shownLabel !== null) {
      show(shownLabel, shownAt, pinned);
    }
  });

  // Toasts: polite, at most `TOASTS`, each for `TOAST_MS` while the region
  // is neither hovered nor focused.
  let region: HTMLElement | undefined;
  /** Shown toasts, oldest first, with their timers (none while held). */
  const toasts = new Map<HTMLElement, ReturnType<typeof setTimeout> | undefined>();
  /** The pointer is over the region; the element in it with focus. */
  let hovering = false;
  let focused: EventTarget | null = null;
  const arm = (toast: HTMLElement): void => {
    clearTimeout(toasts.get(toast));
    const held = hovering || focused !== null;
    toasts.set(toast, held ? undefined : setTimeout(() => drop(toast), TOAST_MS));
  };
  function drop(toast: HTMLElement): void {
    clearTimeout(toasts.get(toast));
    toasts.delete(toast);
    // Removing a focused element need not fire `focusout`.
    const blurred = focused !== null && toast.contains(focused as Node);
    toast.remove();
    if (blurred) {
      focused = null;
      for (const other of toasts.keys()) {
        arm(other);
      }
    }
  }
  if (options.toasts !== false) {
    region = make("div", "ts", "toasts", root);
    region.setAttribute("role", "status");
    region.setAttribute("aria-live", "polite");
    const hold = (event: Event): void => {
      if (event.type.startsWith("pointer")) {
        hovering = event.type === "pointerenter";
      } else {
        focused = event.type === "focusin" ? event.target : null;
      }
      for (const toast of toasts.keys()) {
        arm(toast);
      }
    };
    for (const type of ["pointerenter", "pointerleave", "focusin", "focusout"]) {
      on(region, type, hold);
    }
  }

  return {
    refresh() {
      for (const render of renders) {
        render();
      }
    },
    loading(shown) {
      if (status !== undefined && error !== undefined) {
        status.hidden = !shown;
        error.hidden = true;
        root.toggleAttribute("data-l", shown);
      }
    },
    failed() {
      if (status === undefined || error === undefined) {
        return Promise.reject(new Error("the error state is disabled"));
      }
      status.hidden = true;
      error.hidden = false;
      root.setAttribute("data-l", "");
      return new Promise((resolve) => {
        retry = () => {
          retry = undefined;
          resolve();
        };
      });
    },
    legend: (categories) => legend?.(categories),
    camera(zoom, [min, max]) {
      zoomIn?.setAttribute("aria-disabled", String(zoom >= max * (1 - 1e-4)));
      zoomOut?.setAttribute("aria-disabled", String(zoom <= min * (1 + 1e-4)));
      hide();
    },
    hover(label, at) {
      if (dead) {
        return;
      }
      if (label !== dismissed) {
        dismissed = null;
      }
      if (pinned) {
        return;
      }
      if (label === null || at === null || label === dismissed) {
        hide();
      } else {
        show(label, at, false);
      }
    },
    selected(label) {
      if (dead) {
        return;
      }
      const tap = press;
      const object = label === null ? null : host.describe(label);
      if (tip === undefined || tap === undefined || object === null || label === null) {
        if (shownLabel !== null) {
          show(shownLabel, shownAt, pinned);
        }
        return;
      }
      press = undefined;
      const choose =
        typeof options.tooltip !== "function" && host.prices.types(object.category.key).length > 1;
      if (performance.now() - tap.time < TAP_MS && (tap.touch || choose)) {
        show(label, label === shownLabel ? shownAt : tap.at, true);
      } else if (label === shownLabel) {
        show(label, shownAt, false);
      }
    },
    toast(text) {
      if (region === undefined || dead) {
        return;
      }
      // The same message again replaces the one shown.
      for (const toast of toasts.keys()) {
        if (toast.firstChild?.textContent === text) {
          drop(toast);
        }
      }
      const toast = make("div", "to", "toast", region);
      make("span", "", "", toast).textContent = text;
      const close = button(toast, "toast-dismiss", t("dismiss"), "M6 6l12 12M18 6L6 18");
      close.addEventListener("click", () => drop(toast));
      arm(toast);
      for (const oldest of [...toasts.keys()].slice(0, -TOASTS)) {
        drop(oldest);
      }
    },
    destroy() {
      dead = true;
      hide();
      // Deleting the entry being visited is safe in a `Map`'s iteration.
      for (const toast of toasts.keys()) {
        drop(toast);
      }
      for (const off of cleanup.splice(0)) {
        off();
      }
    },
  };
}
