// "Insert row block…" (07 §15): a row block from numbers, the path that
// needs no pointer. Straight blocks are `insert`'s `rowBlock` draft and
// curved ones its `arcBlock` draft, so the block is exactly what the row
// block and arc block tools make, with the refusals of `draft.rs`.
import type { CommandError, Draft, Node, Session } from "@cavea/designer-core";

/** Seats in one row (02 §14). */
export const MAX_SEATS_PER_ROW = 4_095;

export interface BlockSpec {
  curve: "straight" | "arc";
  rows: number;
  /** Seats per row; for an arc, of the front row (the rows behind fill the same angle). */
  seats: number;
  seatPitch: number;
  rowPitch: number;
  /** The front row's radius, for an arc. */
  radius: number;
  /** The front row's centre, in world chart units. */
  x: number;
  y: number;
  /** Degrees, clockwise; 0 has the front row at the top. */
  rotation: number;
  /** The section to insert into, or `null` for the top level. */
  parent: number | null;
  /** The category its seats take. */
  category: string;
}

export type SpecField = keyof BlockSpec;

/** The smallest arc radius (`draft::MIN_RADIUS`). */
export const MIN_RADIUS = 55;

/** What is wrong with each field, checked before the edit core sees it. */
export function check(
  spec: BlockSpec,
): Partial<Record<SpecField, "whole" | "positive" | "range" | "number">> {
  const out: Partial<Record<SpecField, "whole" | "positive" | "range" | "number">> = {};
  const whole = (f: "rows" | "seats", max: number) => {
    const v = spec[f];
    if (!Number.isInteger(v) || v < 1) out[f] = "whole";
    else if (v > max) out[f] = "range";
  };
  whole("rows", 65_535);
  whole("seats", MAX_SEATS_PER_ROW);
  for (const f of ["seatPitch", "rowPitch"] as const) {
    if (!(Number.isFinite(spec[f]) && spec[f] > 0)) out[f] = "positive";
  }
  if (spec.curve === "arc" && !(Number.isFinite(spec.radius) && spec.radius >= MIN_RADIUS)) {
    out.radius = "range";
  }
  for (const f of ["x", "y", "rotation"] as const) {
    if (!Number.isFinite(spec[f])) out[f] = "number";
  }
  return out;
}

/** The field a refusal of `insert` is about (`draft.rs` names them). */
export function fieldOf(e: CommandError): SpecField | null {
  if (e.code === "limit") return e.data === "seats per row" ? "seats" : null;
  if (e.code !== "invalidValue") return null;
  const map: Record<string, SpecField> = {
    count: "rows",
    span: "seats",
    curve: "radius",
    seatPitch: "seatPitch",
    rowPitch: "rowPitch",
    origin: "x",
  };
  return map[e.data] ?? null;
}

type Transform = { origin: [number, number]; rotation: number };
const rad = (deg: number) => (deg * Math.PI) / 180;

/** A world point in the space of `t` (rotation clockwise, y down). */
export function toLocal(t: Transform, p: [number, number]): [number, number] {
  const [dx, dy] = [p[0] - t.origin[0], p[1] - t.origin[1]];
  const [c, s] = [Math.cos(rad(t.rotation)), Math.sin(rad(t.rotation))];
  return [dx * c + dy * s, -dx * s + dy * c];
}

/** The draft for a block whose front-row centre is `at` and rotation
 * `rotation`, both in its parent's space. */
export function draftOf(spec: BlockSpec, at: [number, number], rotation: number): Draft {
  const { rows, seats, seatPitch, rowPitch } = spec;
  if (spec.curve === "straight") {
    return { kind: "rowBlock", origin: at, rows, seats, seatPitch, rowPitch };
  }
  // The apex points along `direction` from the centre; a block of rotation
  // r faces direction r + 90 (07 §4, arc block).
  const direction = rotation + 90;
  const r = spec.radius;
  const center: [number, number] = [
    at[0] - r * Math.cos(rad(direction)),
    at[1] - r * Math.sin(rad(direction)),
  ];
  return { kind: "arcBlock", center, radius: r, direction, rows, seats, seatPitch, rowPitch };
}

function findNode(nodes: Node[], id: number): Node | null {
  for (const n of nodes) {
    if (n.id === id) return n;
    if (n.type === "section") {
      const child = findNode(n.children, id);
      if (child !== null) return child;
    }
  }
  return null;
}

/**
 * Inserts the block as one undo entry, selects it and returns its id. A
 * refusal throws the session's `DesignerError` with nothing changed.
 *
 * The straight draft has no rotation (the tool draws it unturned), so a
 * turned straight block is made by `insert`, read back from the document,
 * undone, and created turned by one `createNode`: one entry in the end.
 */
export function insertBlock(session: Session, spec: BlockSpec): number {
  const nodes = () => (JSON.parse(session.export()) as { nodes: Node[] }).nodes;
  const children = (all: Node[]): Node[] => {
    if (spec.parent === null) return all;
    const s = findNode(all, spec.parent);
    if (s === null || s.type !== "section") throw new Error(`section ${spec.parent} is gone`);
    return s.children;
  };
  let at: [number, number] = [spec.x, spec.y];
  let rotation = spec.rotation;
  let before: Node[] | null = null;
  if (spec.parent !== null) {
    before = nodes();
    const section = findNode(before, spec.parent);
    if (section === null || section.type !== "section") {
      throw new Error(`section ${spec.parent} is gone`);
    }
    at = toLocal(section, at);
    rotation -= section.rotation;
  }
  const turn = normalise(rotation);
  const turned = spec.curve === "straight" && turn !== 0;
  // The block takes the session's active category: set it for the insert.
  const active = session.toolView().category;
  session.setCategory(spec.category);
  try {
    const index = turned ? children(before ?? nodes()).length : 0;
    const id = session.insert(spec.parent, draftOf(spec, at, turn));
    if (!turned) return id;
    const node = findNode(children(nodes()), id);
    if (node === null) return id;
    session.undo();
    const made = session.apply({
      op: "createNode",
      parent: spec.parent,
      index,
      node: { ...node, rotation: turn },
    }).created[0];
    if (made === undefined) return id;
    session.select(spec.parent === null ? { kind: "top" } : { kind: "section", id: spec.parent }, [
      made,
    ]);
    return made;
  } finally {
    session.setCategory(active);
  }
}

/** Degrees reduced to (−180, 180], as the edit core stores a rotation. */
export function normalise(deg: number): number {
  const r = deg % 360;
  return r > 180 ? r - 360 : r <= -180 ? r + 360 : r;
}
