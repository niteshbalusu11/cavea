// The changelog's source (docs/01-architecture.md §6): the titles of the
// pull requests merged since the last release, which are the subjects of
// `main`'s first-parent commits (squash merges, each ending in `(#N)`).
//
//   bun tools/changelog.ts [ref]          the next release's entries, as Markdown
//   bun tools/changelog.ts --check [ref]  fails if one of them is not a merged pull request
//   --exclude <tag>                       ignores that tag (the release being made, already
//                                         on `ref`), so "since" is the one before it
//
// `ref` defaults to `origin/main`. "Since the last release" is since the
// newest `v*` tag reachable from `ref`, or, before the first release, since
// the commit that added `CHANGELOG.md` (its first section covers the rest).
// The releaser condenses the list into the version's section by hand.
const args = process.argv.slice(2);
const check = args.includes("--check");
const excludeAt = args.indexOf("--exclude");
const exclude = excludeAt === -1 ? undefined : args[excludeAt + 1];
const ref =
  args.find((a, i) => !a.startsWith("--") && (excludeAt === -1 || i !== excludeAt + 1)) ??
  "origin/main";

const git = (...argv: string[]): string => {
  const run = Bun.spawnSync(["git", ...argv], { stderr: "pipe" });
  if (run.exitCode !== 0) {
    throw new Error(`git ${argv.join(" ")}: ${run.stderr.toString().trim()}`);
  }
  return run.stdout.toString().trim();
};

const tag = Bun.spawnSync([
  "git",
  "describe",
  "--tags",
  "--abbrev=0",
  "--match",
  "v*",
  ...(exclude === undefined ? [] : ["--exclude", exclude]),
  ref,
]);
const since =
  tag.exitCode === 0
    ? tag.stdout.toString().trim()
    : git("log", "--diff-filter=A", "--format=%H", ref, "--", "CHANGELOG.md").split("\n").at(-1);
if (since === undefined || since === "") {
  throw new Error(`no release tag and no CHANGELOG.md on ${ref}`);
}
const subjects = git("log", "--first-parent", "--format=%s", `${since}..${ref}`)
  .split("\n")
  .filter((s) => s !== "");
const strays = subjects.filter((s) => !/ \(#\d+\)$/.test(s));
if (check) {
  if (strays.length > 0) {
    console.error(
      `not squash-merged pull requests, so missing from the changelog:\n${strays.join("\n")}`,
    );
    process.exit(1);
  }
  console.log(`${subjects.length} pull requests since ${since.slice(0, 12)}, each with its title`);
} else {
  console.log(subjects.map((s) => `- ${s}`).join("\n"));
}
