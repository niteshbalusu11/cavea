// The buyer's map for the end-to-end check, from `?server=&key=&event=`.
// What the check reads is on `window.buyer`: the map once ready, every error
// it reported, the object under the pointer, the selection and its hold
// token, the times of pointer presses, and `watch`, which records the first
// frame at which a seat shows a status. Times are Unix ms with fractions
// (`performance.timeOrigin` plus the event's or frame's time), comparable
// between the two buyers' pages on this machine.
import { caveaServer, createSeatMap } from "./pkg/index.js";

const query = new URLSearchParams(location.search);
const element = document.getElementById("map");
document.body.style.margin = "0";
element.style.height = "100vh";
const buyer = {
  map: null,
  failed: null,
  errors: [],
  hovered: null,
  selection: { labels: [], holdToken: null },
  presses: [],
  seen: {},
  /** From now, records in `seen[label]` the first frame drawn with `label` in `status`. */
  watch(label, status) {
    delete buyer.seen[label];
    const look = (t) => {
      if (buyer.map?.findObject(label)?.status === status) {
        buyer.seen[label] = performance.timeOrigin + t;
      } else {
        requestAnimationFrame(look);
      }
    };
    requestAnimationFrame(look);
  },
};
window.buyer = buyer;
addEventListener("pointerdown", (e) => buyer.presses.push(performance.timeOrigin + e.timeStamp), {
  capture: true,
});
createSeatMap(element, {
  state: caveaServer({
    baseUrl: query.get("server"),
    publishableKey: query.get("key"),
    event: query.get("event"),
  }),
  onError: (error) => {
    buyer.errors.push(`${error.code}: ${error.message}`);
    // The check's recorder (tools/e2e/tab.ts) makes every reported error a problem.
    window.caveaE2e?.push(`map error ${error.code}: ${error.message}`);
  },
  onObjectHover: (object, at) => {
    buyer.hovered = object === null ? null : { label: object.label, at };
  },
  onSelectionChange: (selection) => {
    buyer.selection = {
      labels: selection.objects.map((o) => o.label),
      holdToken: selection.holdToken,
    };
  },
}).then(
  (map) => {
    buyer.map = map;
  },
  (error) => {
    buyer.failed = error.code ?? String(error);
  },
);
