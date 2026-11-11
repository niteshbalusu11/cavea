// Serves the canvas proof page (./index.html) with Bun's own bundler: the
// designer module from ../wasm (`cargo xtask build-designer` first), the
// glyph atlas from @cavea/renderer, and each preset's draft from `cavea gen`.
//
//   bun packages/designer-core/proof/serve.ts --port <port>
//
// then open http://127.0.0.1:<port>/?preset=stadium-100k. Port 0 (the
// default) picks a free one; the URL is printed.
import page from "./index.html";

const root = new URL("../../../", import.meta.url);
const arg = process.argv.indexOf("--port");
const port = arg >= 0 ? Number(process.argv[arg + 1]) : 0;
const drafts = new Map<string, string>();

function draft(name: string): Response {
  if (!/^[a-z0-9-]+$/.test(name)) {
    return new Response("bad preset", { status: 400 });
  }
  let text = drafts.get(name);
  if (text === undefined) {
    const run = Bun.spawnSync(
      ["cargo", "run", "--release", "-q", "-p", "cavea-cli", "--", "gen", name],
      { cwd: root.pathname, env: { ...process.env, CARGO_BUILD_JOBS: "8" } },
    );
    if (!run.success) {
      return new Response(run.stderr.toString(), { status: 404 });
    }
    text = run.stdout.toString();
    drafts.set(name, text);
  }
  return new Response(text, { headers: { "content-type": "application/json" } });
}

const file = (path: string, type: string) => () =>
  new Response(Bun.file(new URL(path, root)), { headers: { "content-type": type } });

const server = Bun.serve({
  hostname: "127.0.0.1",
  port,
  routes: {
    "/": page,
    "/wasm/cavea_designer.js": file("packages/designer-core/wasm/cavea_designer.js", "text/javascript"),
    "/wasm/cavea_designer_bg.wasm": file(
      "packages/designer-core/wasm/cavea_designer_bg.wasm",
      "application/wasm",
    ),
    "/glyphs.png": file("packages/renderer/assets/glyphs.png", "image/png"),
    "/draft/:name": (req) => draft(req.params.name),
  },
});
console.log(`proof page: ${server.url.href}?preset=theatre`);
