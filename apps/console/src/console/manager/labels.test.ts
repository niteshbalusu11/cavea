// The label template's rule against the Rust implementation's own answers
// (`fixtures/label-cases.json`, made by `CAVEA_BLESS=1 cargo test -p
// cavea-core template`).
import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { compose } from "./labels.ts";

test("every full label composes as cavea_core::template does", () => {
  const table = JSON.parse(
    readFileSync(new URL("fixtures/label-cases.json", import.meta.url), "utf8"),
  ) as { cases: Array<[string, string, string, string, string]> };
  expect(table.cases.length).toBeGreaterThan(200);
  for (const [template, section, row, seat, label] of table.cases) {
    expect([template, section, row, seat, compose(template, section, row, seat)]).toEqual([
      template,
      section,
      row,
      seat,
      label,
    ]);
  }
});
