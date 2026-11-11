// "Insert row block…" against a real session: one undo entry, the block
// selected, where the form says, turned and curved as asked.
import { beforeAll, describe, expect, test } from "bun:test";
import type { Designer, Node } from "@cavea/designer-core";
import { check, insertBlock, type BlockSpec } from "./insert.ts";
import { emptyChart } from "./source.ts";
import { designer } from "./testkit.ts";

let d: Designer;
beforeAll(async () => {
  d = await designer();
});

const spec: BlockSpec = {
  curve: "straight",
  rows: 4,
  seats: 12,
  seatPitch: 55,
  rowPitch: 95,
  radius: 1500,
  x: 300,
  y: -200,
  rotation: 0,
  parent: null,
  category: "standard",
};
const nodes = (s: { export(): string }) => (JSON.parse(s.export()) as { nodes: Node[] }).nodes;

describe("insert row block", () => {
  test("straight, turned or curved: one undo entry, selected, at the position given", () => {
    for (const change of [{}, { rotation: 30 }, { curve: "arc" as const, rotation: -45 }]) {
      const s = d.open(emptyChart("Insert"));
      const id = insertBlock(s, { ...spec, ...change });
      expect(s.selectionView().ids).toEqual([id]);
      expect(s.status().seats).toBeGreaterThanOrEqual(48);
      const [node] = nodes(s);
      expect(node?.type).toBe("rowBlock");
      if (node?.type !== "rowBlock") continue;
      expect(node.origin[0]).toBeCloseTo(300, 6);
      expect(node.origin[1]).toBeCloseTo(-200, 6);
      expect(node.rotation).toBeCloseTo(change.rotation ?? 0, 6);
      expect(node.curve.kind).toBe(change.curve === "arc" ? "arc" : "none");
      expect(s.undo()).toBe(true);
      expect([change, nodes(s)]).toEqual([change, []]);
      expect(s.history().undo).toBeNull();
      s.dispose();
    }
  });

  test("into a section, in its space, with the category chosen and the active one kept", () => {
    const s = d.open(emptyChart("Insert"));
    s.addCategory(null, { key: "vip", label: "VIP", color: "#B45309", accessible: false });
    const a = insertBlock(s, spec);
    const section = s.group([a], null);
    s.setField({ field: "rotation", value: 90 });
    const id = insertBlock(s, {
      ...spec,
      parent: section,
      x: 1000,
      y: 1000,
      rotation: 90,
      category: "vip",
    });
    expect(s.toolView().category).toBe("standard");
    expect(s.selectionView().level).toEqual({ kind: "section", id: section });
    const sec = nodes(s).find((n) => n.id === section);
    const child = sec?.type === "section" ? sec.children.find((n) => n.id === id) : undefined;
    expect(child?.type === "rowBlock" && child.rotation).toBe(0);
    expect(child?.type === "rowBlock" && child.category).toBe("vip");
    // Its world position is the one asked for: the inspector shows the
    // origin in the section's space, so compare through the section's turn.
    if (sec?.type !== "section" || child?.type !== "rowBlock") throw new Error("missing");
    const t = (sec.rotation * Math.PI) / 180;
    const [lx, ly] = child.origin;
    expect(sec.origin[0] + lx * Math.cos(t) - ly * Math.sin(t)).toBeCloseTo(1000, 6);
    expect(sec.origin[1] + lx * Math.sin(t) + ly * Math.cos(t)).toBeCloseTo(1000, 6);
    s.dispose();
  });

  test("the form's checks match the edit core's limits", () => {
    expect(check(spec)).toEqual({});
    expect(check({ ...spec, rows: 0, seats: 4096, seatPitch: 0, x: Number.NaN })).toEqual({
      rows: "whole",
      seats: "range",
      seatPitch: "positive",
      x: "number",
    });
    expect(check({ ...spec, curve: "arc", radius: 54 })).toEqual({ radius: "range" });
    const s = d.open(emptyChart("Insert"));
    // Too many seats for the arc's half turn: the edit core says so.
    expect(() => insertBlock(s, { ...spec, curve: "arc", radius: 55, seats: 400 })).toThrow();
    expect(nodes(s)).toEqual([]);
    s.dispose();
  });
});
