// A chart's page (09 §3.2, CON-006): its name, key, published version and
// draft state; the Versions tab with "create event from this version"; the
// Events tab; Settings with archive. Opening the designer on a chart from
// the server is DSN-035: until then the page offers no such action.
import { A, useParams, useSearchParams } from "@solidjs/router";
import { createSignal, For, Match, Show, Switch } from "solid-js";
import { checkChart, checkEventList } from "../api/check.ts";
import { call } from "../api/client.ts";
import type { Chart, Event, Version } from "../api/generated.ts";
import { loadAll } from "../api/pages.ts";
import { allows } from "../api/permissions.ts";
import { componentSignal, createQuery, type Query } from "../api/query.ts";
import { formatNumber, msg } from "../i18n/messages.ts";
import { workspace } from "../session.ts";
import { Confirm, notify } from "../ui/overlays.tsx";
import { report } from "./errors.ts";
import { EventForm, loadVersions } from "./EventForm.tsx";
import { roleName } from "./Events.tsx";
import {
  Action,
  CopyText,
  formatDateTime,
  ListError,
  ListLoading,
  Tabs,
  table,
  td,
  th,
} from "./kit.tsx";

/** The Events tab reads at most this many pages of events (the API cannot
 * list a chart's events yet) and shows them only when that is all. */
const EVENT_PAGES = 10;

type Tab = "versions" | "events" | "settings";
const TABS: Tab[] = ["versions", "events", "settings"];

/** Whether the draft changed after the published version was made. */
export function draftState(chart: Chart): "unpublished" | "changed" | "published" {
  const v = chart.published_version;
  if (v === null) return "unpublished";
  return Date.parse(chart.draft_updated_at) > Date.parse(v.created_at) ? "changed" : "published";
}

export function ChartDetail() {
  const params = useParams<{ chart: string }>();
  const query = createQuery(
    () => {
      const ws = workspace()?.id;
      return ws === undefined ? null : params.chart;
    },
    async (chart, s) => checkChart(await call("getChart", { path: { chart } }, s)),
  );
  return (
    <>
      <A href="/charts" class="text-sm text-accent underline">
        {msg("chart.back")}
      </A>
      <Switch>
        <Match when={query.state().status === "loading"}>
          <ListLoading label={msg("chart.loading")} rows={3} />
        </Match>
        <Match
          when={(() => {
            const s = query.state();
            return s.status === "error" ? s.error : null;
          })()}
        >
          {(e) => (
            <Show
              when={e().code !== "not_found"}
              fallback={<h1 class="mt-3 text-xl font-semibold text-fg">{msg("chart.notFound")}</h1>}
            >
              <ListError title={msg("chart.failed")} error={e()} onRetry={query.refetch} />
            </Show>
          )}
        </Match>
        <Match
          when={(() => {
            const s = query.state();
            return s.status === "ready" ? s.data : null;
          })()}
        >
          {(chart) => <Detail chart={chart()} query={query} />}
        </Match>
      </Switch>
    </>
  );
}

function Detail(props: { chart: Chart; query: Query<Chart> }) {
  const [params, setParams] = useSearchParams<{ tab?: string }>();
  const tab = (): Tab => TABS.find((t) => t === params.tab) ?? "versions";
  const [creating, setCreating] = createSignal<number | null>(null);
  const state = () => draftState(props.chart);
  return (
    <>
      <div class="mt-3 flex flex-wrap items-start gap-3">
        <div class="mr-auto min-w-0">
          <h1 class="truncate text-xl font-semibold text-fg">{props.chart.name}</h1>
          <CopyText value={props.chart.key} label={msg("chart.copyKey")} />
          <p class="mt-1 text-base text-muted">
            <Show when={props.chart.published_version} fallback={msg("charts.unpublished")}>
              {(v) =>
                msg("charts.published", {
                  seats: formatNumber(v().stats.seats),
                  version: v().number,
                })
              }
            </Show>
            <Show when={state() === "changed"}>
              <span class="ml-2 rounded-sm bg-warning-soft px-1.5 text-sm text-warning">
                {msg("chart.unpublishedChanges")}
              </span>
            </Show>
            <Show when={props.chart.archived_at !== null}>
              <span class="ml-2 rounded-sm bg-warning-soft px-1.5 text-sm text-warning">
                {msg("charts.archivedBadge")}
              </span>
            </Show>
          </p>
        </div>
      </div>
      <TabsFor
        tab={tab()}
        onChange={(t) => setParams({ tab: t === "versions" ? undefined : t }, { replace: true })}
        chart={props.chart}
        query={props.query}
        onCreate={(n) => setCreating(n)}
      />
      <Show when={creating()}>
        {(version) => (
          <EventForm
            charts={[]}
            chart={props.chart}
            version={version()}
            onClose={() => setCreating(null)}
            onCreated={(event) => {
              setCreating(null);
              notify(msg("events.created", { name: event.name ?? event.key }));
            }}
          />
        )}
      </Show>
    </>
  );
}

