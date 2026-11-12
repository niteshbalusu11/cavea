// `bun run test` scans the whole tree except `bunfig.toml`'s
// `[test].pathIgnorePatterns` and what bun itself skips: any directory whose
// name starts with a dot, and `node_modules`. A tracked test file in one of
// those would silently never run: this fails when there is one.
import { expect, test } from "bun:test";
import { join } from "node:path";
import config from "../bunfig.toml";

test("bun run test skips no tracked test file", () => {
  const patterns: string[] = config.test.pathIgnorePatterns;
  const globs = patterns.map((p) => new Bun.Glob(p));
  const ls = Bun.spawnSync(["git", "ls-files", "-z", "*.test.ts", "*.test.tsx"], {
    cwd: join(import.meta.dir, ".."),
  });
  expect(ls.exitCode).toBe(0);
  const tracked = ls.stdout.toString().split("\0").filter(Boolean);
  expect(tracked).toContain("tools/tests.test.ts");
  const skipped = (file: string) =>
    globs.some((g) => g.match(file)) ||
    file
      .split("/")
      .slice(0, -1)
      .some((dir) => dir.startsWith(".") || dir === "node_modules");
  expect(tracked.filter(skipped)).toEqual([]);
});
