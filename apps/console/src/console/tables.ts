// The console's tables (09 §8): column sorting that is honest about
// pagination, and keyboard row navigation.
//
// Sorting. A column sorts one of two ways, or not at all:
// - through the server, when the list's route orders by it (the events
//   list's `order`, by creation): it covers every row, loaded or not;
// - in the page, when every row is loaded (the list's last page has come):
//   sorting rows already here is then the whole truth;
// - not at all while a longer list is only partly loaded: sorting the
//   loaded rows would show the first of them as the first of all.
//
// Row navigation. A plain table (not a grid): each row has one focus stop,
// its primary control (a link or button) or, without one, its first cell.
// Up and Down arrows move between those stops, Home and End go to the first
// and last; Enter runs a primary control as it always does, and on a cell
// moves into the row's first control. Screen readers keep the table's own
// reading commands; Tab still reaches every control in order.

export type Direction = "ascending" | "descending";

export interface Sort<C extends string> {
  column: C;
  direction: Direction;
}

/** How `column` can be sorted now. */
export type SortBy = "server" | "page" | "none";

export function sortBy<C extends string>(
  column: C,
  table: {
    /** The columns the list's route orders by. */
    server: readonly C[];
    /** The columns that sort once every row is loaded. */
    page: readonly C[];
    /** Every row is loaded. */
    complete: boolean;
  },
): SortBy {
  if (table.server.includes(column)) return "server";
  if (table.page.includes(column) && table.complete) return "page";
  return "none";
}

/** A click on `column`'s header: the other direction when it is already
 * sorted by, else `first`. */
export function toggle<C extends string>(
  current: Sort<C> | null,
  column: C,
  first: Direction = "ascending",
): Sort<C> {
  if (current?.column === column) {
    return {
      column,
      direction: current.direction === "ascending" ? "descending" : "ascending",
    };
  }
  return { column, direction: first };
}

/** `aria-sort` for a header: only the column sorted by says so. */
export function ariaSort<C extends string>(
  current: Sort<C> | null,
  column: C,
): Direction | undefined {
  return current?.column === column ? current.direction : undefined;
}

/** The rows in the order asked, stably; ties keep the server's order. */
export function sorted<T, C extends string>(
  rows: readonly T[],
  sort: Sort<C> | null,
  compare: Record<C, (a: T, b: T) => number>,
): T[] {
  if (sort === null) return [...rows];
  const by = compare[sort.column];
  const sign = sort.direction === "ascending" ? 1 : -1;
  const indexed = rows.map((row, i) => ({ row, i }));
  // oxlint-disable-next-line unicorn/no-array-sort -- a new array of our own; `toSorted` is ES2023, past the lib (ES2022).
  indexed.sort((a, b) => sign * by(a.row, b.row) || a.i - b.i);
  return indexed.map((x) => x.row);
}

const text = new Intl.Collator("en", { sensitivity: "base", numeric: true });
/** Comparisons for the columns' kinds of value. */
export const by = {
  text:
    <T>(get: (row: T) => string) =>
    (a: T, b: T) =>
      text.compare(get(a), get(b)),
  number:
    <T>(get: (row: T) => number) =>
    (a: T, b: T) =>
      get(a) - get(b),
  /** ISO instants; `null` (never) sorts before every instant. */
  time:
    <T>(get: (row: T) => string | null) =>
    (a: T, b: T) =>
      (get(a) ?? "").localeCompare(get(b) ?? ""),
};

/** The row a navigation key goes to from row `at` of `count`, or `null`
 * for a key that is not one. */
export function nextRow(key: string, at: number, count: number): number | null {
  if (count === 0) return null;
  switch (key) {
    case "ArrowDown":
      return Math.min(count - 1, at + 1);
    case "ArrowUp":
      return Math.max(0, at - 1);
    case "Home":
      return 0;
    case "End":
      return count - 1;
    default:
      return null;
  }
}

const STOP = "[data-row-stop]";
const CONTROLS = "a[href],button,select,input,textarea,[tabindex]";

/**
 * The tbody's key handler: moves between rows' stops, and from a cell stop
 * into its row with Enter. Keys on anything else (a menu's trigger, a
 * select) are theirs.
 */
export function rowKeys(e: KeyboardEvent): void {
  if (e.altKey || e.ctrlKey || e.metaKey || e.shiftKey || e.defaultPrevented) return;
  const target = e.target;
  if (!(target instanceof HTMLElement) || !target.matches(STOP)) return;
  const body = e.currentTarget;
  if (!(body instanceof HTMLElement)) return;
  if (e.key === "Enter" && target.tagName === "TD") {
    const control = [...(target.closest("tr")?.querySelectorAll<HTMLElement>(CONTROLS) ?? [])].find(
      (c) => c !== target && !c.matches(STOP),
    );
    if (control !== undefined) {
      e.preventDefault();
      control.focus();
    }
    return;
  }
  const stops = [...body.querySelectorAll<HTMLElement>(STOP)];
  const to = nextRow(e.key, stops.indexOf(target), stops.length);
  if (to === null) return;
  e.preventDefault();
  stops[to]?.focus();
}

/** A cell stop's `tabindex`: one cell of the table is in the Tab order
 * (the first row's until another is focused), the others are reached by
 * arrows. Call from the cell's focus event. */
export function rove(cell: HTMLElement): void {
  const body = cell.closest("tbody");
  for (const c of body?.querySelectorAll<HTMLElement>(`td${STOP}`) ?? []) {
    c.tabIndex = c === cell ? 0 : -1;
  }
}