function TabsFor(props: {
  tab: Tab;
  onChange: (t: Tab) => void;
  chart: Chart;
  query: Query<Chart>;
  onCreate: (version: number) => void;
}) {
  return (
    <Tabs
      label={msg("chart.tabs")}
      value={props.tab}
      onChange={props.onChange}
      tabs={[
        {
          value: "versions",
          label: msg("chart.versions"),
          content: () => <Versions chart={props.chart} onCreate={props.onCreate} />,
        },
        {
          value: "events",
          label: msg("chart.events"),
          content: () => <ChartEvents chart={props.chart} />,
        },
        {
          value: "settings",
          label: msg("chart.settings"),
          content: () => <Settings chart={props.chart} query={props.query} />,
        },
      ]}
    />
  );
}

function Versions(props: { chart: Chart; onCreate: (version: number) => void }) {
  const versions = createQuery(
    () => {
      const ws = workspace()?.id;
      return ws === undefined ? null : props.chart.key;
    },
    (chart, s) => loadVersions(chart, s),
  );
  const blocked = () =>
    !allows(workspace(), "createEvent")
      ? msg("events.roleCannot", { role: roleName() })
      : props.chart.archived_at !== null
        ? msg("chart.archivedNoEvents")
        : null;
  return (
    <Switch>
      <Match when={versions.state().status === "loading"}>
        <ListLoading label={msg("chart.versionsLoading")} rows={3} />
      </Match>
      <Match
        when={(() => {
          const s = versions.state();
          return s.status === "error" ? s.error : null;
        })()}
      >
        {(e) => (
          <ListError title={msg("chart.versionsFailed")} error={e()} onRetry={versions.refetch} />
        )}
      </Match>
      <Match
        when={(() => {
          const s = versions.state();
          return s.status === "ready" ? s.data : null;
        })()}
      >
        {(data) => (
          <Show
            when={data().versions.length > 0}
            fallback={<p class="text-base text-muted">{msg("chart.noVersions")}</p>}
          >
            <Show when={!data().complete}>
              <p class="mb-2 text-sm text-muted">
                {msg("chart.versionsPartial", { n: data().versions.length })}
              </p>
            </Show>
            <div class="overflow-x-auto">
              <table class={table}>
                <caption class="sr-only">{msg("chart.versions")}</caption>
                <thead>
                  <tr>
                    <th scope="col" class={th}>
                      {msg("chart.colVersion")}
                    </th>
                    <th scope="col" class={th}>
                      {msg("chart.colPublished")}
                    </th>
                    <th scope="col" class={th}>
                      {msg("chart.colSeats")}
                    </th>
                    <th scope="col" class={th}>
                      <span class="sr-only">{msg("events.colActions")}</span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  <For each={data().versions}>
                    {(v: Version) => (
                      <tr>
                        <td class={td}>
                          {msg("events.versionShort", { number: v.number })}
                          <Show when={v.number === props.chart.published_version?.number}>
                            <span class="ml-2 rounded-sm bg-accent-soft px-1.5 text-xs text-accent">
                              {msg("chart.current")}
                            </span>
                          </Show>
                        </td>
                        <td class={`${td} text-sm text-muted`}>{formatDateTime(v.created_at)}</td>
                        <td class={td}>{formatNumber(v.stats.seats)}</td>
                        <td class={`${td} text-right`}>
                          <Action
                            label={msg("chart.createEvent")}
                            icon="plus"
                            class="h-7"
                            blocked={blocked()}
                            onClick={() => props.onCreate(v.number)}
                          />
                        </td>
                      </tr>
                    )}
                  </For>
                </tbody>
              </table>
            </div>
          </Show>
        )}
      </Match>
    </Switch>
  );
}

/** The chart's events, from a bounded read of every event: the list has
 * no filter by chart (08 §6.2), so the tab shows them only when it read
 * the whole list, and otherwise says so rather than show a part. */
