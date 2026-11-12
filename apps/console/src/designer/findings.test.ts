// Every finding in a person's words (07 §10): each kind of particulars the
// edit core sends has a sentence with every placeholder filled, and on real
// charts the sentences name objects by their names and rows and seats by
// their labels, never by an internal id.
import { beforeAll, expect, test } from "bun:test";
import type { Designer, FindingDetail, FindingView, Session } from "@cavea/designer-core";
import type { Finding } from "../api/generated.ts";
import { msg } from "../i18n/messages.ts";
import { findingText, namesPlace, refused } from "./findings.ts";
import { designer } from "./testkit.ts";

let d: Designer;
beforeAll(async () => {
  d = await designer();
});

type Shape = { union?: Shape[]; object?: { name: string; type: unknown }[] };

/** One sample of each `FindingDetail` variant, from the generated schema:
 * so a variant the edit core gains without a sentence fails here. */
async function variants(): Promise<FindingDetail[]> {
  const schema = (await Bun.file(
    new URL("../../../../packages/designer-core/src/generated/schema.json", import.meta.url),
  ).json()) as { types: Record<string, { type: Shape }> };
  const union = schema.types["FindingDetail"]?.type.union ?? [];
  const out: FindingDetail[] = [];
  for (const v of union) {
    for (const flag of [false, true]) {
      const sample: Record<string, unknown> = {};
      for (const f of v.object ?? []) {
        const t = f.type as unknown;
        sample[f.name] =
          typeof t === "object" && t !== null && "literal" in t
            ? (t as { literal: string }).literal
            : t === "number"
              ? 3
              : t === "boolean"
                ? flag
                : t === "string" || !flag
                  ? "Balcony"
                  : null;
      }
      out.push(sample as FindingDetail);
    }
  }
  return out;
}

const view = (detail: FindingDetail, related: number | null): FindingView => ({
  code: "E001",
  severity: "error",
  node: 12,
  related,
  message: "node 12: compiler words",
  row: 1013,
  seat: 1020,
  stale: false,
  name: "Row block 1",
  kind: "rowBlock",
  relatedName: related === null ? null : "Row block 2",
  relatedKind: related === null ? null : "rowBlock",
  detail,
});

/** What no sentence may hold: a placeholder left, a missing value, an id. */
function plain(text: string): void {
  expect(text).not.toMatch(/[{}]|undefined|null|NaN|\bnode \d|\b1013\b|\b1020\b/);
  expect(text.length).toBeGreaterThan(10);
}

test("every kind of finding has a sentence, with every placeholder filled", async () => {
  const all = await variants();
  expect(new Set(all.map((v) => v.kind)).size).toBeGreaterThan(35);
  for (const detail of all) {
    for (const related of [null, 12, 40]) {
      for (const place of [
        null,
        { row: "C", rowNumber: 3, seat: "A-C-12", seatNumber: 12 },
        { row: null, rowNumber: 3, seat: null, seatNumber: 12 },
      ]) {
        plain(findingText(view(detail, related), place));
      }
    }
  }
});

function validated(s: Session): FindingView[] {
  while (!s.validate(1e9).done);
  return s.validation().findings;
}

const block = (id: number, name: string | null, x: number, rows: { rid: number }[]) => ({
  id,
  type: "rowBlock",
  ...(name === null ? {} : { name }),
  origin: [x, 0],
  seatPitch: 50,
  rowPitch: 100,
  category: "std",
  rowLabels: { scheme: "alpha", start: "A" },
  rows: rows.map((r) => ({ rid: r.rid, seats: 4, oids: [[r.rid + 1, 4]] })),
});

