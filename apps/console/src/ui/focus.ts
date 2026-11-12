// Where keyboard focus goes when the element that had it goes away (09 §8,
// 11 §8; WCAG 2.4.3): a dialog closing after its opener's row was replaced
// or deleted, or a route change that removed the link followed. Focus
// never falls back to the page's body: it goes to the element that took the
// opener's place (the same `data-focus-id`), else to the page's heading.

/** What is remembered of the element that had focus. */
export interface Opener<E> {
  el: E;
  /** `data-focus-id`: the same control in a row drawn again. */
  focusId: string | null;
}

export interface Page<E> {
  connected(el: E): boolean;
  byFocusId(id: string): E | null;
  /** The page's main heading (`h1`), or its main region. */
  heading(): E | null;
}

/** The element to focus in place of `opener`, or `null` when the opener
 * itself is still there (focus can return to it as usual). */
export function replacement<E>(from: Opener<E> | null, page: Page<E>): E | null {
  if (from !== null && page.connected(from.el)) return null;
  const id = from?.focusId ?? null;
  const same = id === null ? null : page.byFocusId(id);
  return same ?? page.heading();
}

const FOCUSABLE = "button, a[href], input, select, textarea, [tabindex]";

/** The browser's page for `replacement`. A `data-focus-id` may be on the
 * control or on an element around it. */
export const documentPage: Page<HTMLElement> = {
  connected: (el) => el.isConnected,
  byFocusId: (id) => {
    const el = document.querySelector<HTMLElement>(`[data-focus-id="${CSS.escape(id)}"]`);
    return el === null || el.matches(FOCUSABLE) ? el : el.querySelector<HTMLElement>(FOCUSABLE);
  },
  heading: () =>
    document.querySelector<HTMLElement>("main h1, h1") ?? document.querySelector("main"),
};

/** What to remember of the element focused now. */
export function opener(): Opener<HTMLElement> | null {
  const el = document.activeElement;
  if (!(el instanceof HTMLElement) || el === document.body) return null;
  const owner = el.closest<HTMLElement>("[data-focus-id]");
  return { el, focusId: owner?.dataset["focusId"] ?? null };
}

/** Focuses `el`, making a heading focusable by script only. */
export function focusOn(el: HTMLElement): void {
  if (el.tabIndex < 0 && !el.hasAttribute("tabindex")) el.setAttribute("tabindex", "-1");
  el.focus({ preventScroll: el.tagName !== "H1" });
}
