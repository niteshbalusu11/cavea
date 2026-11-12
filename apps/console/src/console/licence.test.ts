// The licence in the console's words (12 §7.3, §7.6): every reason the
// server can give has a message, and who sees which banner.
import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import type { ConsoleLicence } from "../api/generated.ts";
import type { Permission } from "../api/permissions.ts";
import { msgOf } from "../i18n/messages.ts";
import { bannerFor, graceEnd, licenceDate } from "./licence.ts";

const CRATES = join(import.meta.dir, "../../../../crates");

test("every reason the server can give for an invalid licence has a message", () => {
  // `cavea_licence::Invalid::as_str`, and the reasons the server adds (`Found::Invalid`).
  const lib = readFileSync(join(CRATES, "cavea-licence/src/lib.rs"), "utf8");
  const asStr = /fn as_str\(self\) -> &'static str \{([\s\S]*?)\n {4}\}/.exec(lib)?.[1] ?? "";
  const server = readFileSync(join(CRATES, "cavea-server/src/licence.rs"), "utf8");
  const reasons = new Set([
    ...[...asStr.matchAll(/=> "(\w+)"/g)].map((m) => m[1] ?? ""),
    ...[...server.matchAll(/Found::Invalid\("(\w+)"\)/g)].map((m) => m[1] ?? ""),
  ]);
  expect(reasons.size).toBeGreaterThanOrEqual(9);
  for (const reason of reasons) {
    expect([reason, msgOf("licence.reason", reason) !== null]).toEqual([reason, true]);
  }
});

const licence = (state: ConsoleLicence["state"], manage = false): ConsoleLicence => ({
  state,
  manage,
  read_only: state === "expired",
  grace_until: state === "grace" ? "2027-02-09" : null,
  reason: null,
  source: null,
  licence: null,
});
const OWNER: Permission[] = ["charts.write", "charts.archive", "events.write", "settings"];
const ADMIN = OWNER;
const EDITOR: Permission[] = ["charts.read", "charts.write", "events.read"];
const OPERATOR: Permission[] = ["charts.read", "events.read", "seats.write"];
const VIEWER: Permission[] = ["charts.read", "events.read"];

test("the banner each person sees in each state (12 §7.3's console column)", () => {
  const people: Array<[string, Permission[], boolean]> = [
    ["server owner", OWNER, true],
    ["admin", ADMIN, false],
    ["editor", EDITOR, false],
    ["operator", OPERATOR, false],
    ["viewer", VIEWER, false],
  ];
  const table = Object.fromEntries(
    people.map(([who, permissions, manage]) => [
      who,
      (["valid", "grace", "expired", "missing", "invalid"] as const).map((state) =>
        bannerFor(licence(state, manage), permissions),
      ),
    ]),
  );
  expect(table).toEqual({
    "server owner": [null, "grace", "expired", "development", "development"],
    admin: [null, "grace", "expired", "development", "development"],
    editor: [null, null, "expired", "development", "development"],
    operator: [null, null, null, "development", "development"],
    viewer: [null, null, null, "development", "development"],
  });
});

test("licence dates are UTC days, in the person's locale", () => {
  expect(licenceDate("2028-01-10", "en-GB")).toBe("10 January 2028");
  expect(licenceDate("2028-01-10", "en-US")).toBe("January 10, 2028");
  expect(licenceDate("not a date", "en")).toBe("not a date");
  expect(graceEnd("2028-01-10", 30)).toBe("2028-02-09");
  expect(graceEnd("2028-02-28", 1)).toBe("2028-02-29");
});
