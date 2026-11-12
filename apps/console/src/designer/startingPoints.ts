// The starting points (07 §16) on any chart, local or on the server: when
// they are offered, when applying one asks first, and a template applied to
// an open chart as one edit (so autosave saves it and undo takes it back).
import type { Category, Command, Floor, Node, Settings } from "@cavea/designer-core";

/** What a starting point can do to the chart open now. */
export type Offer =
  /** Not offered: the person cannot change this chart (a role that reads,
   * someone else editing, an expired licence, an archived chart). */
  | "none"
  /** Applied at once: there is nothing to lose. */
  | "apply"
  /** Applied after asking: it would replace what is there. */
  | "ask";

/**
 * The one rule, for the local designer and a server chart alike. `open` is
 * the chart open now (`null`: none, the local designer's empty state):
 * replacing one with nodes in it, or with changes not saved, asks first.
 */
export function offer(canEdit: boolean, open: { nodes: number; unsaved: boolean } | null): Offer {
  if (!canEdit) return "none";
  if (open === null) return "apply";
  return open.nodes > 0 || open.unsaved ? "ask" : "apply";
}

/** The parts of a chart document a template replaces. */
interface Doc {
  settings: Settings;
  categories: Category[];
  floors: Floor[];
  nodes: Node[];
}

const parse = (text: string): Doc => JSON.parse(text) as Doc;

/**
 * One command that turns the chart `current` (its `export()`) into the
 * template `template` (its document text), keeping the chart's name: every
 * node and floor removed, the template's settings, categories (in its
 * order), floors and nodes put in. `composite` makes it one undo step; a
 * refusal changes nothing.
 */
export function templateCommand(current: string, template: string): Command {
  const now = parse(current);
  const next = parse(template);
  // Settings first: a composite's undo entry is named after its first
  // command ("Chart settings"), which reads better than "Delete".
  const commands: Command[] = [
    { op: "setChart", property: { field: "settings", value: next.settings } },
  ];
  if (now.nodes.length > 0) {
    commands.push({ op: "deleteNodes", ids: now.nodes.map((n) => n.id) });
  }
  for (const f of now.floors) commands.push({ op: "removeFloor", id: f.id, replacement: null });
  // With every floor gone, each one added takes the next id from 1.
  const floors = new Map<number, number>();
  next.floors.forEach((f, i) => {
    commands.push({ op: "addFloor", index: i, floor: { ...f, id: 0 } });
    floors.set(f.id, i + 1);
  });
  // Categories: the others removed first (so a chart near the limit still
  // takes the template's), then the template's updated or added, then put
  // in its order. The nodes are gone already, so nothing refers to them.
  const wanted = new Set(next.categories.map((c) => c.key));
  const kept = new Set<string>();
  for (const c of now.categories) {
    if (wanted.has(c.key)) kept.add(c.key);
    else commands.push({ op: "removeCategory", key: c.key, replacement: null });
  }
  let count = kept.size;
  for (const c of next.categories) {
    if (kept.has(c.key)) commands.push({ op: "updateCategory", key: c.key, category: c });
    else commands.push({ op: "addCategory", index: count++, category: c });
  }
  next.categories.forEach((c, i) => commands.push({ op: "moveCategory", key: c.key, index: i }));
  next.nodes.forEach((n, i) => {
    const floor = "floor" in n && n.floor !== undefined ? floors.get(n.floor) : undefined;
    const node = (floor === undefined ? n : { ...n, floor }) as Node;
    commands.push({ op: "createNode", parent: null, index: i, node });
  });
  return { op: "composite", commands };
}
