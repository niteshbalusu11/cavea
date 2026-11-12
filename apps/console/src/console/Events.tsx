// The events list (09 §4.1, CON-007): a table of the workspace's events,
// paged as the API pages it (08 §6.2), searched, filtered and ordered by the
// server (`eventFilters.ts`, kept in the URL), each row with the seat counts
// the list carries, the create form, and archive.
import { DropdownMenu } from "@kobalte/core/dropdown-menu";
import { A, useSearchParams } from "@solidjs/router";
import {
  createEffect,
  createMemo,
  createSignal,
  createUniqueId,
  For,
  on,
  onCleanup,
  Show,
} from "solid-js";
import { checkChartList, checkEvent, checkEventList } from "../api/check.ts";
import { call } from "../api/client.ts";
import type { Chart, Counts, Event } from "../api/generated.ts";
import { createPages, loadAll } from "../api/pages.ts";
import { refused } from "../api/permissions.ts";
import { componentSignal, createQuery } from "../api/query.ts";
import { formatNumber, msg } from "../i18n/messages.ts";
import { workspace } from "../session.ts";
import { Button } from "../ui/controls.tsx";
import { fieldInputClass } from "../ui/form.tsx";
import { Icon } from "../ui/Icon.tsx";
import { afterMenu, Confirm, menuContent, menuItem, notify } from "../ui/overlays.tsx";
import { report } from "./errors.ts";
import {
  clip,
  DEFAULTS,
  eventsQuery,
  filtered,
  day,
  fromParams,
  localZone,
  MAX_Q,
  rangeProblem,
  toParams,
  type Filters,
  type Order,
  type Shown,
} from "./eventFilters.ts";
import { EventForm } from "./EventForm.tsx";
import { cannot } from "./licence.ts";
import {
  Action,
  Empty,
  formatDateTime,
  LoadMore,
  Paged,
  orderText,
  SortHeader,
  SortNote,
  table,
  tableBox,
  td,
  th,
} from "./kit.tsx";
import { rowKeys, type Sort } from "./tables.ts";

/** Events per page. */
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
/** Why archiving is closed for now: the licence (the role hides it). */
const archiveClosed = () => cannot("archiveEvent", () => "");
const cannotCreate = () =>
  cannot("createEvent", () => msg("events.roleCannot", { role: roleName() }));

/** How long typing pauses before the list is asked again. */
const TYPING_MS = 300;

