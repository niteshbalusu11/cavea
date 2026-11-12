// The events list's search, filters and order (09 §4.1, CON-007): kept in
// the page's URL, so a reload or a shared link shows the same list, and sent
// to `GET /v1/events` as its parameters (08 §6.2), so they cover every
// event, not only the pages loaded. The URL holds what the person typed and
// chose, never a credential.
//
// The creation range is in days of the person's own time zone (D-93): "from
// 3 March until 5 March" is every event created from the first instant of
// 3 March there to the first instant of 6 March there, sent to the server
// as those two instants in UTC. The URL keeps the days, so a shared link
// means the same days in the reader's own zone.
import type { Operations } from "../api/generated.ts";
import { msg } from "../i18n/messages.ts";

export type Shown = "active" | "archived" | "all";
export type Order = "oldest" | "newest";

export interface Filters {
  /** Matched against key and name by the server, every character literal. */
  q: string;
  /** A chart's key; empty for every chart. */
  chart: string;
  shown: Shown;
  order: Order;
  /** The first day of the creation range, `YYYY-MM-DD`; empty for none. */
  from: string;
  /** The last day of the creation range (included); empty for none. */
  to: string;
}

/** The server takes at most this many characters of search text. */
export const MAX_Q = 128;

export const DEFAULTS: Filters = {
  q: "",
  chart: "",
  shown: "active",
  order: "oldest",
  from: "",
  to: "",
};

/** At most `MAX_Q` characters (code points, as the server counts them). */
export const clip = (q: string): string => [...q].slice(0, MAX_Q).join("");

const one = (v: string | string[] | undefined): string => (Array.isArray(v) ? v[0] : v) ?? "";

/** The filters a URL's parameters say; anything unknown is the default. */
export function fromParams(params: Record<string, string | string[] | undefined>): Filters {
  const shown = one(params["shown"]);
  const order = one(params["order"]);
  const from = day(one(params["from"]));
  const to = day(one(params["to"]));
  return {
    q: clip(one(params["q"])),
    chart: one(params["chart"]),
    shown: shown === "archived" || shown === "all" ? shown : "active",
    order: order === "newest" ? "newest" : "oldest",
    from,
    // An inverted range is never applied: its end is dropped.
    to: rangeProblem(from, to) === null ? to : "",
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
    from: f.from === "" ? undefined : f.from,
    to: f.to === "" ? undefined : f.to,
  };
}

export const filtered = (f: Filters): boolean =>
  f.q.trim() !== "" || f.chart !== "" || f.shown !== "active" || f.from !== "" || f.to !== "";

/** `listEvents`' query for one page of `f`. */
export function eventsQuery(
  f: Filters,
  limit: number,
  after: string | undefined,
  zone: string = localZone(),
): NonNullable<Operations["listEvents"]["input"]["query"]> {
  const q = f.q.trim();
  const from = f.from === "" ? null : dayStart(f.from, zone);
  const until = f.to === "" ? null : dayStart(dayAfter(f.to), zone);
  return {
    limit,
    ...(after === undefined ? {} : { after }),
    ...(q === "" ? {} : { q: clip(q) }),
    ...(f.chart === "" ? {} : { chart: f.chart }),
    ...(f.shown === "all" ? {} : { archived: f.shown === "archived" }),
    ...(f.order === "oldest" ? {} : { order: f.order }),
    ...(from === null ? {} : { created_after: instant(from) }),
    ...(until === null ? {} : { created_before: instant(until) }),
  };
}

/** The browser's time zone (an IANA name), or UTC when it has none. */
export function localZone(): string {
  return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
}

const DAY = /^(\d{4})-(\d{2})-(\d{2})$/;

/** `text` when it is a real day as `YYYY-MM-DD` (a date input's value),
 * else empty. */
export function day(text: string): string {
  const m = DAY.exec(text);
  if (m === null) return "";
  const [y, mo, d] = [Number(m[1]), Number(m[2]), Number(m[3])];
  // Before year 1000 is a year still being typed (and `Date.UTC` reads
  // 0 to 99 as 1900 to 1999).
  if (y < 1000) return "";
  const t = new Date(Date.UTC(y, mo - 1, d));
  return t.getUTCFullYear() === y && t.getUTCMonth() === mo - 1 && t.getUTCDate() === d
    ? text
    : "";
}

/** Why a range cannot be applied, or `null`: its end before its start. */
export function rangeProblem(from: string, to: string): string | null {
  return from !== "" && to !== "" && to < from ? msg("events.dateInverted") : null;
}

/** The day after `day` (`YYYY-MM-DD`). */
export function dayAfter(day: string): string {
  const [y = 0, m = 1, d = 1] = day.split("-").map(Number);
  return new Date(Date.UTC(y, m - 1, d + 1)).toISOString().slice(0, 10);
}

const walls = new Map<string, Intl.DateTimeFormat>();

/** The wall clock in `zone` at instant `t` (ms), as if it were UTC (ms). */
function wall(t: number, zone: string): number {
  let format = walls.get(zone);
  if (format === undefined) {
    format = new Intl.DateTimeFormat("en-US", {
      timeZone: zone,
      hourCycle: "h23",
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    });
    walls.set(zone, format);
  }
  const part = (type: string) =>
    Number(format.formatToParts(t).find((p) => p.type === type)?.value ?? 0);
  return Date.UTC(
    part("year"),
    part("month") - 1,
    part("day"),
    part("hour"),
    part("minute"),
    part("second"),
  );
}

/**
 * The first instant of `day` in `zone`: its midnight, or, where the clocks
 * skip midnight (a change to summer time at 00:00), the instant they skip
 * to; where midnight comes twice, the first. Found from the zone's offsets
 * around the day, so a change that day is taken into account.
 */
export function dayStart(day: string, zone: string): Date {
  const [y = 0, m = 1, d = 1] = day.split("-").map(Number);
  const midnight = Date.UTC(y, m - 1, d);
  const H = 3_600_000;
  const offsets = new Set<number>();
  for (const probe of [midnight - 24 * H, midnight, midnight + 24 * H]) {
    const t = Math.floor(probe / 1000) * 1000;
    offsets.add(wall(t, zone) - t);
  }
  const starts = [...offsets]
    .map((o) => midnight - o)
    .filter((t) => new Date(wall(t, zone)).toISOString().slice(0, 10) === day)
    .sort((a, b) => a - b);
  return new Date(starts[0] ?? midnight);
}

/** An instant as the server takes it: `YYYY-MM-DDTHH:MM:SSZ`. */
export const instant = (t: Date): string => t.toISOString().replace(/\.\d{3}Z$/, "Z");
