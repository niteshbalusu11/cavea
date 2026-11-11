// The page each bundler fixture builds, importing the package as a customer
// would: a map of the preset chart served beside the page, with a custom
// adapter. It reports to `window.compat`, which `fixtures/run.ts` reads: the
// first frame drawn, or how the map failed. `run.ts` copies this file into
// each fixture as `src/main.js`.
import { createSeatMap, customState } from "@cavea/renderer";

const errors = [];
window.compat = { errors };
addEventListener("securitypolicyviolation", (e) => errors.push(`csp ${e.violatedDirective}`));
const element = document.getElementById("map");
element.style.height = "400px";
createSeatMap(element, {
  chart: { url: new URL("chart.ccf", document.baseURI) },
  state: customState({
    connect: async () => ({ kind: "map", unavailable: [] }),
    disconnect: () => undefined,
  }),
  onError: (error) => errors.push(error.code),
  onMetrics: (metrics) => {
    if (metrics.marks.first_frame !== undefined) {
      window.compat = { firstFrame: metrics.marks.first_frame, errors };
    }
  },
}).catch((error) => {
  window.compat = { failed: error.code ?? String(error), errors };
});