export function Events() {
  const signal = componentSignal();
  const [params, setParams] = useSearchParams();
  const filters = createMemo(() => fromParams(params), undefined, {
    equals: (a, b) => JSON.stringify(a) === JSON.stringify(b),
  });
  const set = (patch: Partial<Filters>) =>
    setParams(toParams({ ...filters(), ...patch }), { replace: true });
  // The search box shows what is typed at once; the list follows a pause.
  const [typed, setTyped] = createSignal(filters().q);
  let typing: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(typing));
  const onType = (value: string) => {
    setTyped(value);
    clearTimeout(typing);
    typing = setTimeout(() => set({ q: clip(value) }), TYPING_MS);
  };
  // The creation range as picked; applied when it is a range (D-93).
  const [range, setRange] = createSignal({ from: filters().from, to: filters().to });
  const rangeError = () => rangeProblem(day(range().from), day(range().to));
  // The URL changed by navigation (a link, back, Clear): the inputs follow.
  createEffect(
    on(
      () => [filters().from, filters().to] as const,
      ([from, to]) => setRange({ from, to }),
      { defer: true },
    ),
  );
  const onDate = (end: "from" | "to", value: string) => {
    const picked = { ...range(), [end]: value };
    setRange(picked);
    const from = day(picked.from);
    const to = day(picked.to);
    // An inverted range is said on its end and not sent.
    if (rangeProblem(from, to) === null) set({ from, to });
  };
  const dateIds = { zone: createUniqueId(), error: createUniqueId() };
  const clear = () => {
    clearTimeout(typing);
    setTyped("");
    setRange({ from: "", to: "" });
    setParams(toParams(DEFAULTS), { replace: true });
  };

  // A new search, filter or order is a new list: the request for the one
  // before is cancelled (`createQuery`).
  const events = createPages(
    () => {
      const ws = workspace()?.id;
      return ws === undefined ? null : JSON.stringify([ws, filters()]);
    },
    async (_key, after, s) =>
      checkEventList(await call("listEvents", { query: eventsQuery(filters(), PAGE, after) }, s)),
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

  /** The only sortable column: creation, as the server orders it. */
  const createdSort = (): Sort<"created"> => ({
    column: "created",
    direction: filters().order === "newest" ? "descending" : "ascending",
  });
  const [creating, setCreating] = createSignal(false);
  const [archiving, setArchiving] = createSignal<Event | null>(null);

  const more = () => {
    const s = events.state();
    return s.status === "ready" && s.data.next !== null;
  };

  const archive = async (event: Event) => {
    try {
      const answer = checkEvent(await call("archiveEvent", { path: { event: event.key } }, signal));
      events.update((rows) => rows.map((r) => (r.key === answer.key ? answer : r)));
      // The list is the server's view: an archived event leaves "active".
      events.refetch();
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
      <div
        role="search"
        class="mt-5 flex flex-wrap items-center gap-3"
        aria-label={msg("events.filters")}
      >
        <div class="relative w-72 max-w-full">
          <span class="pointer-events-none absolute inset-y-0 left-2 flex items-center text-faint">
            <Icon name="search" />
          </span>
          <input
            type="search"
            class={`${fieldInputClass} pl-7`}
            placeholder={msg("events.searchPlaceholder")}
            aria-label={msg("events.search")}
            maxLength={MAX_Q}
            value={typed()}
            onInput={(e) => onType(e.currentTarget.value)}
          />
        </div>
        <label class="flex items-center gap-2 text-base whitespace-nowrap text-fg">
          {msg("events.chartFilter")}
          <select
            class={`${fieldInputClass} w-auto max-w-56`}
            value={filters().chart}
            onChange={(e) => set({ chart: e.currentTarget.value })}
          >
            <option value="">{msg("events.allCharts")}</option>
            {/* Each option says whether it is chosen: the options are made
                again when the charts load, after the select's value was set. */}
            <For each={chartList()}>
              {(c) => (
                <option value={c.key} selected={c.key === filters().chart}>
                  {c.name}
                </option>
              )}
            </For>
            <Show
              when={filters().chart !== "" && !chartList().some((c) => c.key === filters().chart)}
            >
              <option value={filters().chart} selected>
                {filters().chart}
              </option>
            </Show>
          </select>
        </label>
        <label class="flex items-center gap-2 text-base whitespace-nowrap text-fg">
          {msg("events.shownFilter")}
          <select
            class={`${fieldInputClass} w-auto`}
            value={filters().shown}
            onChange={(e) => set({ shown: e.currentTarget.value as Shown })}
          >
            <option value="active">{msg("events.shownActive")}</option>
            <option value="archived">{msg("events.shownArchived")}</option>
            <option value="all">{msg("events.shownAll")}</option>
          </select>
        </label>
        <label class="flex items-center gap-2 text-base whitespace-nowrap text-fg">
          {msg("events.order")}
          <select
            class={`${fieldInputClass} w-auto`}
            value={filters().order}
            onChange={(e) => set({ order: e.currentTarget.value as Order })}
          >
            <option value="oldest">{msg("events.orderOldest")}</option>
            <option value="newest">{msg("events.orderNewest")}</option>
          </select>
        </label>
        <div class="flex flex-wrap items-center gap-3">
          <label class="flex items-center gap-2 text-base whitespace-nowrap text-fg">
            {msg("events.createdFrom")}
            <input
              type="date"
              class={`${fieldInputClass} w-auto`}
              value={range().from}
              max={day(range().to) || undefined}
              aria-describedby={dateIds.zone}
              onChange={(e) => onDate("from", e.currentTarget.value)}
            />
          </label>
          <label class="flex items-center gap-2 text-base whitespace-nowrap text-fg">
            {msg("events.createdUntil")}
            <input
              type="date"
              class={`${fieldInputClass} w-auto`}
              value={range().to}
              min={day(range().from) || undefined}
              aria-invalid={rangeError() === null ? undefined : true}
              aria-describedby={
                rangeError() === null ? dateIds.zone : `${dateIds.error} ${dateIds.zone}`
              }
              onChange={(e) => onDate("to", e.currentTarget.value)}
            />
          </label>
          <span id={dateIds.zone} class="text-sm text-muted">
            {msg("events.dateZone", { zone: localZone() })}
          </span>
          <Show when={rangeError()}>
            {(problem) => (
              <span id={dateIds.error} role="alert" class="text-sm text-danger">
                {problem()}
              </span>
            )}
          </Show>
        </div>
        <Show when={filtered(filters())}>
          <Button class="h-8" onClick={clear}>
            {msg("events.clearFilters")}
          </Button>
        </Show>
      </div>
      <Paged pages={events} loading={msg("events.loading")} failed={msg("events.failed")}>
        {(rows) => (
          <Show
            when={rows().length > 0}
            fallback={
              <Show
                when={!filtered(filters())}
                fallback={
                  <div class="mt-8" role="status">
                    <p class="text-base text-muted">{msg("events.noMatch")}</p>
                    <Button class="mt-3 h-8" onClick={clear}>
                      {msg("events.clearFilters")}
                    </Button>
                  </div>
                }
              >
                <Empty
                  icon="calendar"
                  title={msg("events.emptyTitle")}
                  body={
                    cannotCreate() === null ? msg("events.emptyBody") : msg("events.emptyReader")
                  }
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
              </Show>
            }
          >
            <p class="mt-3 text-sm text-muted" role="status">
              {more()
                ? msg("events.countMore", { n: rows().length })
                : rows().length === 1
                  ? msg("events.countOne")
                  : msg("events.count", { n: rows().length })}
            </p>
            <div class={`mt-2 ${tableBox}`}>
              <table class={table}>
                <caption class="sr-only">
                  {filters().order === "newest"
                    ? msg("events.captionNewest")
                    : msg("events.caption")}
                </caption>
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
                    {/* The server orders by creation (`order`): every event, not only those loaded. */}
                    <SortHeader
                      label={msg("events.colCreated")}
                      column="created"
                      by="server"
                      sort={createdSort()}
                      first="descending"
                      onSort={(s) =>
                        set({ order: s.direction === "ascending" ? "oldest" : "newest" })
                      }
                    />
                    <th scope="col" class={th}>
                      {msg("events.colStatus")}
                    </th>
                    <th scope="col" class={th}>
                      <span class="sr-only">{msg("events.colActions")}</span>
                    </th>
                  </tr>
                </thead>
                <tbody onKeyDown={rowKeys}>
                  <For each={rows()}>
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
            <SortNote
              said={msg("table.sorted", {
                column: msg("events.colCreated"),
                order: orderText("time", filters().order === "newest" ? "descending" : "ascending"),
              })}
            />
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
  // The role decides whether the menu is there; the licence, whether its item works.
  const canArchive = () =>
    refused(workspace(), "archiveEvent") !== "role" && props.event.archived_at === null;
  return (
    <tr>
      <td class={td}>
        <A
          href={`/events/${encodeURIComponent(props.event.key)}`}
          class="block font-medium text-accent underline"
          data-row-stop
        >
          {props.event.name ?? props.event.key}
        </A>
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
      <td class={`${td} lg:w-64`}>
        <CountsBar counts={props.event.counts} />
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
              data-focus-id={`event-menu-${props.event.key}`}
              class="inline-flex size-7 items-center justify-center rounded-md text-muted hover:bg-hover hover:text-fg"
              aria-label={msg("events.menu", { name: props.event.name ?? props.event.key })}
            >
              <Icon name="more" />
            </DropdownMenu.Trigger>
            <DropdownMenu.Portal>
              <DropdownMenu.Content class={menuContent}>
                <DropdownMenu.Item
                  class={`${menuItem} text-danger`}
                  disabled={archiveClosed() !== null}
                  onSelect={afterMenu(props.onArchive)}
                >
                  <Icon name="trash" />
                  {msg("events.archive")}
                </DropdownMenu.Item>
                <Show when={archiveClosed()}>
                  {(why) => <p class="max-w-64 px-2 py-1 text-sm text-muted">{why()}</p>}
                </Show>
              </DropdownMenu.Content>
            </DropdownMenu.Portal>
          </DropdownMenu>
        </Show>
      </td>
    </tr>
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
