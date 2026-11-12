// The starting points on an open chart: when they ask first, and a
// template applied as one edit, through the real module, that undo takes
// back whole.
import { expect, test } from "bun:test";
import { emptyChart } from "./source.ts";
import { offer, templateCommand } from "./startingPoints.ts";
import { TEMPLATES } from "./templates.ts";
import { designer } from "./testkit.ts";

test("never offered to a chart the person cannot change; asks before replacing anything", () => {
  expect(offer(false, null)).toBe("none");
  expect(offer(false, { nodes: 0, unsaved: false })).toBe("none");
  expect(offer(true, null)).toBe("apply");
  expect(offer(true, { nodes: 0, unsaved: false })).toBe("apply");
  expect(offer(true, { nodes: 3, unsaved: false })).toBe("ask");
  // A local chart emptied but not exported still asks: its categories may be the work.
  expect(offer(true, { nodes: 0, unsaved: true })).toBe("ask");
});

const text = (id: string) => Bun.file(new URL(`./templates/${id}.json`, import.meta.url)).text();

/** What a template gives a chart, ids aside: settings, categories, floors,
 * seats and findings. */
const shape = (doc: string) => {
  const d = JSON.parse(doc) as {
    name: string;
    settings: unknown;
    categories: unknown;
    floors: { name: string; level: number }[];
  };
  return {
    settings: d.settings,
    categories: d.categories,
    floors: d.floors.map((f) => [f.name, f.level]),
  };
};

test("each template applies to an empty chart and over another one, as one undo step", async () => {
  const d = await designer();
  for (const t of TEMPLATES) {
    const template = await text(t.id);
    const direct = d.open(template);
    const want = shape(direct.export());
    direct.dispose();
    for (const start of [
      emptyChart("Grand Hall"),
      await text(t.id === "theatre" ? "arena" : "theatre"),
    ]) {
      const s = d.open(start);
      const before = s.export();
      s.apply(templateCommand(before, template));
      for (let i = 0; i < 1000 && !s.validate(1_000_000).done; i++);
      const after = s.export();
      expect([t.id, shape(after)]).toEqual([t.id, want]);
      // The chart keeps its own name.
      expect(JSON.parse(after).name).toBe(JSON.parse(before).name);
      expect([t.id, s.status().seats, s.validation().findings]).toEqual([t.id, t.seats, []]);
      expect(s.undo()).toBe(true);
      // Everything back but the id counter, which never goes back (ids are not reused).
      const { nextOid: _, ...restored } = JSON.parse(s.export()) as Record<string, unknown>;
      const { nextOid: __, ...original } = JSON.parse(before) as Record<string, unknown>;
      expect(restored).toEqual(original);
      s.dispose();
    }
  }
});
