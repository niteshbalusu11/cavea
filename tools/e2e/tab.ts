// One page of the end-to-end check in a browser context of its own (its own
// cookies and storage), driven as a person drives it: elements found by
// their role and accessible name in the browser's accessibility tree, real
// mouse and key events through the DevTools protocol at coordinates read from
// the page, and every wait a condition with a timeout that says what it
// waits for. Everything the page does that a step did not ask for is
// recorded: uncaught errors and unhandled rejections, console errors,
// Content Security Policy violations, and failed requests.
import type { Browser, Message } from "../chromium.ts";

/** A name to match: exactly, or by a pattern. */
export type Name = string | RegExp;

/** What to find: an accessibility role and, optionally, a name. */
export interface Query {
  /** A role, or several as `a|b`. */
  role: string;
  name?: Name;
  /** Which of several matches, in document order; without it, more than one is an error. */
  nth?: number;
}

/** A node of the accessibility tree, as `Accessibility.queryAXTree` gives it. */
interface AxNode {
  ignored: boolean;
  role?: { value?: string };
  name?: { value?: string };
  properties?: Array<{ name: string; value: { value?: unknown } }>;
  backendDOMNodeId?: number;
}

/** A request that failed: an HTTP status of 400 or more, or no answer at all. */
export interface Failure {
  method: string;
  url: string;
  status: number | string;
}

/** A problem the page had that no step asked for. */
export interface Problem {
  kind: "exception" | "console" | "csp" | "request" | "map";
  text: string;
}

/** The last text a wait read from the page: a timeout shows what was there instead. */
let lastRead: string | undefined;

/**
 * Waits until `probe` gives a value (not undefined, null or false) and
 * returns it. Fails after `ms` with "timed out after … waiting for `what`",
 * then the probe's last error, or the last text it read from the page.
 */
export async function until<T>(
  what: string,
  probe: () => Promise<T | undefined | null | false>,
  ms = 15_000,
): Promise<T> {
  let last = "";
  lastRead = undefined;
  for (const deadline = Date.now() + ms; ;) {
    try {
      const value = await probe();
      if (value !== undefined && value !== null && value !== false) {
        return value;
      }
    } catch (error) {
      last = error instanceof Error ? error.message : String(error);
    }
    if (Date.now() > deadline) {
      const seen =
        last !== ""
          ? ` (${last})`
          : lastRead !== undefined
            ? `; the page showed “${lastRead.slice(0, 400)}”`
            : "";
      throw new Error(`timed out after ${ms / 1000} s waiting for ${what}${seen}`);
    }
    await Bun.sleep(25);
  }
}

const matches = (name: string, want: Name | undefined): boolean =>
  want === undefined || (typeof want === "string" ? name === want : want.test(name));

const describe = (q: Query): string =>
  `${q.role}${q.name === undefined ? "" : ` “${String(q.name)}”`}`;

/** DevTools key definitions for the keys the steps press. */
const KEYS: Record<string, { code: string; keyCode: number; text?: string }> = {
  Enter: { code: "Enter", keyCode: 13, text: "\r" },
  Escape: { code: "Escape", keyCode: 27 },
  Tab: { code: "Tab", keyCode: 9 },
  Backspace: { code: "Backspace", keyCode: 8 },
  ArrowUp: { code: "ArrowUp", keyCode: 38 },
  ArrowDown: { code: "ArrowDown", keyCode: 40 },
  ArrowLeft: { code: "ArrowLeft", keyCode: 37 },
  ArrowRight: { code: "ArrowRight", keyCode: 39 },
};

/**
 * Installed before any script of the page: Content Security Policy
 * violations and unhandled rejections land in `caveaE2e`, which the tab reads.
 * (DevTools reports uncaught errors itself; a rejection is reported only as a
 * console message, so it is caught here too.)
 */
const RECORDER = `(() => {
  const seen = [];
  Object.defineProperty(window, "caveaE2e", { value: seen });
  addEventListener("securitypolicyviolation", (e) =>
    seen.push("csp " + e.violatedDirective + " " + (e.blockedURI || "inline") + " at " + e.sourceFile + ":" + e.lineNumber));
  addEventListener("unhandledrejection", (e) =>
    seen.push("unhandled rejection: " + String(e.reason && e.reason.stack || e.reason).slice(0, 400)));
})();`;

