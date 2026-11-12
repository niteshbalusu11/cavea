// A chart's page (09 §3.2, CON-006): its name, key, published version and
// draft state; the Versions tab with "create event from this version"; the
// Events tab; Settings with archive. Open designer opens its draft in the
// designer (DSN-035).
import { A, useParams, useSearchParams } from "@solidjs/router";
import { createSignal, For, Match, Show, Switch } from "solid-js";
import { checkChart, checkEventList } from "../api/check.ts";
import { call } from "../api/client.ts";
import type { Chart, Version } from "../api/generated.ts";
import { createPages } from "../api/pages.ts";
import { componentSignal, createQuery, type Query } from "../api/query.ts";
import { designerPath } from "../designer/ChartRoute.tsx";
import { formatNumber, msg } from "../i18n/messages.ts";
import { buttonClass } from "../ui/controls.tsx";
import { workspace } from "../session.ts";
import { Confirm, notify } from "../ui/overlays.tsx";
import { report } from "./errors.ts";
import { cannot } from "./licence.ts";
import { EventForm, loadVersions } from "./EventForm.tsx";
import { roleName } from "./Events.tsx";
import {
  Action,
  CopyText,
  formatDateTime,
  ListError,
  ListLoading,
  LoadMore,
  Paged,
  orderText,
  SortHeader,
  SortNote,
  Tabs,
  table,
  tableBox,
  td,
  th,
} from "./kit.tsx";
import { by, rove, rowKeys, sortBy, sorted, type Sort } from "./tables.ts";

/** Who published a version (CON-006): the email when the server gives it
 * (to a role that sees the members, while the publisher is one), else what
 * they were, never an identifier. */
export function versionAuthor(v: Version, seesMembers: boolean): string {
  if (v.created_by_email !== undefined && v.created_by_email !== null) return v.created_by_email;
  if (v.created_by === null) return msg("chart.authorUnknown");
  if (v.created_by.startsWith("key_")) return msg("chart.authorKey");
  // To a role that sees the members, a member with no email given has left.
  return msg(seesMembers ? "chart.authorFormer" : "chart.authorMember");
}
const seesMembers = () => workspace()?.permissions.includes("settings") === true;
const author = (v: Version) => versionAuthor(v, seesMembers());

type VersionColumn = "version" | "published" | "seats" | "author";
const VERSION_ORDER: Record<VersionColumn, (a: Version, b: Version) => number> = {
  version: by.number((v) => v.number),
  published: by.time((v) => v.created_at),
  seats: by.number((v) => v.stats.seats),
  author: by.text(author),
};
const VERSION_KIND = {
  version: "number",
  published: "time",
  seats: "number",
  author: "text",
} as const;
const VERSION_SORTS: VersionColumn[] = ["version", "published", "seats", "author"];

/** Events per page on the Events tab. */
const PAGE = 25;

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
        <A
          href={designerPath(workspace()?.key ?? "", props.chart.key)}
          class={buttonClass("primary", "h-8")}
        >
          {msg("charts.open")}
        </A>
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
  // Read whole (up to its bound), newest first: sorted in the page, unless
  // it stopped short.
  const [sort, setSort] = createSignal<Sort<VersionColumn>>({
    column: "version",
    direction: "descending",
  });
  const blocked = () =>
    cannot("createEvent", () => msg("events.roleCannot", { role: roleName() })) ??
    (props.chart.archived_at !== null ? msg("chart.archivedNoEvents") : null);
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
            <div class={tableBox}>
              <table class={table}>
                <caption class="sr-only">{msg("chart.versions")}</caption>
                <thead>
                  <tr>
                    <For each={VERSION_SORTS}>
                      {(column) => (
                        <SortHeader
                          label={msg(VERSION_LABEL[column])}
                          column={column}
                          by={sortBy(column, {
                            server: [],
                            page: VERSION_SORTS,
                            complete: data().complete,
                          })}
                          sort={data().complete ? sort() : null}
                          first={column === "author" ? "ascending" : "descending"}
                          onSort={setSort}
                        />
                      )}
                    </For>
                    <th scope="col" class={th}>
                      <span class="sr-only">{msg("events.colActions")}</span>
                    </th>
                  </tr>
                </thead>
                <tbody onKeyDown={rowKeys}>
                  <For
                    each={
                      data().complete
                        ? sorted(data().versions, sort(), VERSION_ORDER)
                        : data().versions
                    }
                  >
                    {(v: Version, i) => (
                      <tr>
                        <td
                          class={td}
                          data-row-stop
                          tabIndex={i() === 0 ? 0 : -1}
                          onFocus={(e) => rove(e.currentTarget)}
                        >
                          <Show when={v.thumbnail}>
                            {(t) => (
                              <img
                                src={t().url}
                                alt=""
                                class="mr-2 inline-block h-9 w-12 rounded-sm border border-line bg-canvas object-contain align-middle"
                                loading="lazy"
                              />
                            )}
                          </Show>
                          {msg("events.versionShort", { number: v.number })}
                          <Show when={v.number === props.chart.published_version?.number}>
                            <span class="ml-2 rounded-sm bg-accent-soft px-1.5 text-xs text-accent">
                              {msg("chart.current")}
                            </span>
                          </Show>
                        </td>
                        <td class={`${td} text-sm text-muted`}>{formatDateTime(v.created_at)}</td>
                        <td class={td}>{formatNumber(v.stats.seats)}</td>
                        <td class={`${td} text-sm`}>{author(v)}</td>
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
            <SortNote
              said={
                data().complete
                  ? msg("table.sorted", {
                      column: msg(VERSION_LABEL[sort().column]),
                      order: orderText(VERSION_KIND[sort().column], sort().direction),
                    })
                  : msg("table.unsorted")
              }
              partial={data().complete ? undefined : msg("chart.versionsUnsortable")}
            />
          </Show>
        )}
      </Match>
    </Switch>
  );
}

