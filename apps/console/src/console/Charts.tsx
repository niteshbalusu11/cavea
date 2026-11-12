// The charts list (09 §3.1, CON-005): the workspace's charts as cards,
// searched by name or key, sorted, archived ones on request; create a chart,
// import one from a chart document, and from a card's menu export, duplicate
// or archive it. What the role cannot do is not offered (CON-004).
// A card shows its published version's thumbnail (DSN-040), made by the
// designer as it publishes, or a placeholder, and how many events use the
// chart, active and archived, linking to its Events tab.
import { DropdownMenu } from "@kobalte/core/dropdown-menu";
import { A, useNavigate } from "@solidjs/router";
import { createMemo, createSignal, For, Match, Show, Switch } from "solid-js";
import { checkChartList } from "../api/check.ts";
import { call } from "../api/client.ts";
import type { Chart, ChartDocument } from "../api/generated.ts";
import { refused } from "../api/permissions.ts";
import { asApiError, componentSignal, createQuery } from "../api/query.ts";
import { formatNumber, msg } from "../i18n/messages.ts";
import { loadSession, workspace } from "../session.ts";
import { Button, buttonClass, Tip } from "../ui/controls.tsx";
import { Field, fieldInputClass, FormError } from "../ui/form.tsx";
import { Icon } from "../ui/Icon.tsx";
import { afterMenu, Confirm, Dialog, menuContent, menuItem, notify } from "../ui/overlays.tsx";
import { describe, reference, report } from "./errors.ts";
import { cannot } from "./licence.ts";
import { designerPath } from "../designer/ChartRoute.tsx";
import { NewChart } from "../designer/NewChart.tsx";
import { TEMPLATES, type Template } from "../designer/templates.ts";
import { SelectField } from "./kit.tsx";

/** The largest draft the server takes (08 §6.2). */
const MAX_DOCUMENT_BYTES = 16 * 1024 * 1024;

type Sort = "edited" | "name";

/** Every page of the list: a workspace has tens or hundreds of charts, and
 * search and sort need them all. */
async function loadAll(signal: AbortSignal): Promise<Chart[]> {
  const all: Chart[] = [];
  let after: string | undefined;
  for (;;) {
    const page = checkChartList(
      await call(
        "listCharts",
        { query: after === undefined ? { limit: 100 } : { limit: 100, after } },
        signal,
      ),
    );
    all.push(...page.data);
    if (page.next === null) return all;
    after = page.next;
  }
}

/** How many events use a chart, as its card and its page say it: active
 * ones, and archived ones when there are any. */
export function eventsSummary(events: Chart["events"]): string {
  const { active: n, archived } = events;
  if (archived === 0) {
    return n === 0
      ? msg("charts.eventsNone")
      : n === 1
        ? msg("charts.eventsOne")
        : msg("charts.eventsMany", { n });
  }
  const id =
    n === 0
      ? "charts.eventsActiveNone"
      : n === 1
        ? "charts.eventsActiveOne"
        : "charts.eventsActiveMany";
  return msg(id, { n, archived });
}

/** The chart page's Events tab. */
export const eventsPath = (key: string) => `/charts/${encodeURIComponent(key)}?tab=events`;

/** A chart key from a name: lowercase letters, digits and hyphens. */
export function keyFrom(name: string): string {
  return name
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 128);
}

const dates = new Intl.DateTimeFormat("en", { dateStyle: "medium", timeStyle: "short" });
const formatDate = (iso: string) => {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : dates.format(d);
};

const role = () => msg(`role.${workspace()?.role ?? "viewer"}`);
/** Why a chart cannot be made now: the role, or the expired licence. */
const writeBlocked = () => cannot("createChart", () => msg("charts.roleCannot", { role: role() }));
/** Why archiving is closed for now: the licence (the role hides it). */
const archiveClosed = () => cannot("archiveChart", () => "");
/** The role writes charts (the licence may close it for now). */
const roleWrites = () => refused(workspace(), "createChart") !== "role";
const textProblem = (v: string) =>
  v.trim() === "" || [...v].length > 128 ? msg("charts.textRule") : null;

