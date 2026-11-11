import { Navigate, Route, Router } from "@solidjs/router";
import { For, lazy, type JSX } from "solid-js";
import { render } from "solid-js/web";
import { Account } from "./console/Account.tsx";
import { ChartDetail } from "./console/ChartDetail.tsx";
import { Charts } from "./console/Charts.tsx";
import { Developers } from "./console/Developers.tsx";
import { Events } from "./console/Events.tsx";
import { Frame, Gate, NotBuilt, NotFound, SECTIONS } from "./console/Frame.tsx";
import { Invite } from "./console/Invite.tsx";
import { INVITE_PATHS, takeInviteToken } from "./console/inviteToken.ts";
import { BASE } from "./console/paths.ts";
import { Settings } from "./console/Settings.tsx";
import { Setup } from "./console/Setup.tsx";
import { takeSetupToken } from "./console/setupToken.ts";
import { SignIn } from "./console/SignIn.tsx";
import { DesignerPage } from "./designer/Designer.tsx";
import { localSource } from "./designer/source.ts";
import { loadSession, watchSession } from "./session.ts";
import { followTheme } from "./theme.ts";
import { ToastRegion } from "./ui/overlays.tsx";

followTheme();
// Before the router reads the address: the setup token and an invitation's
// leave the address bar at once (CON-002, CON-012).
takeInviteToken();
takeSetupToken();
void loadSession();
watchSession();
const root = document.getElementById("root");
if (root === null) {
  throw new Error("#root element is missing from index.html");
}

// Two kinds of layout: the console frame, and full-window tools (the
// designer, the event page with its manager). A server-backed chart is another
// route with another `ChartSource` for the same designer (DSN-035).
// Routes are under `BASE`, Vite's `base` (CON-015). The server logs the
// setup link as `/console/setup`: under the root in development, that path
// is the setup page too.
const [charts, ...others] = SECTIONS;
/** The event page loads the renderer: only when an event is opened. */
const EventPage = lazy(async () => ({
  default: (await import("./console/manager/EventPage.tsx")).EventPage,
}));
/** The sections built so far; the others say they are not built yet. */
const SCREENS: Record<string, () => JSX.Element> = {
  "/events": Events,
  "/developers": Developers,
  "/settings": Settings,
};
render(
  () => (
    <>
      <Router base={BASE}>
        <Route path="/" component={() => <Navigate href="/charts" />} />
        <Route path={BASE === "" ? ["/setup", "/console/setup"] : "/setup"} component={Setup} />
        <Route path="/sign-in" component={SignIn} />
        <Route path={INVITE_PATHS} component={Invite} />
        <Route path="/designer/local" component={() => <DesignerPage source={localSource} />} />
        <Route path={["/events/:event", "/events/:event/settings"]} component={EventPage} />
        <Route path="/" component={Frame}>
          <Route
            path="/charts"
            component={() =>
              charts === undefined ? null : (
                <Gate section={charts}>
                  <Charts />
                </Gate>
              )
            }
          />
          <Route
            path="/charts/:chart"
            component={() =>
              charts === undefined ? null : (
                <Gate section={charts}>
                  <ChartDetail />
                </Gate>
              )
            }
          />
          <For each={others}>
            {(s) => (
              <Route
                path={s.path}
                component={() => (
                  <Gate section={s}>{SCREENS[s.path]?.() ?? <NotBuilt section={s.label} />}</Gate>
                )}
              />
            )}
          </For>
          <Route path="/account" component={Account} />
          <Route path="*" component={NotFound} />
        </Route>
      </Router>
      <ToastRegion />
    </>
  ),
  root,
);