test("on a real chart, findings name objects by name and rows and seats by label", () => {
  // Two blocks labelled alike (E001) outside sections; a row with no label.
  const errors = d.open(
    JSON.stringify({
      schema: 1,
      name: "Errors",
      nextOid: 900,
      categories: [{ key: "std", label: "Standard", color: "#2563EB" }],
      nodes: [
        block(10, "Stalls left", 0, [{ rid: 100 }, { rid: 110 }]),
        block(20, "Stalls right", 1000, [{ rid: 200 }, { rid: 210 }]),
        {
          id: 30,
          type: "row",
          name: "Box row",
          origin: [0, 800],
          seatPitch: 50,
          seats: 3,
          path: {
            kind: "polyline",
            points: [
              [0, 0],
              [100, 0],
            ],
          },
          oids: [[300, 3]],
          category: "std",
        },
      ],
    }),
  );
  const found = validated(errors);
  const dup = found.find((f) => f.code === "E001");
  expect(dup).toBeDefined();
  if (dup === undefined) return;
  const text = findingText(dup);
  plain(text);
  expect(text).toContain("Stalls");
  expect(text).toMatch(/labelled A-1|labelled \S+/);
  const empty = found.find((f) => f.code === "E003");
  expect(empty).toBeDefined();
  if (empty?.node === undefined || empty.node === null) return;
  const place = namesPlace(empty) ? errors.findingPlace(empty.node, empty.row, empty.seat) : null;
  const sentence = findingText(empty, place);
  plain(sentence);
  expect(sentence).toContain("Box row");

  // Without errors, the warnings: a single-seat row, an unused category,
  // no focal point, no accessible seat, a section without a label.
  const warnings = d.open(
    JSON.stringify({
      schema: 1,
      name: "Warnings",
      nextOid: 900,
      categories: [
        { key: "std", label: "Standard", color: "#2563EB" },
        { key: "vip", label: "Premium", color: "#B45309" },
      ],
      nodes: [
        {
          id: 40,
          type: "section",
          origin: [0, 0],
          category: "std",
          children: [
            {
              ...block(41, "Front block", 0, [{ rid: 400 }]),
              rows: [{ rid: 400, seats: 1, oids: [[401, 1]] }],
            },
          ],
        },
      ],
    }),
  );
  const warned = validated(warnings);
  const codes = new Set(warned.map((f) => f.code));
  for (const code of ["W102", "W103", "W105", "W106", "W109"]) expect(codes).toContain(code);
  for (const f of warned) {
    const at =
      f.node !== null && namesPlace(f) ? warnings.findingPlace(f.node, f.row, f.seat) : null;
    plain(findingText(f, at));
  }
  const single = warned.find((f) => f.code === "W103");
  if (single?.node !== null && single?.node !== undefined) {
    const where = warnings.findingPlace(single.node, single.row, single.seat);
    expect(findingText(single, where)).toStartWith("Row A of Front block");
  }
  expect(findingText(warned.find((f) => f.code === "W105") as FindingView)).toContain("Premium");
});

test("a refused publish's findings are said as the panel says them, none dropped", () => {
  // The same labels twice (E001), one block unnamed (`cavea gen` makes such
  // charts), and a background picture this workspace lacks (E007, which
  // only the server checks).
  const s = d.open(
    JSON.stringify({
      schema: 1,
      name: "Refused",
      nextOid: 900,
      categories: [{ key: "std", label: "Standard", color: "#2563EB" }],
      nodes: [
        block(10, "Stalls left", 0, [{ rid: 100 }]),
        block(20, null, 1000, [{ rid: 200 }]),
        { id: 50, type: "image", asset: "f".repeat(64), size: [10, 10], role: "background" },
      ],
    }),
  );
  const own = validated(s).filter((f) => f.severity === "error");
  expect(own.length).toBeGreaterThan(0);
  const server: Finding[] = [
    ...own.map((f): Finding => ({
      code: f.code,
      severity: "error",
      node: f.node,
      related: f.related,
      message: f.message,
    })),
    { code: "E007", severity: "error", node: 50, related: null, message: "image 50: no asset" },
    { code: "E042", severity: "error", node: 77, related: null, message: "object 77: new words" },
    { code: "E006", severity: "error", node: null, related: null, message: "more than 3 sections" },
    { code: "W101", severity: "warning", node: 10, related: 20, message: "nodes 10, 20 overlap" },
  ];
  for (const chart of [s, null]) {
    const said = refused(server, chart);
    // Every error, and only errors, in the order the server gave them.
    expect(said.map((x) => x.code)).toEqual([...own.map((f) => f.code), "E007", "E042", "E006"]);
    for (const x of said) {
      plain(x.text);
      expect(x.text).not.toMatch(/\b(10|20|50|77)\b/);
      expect(x.title).not.toContain(x.code);
    }
  }
  const said = refused(server, s);
  own.forEach((f, i) => {
    const place = f.node !== null && namesPlace(f) ? s.findingPlace(f.node, f.row, f.seat) : null;
    expect(said[i]?.text).toBe(findingText(f, place));
  });
  expect(said.some((x) => x.text.includes("Row block 1"))).toBe(true);
  expect(said.at(-3)).toEqual({
    code: "E007",
    title: msg("validation.E007"),
    text: msg("finding.missingPicture", { node: "Image 1" }),
  });
  expect(said.at(-2)?.text).toBe(msg("finding.other", { node: msg("finding.something") }));
});
