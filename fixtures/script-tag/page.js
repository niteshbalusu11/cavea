// `window.Cavea` from the script tag. `?base=<url>` passes `assets.baseUrl`,
// for files served somewhere other than beside the script; `?chart=<url>`
// fetches the chart from elsewhere (the CDN check serves it with the script).
const errors = [];
window.compat = { errors };
addEventListener("securitypolicyviolation", (e) => errors.push(`csp ${e.violatedDirective}`));
const query = new URLSearchParams(location.search);
const base = query.get("base");
const element = document.getElementById("map");
element.style.height = "400px";
window.Cavea.createSeatMap(element, {
  chart: { url: query.get("chart") ?? "chart.ccf" },
  state: window.Cavea.customState({
    connect: async () => ({ kind: "map", unavailable: [] }),
    disconnect: () => undefined,
  }),
  ...(base === null ? {} : { assets: { baseUrl: base } }),
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
