// Each template opens through the real module and is valid after a full
// validation pass: no errors and no warnings (07 §16).
import { expect, test } from "bun:test";
import { TEMPLATES } from "./templates.ts";
import { designer } from "./testkit.ts";

test("each template opens clean, with the seats its card says", async () => {
  const d = await designer();
  for (const t of TEMPLATES) {
    const text = await Bun.file(new URL(`./templates/${t.id}.json`, import.meta.url)).text();
    const s = d.open(text);
    for (let i = 0; i < 1000 && !s.validate(1_000_000).done; i++);
    const v = s.validation();
    expect([t.id, v.complete, v.publishable, v.findings]).toEqual([t.id, true, true, []]);
    expect([t.id, s.status().seats]).toEqual([t.id, t.seats]);
    s.dispose();
  }
});
