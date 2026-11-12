// Headless Chromium over the DevTools protocol, shared by the renderer's
// golden images (tools/golden, docs/13-testing-benchmarks.md §6.1), the
// compatibility checks (fixtures/run.ts, §8) and the end-to-end check
// (tools/e2e, §9). WebGL2 on SwiftShader (software rendering). With `fonts`,
// a fontconfig that sees the pinned fonts only, so the text a page draws does
// not depend on the machine's fonts. Stopped by process id, never by name.
import { mkdir, rm, symlink, writeFile } from "node:fs/promises";
import { join } from "node:path";

/** One message of the protocol: a reply (`id`) or an event (`method`). */
export type Message = {
  id?: number;
  method?: string;
  params?: Record<string, unknown>;
  sessionId?: string;
  result?: Record<string, unknown>;
  error?: { message: string };
};

/** Runs `cmd` and returns its standard output; throws with its output on failure. */
export async function sh(cmd: string[], cwd = process.cwd()): Promise<string> {
  const proc = Bun.spawn(cmd, { cwd, stdout: "pipe", stderr: "pipe" });
  const [out, err, code] = await Promise.all([
    new Response(proc.stdout).text(),
    new Response(proc.stderr).text(),
    proc.exited,
  ]);
  if (code !== 0) {
    throw new Error(
      `${cmd.join(" ")} exited with ${code}:\n${`${out}\n${err}`.trim().slice(-2000)}`,
    );
  }
  return out;
}

/** The flake's Chromium and golden fonts: store paths, built or fetched on first use. */
export async function pinned(root: string): Promise<{ chromium: string; fonts: string }> {
  const out = await sh(
    ["nix", "build", "--no-link", "--print-out-paths", ".#chromium", ".#golden-fonts"],
    root,
  );
  const [chromium = "", fonts = ""] = out.trim().split("\n");
  return { chromium: join(chromium, "bin/chromium"), fonts };
}

/** The longest a DevTools request may go unanswered. */
const REQUEST_MS = 60_000;

/** The faces of the golden fonts the browser sees. */
const FACES = ["DejaVuSans.ttf", "DejaVuSans-Bold.ttf"];

export interface LaunchOptions {
  /** The browser's executable. */
  chromium: string;
  /**
   * Its scratch directory is `/tmp/<name>-<pid>`: the sockets need a short
   * path, and a worktree's is too long. Removed at `close`.
   */
  name: string;
  /** The golden fonts' store path: the browser then sees their two faces only. */
  fonts?: string;
  /** The profile; default inside the scratch directory (removed with it). */
  profile?: string;
  /** A visible window instead of headless (needs a display). */
  headed?: boolean;
  /** More command-line switches. */
  args?: string[];
  /**
   * Called with the process as soon as it is spawned, before the launch
   * finishes, so a caller interrupted meanwhile can stop it by its id.
   */
  spawned?: (proc: Bun.Subprocess) => void;
}

export class Browser {
  private id = 0;
  private readonly pending = new Map<number, (m: Message) => void>();
  /** Why the DevTools connection ended, once it has: every request then fails. */
  private gone: string | undefined;
  private readonly listeners = new Set<(m: Message) => void>();
  private readonly exceptions: string[] = [];

  private constructor(
    private readonly ws: WebSocket,
    private readonly proc: Bun.Subprocess,
    private readonly scratch: string,
    readonly version: string,
  ) {
    // A browser that dies, or a connection that drops, fails what waits on
    // it instead of leaving it waiting.
    const end = (why: string): void => {
      this.gone ??= why;
      for (const [id, settle] of this.pending) {
        this.pending.delete(id);
        settle({ id, error: { message: `the DevTools connection ${why}` } });
      }
    };
    ws.addEventListener("close", () => end("closed"));
    ws.addEventListener("error", () => end("failed"));
    ws.addEventListener("message", (event) => {
      const message = JSON.parse(String(event.data)) as Message;
      if (message.id !== undefined) {
        this.pending.get(message.id)?.(message);
        this.pending.delete(message.id);
        return;
      }
      if (message.method === "Runtime.exceptionThrown") {
        this.exceptions.push(JSON.stringify(message.params).slice(0, 500));
      }
      for (const listener of this.listeners) {
        listener(message);
      }
    });
  }

