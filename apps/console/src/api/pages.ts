// Lists as the API pages them (08 §6.3): `limit` and an `after` cursor,
// `next` null on the last page. No screen assumes a list fits in one page:
// a list screen shows the pages loaded so far with "Load more", and a
// screen that needs a whole list (a chart's versions, the charts to choose
// from) loads every page up to a bound and says when it stopped short.
import { createSignal, onCleanup, type Accessor } from "solid-js";
import { isAbort, type ApiError } from "./client.ts";
import { asApiError, createQuery, type Loaded } from "./query.ts";

export interface Page<T> {
  data: T[];
  next: string | null;
}

export type PageLoader<T> = (after: string | undefined) => Promise<Page<T>>;

/** The first `count` pages (fewer when the list ends sooner), as one. */
export async function loadPages<T>(page: PageLoader<T>, count: number): Promise<Page<T>> {
  const data: T[] = [];
  let next: string | null = null;
  for (let i = 0; i < count; i++) {
    const p: Page<T> = await page(next ?? undefined);
    data.push(...p.data);
    next = p.next;
    if (next === null) break;
  }
  return { data, next };
}

/** Every page, up to `max` pages: `complete` is false when the list goes
 * on past them. */
export async function loadAll<T>(
  page: PageLoader<T>,
  max: number,
): Promise<{ data: T[]; complete: boolean }> {
  const { data, next } = await loadPages(page, max);
  return { data, complete: next === null };
}

export interface Pages<T> {
  /** The rows of the pages loaded so far. */
  state: Accessor<Loaded<Page<T>>>;
  /** Loads the next page and appends it. */
  more(): void;
  /** The next page's request: running, failed, or neither. */
  moreState: Accessor<"idle" | "loading" | ApiError>;
  /** Loads again as many pages as are shown, keeping them meanwhile. */
  refetch(): void;
  /** Changes the rows shown (a row replaced by an action's answer). */
  update(change: (rows: T[]) => T[]): void;
}

/** A paged list for a screen, keyed like `createQuery` (the workspace): a
 * new key starts again from one page, and cancels a "load more" in
 * flight, as a refetch does. */
export function createPages<K, T>(
  key: Accessor<K | null>,
  page: (key: K, after: string | undefined, signal: AbortSignal) => Promise<Page<T>>,
): Pages<T> {
  let pages = 1;
  let last: K | null = null;
  let more: AbortController | null = null;
  const [moreState, setMoreState] = createSignal<"idle" | "loading" | ApiError>("idle");
  const query = createQuery(key, (k, signal) => {
    if (k !== last) pages = 1;
    last = k;
    more?.abort();
    more = null;
    setMoreState("idle");
    return loadPages((after) => page(k, after, signal), pages);
  });
  onCleanup(() => {
    more?.abort();
    more = null;
  });
  return {
    state: query.state,
    moreState,
    refetch: query.refetch,
    update: (change) => query.update((p) => ({ ...p, data: change(p.data) })),
    more: () => {
      const s = query.state();
      const k = last;
      if (s.status !== "ready" || s.refreshing || s.data.next === null || k === null) return;
      if (more !== null) return;
      const mine = new AbortController();
      more = mine;
      setMoreState("loading");
      page(k, s.data.next, mine.signal).then(
        (p) => {
          if (more !== mine) return;
          more = null;
          pages += 1;
          query.update((shown) => ({ data: [...shown.data, ...p.data], next: p.next }));
          setMoreState("idle");
        },
        (e: unknown) => {
          if (more !== mine || isAbort(e)) return;
          more = null;
          setMoreState(asApiError(e));
        },
      );
    },
  };
}
