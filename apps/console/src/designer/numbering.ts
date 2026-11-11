// The numbering panel's model (07 §6): what its controls show, read from
// `numbering()`, and the `Numbering` they make. The schema sets a block's
// row or seat rule whole (03 §2.5), so each group is either left alone
// (each block keeps its own) or, once any of its controls is touched, set
// whole on every block: its other controls then show the value they will
// set, a mixed field taking the scheme's default.
import type {
  AlphaOverflow,
  LabelScheme,
  LabelStart,
  Numbering,
  NumberingView,
  RowDirection,
  RowLabels,
  SeatDirection,
  SeatLabels,
  Shared,
} from "@cavea/designer-core";

/** The row group's controls. List and start fields are the text typed. */
export interface RowControls {
  scheme: LabelScheme;
  start: string;
  step: number;
  skip: string;
  direction: RowDirection;
  prefix: string;
  suffix: string;
  custom: string;
}

export interface SeatControls {
  scheme: LabelScheme;
  start: string;
  step: number;
  direction: SeatDirection;
  continuous: boolean;
  prefix: string;
  suffix: string;
  custom: string;
}

/** The panel's state: `null` for a group left alone. */
export interface Draft {
  rows: RowControls | null;
  seats: SeatControls | null;
  afterZ: AlphaOverflow | null;
  /** Seat numbering continues through the blocks in this order; `null`: off. */
  across: number[] | null;
  keepOverrides: boolean;
}

export const ROW_SCHEMES: LabelScheme[] = ["alpha", "alpha-lower", "numeric", "roman", "custom"];
export const SEAT_SCHEMES: LabelScheme[] = ["numeric", "alpha", "odd", "even", "custom"];
export const ROW_DIRECTIONS: RowDirection[] = ["front-to-back", "back-to-front"];
/** Table directions (`clockwise`, `counterclockwise`) are refused here. */
export const SEAT_DIRECTIONS: SeatDirection[] = ["ltr", "rtl", "center-out", "serpentine"];

export const EMPTY: Draft = {
  rows: null,
  seats: null,
  afterZ: null,
  across: null,
  keepOverrides: false,
};

const pick = <T>(s: Shared<T>, fallback: T): T => (s === "mixed" ? fallback : s.same);
const startText = (s: LabelStart | null) => (s === null ? "" : String(s));
const listText = (l: string[]) => l.join(", ");

/** The row controls a first touch starts from: the blocks' values where
 * they agree, the default (03 §2.5) where they differ. */
export function rowsFrom(view: NumberingView): RowControls {
  const r = view.rows;
  return {
    scheme: r ? pick(r.scheme, "alpha") : "alpha",
    start: r ? startText(pick(r.start, null)) : "",
    step: r ? pick(r.step, 1) : 1,
    skip: r ? listText(pick(r.skip, [])) : "",
    direction: r ? pick(r.direction, "front-to-back") : "front-to-back",
    prefix: r ? pick(r.prefix, "") : "",
    suffix: r ? pick(r.suffix, "") : "",
    custom: r ? listText(pick(r.custom, [])) : "",
  };
}

export function seatsFrom(view: NumberingView): SeatControls {
  const s = view.seats;
  return {
    scheme: s ? pick(s.scheme, "numeric") : "numeric",
    start: s ? startText(pick(s.start, null)) : "",
    step: s ? pick(s.step, 1) : 1,
    direction: s ? pick(s.direction, "ltr") : "ltr",
    continuous: s ? pick(s.continuous, false) : false,
    prefix: s ? pick(s.prefix, "") : "",
    suffix: s ? pick(s.suffix, "") : "",
    custom: s ? listText(pick(s.custom, [])) : "",
  };
}

/** A list typed with commas or new lines; blank entries dropped. */
export const parseList = (text: string): string[] =>
  text
    .split(/[,\n]/)
    .map((s) => s.trim())
    .filter((s) => s !== "");

/** A start typed: a whole number is an ordinal (the scheme's 1st, 2nd…, and
 * for numbers the number itself), anything else a label in the scheme. */
export function parseStart(text: string): LabelStart | undefined {
  const t = text.trim();
  if (t === "") return undefined;
  return /^\d{1,9}$/.test(t) ? Number(t) : t;
}

/** Copies the optional fields of the schema that are set. */
function optional<T extends object>(base: T, extra: Record<string, unknown>): T {
  const out: Record<string, unknown> = { ...(base as Record<string, unknown>) };
  for (const [k, v] of Object.entries(extra)) {
    if (v === undefined || v === "" || (Array.isArray(v) && v.length === 0)) continue;
    out[k] = v;
  }
  return out as T;
}

export function toRowLabels(c: RowControls): RowLabels {
  return optional<RowLabels>(
    { scheme: c.scheme, step: c.step, direction: c.direction },
    {
      start: parseStart(c.start),
      skip: parseList(c.skip),
      prefix: c.prefix,
      suffix: c.suffix,
      custom: c.scheme === "custom" ? parseList(c.custom) : undefined,
    },
  );
}

export function toSeatLabels(c: SeatControls): SeatLabels {
  return optional<SeatLabels>(
    { scheme: c.scheme, step: c.step, direction: c.direction },
    {
      start: parseStart(c.start),
      continuous: c.continuous ? true : undefined,
      prefix: c.prefix,
      suffix: c.suffix,
      custom: c.scheme === "custom" ? parseList(c.custom) : undefined,
    },
  );
}

/** The call the panel makes: preview and apply take the same value. */
export function toNumbering(d: Draft): Numbering {
  return {
    rows: d.rows === null ? null : toRowLabels(d.rows),
    seats: d.seats === null ? null : toSeatLabels(d.seats),
    afterZ: d.afterZ,
    across: d.seats === null ? null : d.across,
    keepOverrides: d.keepOverrides,
  };
}

/** Whether the draft changes anything. */
export const touched = (d: Draft): boolean =>
  d.rows !== null || d.seats !== null || d.afterZ !== null;

/** `order` with `id` moved by `by` places, kept within the list. */
export function reorder(order: number[], id: number, by: number): number[] {
  const from = order.indexOf(id);
  const to = Math.max(0, Math.min(order.length - 1, from + by));
  if (from < 0 || from === to) return order;
  const out = order.filter((x) => x !== id);
  out.splice(to, 0, id);
  return out;
}
