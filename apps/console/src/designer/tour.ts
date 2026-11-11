// The first-run tour (07 §16): five coach marks, draw a block, curve it,
// number it, assign a category, publish. Each step but the last ends by
// itself when the edit it teaches lands on the undo stack (or by Next);
// the last says where Publish is. Shown once per browser
// (`cavea.designer.tour.v1`), and again from the command palette.
import type { HistoryView, SelectionView } from "@cavea/designer-core";
import type { MessageId } from "../i18n/messages.ts";

export const TOUR_KEY = "cavea.designer.tour.v1";

/** Where a step's card points: the tool rail, the canvas, the inspector,
 * the panels' tabs, the Publish button. */
export type Anchor = "rail" | "canvas" | "inspector" | "panels" | "publish";

export interface Step {
  id: "draw" | "curve" | "number" | "category" | "publish";
  anchor: Anchor;
  title: MessageId;
  body: MessageId;
  /** The undo entries that mean the step was done (none: the user ends it). */
  done: readonly string[];
}

export const STEPS: readonly Step[] = [
  { id: "draw", anchor: "rail", title: "tour.draw", body: "tour.drawBody", done: ["createNode"] },
  { id: "curve", anchor: "canvas", title: "tour.curve", body: "tour.curveBody", done: ["curve"] },
  {
    id: "number",
    anchor: "inspector",
    title: "tour.number",
    body: "tour.numberBody",
    done: ["rowLabels", "seatLabels"],
  },
  {
    id: "category",
    anchor: "panels",
    title: "tour.category",
    body: "tour.categoryBody",
    done: ["category", "editSeats"],
  },
  { id: "publish", anchor: "publish", title: "tour.publish", body: "tour.publishBody", done: [] },
];

/**
 * The step to show after the session changed: the next one when the
 * document changed since step `at` began (`since`, the revision then) and
 * the last undo entry is what the step teaches (a new block must be a row
 * block), else `at`.
 */
export function next(
  at: number,
  since: number,
  revision: number,
  history: Pick<HistoryView, "undo" | "gesture">,
  selection: Pick<SelectionView, "kinds">,
): number {
  const step = STEPS[at];
  if (step === undefined || history.gesture || revision === since) return at;
  const key = history.undo;
  if (key === null || !step.done.includes(key)) return at;
  if (step.id === "draw" && !selection.kinds.some((k) => k.kind === "rowBlock")) return at;
  return at + 1;
}