  /**
   * Starts the browser with a fresh profile and waits for its DevTools
   * endpoint. With `fonts`, a fontconfig that sees two faces of them only,
   * DejaVu Sans and its bold, so every family a page names, and every
   * fallback, is one of them.
   */
  static async launch(options: LaunchOptions): Promise<Browser> {
    const scratch = `/tmp/${options.name}-${process.pid}`;
    await mkdir(scratch, { recursive: true });
    const env: Record<string, string | undefined> = { ...process.env, TMPDIR: scratch };
    if (options.fonts !== undefined) {
      const faces = join(scratch, "fonts");
      await mkdir(faces, { recursive: true });
      for (const face of FACES) {
        await symlink(join(options.fonts, "share/fonts/truetype", face), join(faces, face));
      }
      const fontconfig = join(scratch, "fonts.conf");
      await writeFile(
        fontconfig,
        `<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "fonts.dtd">
<fontconfig>
  <dir>${faces}</dir>
  <cachedir>${scratch}/fontcache</cachedir>
  <config></config>
</fontconfig>
`,
      );
      env["FONTCONFIG_FILE"] = fontconfig;
    }
    const proc = Bun.spawn(
      [
        options.chromium,
        ...(options.headed === true ? [] : ["--headless"]),
        "--no-sandbox",
        "--remote-debugging-port=0",
        `--user-data-dir=${options.profile ?? join(scratch, "profile")}`,
        "--use-angle=swiftshader",
        "--enable-unsafe-swiftshader",
        "--no-first-run",
        ...(options.args ?? []),
        "about:blank",
      ],
      { env, stdin: "ignore", stdout: "ignore", stderr: "pipe" },
    );
    options.spawned?.(proc);
    try {
      let output = "";
      const decoder = new TextDecoder();
      const reader = proc.stderr.getReader();
      let url: string | undefined;
      // A browser that stays up without announcing its endpoint must not
      // hold the caller: each read is raced against the deadline.
      const deadline = Date.now() + 30_000;
      while (url === undefined) {
        const left = deadline - Date.now();
        let timer: ReturnType<typeof setTimeout> | undefined;
        const read =
          left <= 0
            ? null
            : await Promise.race([
                reader.read(),
                new Promise<null>((resolve) => {
                  timer = setTimeout(() => resolve(null), left);
                }),
              ]);
        // Cleared, so a pending timer never keeps the caller's process alive.
        clearTimeout(timer);
        if (read === null || read.done) {
          break;
        }
        output += decoder.decode(read.value);
        url = output.match(/DevTools listening on (ws:\/\/\S+)/)?.[1];
      }
      // Drained from here on, so a chatty browser never blocks on a full pipe.
      void (async () => {
        for (;;) {
          const { done } = await reader.read();
          if (done) {
            break;
          }
        }
      })();
      if (url === undefined) {
        throw new Error(`Chromium did not start:\n${output.slice(-1000)}`);
      }
      const ws = new WebSocket(url);
      await new Promise((resolve, reject) => {
        ws.addEventListener("open", resolve);
        ws.addEventListener("error", reject);
      });
      const browser = new Browser(ws, proc, scratch, "");
      const version = String((await browser.send("Browser.getVersion"))["product"]);
      return Object.assign(browser, { version });
    } catch (error) {
      // A launch that fails part-way leaves nothing running or behind.
      proc.kill("SIGKILL");
      await proc.exited;
      await rm(scratch, { recursive: true, force: true });
      throw error;
    }
  }

  /** The browser's process id, for a caller that must stop it on an abrupt exit. */
  get pid(): number {
    return this.proc.pid;
  }

