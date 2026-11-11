// The events list (09 §4.1, CON-007): a table of the workspace's events,
// paged as the API pages it (oldest first, archived included, 08 §6.2),
// with search and filters over the pages loaded (the API has no filter or
// search yet, so the screen says how far they reach), each event's seat
// counts, the create form, and archive.
import { DropdownMenu } from "@kobalte/core/dropdown-menu";
import { A } from "@solidjs/router";
import { createMemo, createSignal, For, Show } from "solid-js";
import { checkChartList, checkEvent, checkEventList } from "../api/check.ts";
import { call } from "../api/client.ts";
import type { Chart, Counts, Event } from "../api/generated.ts";
import { createPages, loadAll } from "../api/pages.ts";
import { allows } from "../api/permissions.ts";
import { componentSignal, createQuery } from "../api/query.ts";
import { formatNumber, msg } from "../i18n/messages.ts";
import { workspace } from "../session.ts";
import { fieldInputClass } from "../ui/form.tsx";
import { Icon } from "../ui/Icon.tsx";
import { afterMenu, Confirm, menuContent, menuItem, notify } from "../ui/overlays.tsx";
import { report } from "./errors.ts";
import { EventForm } from "./EventForm.tsx";
import { Action, Empty, formatDateTime, LoadMore, Paged, table, td, th } from "./kit.tsx";

/** Events per page: each row asks for its counts. */
const PAGE = 25;
/** At most this many pages of charts are read for names and the form. */
const CHART_PAGES = 20;

/** Every chart (for names, the filter and the form), whole up to a bound. */
export async function loadCharts(signal: AbortSignal) {
  return loadAll(
    async (after) =>
      checkChartList(
        await call(
          "listCharts",
          { query: after === undefined ? { limit: 100 } : { limit: 100, after } },
          signal,
        ),
      ),
    CHART_PAGES,
  );
}

export const roleName = () => msg(`role.${workspace()?.role ?? "viewer"}`);
const cannotCreate = () =>
  allows(workspace(), "createEvent") ? null : msg("events.roleCannot", { role: roleName() });

