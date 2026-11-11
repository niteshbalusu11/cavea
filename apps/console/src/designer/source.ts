// Where the designer's chart comes from and goes to. The designer takes a
// `ChartSource`; this slice has the local one (a file opened, a file
// downloaded), and the server-backed draft (DSN-035) is another
// implementation of the same two calls.

export interface ChartSource {
  /** The draft to open: chart document JSON, or `null` for none yet. */
  open(): Promise<string | null>;
  /** Stores the draft; resolves when it is stored. */
  save(draft: string, name: string): Promise<void>;
}

/** A chart with nothing in it and one category (07 §16: the templates of
 * the first-run screen arrive with DSN-042). */
export function emptyChart(name: string): string {
  return JSON.stringify({
    schema: 1,
    name,
    settings: {
      seatSize: 44,
      seatShape: "circle",
      labelTemplate: "{section}-{row}-{seat}",
      tableLabelTemplate: "{section}-{table}-{seat}",
      rowLabelDisplay: "both",
      alphaOverflow: "double",
    },
    nextOid: 1,
    categories: [{ key: "standard", label: "Standard", color: "#2563EB", accessible: false }],
    floors: [],
    nodes: [],
  });
}

/** A file name for a chart's name: what a file system accepts, `.json`. */
export function fileName(name: string): string {
  const base = name
    .replace(/[\\/:*?"<>|\p{Cc}]+/gu, " ")
    .trim()
    .slice(0, 120);
  return `${base === "" ? "chart" : base}.json`;
}

/** The local source: nothing to open at first, and saving downloads the
 * chart document. */
export const localSource: ChartSource = {
  open: () => Promise.resolve(null),
  save(draft, name) {
    const url = URL.createObjectURL(new Blob([draft], { type: "application/json" }));
    const a = document.createElement("a");
    a.href = url;
    a.download = fileName(name);
    a.click();
    // The download has started from the click; the URL is not needed after.
    setTimeout(() => URL.revokeObjectURL(url), 0);
    return Promise.resolve();
  },
};