export class Tab {
  /** Everything the page did that may be a problem, in order. */
  readonly problems: Problem[] = [];
  /** Every request and its outcome, for the failure report. */
  readonly network: string[] = [];
  /** WebSockets the page opened, by URL. */
  readonly sockets: string[] = [];
  /** Console messages of every level, for the failure report. */
  readonly console: string[] = [];
  /** Failed requests a step said to expect: `once` ones are taken off as they come. */
  private readonly expected: Array<{
    method: string;
    path: RegExp;
    status: number | string;
    once: boolean;
  }> = [];
  private readonly requests = new Map<string, { method: string; url: string }>();
  private readonly off: () => void;

  private constructor(
    readonly name: string,
    private readonly browser: Browser,
    readonly session: string,
    private readonly context: string,
  ) {
    this.off = browser.on((m) => this.event(m));
  }

  /** A blank page in a fresh browser context, recording from the start, `width`×`height` CSS px. */
  static async open(browser: Browser, name: string, width = 1440, height = 900): Promise<Tab> {
    const { session, context } = await browser.context();
    const tab = new Tab(name, browser, session, context);
    await tab.send("Page.enable");
    await tab.send("Runtime.enable");
    await tab.send("Network.enable");
    await tab.send("Log.enable");
    await tab.send("Page.addScriptToEvaluateOnNewDocument", { source: RECORDER });
    await tab.send("Emulation.setDeviceMetricsOverride", {
      width,
      height,
      deviceScaleFactor: 1,
      mobile: false,
    });
    // A page in the background throttles its timers and frames; every tab
    // of the run is in front, as two buyers' windows side by side would be.
    await tab.send("Emulation.setFocusEmulationEnabled", { enabled: true });
    return tab;
  }

  send(method: string, params: Record<string, unknown> = {}): Promise<Record<string, unknown>> {
    return this.browser.send(method, params, this.session);
  }

  private event(m: Message): void {
    if (m.sessionId !== this.session || m.params === undefined) {
      return;
    }
    const p = m.params;
    switch (m.method) {
      case "Runtime.exceptionThrown": {
        const d = p["exceptionDetails"] as {
          text?: string;
          exception?: { description?: string };
          url?: string;
        };
        this.problems.push({
          kind: "exception",
          text: `${d.exception?.description ?? d.text ?? "?"}`.slice(0, 600),
        });
        break;
      }
      case "Runtime.consoleAPICalled": {
        const args = (p["args"] as Array<{ value?: unknown; description?: string }>)
          .map((a) => String(a.value ?? a.description ?? ""))
          .join(" ")
          .slice(0, 600);
        this.console.push(`${String(p["type"])}: ${args}`);
        if (p["type"] === "error") {
          this.problems.push({ kind: "console", text: args });
        }
        break;
      }
      case "Log.entryAdded": {
        const entry = p["entry"] as { level: string; source: string; text: string; url?: string };
        this.console.push(`${entry.level} (${entry.source}): ${entry.text}`);
        // Failed requests are judged by the network events, with their exceptions.
        if (entry.level === "error" && entry.source !== "network") {
          this.problems.push({ kind: "console", text: `${entry.source}: ${entry.text}` });
        }
        break;
      }
      case "Network.requestWillBeSent": {
        const r = p["request"] as { method: string; url: string };
        this.requests.set(String(p["requestId"]), { method: r.method, url: r.url });
        break;
      }
      case "Network.responseReceived": {
        const response = p["response"] as { status: number; url: string };
        const r = this.requests.get(String(p["requestId"]));
        const method = r?.method ?? "GET";
        this.network.push(`${response.status} ${method} ${response.url}`);
        if (response.status >= 400) {
          this.failed({ method, url: response.url, status: response.status });
        }
        break;
      }
      case "Network.webSocketCreated":
        this.sockets.push(String(p["url"]));
        this.network.push(`websocket ${String(p["url"])}`);
        break;
      case "Network.loadingFailed": {
        const r = this.requests.get(String(p["requestId"]));
        const why = String(p["blockedReason"] ?? p["errorText"]);
        this.network.push(`failed ${r?.method ?? "?"} ${r?.url ?? "?"}: ${why}`);
        // A request the page cancelled itself (an AbortController, a socket
        // closed on leaving) is not a failure; a refused or broken one is.
        if (p["canceled"] !== true) {
          this.failed({ method: r?.method ?? "?", url: r?.url ?? "?", status: why });
        }
        break;
      }
      default:
    }
  }