export function Events() {
  const signal = componentSignal();
  const events = createPages(
    () => workspace()?.id ?? null,
    async (_ws, after, s) =>
      checkEventList(
        await call(
          "listEvents",
          { query: after === undefined ? { limit: PAGE } : { limit: PAGE, after } },
          s,
        ),
      ),
  );
  const charts = createQuery(
    () => workspace()?.id ?? null,
    (_ws, s) => loadCharts(s),
  );
  const chartList = (): Chart[] => {
    const s = charts.state();
    return s.status === "ready" ? s.data.data : [];
  };
  const chartName = (key: string) => chartList().find((c) => c.key === key)?.name ?? key;

  const [search, setSearch] = createSignal("");
  const [chart, setChart] = createSignal("");
  const [archived, setArchived] = createSignal(false);
  const [creating, setCreating] = createSignal(false);
  const [archiving, setArchiving] = createSignal<Event | null>(null);

  const loaded = (): Event[] => {
    const s = events.state();
    return s.status === "ready" ? s.data.data : [];
  };
  const more = () => {
    const s = events.state();
    return s.status === "ready" && s.data.next !== null;
  };
  const shown = createMemo(() => {
    const q = search().trim().toLocaleLowerCase();
    return loaded().filter(
      (e) =>
        (archived() || e.archived_at === null) &&
        (chart() === "" || e.chart === chart()) &&
        (q === "" ||
          e.key.toLocaleLowerCase().includes(q) ||
          (e.name ?? "").toLocaleLowerCase().includes(q)),
    );
  });
  const filtered = () => search().trim() !== "" || chart() !== "" || !archived();
  const chartsInList = createMemo(() =>
    // oxlint-disable-next-line unicorn/no-array-sort -- sorts a copy of ours; `toSorted` is ES2023, past the lib (ES2022).
    [...new Set(loaded().map((e) => e.chart))].sort(),
  );

  const archive = async (event: Event) => {
    try {
      const answer = checkEvent(await call("archiveEvent", { path: { event: event.key } }, signal));
      events.update((rows) => rows.map((r) => (r.key === answer.key ? answer : r)));
      notify(msg("events.archived", { name: event.name ?? event.key }));
    } catch (e) {
      report(e);
    }
  };

  return (
    <>
      <div class="flex flex-wrap items-center gap-3">
        <h1 class="mr-auto text-xl font-semibold text-fg">{msg("events.title")}</h1>
        <Action
          label={msg("events.new")}
          icon="plus"
          variant="primary"
          blocked={cannotCreate()}
          onClick={() => setCreating(true)}
        />
      </div>
      <Paged pages={events} loading={msg("events.loading")} failed={msg("events.failed")}>
        {(rows) => (
          <Show
            when={rows.length > 0}
            fallback={
              <Empty
                icon="calendar"
                title={msg("events.emptyTitle")}
                body={cannotCreate() === null ? msg("events.emptyBody") : msg("events.emptyReader")}
              >
                <Show when={cannotCreate() === null}>
                  <Action
                    label={msg("events.new")}
                    icon="plus"
                    variant="primary"
                    blocked={null}
                    onClick={() => setCreating(true)}
                  />
                </Show>
              </Empty>
            }
          >
            <div class="mt-5 flex flex-wrap items-center gap-3">
              <div class="relative w-72 max-w-full">
                <span class="pointer-events-none absolute inset-y-0 left-2 flex items-center text-faint">
                  <Icon name="search" />
                </span>
                <input
                  type="search"
                  class={`${fieldInputClass} pl-7`}
                  placeholder={msg("events.searchPlaceholder")}
                  aria-label={msg("events.search")}
                  value={search()}
                  onInput={(e) => setSearch(e.currentTarget.value)}
                />
              </div>
              <label class="flex items-center gap-2 text-base whitespace-nowrap text-fg">
                {msg("events.chartFilter")}
                <select
                  class={`${fieldInputClass} w-auto max-w-56`}
                  value={chart()}
                  onChange={(e) => setChart(e.currentTarget.value)}
                >
                  <option value="">{msg("events.allCharts")}</option>
                  <For each={chartsInList()}>
                    {(key) => <option value={key}>{chartName(key)}</option>}
                  </For>
                </select>
              </label>
              <label class="flex items-center gap-2 text-base text-fg">
                <input
                  type="checkbox"
                  class="size-4 accent-accent-fill"
                  checked={archived()}
                  onChange={(e) => setArchived(e.currentTarget.checked)}
                />
                {msg("events.showArchived")}
              </label>
              <p class="ml-auto text-sm text-muted" role="status">
                {more()
                  ? msg("events.countPartial", { n: shown().length, loaded: rows.length })
                  : msg("events.count", { n: shown().length })}
              </p>
            </div>
            <Show when={more() && filtered()}>
              <p class="mt-2 text-sm text-muted">{msg("events.filterReach")}</p>
            </Show>
            <Show
              when={shown().length > 0}
              fallback={<p class="mt-8 text-base text-muted">{msg("events.noMatch")}</p>}
            >
              <div class="mt-4 overflow-x-auto">
                <table class={table}>
                  <caption class="sr-only">{msg("events.caption")}</caption>
                  <thead>
                    <tr>
                      <th scope="col" class={th}>
                        {msg("events.colEvent")}
                      </th>
                      <th scope="col" class={th}>
                        {msg("events.colChart")}
                      </th>
                      <th scope="col" class={th}>
                        {msg("events.colSeats")}
                      </th>
                      <th scope="col" class={th}>
                        {msg("events.colCreated")}
                      </th>
                      <th scope="col" class={th}>
                        {msg("events.colStatus")}
                      </th>
                      <th scope="col" class={th}>
                        <span class="sr-only">{msg("events.colActions")}</span>
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    <For each={shown()}>
                      {(event) => (
                        <EventRow
                          event={event}
                          chartName={chartName(event.chart)}
                          onArchive={() => setArchiving(event)}
                        />
                      )}
                    </For>
                  </tbody>
                </table>
              </div>
            </Show>
            <LoadMore pages={events} />
          </Show>
        )}
      </Paged>

      <Show when={creating()}>
        <EventForm
          charts={chartList()}
          onClose={() => setCreating(false)}
          onCreated={(event) => {
            setCreating(false);
            notify(msg("events.created", { name: event.name ?? event.key }));
            events.refetch();
          }}
        />
      </Show>
      <Confirm
        open={archiving() !== null}
        title={msg("events.archiveTitle", {
          name: archiving()?.name ?? archiving()?.key ?? "",
        })}
        description={msg("events.archiveBody")}
        confirm={msg("events.archiveConfirm")}
        variant="danger"
        onConfirm={() => {
          const event = archiving();
          if (event !== null) void archive(event);
        }}
        onClose={() => setArchiving(null)}
      />
    </>
  );
}

