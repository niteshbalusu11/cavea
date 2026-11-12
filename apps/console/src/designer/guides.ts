// Where user guides live (07 §5.2, DSN-031). Guides are not chart data: the
// chart document has no place for them, and the server's draft route takes
// a chart document and nothing else. So a server chart's guides are kept in
// this browser, per person, workspace and chart, beside the draft the
// server keeps; a local chart file's last as long as the page, as the chart
// does. The session holds them; this module stores what `savedGuides()`
// gives and hands it back with `restoreGuides`, which takes all or nothing.
import type { FloorGuides } from "@cavea/designer-core";
import { isRecord } from "../storage.ts";

/** The key of a server chart's guides in local storage. */
export const guidesKey = (user: string, workspace: string, chart: string): string =>
  `cavea.designer.guides.v1/${user}/${workspace}/${chart}`;

/** Guides on a floor, at most (the edit core's `MAX_GUIDES`). */
const MAX_GUIDES = 256;
/** Floors with guides kept, at most (02 §14: 255 floors and the implicit one). */
const MAX_FLOORS = 256;

/**
 * Stored guides as the session takes them, or `undefined` for anything else
 * (another build's record, a hand-edited value): each floor `null` or a
 * whole number, each guide an axis and a finite position, within the
 * limits. The session checks them again.
 */
export function parseGuides(raw: unknown): FloorGuides[] | undefined {
  if (!Array.isArray(raw) || raw.length > MAX_FLOORS) return undefined;
  const out: FloorGuides[] = [];
  for (const f of raw) {
    if (!isRecord(f) || !Array.isArray(f["guides"]) || f["guides"].length > MAX_GUIDES) {
      return undefined;
    }
    const floor = f["floor"];
    if (floor !== null && !(Number.isInteger(floor) && (floor as number) >= 0)) return undefined;
    const guides: FloorGuides["guides"] = [];
    for (const g of f["guides"] as unknown[]) {
      if (!isRecord(g)) return undefined;
      const { axis, at } = g;
      if ((axis !== "vertical" && axis !== "horizontal") || typeof at !== "number") {
        return undefined;
      }
      if (!Number.isFinite(at)) return undefined;
      guides.push({ axis, at });
    }
    out.push({ floor: floor as number | null, guides });
  }
  return out;
}

/** Writes guides at most once per `delay` ms, the last ones given; `flush`
 * writes what waits now (the page is hidden or going). */
export function throttledWriter(
  write: (saved: FloorGuides[]) => void,
  delay = 500,
  clock: { set: (run: () => void, ms: number) => unknown; clear: (h: unknown) => void } = {
    set: (run, ms) => setTimeout(run, ms),
    clear: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
  },
): { put(saved: () => FloorGuides[]): void; flush(): void } {
  let waiting: (() => FloorGuides[]) | null = null;
  let timer: unknown = null;
  const flush = () => {
    if (timer !== null) clock.clear(timer);
    timer = null;
    const take = waiting;
    waiting = null;
    if (take !== null) write(take());
  };
  return {
    put(saved) {
      waiting = saved;
      if (timer === null) timer = clock.set(flush, delay);
    },
    flush,
  };
}
