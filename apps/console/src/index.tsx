import { Navigate, Route, Router } from "@solidjs/router";
import { For } from "solid-js";
import { render } from "solid-js/web";
import { Charts, Frame, NotBuilt, NotFound, SECTIONS } from "./console/Frame.tsx";
import { DesignerPage } from "./designer/Designer.tsx";
import { localSource } from "./designer/source.ts";
import { followTheme } from "./theme.ts";
import { ToastRegion } from "./ui/overlays.tsx";

followTheme();
const root = document.getElementById("root");
if (root === null) {
  throw new Error("#root element is missing from index.html");
}

// Two kinds of layout: the console frame, and full-window tools (the
// designer; later the event manager). A server-backed chart is another
// route with another `ChartSource` for the same designer (DSN-035).
render(
  () => (
    <>
      <Router>
        <Route path="/" component={() => <Navigate href="/charts" />} />
        <Route path="/designer/local" component={() => <DesignerPage source={localSource} />} />
        <Route path="/" component={Frame}>
          <Route path="/charts" component={Charts} />
          <For each={SECTIONS.slice(1)}>
            {(s) => <Route path={s.path} component={() => <NotBuilt section={s.label} />} />}
          </For>
          <Route path="*" component={NotFound} />
        </Route>
      </Router>
      <ToastRegion />
    </>
  ),
  root,
);
