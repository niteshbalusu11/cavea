// The designer module across its boundary, under bun: every call of the
// table once, each reply checked at run time against the generated schema,
// and each kind of failure. Needs `cargo xtask build-designer` (the WASM)
// and cargo (the presets). `CAVEA_MEASURE=1` adds the per-call timings.
import { beforeAll, describe, expect, test } from "bun:test";

import type { Bindings, Handle } from "./bindings.ts";
import schema from "./generated/schema.json";
import {
  DesignerError,
  loadDesigner,
  Session,
  type Designer,
  type DesignerFailure,
  type Input,
  type Numbering,
  type Pointer,
} from "./index.ts";

const root = new URL("../../../", import.meta.url);
const wasm = new URL("packages/designer-core/wasm/", root);

/** A preset's chart document, from `cavea gen`. */
function preset(name: string): string {
  const run = Bun.spawnSync(
    ["cargo", "run", "--release", "-q", "-p", "cavea-cli", "--", "gen", name],
    { cwd: root.pathname, env: { ...process.env, CARGO_BUILD_JOBS: "8" } },
  );
  if (!run.success) {
    throw new Error(`cavea gen ${name}: ${run.stderr.toString()}`);
  }
  return run.stdout.toString();
}

// ------------------------------------------------- the schema, at run time

type T =
  | "number"
  | "string"
  | "boolean"
  | "null"
  | { literal: string }
  | { array: T }
  | { tuple: T[] }
  | { record: T }
  | { union: T[] }
  | { all: T[] }
  | { object: { name: string; type: T; optional: boolean }[] }
  | { ref: string; args: T[] }
  | { param: string };
type Env = Map<string, [T, Env]>;

const types = schema.types as Record<string, { params: string[]; type: T }>;
const methods = schema.methods as Record<string, { call: string; returns: T }>;

const show = (v: unknown): string => JSON.stringify(v)?.slice(0, 80) ?? String(v);

function isObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** The field names an object type allows. */
function keys(t: T): string[] {
  if (typeof t === "string") return [];
  if ("object" in t) return t.object.map((p) => p.name);
  if ("ref" in t) return keys(types[t.ref]?.type ?? "null");
  if ("all" in t) return t.all.flatMap(keys);
  return [];
}

/** Why `v` is not of type `t`, or `null`. `open` allows other fields. */
function check(v: unknown, t: T, env: Env, path: string, open = false): string | null {
  const fail = (what: string) => `${path}: expected ${what}, got ${show(v)}`;
  if (typeof t === "string") {
    switch (t) {
      case "number":
        return typeof v === "number" && Number.isFinite(v) ? null : fail("a number");
      case "string":
        return typeof v === "string" ? null : fail("a string");
      case "boolean":
        return typeof v === "boolean" ? null : fail("a boolean");
      case "null":
        return v === null ? null : fail("null");
    }
  }
  if ("literal" in t) return v === t.literal ? null : fail(show(t.literal));
  if ("array" in t) {
    if (!Array.isArray(v)) return fail("an array");
    for (const [i, item] of v.entries()) {
      const problem = check(item, t.array, env, `${path}[${i}]`);
      if (problem !== null) return problem;
    }
    return null;
  }
  if ("tuple" in t) {
    if (!Array.isArray(v) || v.length !== t.tuple.length) return fail(`${t.tuple.length} items`);
    for (const [i, item] of t.tuple.entries()) {
      const problem = check(v[i], item, env, `${path}[${i}]`);
      if (problem !== null) return problem;
    }
    return null;
  }
  if ("record" in t) {
    if (!isObject(v)) return fail("an object");
    for (const [k, item] of Object.entries(v)) {
      const problem = check(item, t.record, env, `${path}.${k}`);
      if (problem !== null) return problem;
    }
    return null;
  }
  if ("union" in t) {
    const problems = t.union.map((m) => check(v, m, env, path, open));
    return problems.includes(null) ? null : `${path}: no member matches: ${problems.join("; ")}`;
  }
  if ("all" in t) {
    for (const part of t.all) {
      const problem = check(v, part, env, path, true);
      if (problem !== null) return problem;
    }
    const allowed = new Set(keys(t));
    const extra = isObject(v) ? Object.keys(v).filter((k) => !allowed.has(k)) : [];
    return open || extra.length === 0 ? null : `${path}: unexpected fields ${extra.join(", ")}`;
  }
  if ("object" in t) {
    if (!isObject(v)) return fail("an object");
    for (const p of t.object) {
      if (!(p.name in v)) {
        if (!p.optional) return `${path}: missing ${p.name}`;
        continue;
      }
      const problem = check(v[p.name], p.type, env, `${path}.${p.name}`);
      if (problem !== null) return problem;
    }
    const allowed = new Set(t.object.map((p) => p.name));
    const extra = Object.keys(v).filter((k) => !allowed.has(k));
    return open || extra.length === 0 ? null : `${path}: unexpected fields ${extra.join(", ")}`;
  }
  if ("ref" in t) {
    const decl = types[t.ref];
    if (decl === undefined) return `${path}: no type ${t.ref}`;
    const inner: Env = new Map(
      decl.params.map((p, i): [string, [T, Env]] => [p, [t.args[i] ?? "null", env]]),
    );
    return check(v, decl.type, inner, `${path}<${t.ref}>`, open);
  }
  const bound = env.get(t.param);
  return bound === undefined
    ? `${path}: unbound ${t.param}`
    : check(v, bound[0], bound[1], path, open);
}

