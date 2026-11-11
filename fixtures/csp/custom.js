// The package's ES module, served from this origin under `pkg/`, with a
// custom adapter: no bundler, so nothing but the SDK's own requests.
import { createSeatMap, customState } from "./pkg/index.js";

const errors = [];
window.compat = { errors };
addEventListener("securitypolicyviolation", (e) => errors.push(`csp ${e.violatedDirective}`));
const element = document.getElementById("map");
element.style.height = "400px";
createSeatMap(element, {
  chart: { url: "chart.ccf" },
  state: customState({
    connect: async () => ({ kind: "map", unavailable: [] }),
    disconnect: () => undefined,
  }),
  ui: { loading: false },
  onError: (error) => errors.push(error.code),
  onMetrics: (metrics) => {
    if (metrics.marks.first_frame !== undefined) {
      window.compat = { firstFrame: metrics.marks.first_frame, errors };
    }
  },
}).catch((error) => {
  window.compat = { failed: error.code ?? String(error), errors };
});
