// S1 of the benchmark page (bench.ts): one cold load to chart-ready in this
// frame, which loads the SDK and nothing else, so what it fetches is the
// buyer's payload. The chart's bytes come from the parent, which compiled
// them; the marks and the resources fetched go back to it.
import { createSeatMap, customState } from "@cavea/renderer";

const container = document.getElementById("map") as HTMLDivElement;
/** What this frame fetched: file name, bytes over the network (0 from cache) and encoded size. */
const fetched = (): Array<{ name: string; transfer: number; encoded: number }> =>
  performance
    .getEntriesByType("resource")
    .filter((r): r is PerformanceResourceTiming => r instanceof PerformanceResourceTiming)
    .map((r) => ({
      name: (new URL(r.name).pathname.split("/").at(-1) ?? "").slice(0, 120),
      transfer: r.transferSize,
      encoded: r.encodedBodySize,
    }));

/** Sends the marks and the payload once the glyph atlas is in (it is not on the path to chart-ready), or after 3 s. */
async function report(marks: Record<string, number>): Promise<void> {
  for (
    let waited = 0;
    waited < 3000 && !fetched().some((r) => r.name.endsWith(".png"));
    waited += 50
  ) {
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  parent.postMessage({ marks, payload: fetched() }, location.origin);
}

addEventListener(
  "message",
  (e: MessageEvent<Uint8Array>) => {
    if (e.origin !== location.origin || !(e.data instanceof Uint8Array)) {
      return;
    }
    void createSeatMap(container, {
      chart: { bytes: e.data },
      state: customState({
        connect: () => Promise.resolve({ kind: "map", unavailable: [] }),
        disconnect: () => undefined,
      }),
      ui: { loading: false },
      onReady(info) {
        void report(info.marks);
      },
    });
  },
  { once: true },
);
parent.postMessage("ready", location.origin);
