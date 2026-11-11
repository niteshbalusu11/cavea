// The new-chart screen's templates (07 §16): ordinary chart documents,
// loaded when chosen. Theatre and arena bowl are `cavea gen theatre` (its
// stalls floor) and `cavea gen arena` with every block cut to its first
// rows (10 and 5); the banquet is written by hand with long tables, since
// round tables do not compile yet. Each compiles without findings
// (templates.test.ts).

export interface Template {
  id: "theatre" | "arena" | "banquet";
  seats: number;
  load: () => Promise<string>;
}

export const TEMPLATES: Template[] = [
  {
    id: "theatre",
    seats: 561,
    load: () => import("./templates/theatre.json?raw").then((m) => m.default),
  },
  {
    id: "arena",
    seats: 1680,
    load: () => import("./templates/arena.json?raw").then((m) => m.default),
  },
  {
    id: "banquet",
    seats: 156,
    load: () => import("./templates/banquet.json?raw").then((m) => m.default),
  },
];
