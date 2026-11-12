// `bun run test` scans the whole tree except `bunfig.toml`'s
// `[test].pathIgnorePatterns`. A tracked test file one of those patterns
// matched would silently never run: this fails when there is one.
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
  expect(tracked.filter((file) => globs.some((g) => g.match(file)))).toEqual([]);
});