/** The methods called through `checked`, each reply checked. */
const called = new Set<string>();

/** `session` with every reply checked against the generated schema. */
function checked(session: Session): Session {
  return new Proxy(session, {
    get(target, prop) {
      const value: unknown = Reflect.get(target, prop, target);
      if (typeof value !== "function") return value;
      const method = typeof prop === "string" ? methods[prop] : undefined;
      if (method === undefined) return value.bind(target);
      return (...args: unknown[]) => {
        const result: unknown = value.apply(target, args);
        const problem = check(result ?? null, method.returns, new Map(), String(prop));
        if (problem !== null) throw new Error(`reply off the generated types: ${problem}`);
        called.add(String(prop));
        return result;
      };
    },
  });
}

/** The failure a call threw. */
function failure(run: () => unknown): DesignerFailure {
  try {
    run();
  } catch (error) {
    if (error instanceof DesignerError) return error.failure;
    throw error;
  }
  throw new Error("expected a DesignerError");
}

/** A call the typed API cannot make. */
function raw(session: Session, method: string, args: unknown[]): unknown {
  return (session as unknown as { call(m: string, a: unknown[]): unknown }).call(method, args);
}

function pointer(world: [number, number], time: number): Pointer {
  return {
    world,
    screen: world,
    zoom: 1,
    button: "primary",
    kind: "mouse",
    mods: { shift: false, alt: false, ctrl: false },
    time,
  };
}

// ---------------------------------------------------------------- the test

let designer: Designer;
let theatre: string;

beforeAll(async () => {
  const file = Bun.file(new URL("cavea_designer_bg.wasm", wasm));
  if (!(await file.exists())) {
    throw new Error("the designer WASM is missing: run `cargo xtask build-designer` first");
  }
  designer = await loadDesigner({ wasm: await file.arrayBuffer() });
  theatre = preset("theatre");
});

