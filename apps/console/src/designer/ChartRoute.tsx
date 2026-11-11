// The designer on a server chart: `/designer/<workspace key>/<chart key>`,
// for a signed-in member of that workspace whose role may read charts. The
// workspace is in the address, so a link opens the same chart for anyone,
// whatever workspace they last chose, and every request names it.
import { A, Navigate, useLocation, useParams } from "@solidjs/router";
import { createMemo, Match, Show, Switch } from "solid-js";
import { allows } from "../api/permissions.ts";
import { here, signInFor } from "../console/paths.ts";
import { Failed, Loading } from "../console/states.tsx";
import { msg } from "../i18n/messages.ts";
import { chooseWorkspace, me, session } from "../session.ts";
import { DesignerPage } from "./Designer.tsx";
import type { ServerSetup } from "./ServerChart.tsx";
import { localSource } from "./source.ts";

/** The designer's address for a chart. */
export const designerPath = (workspace: string, chart: string): string =>
  `/designer/${encodeURIComponent(workspace)}/${encodeURIComponent(chart)}`;

type Opened = { kind: "chart"; setup: ServerSetup } | { kind: "refused"; title: string };

export function ChartRoute() {
  const params = useParams<{ workspace: string; chart: string }>();
  const location = useLocation();
  // Decided once signed in, and kept while the page shows that chart: a
  // session that ends under the editor is the editor's to handle (its
  // work is written locally before it sends the person to sign in).
  let latched: { at: string; opened: Opened } | null = null;
  const opened = createMemo((): Opened | null => {
    const at = `${params.workspace}/${params.chart}`;
    if (latched?.at === at) return latched.opened;
    const m = me();
    if (m === null) return null;
    const w = m.workspaces.find((x) => x.key === params.workspace);
    let result: Opened;
    if (w === undefined) {
      result = {
        kind: "refused",
        title: msg("serverChart.noWorkspace", { workspace: params.workspace }),
      };
    } else if (!allows(w, "getDraft")) {
      result = {
        kind: "refused",
        title: msg("serverChart.noRole", { role: msg(`role.${w.role}`) }),
      };
    } else {
      chooseWorkspace(w.id);
      result = {
        kind: "chart",
        setup: {
          ref: { workspace: w.id, key: params.chart },
          user: m.user.id,
          canWrite: allows(w, "saveDraft"),
          canPublish: allows(w, "publishChart"),
          role: msg(`role.${w.role}`),
        },
      };
    }
    latched = { at, opened: result };
    return result;
  });

  return (
    <Show
      when={opened()}
      keyed
      fallback={
        <Switch>
          <Match when={session().kind === "failed"}>
            <Failed />
          </Match>
          <Match when={session().kind === "signedOut"}>
            <Navigate href={signInFor(here(location))} />
          </Match>
          <Match when={true}>
            <Loading />
          </Match>
        </Switch>
      }
    >
      {(o) =>
        o.kind === "chart" ? (
          <DesignerPage source={localSource} server={o.setup} />
        ) : (
          <main class="flex h-full flex-col items-center justify-center bg-app p-6 text-center">
            <h1 class="text-lg font-semibold text-fg">{o.title}</h1>
            <A href="/charts" class="mt-2 text-base text-accent underline">
              {msg("serverChart.back")}
            </A>
          </main>
        )
      }
    </Show>
  );
}
