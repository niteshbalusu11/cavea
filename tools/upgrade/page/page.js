// The map for the upgrade test's older-renderer check, from
// `?server=&key=&event=`, using only what every packaged renderer has had
// (`createSeatMap`, `caveaServer`, `findObject`, `select`). What the check
// reads is on `window.page`.
import { caveaServer, createSeatMap } from "./pkg/index.js";

const query = new URLSearchParams(location.search);
const page = { map: null, failed: null, errors: [], selection: { holdToken: null } };
window.page = page;
createSeatMap(document.getElementById("map"), {
  state: caveaServer({
    baseUrl: query.get("server"),
    publishableKey: query.get("key"),
    event: query.get("event"),
  }),
  onError: (error) => page.errors.push(`${error.code}: ${error.message}`),
  onSelectionChange: (selection) => {
    page.selection = { holdToken: selection.holdToken };
  },
}).then(
  (map) => {
    page.map = map;
  },
  (error) => {
    page.failed = error.code ?? String(error);
  },
);
