// The demo (index.html) and the benchmark page (bench.html, its cold-load
// frame bench-load.html). The commit is built into the benchmark's results.
import { execSync } from "node:child_process";
import { resolve } from "node:path";
import { defineConfig } from "vite";

const commit = (): string => {
  try {
    const head = execSync("git rev-parse --short=12 HEAD", { encoding: "utf8" }).trim();
    const dirty = execSync("git status --porcelain", { encoding: "utf8" }).trim() !== "";
    return dirty ? `${head}-dirty` : head;
  } catch {
    return "unknown";
  }
};

export default defineConfig({
  define: { CAVEA_COMMIT: JSON.stringify(commit()) },
  build: {
    rollupOptions: {
      input: {
        main: resolve(import.meta.dirname, "index.html"),
        bench: resolve(import.meta.dirname, "bench.html"),
        load: resolve(import.meta.dirname, "bench-load.html"),
      },
    },
  },
});