const VERSION_LABEL = {
  version: "chart.colVersion",
  published: "chart.colPublished",
  seats: "chart.colSeats",
  author: "chart.colAuthor",
} as const;

/** The chart's events, from the list filtered by chart (08 §6.2), a page
 * at a time, as the events list pages: in order of creation, which the
 * server orders by (oldest first, or newest), so the order covers every
 * event, loaded or not. */
function ChartEvents(props: { chart: Chart }) {
  const [sort, setSort] = createSignal<Sort<"created">>({
    column: "created",
    direction: "ascending",
  });
  const events = createPages(
    () => {
      const ws = workspace()?.id;
      return ws === undefined ? null : JSON.stringify([props.chart.key, sort().direction]);
    },
    async (_key, after, s) => {
      const chart = props.chart.key;
      const order = sort().direction === "ascending" ? "oldest" : "newest";
      const query = { chart, limit: PAGE, order } as const;
      return checkEventList(
        await call("listEvents", { query: after === undefined ? query : { ...query, after } }, s),
      );
    },
  );
  return (
    <Paged pages={events} loading={msg("chart.eventsLoading")} failed={msg("events.failed")}>
      {(rows) => (
        <Show
          when={rows().length > 0}
          fallback={<p class="text-base text-muted">{msg("chart.noEvents")}</p>}
        >
          <div class={tableBox}>
            <table class={table}>
              <caption class="sr-only">{msg("chart.events")}</caption>
              <thead>
                <tr>
                  <th scope="col" class={th}>
                    {msg("events.colEvent")}
                  </th>
                  <th scope="col" class={th}>
                    {msg("chart.colVersion")}
                  </th>
                  <SortHeader
                    label={msg("events.colCreated")}
                    column="created"
                    by="server"
                    sort={sort()}
                    first="descending"
                    onSort={setSort}
                  />
                  <th scope="col" class={th}>
                    {msg("events.colStatus")}
                  </th>
                </tr>
              </thead>
              <tbody onKeyDown={rowKeys}>
                <For each={rows()}>
                  {(e) => (
                    <tr>
                      <td class={td}>
                        <A
                          href={`/events/${encodeURIComponent(e.key)}`}
                          class="block font-medium text-accent underline"
                          data-row-stop
                        >
                          {e.name ?? e.key}
                        </A>
                        <span class="block font-mono text-sm text-muted">{e.key}</span>
                      </td>
                      <td class={`${td} text-sm text-muted`}>
                        {msg("events.versionShort", { number: e.version.number })}
                      </td>
                      <td class={`${td} text-sm whitespace-nowrap text-muted`}>
                        {formatDateTime(e.created_at)}
                      </td>
                      <td class={td}>
                        <Show
                          when={e.archived_at === null}
                          fallback={
                            <span class="rounded-sm bg-warning-soft px-1.5 text-xs text-warning">
                              {msg("events.archivedBadge")}
                            </span>
                          }
                        >
                          <span class="text-sm text-muted">{msg("events.active")}</span>
                        </Show>
                      </td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </div>
          <SortNote
            said={msg("table.sorted", {
              column: msg("events.colCreated"),
              order: orderText("time", sort().direction),
            })}
          />
          <LoadMore pages={events} />
        </Show>
      )}
    </Paged>
  );
}

function Settings(props: { chart: Chart; query: Query<Chart> }) {
  const signal = componentSignal();
  const [confirming, setConfirming] = createSignal(false);
  const blocked = () =>
    cannot("archiveChart", () => msg("chart.archiveRole", { role: roleName() })) ??
    (props.chart.archived_at !== null ? msg("chart.alreadyArchived") : null);
  // Shown archived at once (09 §8); put back, saying so, if refused.
  const archive = async () => {
    const before = props.chart.archived_at;
    const name = props.chart.name;
    props.query.update((c) => ({ ...c, archived_at: new Date().toISOString() }));
    try {
      const chart = checkChart(
        await call("archiveChart", { path: { chart: props.chart.key } }, signal),
      );
      props.query.update(() => chart);
      notify(msg("charts.archived", { name: chart.name }));
    } catch (e) {
      props.query.update((c) => ({ ...c, archived_at: before }));
      if (!signal.aborted) report(e, msg("charts.archiveBack", { name }));
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