  private failed(f: Failure): void {
    const path = (() => {
      try {
        return new URL(f.url).pathname;
      } catch {
        return f.url;
      }
    })();
    const i = this.expected.findIndex(
      (e) => e.method === f.method && e.path.test(path) && e.status === f.status,
    );
    if (i >= 0) {
      if (this.expected[i]?.once === true) this.expected.splice(i, 1);
      return;
    }
    this.problems.push({ kind: "request", text: `${f.method} ${f.url} → ${f.status}` });
  }

  /** Takes one failed request matching this as expected (a step's own business), not a problem. */
  expect(method: string, path: RegExp, status: number | string): void {
    this.expected.push({ method, path, status, once: true });
  }

  /** Takes every failed request matching this as expected, for as long as the tab is open. */
  allow(method: string, path: RegExp, status: number | string): void {
    this.expected.push({ method, path, status, once: false });
  }

  /** Policy violations and unhandled rejections the page recorded, moved into `problems`. */
  async collect(): Promise<void> {
    const seen = await this.evaluate<string[]>(
      "(() => { const s = window.caveaE2e; return s ? s.splice(0) : []; })()",
    ).catch(() => []);
    for (const text of seen) {
      const kind = text.startsWith("csp") ? "csp" : text.startsWith("map") ? "map" : "exception";
      this.problems.push({ kind, text });
    }
  }

  async goto(url: string): Promise<void> {
    await this.send("Page.navigate", { url });
    await until(`${url} to load`, () =>
      this.evaluate<boolean>("document.readyState === 'complete'"),
    );
  }

  /** Evaluates `expression` in the page, awaiting a promise, and returns its value. */
  async evaluate<T>(expression: string): Promise<T> {
    const reply = await this.send("Runtime.evaluate", {
      expression,
      awaitPromise: true,
      returnByValue: true,
    });
    const details = reply["exceptionDetails"] as
      | { exception?: { description?: string }; text?: string }
      | undefined;
    if (details !== undefined) {
      throw new Error(details.exception?.description ?? details.text ?? "evaluation failed");
    }
    return (reply["result"] as { value: T }).value;
  }

  // ------------------------------------------------------------- finding

  /** The accessibility nodes under `within` (or the document) matching `q`, shown ones only. */
  async all(q: Query, within?: number): Promise<Array<{ id: number; name: string; node: AxNode }>> {
    let root = within;
    if (root === undefined) {
      const { root: doc } = (await this.send("DOM.getDocument", { depth: 0 })) as {
        root: { backendNodeId: number };
      };
      root = doc.backendNodeId;
    }
    const nodes: AxNode[] = [];
    // `a|b`: either role (Chromium names a modal dialog `alertdialog`).
    for (const role of q.role.split("|")) {
      const found = (await this.send("Accessibility.queryAXTree", {
        backendNodeId: root,
        role,
      })) as {
        nodes: AxNode[];
      };
      nodes.push(...found.nodes);
    }
    return nodes
      .filter((n) => !n.ignored && n.backendDOMNodeId !== undefined)
      .map((n) => ({ id: n.backendDOMNodeId ?? 0, name: n.name?.value ?? "", node: n }))
      .filter((n) => matches(n.name, q.name))
      .filter((_, i) => q.nth === undefined || i === q.nth);
  }

  /** The one element matching `q`, waiting for it; more than one is an error. */
  async find(q: Query, within?: number, ms = 15_000): Promise<number> {
    return until(
      `${describe(q)} on ${this.name}`,
      async () => {
        const found = await this.all(q, within);
        if (found.length > 1) {
          throw new Error(`${found.length} match: ${found.map((f) => f.name).join(" | ")}`);
        }
        return found[0]?.id;
      },
      ms,
    );
  }