  send(
    method: string,
    params: Record<string, unknown> = {},
    sessionId?: string,
  ): Promise<Record<string, unknown>> {
    if (this.gone !== undefined) {
      return Promise.reject(new Error(`${method}: the DevTools connection ${this.gone}`));
    }
    const id = ++this.id;
    this.ws.send(
      JSON.stringify({ id, method, params, ...(sessionId === undefined ? {} : { sessionId }) }),
    );
    return new Promise((resolve, reject) => {
      // No request of these checks takes a minute: one that does is a hung browser.
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${method}: no answer from the browser in ${REQUEST_MS / 1000} s`));
      }, REQUEST_MS);
      this.pending.set(id, (m) => {
        clearTimeout(timer);
        if (m.error === undefined) resolve(m.result ?? {});
        else reject(new Error(`${method}: ${m.error.message}`));
      });
    });
  }

  /** Calls `listener` with every event (not replies) until the returned function is called. */
  on(listener: (m: Message) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /** A fresh browser context (its own cookies and storage) with one blank page attached. */
  async context(): Promise<{ session: string; context: string; target: string }> {
    const { browserContextId } = await this.send("Target.createBrowserContext");
    const { targetId } = await this.send("Target.createTarget", {
      url: "about:blank",
      browserContextId,
    });
    const { sessionId } = await this.send("Target.attachToTarget", { targetId, flatten: true });
    return {
      session: String(sessionId),
      context: String(browserContextId),
      target: String(targetId),
    };
  }

  /** Opens `url` in a fresh browser context and returns a session on it once loaded. */
  async open(url: string): Promise<Page> {
    const { session, context } = await this.context();
    const page = new Page(this, session, context);
    await this.send("Runtime.enable", {}, page.session);
    await this.send("Page.enable", {}, page.session);
    await this.send("Page.navigate", { url }, page.session);
    return page;
  }

  /** Uncaught page exceptions so far. */
  errors(): string[] {
    return [...this.exceptions];
  }

  /**
   * Asks the browser to close (so its helper processes finish writing the
   * profile), stops it by process id if it has not gone within 5 s, then
   * removes its directory.
   */
  async close(): Promise<void> {
    await Promise.race([this.send("Browser.close").catch(() => undefined), Bun.sleep(5000)]);
    const exited = await Promise.race([
      this.proc.exited.then(() => true),
      Bun.sleep(5000).then(() => false),
    ]);
    if (!exited) {
      this.proc.kill();
      await this.proc.exited;
    }
    this.ws.close();
    await rm(this.scratch, { recursive: true, force: true });
  }
}

export class Page {
  constructor(
    private readonly browser: Browser,
    readonly session: string,
    private readonly context: string,
  ) {}

  /** Evaluates `expression` in the page, awaiting a promise, and returns its value. */
  async evaluate<T>(expression: string): Promise<T> {
    const reply = await this.browser.send(
      "Runtime.evaluate",
      { expression, awaitPromise: true, returnByValue: true },
      this.session,
    );
    const details = reply["exceptionDetails"] as
      | { exception?: { description?: string }; text?: string }
      | undefined;
    if (details !== undefined) {
      throw new Error(details.exception?.description ?? details.text ?? "evaluation failed");
    }
    return (reply["result"] as { value: T }).value;
  }

  /** Waits until `expression` is truthy in the page, or fails after `ms`. */
  async waitFor(expression: string, ms = 30_000): Promise<void> {
    for (const deadline = Date.now() + ms; Date.now() < deadline; await Bun.sleep(50)) {
      if (await this.evaluate<boolean>(`Boolean(${expression})`).catch(() => false)) {
        return;
      }
    }
    throw new Error(`timed out waiting for ${expression}`);
  }

  async close(): Promise<void> {
    await this.browser.send("Target.disposeBrowserContext", { browserContextId: this.context });
  }
}
