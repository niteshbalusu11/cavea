// The wrapper under React's own client and server renderers. bun has no DOM,
// so `installDom` fakes the little React needs to mount one `<div>`; the map
// is a recording fake standing in for `createSeatMap`, whose own behaviour is
// tested in `@cavea/renderer`.
import { afterEach, beforeEach, expect, mock, test } from "bun:test";
import type { SeatMap as SeatMapInstance, SeatMapOptions } from "@cavea/renderer";

interface Created {
  options: SeatMapOptions;
  calls: string[];
  resolve: () => void;
  map: SeatMapInstance;
}
const created: Created[] = [];

mock.module("@cavea/renderer", () => ({
  createSeatMap(_element: unknown, options: SeatMapOptions): Promise<SeatMapInstance> {
    const calls: string[] = [];
    const map = new Proxy({} as SeatMapInstance, {
      // Not a thenable: a promise resolves with it as it is.
      get: (_, name) =>
        name === "then"
          ? undefined
          : (arg: unknown) => calls.push(`${String(name)} ${JSON.stringify(arg)}`),
    });
    return new Promise((resolve, reject) => {
      const entry = { options, calls, map, resolve: () => resolve(map) };
      created.push(entry);
      options.signal?.addEventListener("abort", () => {
        calls.push("aborted");
        reject(Object.assign(new Error("aborted"), { code: "aborted" }));
      });
    });
  },
}));

const React = await import("react");
const { createRoot } = await import("react-dom/client");
const { renderToString } = await import("react-dom/server");
const { act } = React;
const { SeatMap } = await import("./index.ts");

/** Just enough DOM for react-dom to mount host elements in a container. */
class Node extends EventTarget {
  readonly childNodes: Node[] = [];
  parentNode: Node | null = null;
  readonly style: Record<string, string> = {};
  readonly attributes = new Map<string, string>();
  constructor(
    readonly nodeName: string,
    readonly nodeType: number,
    readonly ownerDocument: Document | null,
  ) {
    super();
  }
  get tagName(): string {
    return this.nodeName;
  }
  get firstChild(): Node | null {
    return this.childNodes[0] ?? null;
  }
  appendChild(child: Node): Node {
    return this.insertBefore(child, null);
  }
  insertBefore(child: Node, before: Node | null): Node {
    child.parentNode?.removeChild(child);
    const at = before === null ? -1 : this.childNodes.indexOf(before);
    this.childNodes.splice(at < 0 ? this.childNodes.length : at, 0, child);
    child.parentNode = this;
    return child;
  }
  removeChild(child: Node): Node {
    this.childNodes.splice(this.childNodes.indexOf(child), 1);
    child.parentNode = null;
    return child;
  }
  setAttribute(name: string, value: string): void {
    this.attributes.set(name, String(value));
  }
  removeAttribute(name: string): void {
    this.attributes.delete(name);
  }
  getAttribute(name: string): string | null {
    return this.attributes.get(name) ?? null;
  }
}
class Document extends Node {
  activeElement: Node | null = null;
  readonly body: Node;
  constructor() {
    super("#document", 9, null);
    this.body = this.createElement("body");
  }
  createElement(tag: string): Node {
    return new Node(tag.toUpperCase(), 1, this);
  }
}

let restore: () => void = () => undefined;
function installDom(): Node {
  const doc = new Document();
  const globals = {
    window: globalThis,
    document: doc,
    navigator: { userAgent: "bun" },
    HTMLElement: Node,
    // react-dom looks for focused iframes with `instanceof`.
    HTMLIFrameElement: Document,
    IS_REACT_ACT_ENVIRONMENT: true,
  };
  const before = Object.fromEntries(
    Object.keys(globals).map((k) => [k, Object.getOwnPropertyDescriptor(globalThis, k)]),
  );
  for (const [key, value] of Object.entries(globals)) {
    Object.defineProperty(globalThis, key, { value, configurable: true, writable: true });
  }
  restore = () => {
    for (const [key, descriptor] of Object.entries(before)) {
      if (descriptor === undefined) {
        delete (globalThis as Record<string, unknown>)[key];
      } else {
        Object.defineProperty(globalThis, key, descriptor);
      }
    }
  };
  const container = doc.createElement("div");
  doc.body.appendChild(container);
  return container;
}

/** Lets the wrapper's microtask and the map's promise settle inside `act`. */
const settle = (): Promise<void> => act(async () => await Promise.resolve());

let warnings: string[] = [];
const warn = console.warn;
beforeEach(() => {
  created.length = 0;
  warnings = [];
  console.warn = (message: string) => warnings.push(message);
});
afterEach(() => {
  restore();
  console.warn = warn;
});

