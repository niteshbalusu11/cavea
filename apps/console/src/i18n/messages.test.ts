// The catalogue names every value the edit core can show the user: read
// from the generated schema and the compiler's finding codes, so this fails
// when either gains one without a message.
import { expect, test } from "bun:test";
import schema from "../../../../packages/designer-core/src/generated/schema.json";
import { GROUPS } from "../designer/shortcuts.ts";
import { catalogue, historyName, msg } from "./messages.ts";

type T = { literal?: string; union?: T[]; object?: { name: string; type: T }[] };
const types = schema.types as unknown as Record<string, { type: T }>;

/** The literals of a union type, or of field `tag` of each of its objects. */
function literals(name: string, tag?: string): string[] {
  const union = types[name]?.type.union ?? [];
  return union.flatMap((t) => {
    if (tag === undefined) return t.literal === undefined ? [] : [t.literal];
    const field = t.object?.find((f) => f.name === tag);
    return field?.type.literal === undefined ? [] : [field.type.literal];
  });
}

const missing = (prefix: string, keys: string[]) =>
  keys.filter((k) => !(`${prefix}.${k}` in catalogue));

test("every node kind, tool, unit and error code has a message", () => {
  const groups: [string, string[]][] = [
    ["kind", literals("NodeKind")],
    ["kinds", literals("NodeKind")],
    ["kindOne", literals("NodeKind")],
    ["scheme", literals("LabelScheme")],
    ["rowDirection", literals("RowDirection")],
    ["seatDirection", literals("SeatDirection")],
    ["group", [...GROUPS, "tree"]],
    ["tool", literals("ToolId")],
    ["unit", literals("Unit")],
    ["commandError", literals("CommandError", "code")],
    ["importError", literals("ImportError", "code")],
    ["field", literals("Field", "field")],
    ["seatShape", literals("SeatShape")],
    ["rowLabelDisplay", literals("RowLabelDisplay")],
    ["alphaOverflow", literals("AlphaOverflow")],
    ["align", literals("Align")],
    ["layer", literals("Layer")],
    ["imageRole", literals("ImageRole")],
    ["shapeKind", literals("ShapeKind")],
    ["curve", literals("Curve", "kind")],
  ];
  for (const [prefix, keys] of groups) {
    expect(keys.length).toBeGreaterThan(0);
    expect([prefix, missing(prefix, keys)]).toEqual([prefix, []]);
  }
});

test("every undo entry key has a name", async () => {
  // Command ops and property fields, inspector fields, and the session's
  // own names (crates/cavea-edit: `Command::label`, `apply_as`, a gesture).
  const keys = [
    ...literals("Command", "op"),
    ...literals("Property", "field"),
    ...literals("Field", "field"),
    "chartName",
    "chartSettings",
  ];
  const edit = new URL("../../../../crates/cavea-edit/src/", import.meta.url);
  for (const file of ["editor.rs", "session/shell.rs"]) {
    const text = await Bun.file(new URL(file, edit)).text();
    for (const m of text.matchAll(/apply_as\([^)]*Some\("(\w+)"\)|unwrap_or\("(\w+)"\)/g))
      keys.push(m[1] ?? m[2] ?? "");
  }
  expect(keys).toContain("moveLayers");
  expect(keys).toContain("gesture");
  expect(
    keys.filter((k) => !(`history.${k}` in catalogue) && !(`field.${k}` in catalogue)),
  ).toEqual([]);
  expect(historyName("rowPitch")).toBe("Row pitch");
});

test("every validation code the compiler writes has a title", async () => {
  const text = await Bun.file(
    new URL("../../../../crates/cavea-compiler/src/finding.rs", import.meta.url),
  ).text();
  const codes = [...text.matchAll(/Self::\w+ => "(E\d{3}|W\d{3}|unsupported)"/g)].map(
    (m) => m[1] ?? "",
  );
  expect(codes.length).toBeGreaterThan(20);
  expect(missing("validation", codes)).toEqual([]);
});

test("placeholders are filled, numbers through Intl", () => {
  expect(msg("status.seats", { n: 100000 })).toBe("100,000 seats");
  expect(msg("topbar.undoWhat", { what: "Move" })).toBe("Undo Move");
});
