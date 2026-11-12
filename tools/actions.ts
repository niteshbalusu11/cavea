// The GitHub Actions the workflows use, pinned by full commit SHA with the
// version in a comment (12 §3 T15, D-85). There is no dependency bot: the
// releaser runs this at each release, and whoever touches a workflow may.
//
//   bun tools/actions.ts --check   fails unless every `uses:` is a 40-digit SHA with `# vX.Y.Z`
//   bun tools/actions.ts           lists each action's pin and the newest release of its major (needs `gh`)
//   bun tools/actions.ts --update  rewrites each pin to that newest release, in place; review the diff
//
// The newest release is the newest `vX.Y.Z` tag with the pinned major (a new
// major is reported, never taken: it may change what the step does).
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const DIR = join(import.meta.dir, "../.github/workflows");
const USES = /^(\s*-?\s*uses:\s*)([\w.-]+\/[\w./-]+)@(\S+)(.*)$/;
const PINNED = /^[0-9a-f]{40}$/;
const VERSION = /^\s*#\s*(v\d+\.\d+\.\d+)\s*$/;

const args = process.argv.slice(2);
const check = args.includes("--check");
const update = args.includes("--update");

interface Use {
  file: string;
  line: number;
  action: string;
  ref: string;
  version: string | undefined;
}

const files = readdirSync(DIR)
  .filter((f) => f.endsWith(".yml") || f.endsWith(".yaml"))
  .sort();
const uses: Use[] = [];
for (const file of files) {
  readFileSync(join(DIR, file), "utf8")
    .split("\n")
    .forEach((text, i) => {
      const m = USES.exec(text);
      if (m) {
        uses.push({
          file,
          line: i + 1,
          action: m[2]!,
          ref: m[3]!,
          version: VERSION.exec(m[4] ?? "")?.[1],
        });
      }
    });
}

const loose = uses.filter((u) => !PINNED.test(u.ref) || u.version === undefined);
if (check) {
  for (const u of loose) {
    console.error(
      `${u.file}:${u.line}: ${u.action}@${u.ref} is not pinned by commit SHA with "# vX.Y.Z"`,
    );
  }
  if (loose.length > 0) process.exit(1);
  console.log(`${uses.length} action uses in ${files.length} workflows, each pinned by commit SHA`);
  process.exit(0);
}

function gh(path: string): string {
  const run = Bun.spawnSync(["gh", "api", "--paginate", path, "-q", ".[] | .name"], {
    stderr: "pipe",
  });
  if (run.exitCode !== 0) throw new Error(`gh api ${path}: ${run.stderr.toString().trim()}`);
  return run.stdout.toString();
}

function sha(repo: string, tag: string): string {
  const run = Bun.spawnSync(["gh", "api", `repos/${repo}/commits/${tag}`, "-q", ".sha"], {
    stderr: "pipe",
  });
  if (run.exitCode !== 0) throw new Error(`gh api commits/${tag}: ${run.stderr.toString().trim()}`);
  return run.stdout.toString().trim();
}

const semver = (v: string): number[] => v.slice(1).split(".").map(Number);
const newer = (a: string, b: string): boolean => {
  const [x, y] = [semver(a), semver(b)];
  for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return (x[i] ?? 0) > (y[i] ?? 0);
  return false;
};

const latest = new Map<string, { tag: string; sha: string; major: string | undefined }>();
for (const u of uses) {
  if (latest.has(u.action) || u.version === undefined) continue;
  // `owner/repo/path` actions live in `owner/repo`.
  const repo = u.action.split("/").slice(0, 2).join("/");
  const major = u.version.split(".")[0]!;
  const tags = gh(`repos/${repo}/tags?per_page=100`)
    .split("\n")
    .filter((t) => /^v\d+\.\d+\.\d+$/.test(t));
  const same = tags.filter((t) => t.startsWith(`${major}.`)).reduce((a, b) => (newer(b, a) ? b : a), u.version);
  const top = tags.reduce((a, b) => (newer(b, a) ? b : a), u.version);
  latest.set(u.action, {
    tag: same,
    sha: same === u.version ? u.ref : sha(repo, same),
    major: top.split(".")[0] === major ? undefined : top,
  });
}

for (const u of uses) {
  const l = latest.get(u.action);
  const note = l === undefined ? "not pinned" : l.tag === u.version ? "newest" : `${l.tag} available`;
  const major = l?.major ? `; new major ${l.major} (not taken)` : "";
  console.log(`${u.file}:${u.line} ${u.action} ${u.version ?? u.ref}: ${note}${major}`);
}

if (update) {
  for (const file of files) {
    const path = join(DIR, file);
    const text = readFileSync(path, "utf8")
      .split("\n")
      .map((line) => {
        const m = USES.exec(line);
        const l = m ? latest.get(m[2]!) : undefined;
        return m && l ? `${m[1]}${m[2]}@${l.sha} # ${l.tag}` : line;
      })
      .join("\n");
    writeFileSync(path, text);
  }
  console.log("rewritten: review `git diff .github/workflows`");
}
