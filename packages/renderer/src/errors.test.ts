// The error model (05 §9, WEB-017): which failures are tried again and how,
// what the buyer sees, what the host is told, and how the host cancels.
import { describe, expect, test } from "bun:test";
import { customState } from "./adapter.ts";
import type { CaveaError } from "./errors.ts";
import type { FakeNode } from "./fakedom.ts";
import { fakeEnv, flush, sleep } from "./fakes.ts";
import type { SeatMapOptions } from "./options.ts";
import { createMap } from "./seatmap.ts";

const http = (status: number): Error => Object.assign(new Error(`HTTP ${status}`), { status });
const part = (root: FakeNode, name: string): FakeNode | undefined => root.parts(name)[0];
const shown = (root: FakeNode, name: string): boolean => part(root, name)?.hidden === false;

/** A map whose chart fetches fail with `failures` in turn, then succeed. */
function start(failures: unknown[], options: SeatMapOptions = {}) {
  const f = fakeEnv();
  const errors: CaveaError[] = [];
  let fetches = 0;
  f.env.fetchBytes = () => {
    const failure = failures[fetches++];
    return failure === undefined ? Promise.resolve(new Uint8Array([1])) : Promise.reject(failure);
  };
  let settled: "resolved" | "rejected" | undefined;
  const promise = createMap(f.env, {} as HTMLElement, {
    chart: { url: "c.ccf" },
    onError: (e) => errors.push(e),
    ...options,
  });
  promise.then(
    () => (settled = "resolved"),
    () => (settled = "rejected"),
  );
  f.wasm.resolve(() => f.view.engine);
  return {
    ...f,
    promise,
    errors,
    codes: () => errors.map((e) => `${e.code}${e.fatal ? "!" : ""}`),
    fetches: () => fetches,
    settled: () => settled,
  };
}

describe("retries", () => {
  test("a chart that fails for a while is tried again with backoff, quietly", async () => {
    const m = start([new TypeError("network"), http(503), http(429)]);
    const map = await m.promise;
    expect([m.fetches(), m.codes(), shown(m.root, "error")]).toEqual([4, [], false]);
    map.destroy();
  });

  test("never what cannot succeed: a 404, a corrupt chart, blocked or broken WASM", async () => {
    const gone = start([http(410)]);
    await expect(gone.promise).rejects.toMatchObject({ code: "chart_fetch_failed", fatal: true });
    expect([gone.fetches(), gone.state.removed]).toEqual([1, true]);

    const corrupt = start([]);
    corrupt.view.answers["loadChart"] = () => {
      throw new Error("BadChart: not a CCF");
    };
    await expect(corrupt.promise).rejects.toMatchObject({ code: "bad_chart", fatal: true });

    // A Content Security Policy refusal is a CompileError.
    const f = fakeEnv();
    let compiles = 0;
    f.env.wasm = () => {
      compiles += 1;
      return Promise.reject(
        Object.assign(new Error("Refused to compile"), { name: "CompileError" }),
      );
    };
    const blocked = createMap(f.env, {} as HTMLElement, { chart: { url: "c.ccf" } });
    await expect(blocked).rejects.toMatchObject({ code: "wasm_unavailable", fatal: true });
    expect([compiles, f.state.removed]).toEqual([1, true]);
  });

  test("the WASM, the first state and the atlas are tried again too", async () => {
    const f = fakeEnv();
    let [wasms, connects, atlases] = [0, 0, 0];
    f.env.wasm = () =>
      ++wasms < 3 ? Promise.reject(new TypeError("fetch")) : Promise.resolve(() => f.view.engine);
    f.env.atlas = () => (++atlases, Promise.reject(new TypeError("fetch")));
    const state = customState({
      connect: () =>
        ++connects < 2
          ? Promise.reject(http(502))
          : Promise.resolve({ kind: "map", unavailable: [] }),
      disconnect() {},
    });
    const map = await createMap(f.env, {} as HTMLElement, {
      chart: { bytes: new Uint8Array([1]) },
      state,
    });
    await sleep(30);
    // The atlas gives up after three retries; the map carries on without labels.
    expect([wasms, connects, atlases]).toEqual([3, 2, 4]);
    map.destroy();
  });
});

