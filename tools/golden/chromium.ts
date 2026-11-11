// Headless Chromium over the DevTools protocol, for the renderer's golden
// images (docs/13-testing-benchmarks.md §6.1): the flake's Chromium and fonts
// (`nix build .#chromium .#golden-fonts`, pinned by flake.lock), WebGL2 on
// SwiftShader (software rendering), and a fontconfig that sees the pinned
// fonts only, so the text the page draws does not depend on the machine's
// fonts. Launched as `fixtures/run.ts` launches it; stopped by process id.
import { mkdir, rm, symlink, writeFile } from "node:fs/promises";
import { join } from "node:path";

type Message = {
  id?: number;
  method?: string;
  params?: Record<string, unknown>;
  sessionId?: string;
  result?: Record<string, unknown>;
  error?: { message: string };
};

/** Runs `cmd` and returns its standard output; throws with its output on failure. */
export async function sh(cmd: string[], cwd?: string): Promise<string> {
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

/** The faces of the golden fonts the browser sees. */
const FACES = ["DejaVuSans.ttf", "DejaVuSans-Bold.ttf"];

export class Browser {
  private id = 0;
  private readonly pending = new Map<number, (m: Message) => void>();
  private readonly exceptions: string[] = [];

  private constructor(
    private readonly ws: WebSocket,
    private readonly proc: Bun.Subprocess,
    private readonly scratch: string,
    readonly version: string,
  ) {
    ws.addEventListener("message", (event) => {
      const message = JSON.parse(String(event.data)) as Message;
      if (message.id !== undefined) {
        this.pending.get(message.id)?.(message);
        this.pending.delete(message.id);
      } else if (message.method === "Runtime.exceptionThrown") {
        this.exceptions.push(JSON.stringify(message.params).slice(0, 500));
      }
    });
  }

  /**
   * Starts `chromium` headless with a fresh profile and a fontconfig that
   * sees two faces of `fonts` only, DejaVu Sans and its bold, so every
   * family a page names, and every fallback, is one of them. Its sockets need a short path: the worktree's is
   * too long, so a directory under /tmp named by this process, removed at `close`.
   */
  static async launch(chromium: string, fonts: string): Promise<Browser> {
    const scratch = `/tmp/cavea-golden-${process.pid}`;
    await mkdir(scratch, { recursive: true });
    const faces = join(scratch, "fonts");
    await mkdir(faces, { recursive: true });
    for (const face of FACES) {
      await symlink(join(fonts, "share/fonts/truetype", face), join(faces, face));
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
    const proc = Bun.spawn(
      [
        chromium,
        "--headless",
        "--no-sandbox",
        "--remote-debugging-port=0",
        `--user-data-dir=${join(scratch, "profile")}`,
        "--use-angle=swiftshader",
        "--enable-unsafe-swiftshader",
        "--no-first-run",
        "--disable-lcd-text",
        "--force-color-profile=srgb",
        "about:blank",
      ],
      {
        env: { ...process.env, TMPDIR: scratch, FONTCONFIG_FILE: fontconfig },
        stdin: "ignore",
        stdout: "ignore",
        stderr: "pipe",
      },
    );
    let output = "";
    const decoder = new TextDecoder();
    const reader = proc.stderr.getReader();
    let url: string | undefined;
    for (const deadline = Date.now() + 30_000; url === undefined && Date.now() < deadline;) {
      const { value, done } = await reader.read();
      if (done) {
        break;
      }
      output += decoder.decode(value);
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
      proc.kill();
      await rm(scratch, { recursive: true, force: true });
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
  }

  send(
    method: string,
    params: Record<string, unknown> = {},
    sessionId?: string,
  ): Promise<Record<string, unknown>> {
    const id = ++this.id;
    this.ws.send(
      JSON.stringify({ id, method, params, ...(sessionId === undefined ? {} : { sessionId }) }),
    );
    return new Promise((resolve, reject) => {
      this.pending.set(id, (m) =>
        m.error === undefined
          ? resolve(m.result ?? {})
          : reject(new Error(`${method}: ${m.error.message}`)),
      );
    });
  }

  /** Opens `url` in a fresh browser context and returns a session on it once loaded. */
  async open(url: string): Promise<Page> {
    const { browserContextId } = await this.send("Target.createBrowserContext");
    const { targetId } = await this.send("Target.createTarget", {
      url: "about:blank",
      browserContextId,
    });
    const { sessionId } = await this.send("Target.attachToTarget", { targetId, flatten: true });
    const page = new Page(this, String(sessionId), String(browserContextId));
    await this.send("Runtime.enable", {}, page.session);
    await this.send("Page.enable", {}, page.session);
    await this.send("Page.navigate", { url }, page.session);
    return page;
  }

  /** Uncaught page exceptions so far. */
  errors(): string[] {
    return [...this.exceptions];
  }

  async close(): Promise<void> {
    this.ws.close();
    this.proc.kill();
    await this.proc.exited;
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
