// What every published package's `dist/package.json` says about where it
// comes from and under which licence (D-91), shared by the packages' build
// scripts. The licence is the owner's legal document (12 §8): `LICENSE` at
// the repository's root. Until it exists each package is `private`, so npm
// refuses to publish it, and `cargo xtask release` refuses first, saying why.
import { copyFile, readFile } from "node:fs/promises";
import { join } from "node:path";

const LICENCE = join(import.meta.dir, "../LICENSE");

/** The fields for `dist/package.json`, from the package's own `package.json` in `dir`. */
export async function publishFields(dir: string): Promise<Record<string, unknown>> {
  const source = JSON.parse(await readFile(join(dir, "package.json"), "utf8")) as {
    repository?: unknown;
  };
  const licensed = await Bun.file(LICENCE).exists();
  return {
    private: !licensed,
    license: licensed ? "SEE LICENSE IN LICENSE" : "UNLICENSED",
    // npm provenance checks this against the repository that built it.
    repository: source.repository,
    publishConfig: { access: "public" },
  };
}

/** Copies the licence into `dist`, when there is one. */
export async function copyLicence(dist: string): Promise<void> {
  if (await Bun.file(LICENCE).exists()) {
    await copyFile(LICENCE, join(dist, "LICENSE"));
  }
}
