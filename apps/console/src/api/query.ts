// Server state for a screen: one request per key, cancelled when the key
// changes or the screen goes, and never a previous key's data shown under
// the next (a workspace switch starts from "loading"). Refetching keeps
// the data on screen until the new answer arrives.
import { createEffect, createSignal, on, onCleanup, type Accessor } from "solid-js";
import { ApiError, isAbort } from "./client.ts";

export type Loaded<T> =
  | { status: "loading" }
  | { status: "ready"; data: T; refreshing: boolean }
  | { status: "error"; error: ApiError };

export interface Query<T> {
  state: Accessor<Loaded<T>>;
  /** Loads again, keeping what is shown meanwhile. */
  refetch(): void;
  /** Changes the data shown, when there is some (a page appended, a row
   * replaced by the server's answer to an action). */
  update(change: (data: T) => T): void;
}

export function createQuery<K, T>(
  key: Accessor<K | null>,
  load: (key: K, signal: AbortSignal) => Promise<T>,
): Query<T> {
  const [state, setState] = createSignal<Loaded<T>>({ status: "loading" });
  let controller: AbortController | null = null;
  let current: K | null = null;

  const run = (k: K, keep: boolean) => {
    controller?.abort();
    const mine = new AbortController();
    controller = mine;
    const shown = state();
    setState(
      keep && shown.status === "ready" ? { ...shown, refreshing: true } : { status: "loading" },
    );
    load(k, mine.signal).then(
      (data) => {
        if (controller === mine) setState({ status: "ready", data, refreshing: false });
      },
      (e: unknown) => {
        if (controller !== mine || isAbort(e)) return;
        setState({ status: "error", error: asApiError(e) });
      },
    );
  };

  createEffect(
    on(key, (k) => {
      current = k;
      if (k === null) {
        controller?.abort();
        controller = null;
        setState({ status: "loading" });
      } else {
        run(k, false);
      }
    }),
  );
  onCleanup(() => controller?.abort());
  return {
    state,
    refetch: () => {
      if (current !== null) run(current, true);
    },
    update: (change) => {
      const shown = state();
      if (shown.status === "ready") setState({ ...shown, data: change(shown.data) });
    },
  };
}

/** Any rejection as an `ApiError`; a fault of the console's own becomes
 * `unexpected`. */
export function asApiError(e: unknown): ApiError {
  if (e instanceof ApiError) return e;
  return new ApiError(0, "unexpected", null, {}, null, null, String(e));
}

/** A signal that aborts when the calling component goes: for the requests
 * its actions make. */
export function componentSignal(): AbortSignal {
  const controller = new AbortController();
  onCleanup(() => controller.abort());
  return controller.signal;
}