function ChartEvents(props: { chart: Chart }) {
  const all = createQuery(
    () => workspace()?.id ?? null,
    (_ws, s) =>
      loadAll(
        async (after) =>
          checkEventList(
            await call(
              "listEvents",
              { query: after === undefined ? { limit: 100 } : { limit: 100, after } },
              s,
            ),
          ),
        EVENT_PAGES,
      ),
  );
  // Newest first; the list comes oldest first.
  // oxlint-disable-next-line unicorn/no-array-reverse -- reverses a copy of ours; `toReversed` is ES2023, past the lib (ES2022).
  const mine = (events: Event[]) => events.filter((e) => e.chart === props.chart.key).reverse();
  return (
    <Switch>
      <Match when={all.state().status === "loading"}>
        <ListLoading label={msg("chart.eventsLoading")} rows={3} />
      </Match>
      <Match
        when={(() => {
          const s = all.state();
          return s.status === "error" ? s.error : null;
        })()}
      >
        {(e) => <ListError title={msg("events.failed")} error={e()} onRetry={all.refetch} />}
      </Match>
      <Match
        when={(() => {
          const s = all.state();
          return s.status === "ready" ? s.data : null;
        })()}
      >
        {(data) => (
          <Show
            when={data().complete}
            fallback={
              <p class="max-w-xl text-base text-muted">
                {msg("chart.eventsTooMany", { n: EVENT_PAGES * 100 })}{" "}
                <A href="/events" class="text-accent underline">
                  {msg("chart.toEvents")}
                </A>
              </p>
            }
          >
            <Show
              when={mine(data().data).length > 0}
              fallback={<p class="text-base text-muted">{msg("chart.noEvents")}</p>}
            >
              <ul class="divide-y divide-line rounded-md border border-line">
                <For each={mine(data().data)}>
                  {(e) => (
                    <li class="flex flex-wrap items-center gap-3 px-3 py-2">
                      <span class="min-w-0 flex-1">
                        <span class="block font-medium text-fg">{e.name ?? e.key}</span>
                        <span class="block font-mono text-sm text-muted">{e.key}</span>
                      </span>
                      <span class="text-sm text-muted">
                        {msg("events.versionShort", { number: e.version.number })}
                      </span>
                      <span class="text-sm text-muted">{formatDateTime(e.created_at)}</span>
                      <Show when={e.archived_at !== null}>
                        <span class="rounded-sm bg-warning-soft px-1.5 text-xs text-warning">
                          {msg("events.archivedBadge")}
                        </span>
                      </Show>
                    </li>
                  )}
                </For>
              </ul>
            </Show>
          </Show>
        )}
      </Match>
    </Switch>
  );
}

function Settings(props: { chart: Chart; query: Query<Chart> }) {
  const signal = componentSignal();
  const [confirming, setConfirming] = createSignal(false);
  const blocked = () =>
    !allows(workspace(), "archiveChart")
      ? msg("chart.archiveRole", { role: roleName() })
      : props.chart.archived_at !== null
        ? msg("chart.alreadyArchived")
        : null;
  const archive = async () => {
    try {
      const chart = checkChart(
        await call("archiveChart", { path: { chart: props.chart.key } }, signal),
      );
      props.query.update(() => chart);
      notify(msg("charts.archived", { name: chart.name }));
    } catch (e) {
      report(e);
    }
  };
  return (
    <div class="max-w-xl space-y-6">
      <section>
        <h2 class="text-base font-semibold text-fg">{msg("charts.key")}</h2>
        <p class="mt-1 text-base text-muted">{msg("chart.keyFixed")}</p>
        <div class="mt-2">
          <CopyText value={props.chart.key} label={msg("chart.copyKey")} />
        </div>
      </section>
      <section>
        <h2 class="text-base font-semibold text-fg">{msg("chart.archiveTitle")}</h2>
        <p class="mt-1 text-base text-muted">{msg("charts.archiveBody")}</p>
        <Action
          class="mt-3 h-8"
          label={msg("charts.archive")}
          icon="trash"
          variant="danger"
          blocked={blocked()}
          onClick={() => setConfirming(true)}
        />
      </section>
      <Confirm
        open={confirming()}
        title={msg("charts.archiveTitle", { name: props.chart.name })}
        description={msg("charts.archiveBody")}
        confirm={msg("charts.archiveConfirm")}
        variant="danger"
        onConfirm={() => void archive()}
        onClose={() => setConfirming(false)}
      />
    </div>
  );
}