  /** Whether something matching `q` is shown now. */
  async has(q: Query, within?: number): Promise<boolean> {
    return (await this.all(q, within)).length > 0;
  }

  /** Waits until nothing matching `q` is shown. */
  async gone(q: Query, within?: number, ms = 15_000): Promise<void> {
    await until(
      `${describe(q)} to go from ${this.name}`,
      async () => !(await this.has(q, within)),
      ms,
    );
  }

  /** The accessible name of the element matching `q`. */
  async nameOf(q: Query, within?: number): Promise<string> {
    await this.find(q, within);
    return (await this.all(q, within))[0]?.name ?? "";
  }

  /** Calls `fn` (a function's source) on the element, returning its value. */
  async call<T>(id: number, fn: string): Promise<T> {
    const { object } = (await this.send("DOM.resolveNode", { backendNodeId: id })) as {
      object: { objectId: string };
    };
    const reply = await this.send("Runtime.callFunctionOn", {
      objectId: object.objectId,
      functionDeclaration: fn,
      returnByValue: true,
      awaitPromise: true,
    });
    return (reply["result"] as { value: T }).value;
  }

  /** An element's text as shown, whitespace collapsed. */
  async text(id: number): Promise<string> {
    lastRead = await this.call<string>(
      id,
      "function () { return (this.innerText ?? this.textContent ?? '').replace(/\\s+/g, ' ').trim(); }",
    );
    return lastRead;
  }

  /** An input's value. */
  value(id: number): Promise<string> {
    return this.call<string>(id, "function () { return this.value; }");
  }

  /** The element's centre on the page in CSS px, scrolled into view; fails while it has no box. */
  async centre(id: number): Promise<[number, number]> {
    await this.send("DOM.scrollIntoViewIfNeeded", { backendNodeId: id }).catch(() => undefined);
    const { quads } = (await this.send("DOM.getContentQuads", { backendNodeId: id })) as {
      quads: number[][];
    };
    const q = quads[0];
    if (q === undefined || q.length < 8) {
      throw new Error("it is not shown");
    }
    const xs = [q[0], q[2], q[4], q[6]] as number[];
    const ys = [q[1], q[3], q[5], q[7]] as number[];
    return [(Math.min(...xs) + Math.max(...xs)) / 2, (Math.min(...ys) + Math.max(...ys)) / 2];
  }

  /** The element's box: left, top, width, height in CSS px. */
  async box(id: number): Promise<[number, number, number, number]> {
    return this.call<[number, number, number, number]>(
      id,
      "function () { const r = this.getBoundingClientRect(); return [r.left, r.top, r.width, r.height]; }",
    );
  }

  // --------------------------------------------------------------- input

  async mouse(
    type: "mouseMoved" | "mousePressed" | "mouseReleased",
    x: number,
    y: number,
    buttons = type === "mousePressed" ? 1 : 0,
  ): Promise<void> {
    await this.send("Input.dispatchMouseEvent", {
      type,
      x,
      y,
      button: type === "mouseMoved" ? (buttons === 0 ? "none" : "left") : "left",
      buttons,
      clickCount: type === "mouseMoved" ? 0 : 1,
      pointerType: "mouse",
    });
  }

  /** A left click at a point: the pointer moves there, presses and releases. */
  async clickAt(x: number, y: number): Promise<void> {
    await this.mouse("mouseMoved", x, y);
    await this.mouse("mousePressed", x, y);
    await this.mouse("mouseReleased", x, y);
  }

  /** A left-button drag from `a` to `b` in `steps` moves. */
  async drag(a: [number, number], b: [number, number], steps = 8): Promise<void> {
    await this.mouse("mouseMoved", a[0], a[1]);
    await this.mouse("mousePressed", a[0], a[1]);
    for (let i = 1; i <= steps; i++) {
      const t = i / steps;
      await this.mouse("mouseMoved", a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, 1);
    }
    await this.mouse("mouseReleased", b[0], b[1]);
  }

