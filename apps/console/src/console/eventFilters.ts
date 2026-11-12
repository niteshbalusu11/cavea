// The events list's search, filters and order (09 §4.1, CON-007): kept in
// the page's URL, so a reload or a shared link shows the same list, and sent
// to `GET /v1/events` as its parameters (08 §6.2), so they cover every
// event, not only the pages loaded. The URL holds what the person typed and
// chose, never a credential.
import type { Operations } from "../api/generated.ts";

export type Shown = "active" | "archived" | "all";
export type Order = "oldest" | "newest";

export interface Filters {
  /** Matched against key and name by the server, every character literal. */
  q: string;
  /** A chart's key; empty for every chart. */
  chart: string;
  shown: Shown;
  order: Order;
}

/** The server takes at most this many characters of search text. */
export const MAX_Q = 128;

export const DEFAULTS: Filters = { q: "", chart: "", shown: "active", order: "oldest" };

/** At most `MAX_Q` characters (code points, as the server counts them). */
export const clip = (q: string): string => [...q].slice(0, MAX_Q).join("");

const one = (v: string | string[] | undefined): string => (Array.isArray(v) ? v[0] : v) ?? "";

/** The filters a URL's parameters say; anything unknown is the default. */
export function fromParams(params: Record<string, string | string[] | undefined>): Filters {
  const shown = one(params["shown"]);
  const order = one(params["order"]);
  return {
    q: clip(one(params["q"])),
    chart: one(params["chart"]),
    shown: shown === "archived" || shown === "all" ? shown : "active",
    order: order === "newest" ? "newest" : "oldest",
  };
}

/** The URL's parameters for `f`: a default is left out (`undefined`
 * removes it), so the plain list has a plain URL. */
export function toParams(f: Filters): Record<string, string | undefined> {
  return {
    q: f.q === "" ? undefined : f.q,
    chart: f.chart === "" ? undefined : f.chart,
    shown: f.shown === "active" ? undefined : f.shown,
    order: f.order === "oldest" ? undefined : f.order,
  };
}

export const filtered = (f: Filters): boolean =>
  f.q.trim() !== "" || f.chart !== "" || f.shown !== "active";

/** `listEvents`' query for one page of `f`. */
export function eventsQuery(
  f: Filters,
  limit: number,
  after: string | undefined,
): NonNullable<Operations["listEvents"]["input"]["query"]> {
  const q = f.q.trim();
  return {
    limit,
    ...(after === undefined ? {} : { after }),
    ...(q === "" ? {} : { q: clip(q) }),
    ...(f.chart === "" ? {} : { chart: f.chart }),
    ...(f.shown === "all" ? {} : { archived: f.shown === "archived" }),
    ...(f.order === "oldest" ? {} : { order: f.order }),
  };
}