function mount(): {
  render: (props: Record<string, unknown>, strict?: boolean) => Promise<void>;
  unmount: () => Promise<void>;
} {
  const root = createRoot(installDom() as unknown as HTMLElement);
  return {
    render: (props, strict = false) =>
      act(async () => {
        const map = React.createElement(SeatMap, props);
        root.render(strict ? React.createElement(React.StrictMode, null, map) : map);
      }),
    unmount: () => act(async () => root.unmount()),
  };
}

const noop = (): void => undefined;
const state = { connect: () => Promise.reject(new Error("unused")), disconnect: () => undefined };

test("Strict Mode's mount, unmount and mount creates one map, and unmounting destroys it", async () => {
  const view = mount();
  await view.render({ state, chart: { url: "/a.ccf" } }, true);
  await settle();
  expect(created).toHaveLength(1);
  created[0]!.resolve();
  await settle();
  expect(created[0]!.calls).toEqual([]);
  await view.unmount();
  // Aborting the signal after the map is ready is how it is destroyed (05 §9).
  expect(created[0]!.calls).toEqual(["aborted"]);
});

test("a changed setter prop calls the method; the map is not created again", async () => {
  const ref = React.createRef<SeatMapInstance | null>();
  const view = mount();
  const pricing = [{ category: "a", price: 100 }];
  await view.render({ ref, state, chart: { url: "/a.ccf" }, theme: "dark", pricing });
  await settle();
  created[0]!.resolve();
  await settle();
  expect(ref.current).toBe(created[0]!.map);
  // Equal values in new objects, and a new callback, change nothing.
  await view.render({
    ref,
    state,
    chart: { url: "/a.ccf" },
    theme: "dark",
    pricing: [{ category: "a", price: 100 }],
    onReady: noop,
  });
  await view.render({
    ref,
    state,
    chart: { url: "/a.ccf" },
    theme: "light",
    pricing,
    locale: "fr",
  });
  expect(created).toHaveLength(1);
  expect(created[0]!.calls).toEqual(['setTheme "light"', 'setLocale "fr"']);
  await view.render({
    ref,
    state,
    chart: { url: "/a.ccf" },
    pricing,
    locale: "fr",
    selection: { max: 2 },
  });
  expect(created[0]!.calls.at(-1)).toBe('setTheme "auto"');
  expect(warnings).toEqual([expect.stringContaining('prop "selection" changed')]);
  // A new adapter is a new map.
  await view.render({ ref, state: { ...state }, chart: { url: "/a.ccf" } });
  await settle();
  expect(created).toHaveLength(2);
  expect(created[0]!.calls.at(-1)).toBe("aborted");
  expect(ref.current).toBeNull();
  await view.unmount();
});

test("callbacks reach the latest prop without recreating the map", async () => {
  const seen: string[] = [];
  const view = mount();
  await view.render({ state, onHoldPending: () => seen.push("first") });
  await settle();
  await view.render({ state, onHoldPending: () => seen.push("second") });
  created[0]!.options.onHoldPending?.();
  expect(seen).toEqual(["second"]);
  // Added after creation: called too.
  await view.render({ state, onHoldPending: noop, onSelectionValid: () => seen.push("added") });
  created[0]!.options.onSelectionValid?.();
  expect(seen).toEqual(["second", "added"]);
  expect(created).toHaveLength(1);
  await view.unmount();
});

test("formatPrice, fixed at creation, keeps formatting when the prop is taken away", async () => {
  const view = mount();
  await view.render({ state, formatPrice: () => "first" });
  await settle();
  await view.render({ state, formatPrice: () => "second" });
  const format = created[0]!.options.formatPrice as (minor: number) => string;
  expect(format(100)).toBe("second");
  await view.render({ state });
  expect(format(100)).toBe("first");
  await view.unmount();
});

test("unmounting while the map loads aborts it, and the rejection goes nowhere", async () => {
  const errors: unknown[] = [];
  const view = mount();
  await view.render({ state, onError: (e: unknown) => errors.push(e) });
  await settle();
  await view.unmount();
  await settle();
  expect(created[0]!.calls).toEqual(["aborted"]);
  expect(created[0]!.options.signal?.aborted).toBe(true);
  expect(errors).toEqual([]);
});

test("a new adapter on every render warns", async () => {
  const view = mount();
  for (let i = 0; i < 4; i++) {
    await view.render({ state: { ...state } });
  }
  expect(warnings).toEqual([expect.stringContaining("recreated on three renders in a row")]);
  await view.unmount();
});

test("the server renders the container alone and touches no DOM", () => {
  expect(typeof document).toBe("undefined");
  const html = renderToString(
    React.createElement(SeatMap, { state, className: "map", style: { height: 520 } }),
  );
  expect(html).toBe('<div class="map" style="height:520px"></div>');
  expect(created).toHaveLength(0);
});
