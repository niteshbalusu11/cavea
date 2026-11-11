// `caveaServer` from `?server=&key=&event=&transport=`: the chart, the event,
// its state and the realtime stream all come from the server's origin.
import { caveaServer, createSeatMap } from "./pkg/index.js";

const errors = [];
window.compat = { errors };
addEventListener("securitypolicyviolation", (e) => errors.push(`csp ${e.violatedDirective}`));
const query = new URLSearchParams(location.search);
const element = document.getElementById("map");
element.style.height = "400px";
createSeatMap(element, {
  state: caveaServer({
    baseUrl: query.get("server"),
    publishableKey: query.get("key"),
    event: query.get("event"),
    transport: query.get("transport"),
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