describe("the designer module", () => {
  test("matches the committed declaration of its bindings", async () => {
    const bindings = (await import(String(new URL("cavea_designer.js", wasm)))) as Bindings;
    const handle: Handle = new bindings.Designer();
    expect([typeof handle.import, typeof handle.call, typeof handle.free]).toEqual([
      "function",
      "function",
      "function",
    ]);
    // Before a chart is open, every call is refused.
    expect(JSON.parse(handle.call("history", "[]"))).toEqual({ err: { kind: "noChart" } });
    handle.free();
  });

  test("runs every call, each reply as the generated types say", () => {
    const s = checked(designer.open(theatre));
    const original = s.export();
    expect(JSON.parse(original)).toEqual(JSON.parse(theatre));

    // The frame loop: everything the first time, then nothing.
    expect(s.changed()).toHaveLength(14);
    expect(s.changed()).toEqual([]);

    // Every view.
    expect(s.history()).toEqual({ undo: null, redo: null, gesture: false });
    expect(s.title()).toBe(JSON.parse(theatre).name);
    const floors = s.floors();
    expect(floors.floors.map((f) => f.id)).toEqual([1, 2]);
    const seats = s.status().seats;
    expect(s.selectionView().count).toBe(0);
    expect(s.fit()).not.toBeNull();
    expect(s.toolView().tool).toBe("select");
    expect(s.inspector().chart?.name).toBe(s.title());
    const top = s.layers(0, 50);
    expect(top.rows.length).toBe(top.total);
    expect(s.categories().categories.map((c) => c.key)).toEqual([
      "premium",
      "standard",
      "value",
      "access",
    ]);
    s.numbering();
    expect(s.numberingPreview()).toBeNull();
    s.validation();
    expect(s.overlay().outlines).toEqual([]);
    expect(s.cursor()).toBe("default");
    expect(s.guides()).toEqual([]);
    s.savedGuides();

    // Input: draw a row block with the pointer, far from the theatre.
    s.setTool("rowBlock");
    const input = (event: Input) => s.input(event);
    input({ kind: "pointerDown", data: pointer([20_000, 20_000], 0) });
    input({ kind: "pointerMove", data: pointer([20_400, 20_300], 16) });
    expect(s.overlay().readout?.kind).toBe("block");
    expect(s.history().gesture).toBe(true);
    input({ kind: "pointerUp", data: pointer([20_400, 20_300], 32) });
    expect(input({ kind: "keyDown", data: ["escape", pointer([0, 0], 0).mods] })).toEqual({
      pan: null,
    });
    input({ kind: "pointerCancel" });
    expect(s.history().undo).toBe("createNode");
    expect(s.changed()).toEqual(expect.arrayContaining(["history", "status", "layers"]));
    const block = s.layers(0, 50).rows.find((r) => r.kind === "rowBlock")?.id;
    if (block === undefined) throw new Error("no row block drawn");
    expect(s.status().seats).toBeGreaterThan(seats);
    s.setTool("select");

    // Selection.
    s.select({ kind: "top" }, [block]);
    expect(s.selectionView().count).toBe(1);
    s.selectAll();
    s.select({ kind: "top" }, [block]);

    // The inspector, with a scrub: one undo entry for three values.
    s.beginScrub();
    s.setField({ field: "rowPitch", value: 100 });
    s.setField({ field: "rowPitch", value: 110 });
    s.endScrub();
    expect(s.inspector().rowBlock?.rowPitch.value).toEqual({ same: 110 });
    expect(s.history().undo).toBe("rowPitch");
    s.beginScrub();
    s.setField({ field: "rowPitch", value: 300 });
    s.cancelScrub();
    expect(s.inspector().rowBlock?.rowPitch.value).toEqual({ same: 110 });

    // Layers: a window, expansion and a move.
    const first = s.layers(0, 1).rows[0];
    if (first === undefined) throw new Error("no layers");
    s.expandLayer(first.id, true);
    expect(s.revealLayer(first.id)).toBe(0);
    s.moveLayers([block], { kind: "before", id: first.id });
    expect(s.layers(0, 1).rows[0]?.id).toBe(block);
    expect(s.history().undo).toBe("moveLayers");
    expect(s.clickLayer(block, "replace")).toBe(true);
    s.lockLayer(block, true);
    s.lockLayer(block, false);
    s.hideLayer(block, true);
    s.hideLayer(block, false);

    // Numbering: preview, sample, apply.
    s.select({ kind: "top" }, [block]);
    expect(s.numbering().nodes.map((n) => n.id)).toEqual([block]);
    const numbering: Numbering = {
      rows: { scheme: "roman", step: 1, direction: "front-to-back" },
      seats: null,
      afterZ: null,
      across: null,
      keepOverrides: false,
    };
    const preview = s.previewNumbering(numbering);
    expect(preview.rows[0]?.label).toBe("I");
    expect(s.numberingPreview()).toEqual(preview);
    const seat = preview.rows[0]?.seats[0];
    if (seat === undefined) throw new Error("no seat previewed");
    expect(s.sample(block, seat.oid)).toContain("I");
    s.cancelNumbering();
    s.number(numbering);
    // A composite is named by its first command: the blocks' row labels.
    expect(s.history().undo).toBe("rowLabels");

    // Categories.
    s.addCategory(null, { key: "vip", label: "VIP", color: "#7C3AED", accessible: false });
    s.updateCategory("vip", { key: "vip", label: "VIP", color: "#6D28D9", accessible: false });
    s.moveCategory("vip", 0);
    s.setCategory("vip");
    s.mergeCategory("value", "standard");
    s.deleteCategory("vip", null);
    expect(s.categories().categories.map((c) => c.key)).toEqual(["premium", "standard", "access"]);
    expect(s.contrast("#FFFF00")?.weak).toBe(true);
    expect(s.contrast("blue")).toBeNull();
    expect(s.tooSimilar("#000000", "#000001")).toBe(true);

    // Editing at the rows and seats levels of the block.
    s.select({ kind: "top" }, [block]);
    expect(s.stepIn()).toBe(true);
    expect(s.selectionView().level).toEqual({ kind: "rows", id: block });
    s.rowSeats("end", 1);
    s.shiftRows(5);
    s.setRowLabel("Z");
    s.setRowLabel(null);
    s.cutAisle([20_200, 20_000]);
    s.openAisle([20_200, 20_000], 1);
    s.stepOut();
    s.select({ kind: "top" }, [block]);
    s.stepIn();
    s.stepIn();
    s.editSeats({ kind: "flag", flag: "accessible", on: true });
    s.stepOut();
    s.stepOut();

    // Editing at the node level.
    s.select({ kind: "top" }, [block]);
    const copies = s.duplicate([0, 400]).created;
    expect(copies).toHaveLength(1);
    s.select({ kind: "top" }, [block, ...copies]);
    s.align("left");
    s.distribute("vertical");
    s.flip("horizontal");
    s.order("front");
    s.setLocked(true);
    s.setLocked(false);
    s.setHidden(true);
    s.setHidden(false);
    // Two copies of each of the two selected blocks.
    expect(s.array(2, { kind: "offset", data: [0, 400] }).created).toHaveLength(4);
    s.select({ kind: "top" }, copies);
    const text = s.copy();
    if (text === null) throw new Error("nothing copied");
    expect(s.cut()).toBe(text);
    expect(s.paste(text, [25_000, 25_000]).created).toHaveLength(1);
    const pasted = s.selectionView();
    expect(pasted.count).toBe(1);
    s.delete();
    const section = s.group([block], null);
    s.convertOutline(section);
    s.dissolve(section);
    const inserted = s.insert(null, {
      kind: "rowBlock",
      origin: [30_000, 30_000],
      rows: 2,
      seats: 3,
      seatPitch: 55,
      rowPitch: 95,
    });
    s.select({ kind: "top" }, [inserted]);
    s.setFocalPoint([1, 2]);
    s.setText("ignored without a text field");
    s.setSnap({ ...s.toolView().snap, grid: false });
    expect(s.toolView().snap.grid).toBe(false);
    expect(s.focus(inserted, null, null)).not.toBeNull();

    // Floors and a command.
    const floor = s.addFloor("Roof", 3);
    s.renameFloor(floor, "Terrace");
    expect(s.setFloor(floor)).toBe(true);
    expect(s.setFloor(99)).toBe(false);
    s.deleteFloor(floor, null);
    s.setFloor(1);
    s.apply({ op: "setChart", property: { field: "name", value: "Renamed" } });
    expect(s.title()).toBe("Renamed");

    // Guides.
    expect(s.addGuide({ axis: "vertical", at: 10 })).toBe(0);
    s.moveGuide(0, 20);
    const saved = s.savedGuides();
    expect(s.removeGuide(0)).toBe(true);
    expect(s.removeGuide(0)).toBe(false);
    s.restoreGuides(saved);
    expect(s.guides()).toEqual([{ axis: "vertical", at: 20 }]);
    s.clearGuides();

    // Validation to completion.
    let steps = 0;
    while (!s.validate(500).done) steps += 1;
    expect(steps).toBeGreaterThan(0);
    expect(s.validation().complete).toBe(true);

    // Undo everything, then redo it: the same documents, byte for byte.
    const edited = s.export();
    let undone = 0;
    while (s.undo()) undone += 1;
    expect(undone).toBeGreaterThan(30);
    expect(s.undo()).toBe(false);
    // Ids handed out stay reserved (07 §9): only `nextOid` differs.
    const back = JSON.parse(s.export()) as { nextOid: number };
    const before = JSON.parse(original) as { nextOid: number };
    expect(back.nextOid).toBeGreaterThan(before.nextOid);
    expect(s.export()).toBe(
      original.replace(`"nextOid":${before.nextOid}`, `"nextOid":${back.nextOid}`),
    );
    while (s.redo()) undone -= 1;
    expect(undone).toBe(0);
    expect(s.export()).toBe(edited);

    // What was saved opens again as it was.
    const again = designer.open(edited);
    expect(again.export()).toBe(edited);
    again.dispose();

    // Every call of the table crossed the boundary at least once.
    const missing = Object.keys(methods).filter((m) => !called.has(m));
    expect(missing).toEqual([]);
    s.dispose();
  });

  test("reports each failure as its typed code", () => {
    // Import refusals.
    const json = failure(() => designer.open("{"));
    expect(json.kind === "import" && json.error.code).toBe("json");
    if (json.kind === "import" && json.error.code === "json") {
      expect(json.error.line).toBe(1);
    }
    const doc = JSON.parse(theatre) as Record<string, unknown>;
    const schemaFailure = failure(() => designer.open(JSON.stringify({ ...doc, schema: 99 })));
    expect(schemaFailure).toEqual({ kind: "import", error: { code: "schema", schema: 99 } });

    const s = designer.open(theatre);
    // Refused commands, with their data; nothing changed.
    const before = s.export();
    const section = s.layers(0, 1).rows[0]?.id ?? 0;
    s.select({ kind: "top" }, [section]);
    expect(failure(() => s.setField({ field: "rowPitch", value: 5 }))).toEqual({
      kind: "command",
      error: { code: "notApplicable", data: { id: section, field: "rowPitch" } },
    });
    expect(failure(() => s.setField({ field: "x", value: 1e9 }))).toEqual({
      kind: "command",
      error: { code: "invalidValue", data: "x" },
    });
    expect(failure(() => s.deleteCategory("standard", null))).toMatchObject({
      kind: "command",
      error: { code: "categoryInUse", data: { key: "standard" } },
    });
    expect(failure(() => s.renameFloor(99, "x"))).toEqual({
      kind: "command",
      error: { code: "missingFloor", data: 99 },
    });
    expect(s.export()).toBe(before);

    // Arguments that do not match the call.
    const args = (method: string, a: unknown[]) => failure(() => raw(s, method, a)).kind;
    expect(args("layers", ["0", 1])).toBe("arguments");
    expect(args("layers", [0])).toBe("arguments");
    expect(args("layers", [0, 1, 2])).toBe("arguments");
    expect(args("history", [1])).toBe("arguments");
    expect(args("layers", [-1, 1])).toBe("arguments");
    expect(args("setField", [{ field: "rowPitch", value: 1 }])).toBe("unknownCall");
    expect(args("set_field", [{ field: "rowPitch", value: Number.NaN }])).toBe("arguments");
    expect(args("set_tool", ["pencil"])).toBe("arguments");
    expect(args("apply", [{ op: "moveNodes", ids: [], delta: [0, 0], extra: 1 }])).toBe(
      "arguments",
    );
    expect(failure(() => raw(s, "nope", []))).toEqual({ kind: "unknownCall", name: "nope" });
    // NaN would cross as `null`, which clears a nullable field: refused first.
    expect(failure(() => s.setField({ field: "floor", value: Number.NaN }))).toMatchObject({
      kind: "arguments",
    });
    // Nor can a BigInt or a cycle, which plain JavaScript can pass.
    expect(args("set_field", [{ field: "floor", value: 1n }])).toBe("arguments");
    const cycle: Record<string, unknown> = {};
    cycle["self"] = cycle;
    expect(args("set_field", [cycle])).toBe("arguments");
    const hostile = {
      get value(): never {
        throw Object.create(null) as never;
      },
    };
    expect(args("set_field", [hostile])).toBe("arguments");
    expect(s.export()).toBe(before);

    // A disposed session.
    s.dispose();
    s.dispose();
    expect(s.disposed).toBe(true);
    expect(failure(() => s.history())).toEqual({ kind: "disposed" });
  });

  test("turns a trap into a panic that breaks the module, not a hang or a crash", () => {
    let freed = 0;
    const trapping: Handle = {
      import: () => '{"ok":null}',
      call: () => {
        throw new WebAssembly.RuntimeError("unreachable");
      },
      free: () => {
        freed += 1;
      },
    };
    const module = { bindings: {} as Bindings, broken: null as string | null };
    const one = new Session(module, trapping);
    const two = new Session(module, trapping);
    expect(failure(() => one.history())).toEqual({ kind: "panic", message: "unreachable" });
    expect(failure(() => one.history())).toEqual({ kind: "panic", message: "unreachable" });
    expect(failure(() => two.status())).toEqual({ kind: "panic", message: "unreachable" });
    two.dispose();
    expect(freed).toBe(0);
  });

  test("a trap while opening breaks the module for every later load", async () => {
    // Bindings whose import traps, counting what is freed.
    const stub = new URL("target/designer-trap-stub.js", root);
    await Bun.write(
      stub,
      `export default async () => {};
       export const counts = { made: 0, freed: 0 };
       export class Designer {
         constructor() { counts.made += 1; }
         import() { throw new WebAssembly.RuntimeError("unreachable"); }
         call() { return '{"ok":null}'; }
         free() { counts.freed += 1; }
       }`,
    );
    const { counts } = (await import(String(stub))) as { counts: { made: number; freed: number } };
    const first = await loadDesigner({ bindings: stub, wasm: new Uint8Array() });
    expect(failure(() => first.open("{}"))).toEqual({ kind: "panic", message: "unreachable" });
    const again = await loadDesigner({ bindings: stub, wasm: new Uint8Array() });
    expect(failure(() => again.open("{}"))).toEqual({ kind: "panic", message: "unreachable" });
    expect(counts).toEqual({ made: 1, freed: 0 });
  });
});

