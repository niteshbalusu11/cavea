// The numbering panel's controls against a real session: each control set
// and applied reads back the same through `numbering()`, "Mixed" starts from
// the defaults, and overrides and across-blocks order do what the panel says.
import { beforeAll, describe, expect, test } from "bun:test";
import type { Designer, Session } from "@cavea/designer-core";
import {
  EMPTY,
  ROW_DIRECTIONS,
  ROW_SCHEMES,
  rowsFrom,
  SEAT_DIRECTIONS,
  SEAT_SCHEMES,
  seatsFrom,
  toNumbering,
  type Draft,
  type RowControls,
  type SeatControls,
} from "./numbering.ts";
import { emptyChart } from "./source.ts";
import { designer } from "./testkit.ts";

let d: Designer;
beforeAll(async () => {
  d = await designer();
});

/** Two blocks of 3 rows × 10 seats, both selected. */
function chart(): Session {
  const s = d.open(emptyChart("Numbering"));
  for (const x of [0, 2000]) {
    s.insert(null, {
      kind: "rowBlock",
      origin: [x, 0],
      rows: 3,
      seats: 10,
      seatPitch: 55,
      rowPitch: 95,
    });
  }
  s.selectAll();
  return s;
}

const rows0: RowControls = {
  scheme: "alpha",
  start: "",
  step: 1,
  skip: "",
  direction: "front-to-back",
  prefix: "",
  suffix: "",
  custom: "",
};
const seats0: SeatControls = {
  scheme: "numeric",
  start: "",
  step: 1,
  direction: "ltr",
  continuous: false,
  prefix: "",
  suffix: "",
  custom: "",
};

describe("the numbering panel's controls", () => {
  test("each row control round-trips through numbering()", () => {
    const cases: Partial<RowControls>[] = [
      ...ROW_SCHEMES.map((scheme) =>
        scheme === "custom" ? { scheme, custom: "Front, Middle, Back" } : { scheme },
      ),
      { start: "C" },
      { start: "4", scheme: "numeric" },
      { step: 2 },
      { skip: "I, O" },
      ...ROW_DIRECTIONS.map((direction) => ({ direction })),
      { prefix: "Row ", suffix: "!" },
    ];
    for (const change of cases) {
      const s = chart();
      const rows = { ...rows0, ...change };
      s.number(toNumbering({ ...EMPTY, rows }));
      const view = s.numbering();
      expect([change, rowsFrom(view)]).toEqual([change, rows]);
      expect(view.seats?.scheme).toEqual({ same: "numeric" });
      s.dispose();
    }
  });

  test("each seat control round-trips through numbering()", () => {
    const cases: Partial<SeatControls>[] = [
      ...SEAT_SCHEMES.map((scheme) =>
        scheme === "custom" ? { scheme, custom: "a, b, c, d, e, f, g, h, i, j" } : { scheme },
      ),
      { start: "101" },
      { step: 2 },
      ...SEAT_DIRECTIONS.map((direction) => ({ direction })),
      { continuous: true },
      { prefix: "S", suffix: "-x" },
    ];
    for (const change of cases) {
      const s = chart();
      const seats = { ...seats0, ...change };
      s.number(toNumbering({ ...EMPTY, seats }));
      expect([change, seatsFrom(s.numbering())]).toEqual([change, seats]);
      s.dispose();
    }
  });

  test("After Z is the chart's, and a group left alone keeps each block's own", () => {
    const s = chart();
    s.number(toNumbering({ ...EMPTY, afterZ: "spreadsheet" }));
    expect(s.numbering().afterZ).toBe("spreadsheet");
    // Number only the first block, then select both: its fields differ.
    s.select({ kind: "top" }, [s.selectionView().ids[0] ?? -1]);
    s.number(toNumbering({ ...EMPTY, rows: { ...rows0, scheme: "roman" } }));
    s.selectAll();
    const view = s.numbering();
    expect(view.rows?.scheme).toBe("mixed");
    expect(view.rows?.step).toEqual({ same: 1 });
    // A first touch starts from the defaults where they differ.
    expect(rowsFrom(view).scheme).toBe("alpha");
    s.dispose();
  });

  test("left to right is as seen from the focal point", () => {
    const s = chart();
    s.addFloor("Hall", 0);
    // Behind the blocks (rows run towards +y): they face away from it.
    s.setFocalPoint([1000, 5000]);
    s.selectAll();
    const view = s.numbering();
    expect(view.nodes.map((n) => n.facing)).toEqual(["reversed", "reversed"]);
    s.number(toNumbering({ ...EMPTY, seats: { ...seats0, direction: "ltr" } }));
    expect(seatsFrom(s.numbering()).direction).toBe("ltr");
    const doc = JSON.parse(s.export()) as { nodes: { seatLabels: { direction: string } }[] };
    expect(doc.nodes.map((n) => n.seatLabels.direction)).toEqual(["rtl", "rtl"]);
    s.dispose();
  });

  test("overrides are cleared unless kept, and across continues in the order given", () => {
    const s = chart();
    const [a, b] = s.selectionView().ids;
    // Every row of the first block labelled by hand.
    s.select({ kind: "top" }, [a ?? -1]);
    s.stepIn();
    s.setRowLabel("VIP");
    s.select({ kind: "top" }, [a ?? -1, b ?? -1]);
    expect(s.numbering().nodes.map((n) => n.rowOverrides)).toEqual([3, 0]);
    const kept: Draft = { ...EMPTY, rows: rows0, keepOverrides: true };
    s.number(toNumbering(kept));
    expect(s.numbering().nodes.map((n) => n.rowOverrides)).toEqual([3, 0]);
    s.number(toNumbering({ ...kept, keepOverrides: false }));
    expect(s.numbering().nodes.map((n) => n.rowOverrides)).toEqual([0, 0]);

    // Across, second block first: it starts at 1, the first after its 10.
    const across: Draft = { ...EMPTY, seats: seats0, across: [b ?? -1, a ?? -1] };
    const preview = s.previewNumbering(toNumbering(across));
    const first = (node: number | undefined) =>
      preview.rows.find((r) => r.node === node)?.seats[0]?.label;
    expect([first(b), first(a)]).toEqual(["1", "11"]);
    s.dispose();
  });
});