/** A new chart's starting point (07 §16): blank, a template, or a
 * reference image (placed in the designer, which opens after creating). */
export type Start = "blank" | "image" | Template["id"];

/** What the form for a new chart starts from. */
type Draft =
  | { from: "empty"; start: Start }
  | { from: "file"; document: ChartDocument; name: string }
  | { from: "chart"; chart: Chart };

/** A button that makes a chart, or the same shown disabled with the reason. */
const writeButton = (
  label: string,
  icon: "plus" | "download",
  run: () => void,
  primary: boolean,
) => (
  <Show
    when={writeBlocked() === null}
    fallback={
      <Tip label={writeBlocked() ?? ""}>
        <span
          tabIndex={0}
          aria-disabled="true"
          aria-label={`${label}: ${writeBlocked() ?? ""}`}
          class={buttonClass(primary ? "primary" : "secondary", "h-8")}
        >
          <Icon name={icon} />
          {label}
        </span>
      </Tip>
    }
  >
    <Button variant={primary ? "primary" : "secondary"} icon={icon} class="h-8" onClick={run}>
      {label}
    </Button>
  </Show>
);

export function Charts() {
  const signal = componentSignal();
  const navigate = useNavigate();
  const list = createQuery(
    () => workspace()?.id ?? null,
    (_ws, s) => loadAll(s),
  );
  const [search, setSearch] = createSignal("");
  const [sort, setSort] = createSignal<Sort>("edited");
  const [archived, setArchived] = createSignal(false);
  const [form, setForm] = createSignal<Draft | null>(null);
  const [archiving, setArchiving] = createSignal<Chart | null>(null);
  let file: HTMLInputElement | undefined;

  const charts = () => {
    const s = list.state();
    return s.status === "ready" ? s.data : [];
  };
  const active = () => charts().filter((c) => c.archived_at === null);
  const shown = createMemo(() => {
    const q = search().trim().toLocaleLowerCase();
    const rows = charts().filter(
      (c) =>
        (archived() || c.archived_at === null) &&
        (q === "" ||
          c.name.toLocaleLowerCase().includes(q) ||
          c.key.toLocaleLowerCase().includes(q)),
    );
    // oxlint-disable-next-line unicorn/no-array-sort -- `rows` is ours; `toSorted` is ES2023, past the lib (ES2022).
    return rows.sort((a, b) =>
      sort() === "name"
        ? a.name.localeCompare(b.name, "en", { sensitivity: "base" }) || a.key.localeCompare(b.key)
        : b.draft_updated_at.localeCompare(a.draft_updated_at),
    );
  });

  const importFile = async (f: File) => {
    if (f.size > MAX_DOCUMENT_BYTES) return notify(msg("charts.importTooLarge"), "error");
    let document: unknown;
    try {
      document = JSON.parse(await f.text());
    } catch {
      return notify(msg("charts.importNotJson"), "error");
    }
    if (typeof document !== "object" || document === null || Array.isArray(document)) {
      return notify(msg("charts.importNotJson"), "error");
    }
    const named = (document as Record<string, unknown>)["name"];
    const fallback = f.name.replace(/\.json$/i, "");
    setForm({
      from: "file",
      document: document as ChartDocument,
      name: typeof named === "string" && named.trim() !== "" ? named : fallback,
    });
  };

  const exportChart = async (chart: Chart) => {
    try {
      const document = await call("getDraft", { path: { chart: chart.key } }, signal);
      const blob = new Blob([JSON.stringify(document, null, 2)], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const a = Object.assign(window.document.createElement("a"), {
        href: url,
        download: `${chart.key}.json`,
      });
      a.click();
      setTimeout(() => URL.revokeObjectURL(url), 0);
    } catch (e) {
      report(e);
    }
  };

  // Shown archived at once (09 §8): the card leaves the list, or shows
  // Archived; put back, saying so, if the server refuses.
  const archive = async (chart: Chart) => {
    const mark = (archived_at: string | null) =>
      list.update((all) => all.map((c) => (c.key === chart.key ? { ...c, archived_at } : c)));
    mark(new Date().toISOString());
    try {
      await call("archiveChart", { path: { chart: chart.key } }, signal);
      notify(msg("charts.archived", { name: chart.name }));
      list.refetch();
    } catch (e) {
      mark(chart.archived_at);
      if (!signal.aborted) report(e, msg("charts.archiveBack", { name: chart.name }));
    }
  };

  const newChart = () =>
    writeButton(msg("charts.new"), "plus", () => setForm({ from: "empty", start: "blank" }), true);
  const importChart = () =>
    writeButton(msg("charts.import"), "download", () => file?.click(), false);

  return (
    <>
      <input
        ref={(el) => (file = el)}
        type="file"
        accept=".json,application/json"
        class="hidden"
        tabIndex={-1}
        aria-hidden="true"
        onChange={(e) => {
          const f = e.currentTarget.files?.[0];
          e.currentTarget.value = "";
          if (f !== undefined) void importFile(f);
        }}
      />
      <div class="flex flex-wrap items-center gap-3">
        <h1 class="mr-auto text-xl font-semibold text-fg">{msg("charts.title")}</h1>
        <Button variant="ghost" icon="file" class="h-8" onClick={() => navigate("/designer/local")}>
          {msg("charts.openDesigner")}
        </Button>
        {importChart()}
        {newChart()}
      </div>

      <Switch>
        <Match when={list.state().status === "loading"}>
          <p role="status" class="sr-only">
            {msg("charts.loading")}
          </p>
          <div
            class="mt-6 grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-4"
            aria-hidden="true"
          >
            <For each={[0, 1, 2, 3]}>
              {() => <div class="h-60 animate-pulse rounded-lg bg-hover" />}
            </For>
          </div>
        </Match>
        <Match
          when={(() => {
            const s = list.state();
            return s.status === "error" ? s.error : null;
          })()}
        >
          {(error) => (
            <div role="alert" class="mt-6 max-w-xl rounded-lg border border-line p-4">
              <p class="text-base text-fg">{msg("charts.failed")}</p>
              <p class="mt-1 text-base text-muted">{describe(error())}</p>
              <Show when={reference(error())}>
                {(r) => <p class="mt-1 text-sm text-faint">{r()}</p>}
              </Show>
              <Button class="mt-3 h-8" onClick={() => list.refetch()}>
                {msg("action.retry")}
              </Button>
            </div>
          )}
        </Match>
        <Match when={charts().length === 0 || (active().length === 0 && !archived())}>
          <div class="mt-10 flex max-w-2xl flex-col items-start gap-3 rounded-lg border border-dashed border-line-strong p-8">
            <Icon name="chart" class="size-8 text-faint" />
            <h2 class="text-lg font-semibold text-fg">{msg("charts.emptyTitle")}</h2>
            <p class="text-base text-muted">
              {roleWrites() ? msg("charts.emptyBody") : msg("charts.emptyReader")}
            </p>
            <Show when={roleWrites()}>
              <Show
                when={writeBlocked() === null}
                fallback={
                  <div class="flex gap-2">
                    {newChart()}
                    {importChart()}
                  </div>
                }
              >
                {/* The designer's starting points; each asks for the chart's name first. */}
                <NewChart
                  choices={{
                    empty: () => setForm({ from: "empty", start: "blank" }),
                    template: (t) => setForm({ from: "empty", start: t.id }),
                    image: () => setForm({ from: "empty", start: "image" }),
                    open: () => file?.click(),
                  }}
                  openLabel={msg("charts.import")}
                />
              </Show>
            </Show>
            <Show when={charts().length > 0}>
              <button
                type="button"
                class="text-base text-accent underline"
                onClick={() => setArchived(true)}
              >
                {msg("charts.showArchivedCount", { n: charts().length })}
              </button>
            </Show>
          </div>
        </Match>
        <Match when={true}>
          <div class="mt-5 flex flex-wrap items-center gap-3">
            <div class="relative w-72 max-w-full">
              <span class="pointer-events-none absolute inset-y-0 left-2 flex items-center text-faint">
                <Icon name="search" />
              </span>
              <input
                type="search"
                class={`${fieldInputClass} pl-7`}
                placeholder={msg("charts.searchPlaceholder")}
                aria-label={msg("charts.search")}
                value={search()}
                onInput={(e) => setSearch(e.currentTarget.value)}
              />
            </div>
            <label class="flex items-center gap-2 text-base whitespace-nowrap text-fg">
              {msg("charts.sort")}
              <select
                class={`${fieldInputClass} w-auto`}
                value={sort()}
                onChange={(e) => setSort(e.currentTarget.value === "name" ? "name" : "edited")}
              >
                <option value="edited">{msg("charts.sortEdited")}</option>
                <option value="name">{msg("charts.sortName")}</option>
              </select>
            </label>
            <label class="flex items-center gap-2 text-base text-fg">
              <input
                type="checkbox"
                class="size-4 accent-accent-fill"
                checked={archived()}
                onChange={(e) => setArchived(e.currentTarget.checked)}
              />
              {msg("charts.showArchived")}
            </label>
            <p class="ml-auto text-sm text-muted" role="status">
              {shown().length === 1
                ? msg("charts.countOne")
                : msg("charts.count", { n: shown().length })}
            </p>
          </div>
          <Show
            when={shown().length > 0}
            fallback={
              <div class="mt-8 text-base text-muted">
                <p>{msg("charts.noMatch", { query: search().trim() })}</p>
                <button
                  type="button"
                  class="mt-2 text-accent underline"
                  onClick={() => setSearch("")}
                >
                  {msg("charts.clearSearch")}
                </button>
              </div>
            }
          >
            <ul class="mt-4 grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-4">
              <For each={shown()}>
                {(chart) => (
                  <ChartCard
                    chart={chart}
                    onExport={() => void exportChart(chart)}
                    onDuplicate={() => setForm({ from: "chart", chart })}
                    onArchive={() => setArchiving(chart)}
                  />
                )}
              </For>
            </ul>
          </Show>
        </Match>
      </Switch>

      <Show when={form()} keyed>
        {(draft) => (
          <ChartForm
            draft={draft}
            onClose={() => setForm(null)}
            onCreated={(chart, start) => {
              setForm(null);
              notify(msg("charts.created", { name: chart.name }));
              // A reference image is placed in the designer, which offers it on the new chart.
              if (start === "image") navigate(designerPath(workspace()?.key ?? "", chart.key));
              else list.refetch();
            }}
          />
        )}
      </Show>
      <Confirm
        open={archiving() !== null}
        title={msg("charts.archiveTitle", { name: archiving()?.name ?? "" })}
        description={msg("charts.archiveBody")}
        confirm={msg("charts.archiveConfirm")}
        variant="danger"
        onConfirm={() => {
          const chart = archiving();
          if (chart !== null) void archive(chart);
        }}
        onClose={() => setArchiving(null)}
      />
    </>
  );
}

function ChartCard(props: {
  chart: Chart;
  onExport: () => void;
  onDuplicate: () => void;
  onArchive: () => void;
}) {
  const version = () => props.chart.published_version;
  const navigate = useNavigate();
  // The role decides whether an item is there; the licence, whether it works.
  const canArchive = () =>
    refused(workspace(), "archiveChart") !== "role" && props.chart.archived_at === null;
  return (
    <li class="flex flex-col overflow-hidden rounded-lg border border-line bg-surface">
      <div class="flex aspect-[4/3] items-center justify-center border-b border-line bg-canvas text-faint">
        <Show
          when={version()?.thumbnail}
          fallback={
            <>
              <Icon name="chart" class="size-8" />
              <span class="sr-only">{msg("charts.noThumbnail")}</span>
            </>
          }
        >
          {(t) => (
            <img
              src={t().url}
              alt={msg("charts.thumbnail", { name: props.chart.name })}
              class="size-full object-contain"
              loading="lazy"
            />
          )}
        </Show>
      </div>
      <div class="flex items-start gap-2 p-3">
        <div class="min-w-0 flex-1">
          <h2 class="truncate text-base font-medium text-fg" title={props.chart.name}>
            <A href={`/charts/${encodeURIComponent(props.chart.key)}`} class="hover:underline">
              {props.chart.name}
            </A>
          </h2>
          <p class="truncate font-mono text-sm text-muted" title={props.chart.key}>
            {props.chart.key}
          </p>
          <p class="mt-1.5 text-sm text-muted">
            <Show when={version()} fallback={msg("charts.unpublished")}>
              {(v) =>
                msg("charts.published", {
                  seats: formatNumber(v().stats.seats),
                  version: v().number,
                })
              }
            </Show>
          </p>
          <p class="text-sm text-muted">
            <A
              href={eventsPath(props.chart.key)}
              class="hover:underline"
              aria-label={msg("charts.eventsLink", {
                events: eventsSummary(props.chart.events),
                name: props.chart.name,
              })}
            >
              {eventsSummary(props.chart.events)}
            </A>
          </p>
          <p class="text-sm text-muted">
            {msg("charts.edited", { when: formatDate(props.chart.draft_updated_at) })}
          </p>
          <Show when={props.chart.archived_at !== null}>
            <span class="mt-1.5 inline-block rounded-sm bg-warning-soft px-1.5 text-xs text-warning">
              {msg("charts.archivedBadge")}
            </span>
          </Show>
        </div>
        <DropdownMenu gutter={4} placement="bottom-end">
          <DropdownMenu.Trigger
            class="inline-flex size-7 shrink-0 items-center justify-center rounded-md text-muted hover:bg-hover hover:text-fg"
            aria-label={msg("charts.menu", { name: props.chart.name })}
          >
            <Icon name="more" />
          </DropdownMenu.Trigger>
          <DropdownMenu.Portal>
            <DropdownMenu.Content class={menuContent}>
              <DropdownMenu.Item
                class={menuItem}
                onSelect={() => navigate(designerPath(workspace()?.key ?? "", props.chart.key))}
              >
                <Icon name="chart" />
                {msg("charts.open")}
              </DropdownMenu.Item>
              <DropdownMenu.Item class={menuItem} onSelect={props.onExport}>
                <Icon name="download" />
                {msg("charts.export")}
              </DropdownMenu.Item>
              <Show when={roleWrites()}>
                <DropdownMenu.Item
                  class={menuItem}
                  disabled={writeBlocked() !== null}
                  onSelect={afterMenu(props.onDuplicate)}
                >
                  <Icon name="file" />
                  {msg("charts.duplicate")}
                </DropdownMenu.Item>
              </Show>
              <Show when={canArchive()}>
                <DropdownMenu.Item
                  class={`${menuItem} text-danger`}
                  disabled={archiveClosed() !== null}
                  onSelect={afterMenu(props.onArchive)}
                >
                  <Icon name="trash" />
                  {msg("charts.archive")}
                </DropdownMenu.Item>
              </Show>
              <Show when={roleWrites() && archiveClosed()}>
                {(why) => <p class="max-w-64 px-2 py-1 text-sm text-muted">{why()}</p>}
              </Show>
            </DropdownMenu.Content>
          </DropdownMenu.Portal>
        </DropdownMenu>
      </div>
    </li>
  );
}

/** The form for a new chart: empty, from an imported document, or a copy
 * of another chart's draft. The key follows the name until it is edited. */
function ChartForm(props: {
  draft: Draft;
  onClose: () => void;
  /** With the starting point chosen in the form. */
  onCreated: (c: Chart, start: Start) => void;
}) {
  const signal = componentSignal();
  const initial =
    props.draft.from === "file"
      ? props.draft.name
      : props.draft.from === "chart"
        ? msg("charts.copyName", { name: props.draft.chart.name })
        : "";
  const [name, setName] = createSignal(initial);
  const [start, setStart] = createSignal<Start>(
    props.draft.from === "empty" ? props.draft.start : "blank",
  );
  const [key, setKey] = createSignal(keyFrom(initial));
  const [keyEdited, setKeyEdited] = createSignal(false);
  const [errors, setErrors] = createSignal<{ name: string | null; key: string | null }>({
    name: null,
    key: null,
  });
  const [failure, setFailure] = createSignal<{ text: string; detail?: string } | null>(null);
  const [busy, setBusy] = createSignal(false);

  const check = (field: "name" | "key") => {
    const problem = textProblem(field === "name" ? name() : key());
    setErrors((e) => ({ ...e, [field]: problem }));
    return problem === null;
  };

  const title =
    props.draft.from === "file"
      ? msg("charts.importTitle")
      : props.draft.from === "chart"
        ? msg("charts.duplicateTitle", { name: props.draft.chart.name })
        : msg("charts.newTitle");

  const submit = async (e: SubmitEvent) => {
    e.preventDefault();
    if (![check("name"), check("key")].every(Boolean) || busy() || writeBlocked() !== null) return;
    setBusy(true);
    setFailure(null);
    try {
      const draft = props.draft;
      let document: ChartDocument | undefined;
      if (draft.from === "file") document = draft.document;
      const template = TEMPLATES.find((t) => t.id === start());
      if (draft.from === "empty" && template !== undefined) {
        document = JSON.parse(await template.load()) as ChartDocument;
      }
      if (draft.from === "chart") {
        document = await call("getDraft", { path: { chart: draft.chart.key } }, signal);
      }
      // The document carries its own name: the chart's, as the server's
      // empty draft does.
      if (document !== undefined) document = { ...document, name: name().trim() };
      const body =
        document === undefined
          ? { key: key().trim(), name: name().trim() }
          : { key: key().trim(), name: name().trim(), document };
      props.onCreated(await call("createChart", { body }, signal), start());
    } catch (err) {
      const error = asApiError(err);
      if (signal.aborted || error.sessionLost) return;
      const field = error.details["field"];
      if (error.code === "already_exists") {
        setErrors((x) => ({ ...x, key: msg("charts.keyTaken") }));
      } else if (error.reason === "invalid_field" && (field === "key" || field === "name")) {
        setErrors((x) => ({ ...x, [field]: msg("charts.textRule") }));
      } else if (error.reason === "document") {
        setFailure({ text: msg("charts.badDocument", { message: error.message }) });
      } else {
        // The role may have changed since `me` was read: follow it.
        if (error.code === "forbidden") void loadSession();
        const detail = reference(error);
        setFailure(
          detail === undefined ? { text: describe(error) } : { text: describe(error), detail },
        );
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open title={title} heading onClose={props.onClose}>
      <form class="space-y-4 px-5 pt-2 pb-5" noValidate onSubmit={(e) => void submit(e)}>
        <Field
          label={msg("charts.name")}
          value={name()}
          maxLength={128}
          onInput={(v) => {
            setName(v);
            if (!keyEdited()) setKey(keyFrom(v));
            if (errors().name !== null) check("name");
          }}
          onBlur={() => check("name")}
          error={errors().name}
        />
        <Field
          label={msg("charts.key")}
          hint={msg("charts.keyHint")}
          mono
          value={key()}
          maxLength={128}
          onInput={(v) => {
            setKey(v);
            setKeyEdited(true);
            if (errors().key !== null) check("key");
          }}
          onBlur={() => check("key")}
          error={errors().key}
        />
        <Show when={props.draft.from === "empty"}>
          <SelectField<Start>
            label={msg("charts.start")}
            value={start()}
            options={[
              { value: "blank", label: msg("charts.startBlank") },
              ...TEMPLATES.map((t) => ({
                value: t.id,
                label: msg("charts.startTemplate", {
                  template: msg(`template.${t.id}`),
                  seats: formatNumber(t.seats),
                }),
              })),
              { value: "image", label: msg("charts.startImage") },
            ]}
            hint={start() === "image" ? msg("charts.startImageHint") : msg("charts.startHint")}
            onChange={setStart}
          />
        </Show>
        <FormError message={failure()?.text ?? null} detail={failure()?.detail} />
        {/* The licence may expire while the dialog is open: it follows. */}
        <Show when={writeBlocked()}>{(why) => <p class="text-base text-warning">{why()}</p>}</Show>
        <div class="flex justify-end gap-2">
          <Button class="h-8" onClick={props.onClose}>
            {msg("action.cancel")}
          </Button>
          <Button
            type="submit"
            variant="primary"
            class="h-8"
            disabled={busy() || writeBlocked() !== null}
          >
            {busy() ? msg("charts.creating") : msg("charts.create")}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
