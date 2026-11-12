// `cavea-server` as a process of the end-to-end run: started on a port of
// its own, its whole output kept (the log check reads it), stopped by its
// process id with SIGTERM as a supervisor stops it.
import { writeFile } from "node:fs/promises";
import { join } from "node:path";

export const freePort = (): number => {
  const server = Bun.listen({ hostname: "127.0.0.1", port: 0, socket: { data() {} } });
  const port = server.port;
  server.stop(true);
  return port;
};

/** `cavea-server` run as a process of this run, its whole output kept and written to a file. */
export class Server {
  log = "";
  private proc: Bun.Subprocess<"ignore", "pipe", "pipe"> | undefined;
  private drained: Promise<void> = Promise.resolve();

  constructor(
    private readonly binary: string,
    private readonly env: Record<string, string>,
    readonly base: string,
    /** Its working directory; its output goes to `server.log` there. */
    private readonly dir: string,
  ) {}

  /** Starts it and waits for `/readyz`. */
  async start(): Promise<void> {
    const proc = Bun.spawn([this.binary], {
      cwd: this.dir,
      env: { PATH: process.env["PATH"] ?? "", ...this.env },
      stdin: "ignore",
      stdout: "pipe",
      stderr: "pipe",
    });
    this.proc = proc;
    const decoder = new TextDecoder();
    this.drained = Promise.all(
      [proc.stdout, proc.stderr].map(async (stream) => {
        for await (const chunk of stream) {
          this.log += decoder.decode(chunk);
        }
      }),
    ).then(() => writeFile(join(this.dir, "server.log"), this.log));
    const deadline = Date.now() + 30_000;
    for (;;) {
      const ok = await fetch(`${this.base}/readyz`).then(
        (r) => r.status === 200,
        () => false,
      );
      if (ok) return;
      if (proc.exitCode !== null || Date.now() > deadline) {
        // Stopped first: its output ends only when it does.
        await this.stop();
        await this.drained;
        throw new Error(
          `cavea-server did not become ready (exit ${proc.exitCode}):\n${this.log.slice(-2000)}`,
        );
      }
      await Bun.sleep(50);
    }
  }

  /** Stops it with SIGTERM and waits for it (stopped with SIGKILL after 30 s). Its exit code and the seconds it took. */
  async stop(): Promise<{ code: number | null; seconds: number }> {
    const proc = this.proc;
    if (proc === undefined || proc.exitCode !== null) {
      return { code: proc?.exitCode ?? null, seconds: 0 };
    }
    const from = performance.now();
    proc.kill("SIGTERM");
    const exited = await Promise.race([
      proc.exited.then(() => true),
      Bun.sleep(30_000).then(() => false),
    ]);
    if (!exited) {
      proc.kill("SIGKILL");
      await proc.exited;
    }
    await this.drained;
    return { code: exited ? proc.exitCode : null, seconds: (performance.now() - from) / 1000 };
  }
}
