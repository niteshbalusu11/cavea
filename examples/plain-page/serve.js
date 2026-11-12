// Serves this directory on http://localhost:8000, for trying the page: any
// static web server does the same. It has no checkout: /api/checkout is
// answered by your backend (see the Node backend example).
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize } from "node:path";

const TYPES = { ".html": "text/html", ".js": "text/javascript", ".wasm": "application/wasm", ".png": "image/png" };
const root = import.meta.dirname;
const port = Number(process.env.PORT ?? 8000);

createServer(async (req, res) => {
  const path = normalize(new URL(req.url ?? "/", "http://localhost").pathname).replace(/^\/+/, "");
  const file = path === "" ? "index.html" : path;
  try {
    const body = await readFile(join(root, file));
    res.writeHead(200, { "content-type": TYPES[extname(file)] ?? "application/octet-stream" });
    res.end(body);
  } catch {
    res.writeHead(404, { "content-type": "text/plain" });
    res.end("not found");
  }
}).listen(port, () => console.log(`http://localhost:${port}`));