  /** Waits for an enabled element matching `q` and clicks its centre. */
  async click(q: Query, within?: number): Promise<number> {
    const id = await until(`${describe(q)} on ${this.name} to be enabled`, async () => {
      const found = await this.all(q, within);
      if (found.length > 1) {
        throw new Error(`${found.length} match: ${found.map((f) => f.name).join(" | ")}`);
      }
      const f = found[0];
      if (f === undefined) return undefined;
      const disabled = f.node.properties?.some(
        (p) => p.name === "disabled" && p.value.value === true,
      );
      return disabled === true ? undefined : f.id;
    });
    const [x, y] = await until(`${describe(q)} on ${this.name} to be shown`, () => this.centre(id));
    await this.clickAt(x, y);
    return id;
  }

  /** Presses a named key (`Enter`, `Tab`, `ArrowDown`…) with optional modifiers. */
  async key(name: string, modifiers = 0): Promise<void> {
    const k = KEYS[name];
    if (k === undefined) {
      throw new Error(`no key definition for ${name}`);
    }
    const base = { key: name, code: k.code, windowsVirtualKeyCode: k.keyCode, modifiers };
    await this.send("Input.dispatchKeyEvent", {
      type: k.text === undefined ? "rawKeyDown" : "keyDown",
      ...base,
      ...(k.text === undefined ? {} : { text: k.text }),
    });
    await this.send("Input.dispatchKeyEvent", { type: "keyUp", ...base });
  }

  /** Types `text` a character at a time, as key presses, into what has focus. */
  async type(text: string): Promise<void> {
    for (const ch of text) {
      const upper = ch.toUpperCase();
      const code = /[a-z]/i.test(ch) ? `Key${upper}` : /[0-9]/.test(ch) ? `Digit${ch}` : "";
      await this.send("Input.dispatchKeyEvent", { type: "keyDown", key: ch, code, text: ch });
      await this.send("Input.dispatchKeyEvent", { type: "keyUp", key: ch, code });
    }
  }

  /** Clicks the field matching `q`, selects what it holds, and types `text` over it. */
  async enter(q: Query, text: string, within?: number): Promise<void> {
    const id = await this.click(q, within);
    await this.call(id, "function () { this.select?.(); }");
    if (text === "") {
      await this.key("Backspace");
    } else {
      await this.type(text);
    }
    const now = await this.value(id);
    if (now !== text) {
      throw new Error(`${describe(q)} holds “${now}” after typing, not what was typed`);
    }
  }

  // ------------------------------------------------------------- reports

  /** The accessibility tree as an outline of roles and names: what the steps can find. */
  async outline(): Promise<string> {
    const { nodes } = (await this.send("Accessibility.getFullAXTree")) as {
      nodes: Array<AxNode & { nodeId: string; parentId?: string; childIds?: string[] }>;
    };
    const byId = new Map(nodes.map((n) => [n.nodeId, n]));
    const lines: string[] = [];
    const walk = (id: string, depth: number): void => {
      const n = byId.get(id);
      if (n === undefined) return;
      const shown =
        !n.ignored && !["none", "generic", "InlineTextBox"].includes(n.role?.value ?? "");
      if (shown) {
        const state = (n.properties ?? [])
          .filter((p) =>
            ["disabled", "pressed", "checked", "selected", "expanded"].includes(p.name),
          )
          .map((p) => `${p.name}=${String(p.value.value)}`)
          .join(" ");
        lines.push(
          `${"  ".repeat(depth)}${n.role?.value ?? "?"} “${n.name?.value ?? ""}”${state ? ` ${state}` : ""}`,
        );
      }
      for (const c of n.childIds ?? []) walk(c, shown ? depth + 1 : depth);
    };
    const top = nodes.find((n) => n.parentId === undefined);
    if (top !== undefined) walk(top.nodeId, 0);
    return lines.join("\n");
  }

  async screenshot(): Promise<Uint8Array> {
    const { data } = (await this.send("Page.captureScreenshot", { format: "png" })) as {
      data: string;
    };
    return new Uint8Array(Buffer.from(data, "base64"));
  }

  async close(): Promise<void> {
    this.off();
    await this.browser
      .send("Target.disposeBrowserContext", { browserContextId: this.context })
      .catch(() => undefined);
  }
}
