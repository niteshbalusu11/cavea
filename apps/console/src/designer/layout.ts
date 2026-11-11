// The designer's layout preferences (07 §3: "Layout preferences persist per
// user"): the side column's width, the inspector's share of it, what is
// collapsed and the panel tab shown. Stored versioned in local storage;
// anything unreadable or out of range falls back to the default.
import { isRecord } from "../storage.ts";

export type PanelTab = "layers" | "categories" | "validation";
export const TABS: readonly PanelTab[] = ["layers", "categories", "validation"];

export interface Layout {
  /** The right column, CSS px. */
  side: number;
  /** The inspector's share of the column's height, 0 to 1. */
  split: number;
  inspectorOpen: boolean;
  panelsOpen: boolean;
  tab: PanelTab;
}

export const LAYOUT_KEY = "cavea.designer.layout.v1";
export const SIDE = { min: 260, max: 520 } as const;
export const SPLIT = { min: 0.2, max: 0.8 } as const;
export const DEFAULT_LAYOUT: Layout = {
  side: 300,
  split: 0.5,
  inspectorOpen: true,
  panelsOpen: true,
  tab: "layers",
};

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
const num = (v: unknown, lo: number, hi: number, d: number) =>
  typeof v === "number" && Number.isFinite(v) ? clamp(v, lo, hi) : d;

/** A stored layout, each field checked on its own: one bad field keeps the
 * others. */
export function parseLayout(raw: unknown): Layout {
  const d = DEFAULT_LAYOUT;
  if (!isRecord(raw)) return d;
  return {
    side: num(raw["side"], SIDE.min, SIDE.max, d.side),
    split: num(raw["split"], SPLIT.min, SPLIT.max, d.split),
    inspectorOpen:
      typeof raw["inspectorOpen"] === "boolean" ? raw["inspectorOpen"] : d.inspectorOpen,
    panelsOpen: typeof raw["panelsOpen"] === "boolean" ? raw["panelsOpen"] : d.panelsOpen,
    tab: TABS.find((t) => t === raw["tab"]) ?? d.tab,
  };
}