function EventRow(props: { event: Event; chartName: string; onArchive: () => void }) {
  const canArchive = () => allows(workspace(), "archiveEvent") && props.event.archived_at === null;
  return (
    <tr>
      <td class={td}>
        <span class="block font-medium">{props.event.name ?? props.event.key}</span>
        <span class="block font-mono text-sm text-muted">{props.event.key}</span>
      </td>
      <td class={td}>
        <A href={`/charts/${encodeURIComponent(props.event.chart)}`} class="text-accent underline">
          {props.chartName}
        </A>
        <span class="block text-sm text-muted">
          {msg("events.versionShort", { number: props.event.version.number })}
        </span>
      </td>
      <td class={`${td} w-64`}>
        <EventCounts event={props.event.key} />
      </td>
      <td class={`${td} text-sm whitespace-nowrap text-muted`}>
        {formatDateTime(props.event.created_at)}
      </td>
      <td class={td}>
        <Show
          when={props.event.archived_at === null}
          fallback={
            <span class="rounded-sm bg-warning-soft px-1.5 text-xs text-warning">
              {msg("events.archivedBadge")}
            </span>
          }
        >
          <span class="text-sm text-muted">{msg("events.active")}</span>
        </Show>
      </td>
      <td class={`${td} w-10`}>
        <Show when={canArchive()}>
          <DropdownMenu gutter={4} placement="bottom-end">
            <DropdownMenu.Trigger
              class="inline-flex size-7 items-center justify-center rounded-md text-muted hover:bg-hover hover:text-fg"
              aria-label={msg("events.menu", { name: props.event.name ?? props.event.key })}
            >
              <Icon name="more" />
            </DropdownMenu.Trigger>
            <DropdownMenu.Portal>
              <DropdownMenu.Content class={menuContent}>
                <DropdownMenu.Item
                  class={`${menuItem} text-danger`}
                  onSelect={afterMenu(props.onArchive)}
                >
                  <Icon name="trash" />
                  {msg("events.archive")}
                </DropdownMenu.Item>
              </DropdownMenu.Content>
            </DropdownMenu.Portal>
          </DropdownMenu>
        </Show>
      </td>
    </tr>
  );
}

/** Booked, held, blocked and free seats as a bar and in words. The list
 * does not carry counts; each row reads its event's (08 §6.2). */
function EventCounts(props: { event: string }) {
  const counts = createQuery(
    () => {
      const ws = workspace()?.id;
      return ws === undefined ? null : props.event;
    },
    async (event, s) => checkEvent(await call("getEvent", { path: { event } }, s)).counts ?? null,
  );
  return (
    <Show
      when={(() => {
        const s = counts.state();
        return s.status === "ready" ? s.data : null;
      })()}
      fallback={
        <span class="text-sm text-faint">
          {counts.state().status === "error" ? msg("events.countsFailed") : "…"}
        </span>
      }
    >
      {(c) => <CountsBar counts={c()} />}
    </Show>
  );
}

export function CountsBar(props: { counts: Counts }) {
  const total = () => {
    const c = props.counts;
    const custom = Object.values(c.custom).reduce((a, b) => a + b, 0);
    return c.free + c.held + c.booked + c.blocked + custom;
  };
  const pct = (n: number) => (total() === 0 ? 0 : (n / total()) * 100);
  return (
    <div>
      <div class="flex h-2 overflow-hidden rounded-full bg-line" aria-hidden="true">
        <span class="bg-accent-fill" style={{ width: `${pct(props.counts.booked)}%` }} />
        <span class="bg-warning" style={{ width: `${pct(props.counts.held)}%` }} />
        <span class="bg-muted" style={{ width: `${pct(props.counts.blocked)}%` }} />
      </div>
      <p class="mt-1 text-sm text-muted">
        {msg("events.counts", {
          booked: formatNumber(props.counts.booked),
          held: formatNumber(props.counts.held),
          free: formatNumber(props.counts.free),
        })}
      </p>
    </div>
  );
}