describe("error state", () => {
  test("after the retries: the error, recoverable, the promise pending; Retry starts again", async () => {
    const m = start(Array.from({ length: 4 }, () => http(503)));
    await sleep(50);
    expect([
      m.fetches(),
      m.codes(),
      m.settled(),
      shown(m.root, "error"),
      shown(m.root, "retry"),
    ]).toEqual([4, ["chart_fetch_failed"], undefined, true, true]);
    expect(part(m.root, "error")?.textContent).toBe("The seat map could not be loaded.Try again");
    part(m.root, "retry")?.click();
    (await m.promise).destroy();
    expect(m.fetches()).toBe(5);
  });

  test("without it the start rejects after the retries", async () => {
    const m = start(
      Array.from({ length: 4 }, () => http(503)),
      { ui: { loading: false } },
    );
    await expect(m.promise).rejects.toMatchObject({ code: "chart_fetch_failed", fatal: true });
    expect(m.fetches()).toBe(4);
  });

  test("offline: no tries, the offline message, and loading resumes when back online", async () => {
    const m = start([new TypeError("network")]);
    m.setUsable(false);
    await sleep(20);
    expect([m.fetches(), shown(m.root, "error"), shown(m.root, "retry")]).toEqual([1, true, false]);
    expect(part(m.root, "error")?.firstChild?.textContent).toBe(
      "You are offline. The seat map will load when you are back online.",
    );
    m.setUsable(true);
    (await m.promise).destroy();
    expect([m.fetches(), m.codes()]).toEqual([2, []]);
  });

  test("hidden: no tries until the page is shown again", async () => {
    const m = start([new TypeError("network")]);
    m.setUsable(true, false);
    await sleep(20);
    expect([m.fetches(), shown(m.root, "loading"), shown(m.root, "error")]).toEqual([
      1,
      true,
      false,
    ]);
    m.setUsable(true, true);
    (await m.promise).destroy();
    expect(m.fetches()).toBe(2);
  });

  test("a context lost for good: reported, and said in place of the map until it returns", async () => {
    const m = start([]);
    const map = await m.promise;
    m.attached()?.onFrame?.(0, 0);
    m.attached()?.onContext?.(false);
    await sleep(50);
    expect([m.codes(), shown(m.root, "error"), shown(m.root, "retry")]).toEqual([
      ["context_lost"],
      true,
      false,
    ]);
    expect(part(m.root, "error")?.firstChild?.textContent).toBe(
      "The seat map cannot be drawn right now.",
    );
    m.attached()?.onContext?.(true);
    expect(shown(m.root, "error")).toBe(false);
    map.destroy();
  });
});

describe("error state over a drawn map", () => {
  test("a first state that keeps failing keeps its Retry after the first frame", async () => {
    const f = fakeEnv();
    let connects = 0;
    const state = customState({
      connect: () =>
        ++connects <= 4
          ? Promise.reject(http(503))
          : Promise.resolve({ kind: "map", unavailable: [] }),
      disconnect() {},
    });
    const promise = createMap(f.env, {} as HTMLElement, { chart: { url: "c.ccf" }, state });
    f.load();
    await sleep(50);
    f.attached()?.onFrame?.(0, 0);
    expect([shown(f.root, "error"), shown(f.root, "retry")]).toEqual([true, true]);
    part(f.root, "retry")?.click();
    const map = await promise;
    expect([connects, shown(f.root, "error"), shown(f.root, "loading")]).toEqual([5, false, false]);
    map.destroy();
  });

  test("without the loading state, offline and a lost context reject nothing", async () => {
    const rejections: unknown[] = [];
    const listener = (reason: unknown): void => {
      rejections.push(reason);
    };
    process.on("unhandledRejection", listener);
    try {
      const m = start([new TypeError("network")], { ui: { loading: false } });
      m.setUsable(false);
      await sleep(20);
      m.setUsable(true);
      const map = await m.promise;
      m.attached()?.onContext?.(false);
      await sleep(50);
      expect(m.codes()).toEqual(["context_lost"]);
      map.destroy();
      await flush();
      expect(rejections).toEqual([]);
    } finally {
      process.off("unhandledRejection", listener);
    }
  });
});

describe("cancelling", () => {
  test("aborting the signal while the error shows rejects and removes everything", async () => {
    const controller = new AbortController();
    let disconnected = 0;
    const m = start(
      Array.from({ length: 4 }, () => http(503)),
      {
        signal: controller.signal,
        state: customState({
          connect: () => new Promise(() => {}),
          disconnect: () => (disconnected += 1),
        }),
      },
    );
    await sleep(50);
    expect(shown(m.root, "error")).toBe(true);
    controller.abort();
    await expect(m.promise).rejects.toMatchObject({ code: "aborted", fatal: true });
    expect([disconnected, m.state.removed, m.state.detached]).toEqual([1, true, true]);
  });

  test("an aborted signal never starts; aborting after ready destroys the map", async () => {
    const m = start([], { signal: AbortSignal.abort() });
    await expect(m.promise).rejects.toMatchObject({ code: "aborted" });
    const controller = new AbortController();
    const ready = start([], { signal: controller.signal });
    await ready.promise;
    controller.abort();
    await flush();
    expect(ready.state.removed).toBe(true);
  });
});