/** Prints the mean time of `run` over `n` calls. */
function measure(label: string, n: number, run: (i: number) => unknown): void {
  run(0);
  const start = Bun.nanoseconds();
  for (let i = 0; i < n; i++) run(i);
  const us = (Bun.nanoseconds() - start) / n / 1000;
  console.log(`${label.padEnd(48)} ${us.toFixed(2)} µs`);
}

const hover = (i: number): Input => ({ kind: "pointerMove", data: pointer([i % 1000, 0], i) });

describe.skipIf(process.env["CAVEA_MEASURE"] === undefined)("boundary cost", () => {
  test("per call on stadium-100k (bun's engine, not a browser)", () => {
    const s = designer.open(preset("stadium-100k"));
    s.changed();
    measure("changed(), nothing changed", 100_000, () => s.changed());
    measure("cursor() (the boundary alone)", 100_000, () => s.cursor());
    measure("input(), a hover move", 20_000, (i) => s.input(hover(i)));
    measure("changed() after input()", 20_000, (i) => {
      s.input(hover(i));
      return s.changed();
    });
    measure("overlay(), nothing selected", 20_000, () => s.overlay());
    // As the native measurement (07 §14): a section's block, with the
    // section's automatic outline.
    const section = s.layers(0, 1).rows[0]?.id ?? 0;
    s.select({ kind: "top" }, [section]);
    s.stepIn();
    measure("overlay(), a block in a section", 5_000, () => s.overlay());
    // Every top-level node: 180 automatic outlines, 44 ms natively.
    s.select({ kind: "top" }, []);
    s.selectAll();
    measure(`overlay(), all ${s.selectionView().count} top-level nodes`, 5, () => s.overlay());
    measure("export() (the whole draft)", 20, () => s.export());
    s.dispose();
  }, 120_000);
});
