// Pieces the console's list and form screens share (09 §8): paged-list
// states (loading, error with Retry, "load more"), a gated action that says
// why it is unavailable, form controls beyond text fields, a one-time
// secret's copy field, tabs, and dates through `Intl`.
import { Tabs as KTabs } from "@kobalte/core/tabs";
import { Tooltip } from "@kobalte/core/tooltip";
import { createSignal, createUniqueId, For, Show, type Accessor, type JSX } from "solid-js";
import type { ApiError } from "../api/client.ts";
import type { Pages } from "../api/pages.ts";
import { msg, type MessageId } from "../i18n/messages.ts";
import { Button, buttonClass, Tip, type Variant } from "../ui/controls.tsx";
import { fieldInputClass } from "../ui/form.tsx";
import { Icon, type IconName } from "../ui/Icon.tsx";
import { describe, reference } from "./errors.ts";
import {
  ariaSort,
  sortBy,
  sorted,
  toggle,
  type Direction,
  type Sort,
  type SortBy,
} from "./tables.ts";

const dateTimes = new Intl.DateTimeFormat("en", { dateStyle: "medium", timeStyle: "short" });
const dates = new Intl.DateTimeFormat("en", { dateStyle: "medium" });

function parsed(iso: string, format: Intl.DateTimeFormat): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : format.format(d);
}
export const formatDateTime = (iso: string): string => parsed(iso, dateTimes);
export const formatDate = (iso: string): string => parsed(iso, dates);

/** A table's classes: a sticky header, rows separated by lines. */
export const table = "w-full border-collapse text-left text-base";
export const th =
  "sticky top-0 z-10 border-b border-line bg-surface px-3 py-2 text-sm font-medium text-muted";
export const td = "border-b border-line px-3 py-2 align-middle text-fg";
/** Around a table: its own scroll box, both ways and at most about a
 * window high, so its sticky header stays in view (09 §8) and a table wider
 * than the window (from 768 px) scrolls sideways without the page. */
export const tableBox = "max-h-[calc(100dvh-12rem)] overflow-auto";

/** A column's header: a button that sorts by it when it can (`sortBy`),
 * with `aria-sort` on the header naming the order. */
export function SortHeader<C extends string>(props: {
  label: string;
  column: C;
  sort: Sort<C> | null;
  by: SortBy;
  onSort: (sort: Sort<C>) => void;
  /** The first direction a click asks for (dates: newest first). */
  first?: Direction;
}) {
  const icon = (): IconName => {
    const d = ariaSort(props.sort, props.column);
    return d === "ascending" ? "sortUp" : d === "descending" ? "sortDown" : "sortable";
  };
  return (
    <th scope="col" class={th} aria-sort={ariaSort(props.sort, props.column)}>
      <Show when={props.by !== "none"} fallback={props.label}>
        <button
          type="button"
          class="-mx-1 inline-flex items-center gap-1 rounded-sm px-1 hover:text-fg"
          onClick={() => props.onSort(toggle(props.sort, props.column, props.first))}
        >
          {props.label}
          <Icon
            name={icon()}
            class={props.sort?.column === props.column ? "shrink-0 text-fg" : "shrink-0 text-faint"}
          />
        </button>
      </Show>
    </th>
  );
}

/** Says, to everyone, how a table is sorted now and, while it is only
 * partly loaded, that sorting by its other columns waits for the rest. */
export function SortNote(props: { said: string; partial?: string | undefined }) {
  return (
    <>
      <p class="sr-only" role="status">
        {props.said}
      </p>
      <Show when={props.partial}>{(text) => <p class="mt-2 text-sm text-muted">{text()}</p>}</Show>
    </>
  );
}

/** What a column holds, for how its order is said. */
export type Kind = "text" | "number" | "time";
/** "A to Z", "newest first": an order in words. */
export const orderText = (kind: Kind, direction: Direction): string =>
  msg(`table.${kind}.${direction}`);

/** Every row of a paged list is loaded: its last page has come. */
export const complete = <T,>(pages: Pages<T>): boolean => {
  const s = pages.state();
  return s.status === "ready" && s.data.next === null;
};

/**
 * Sorting in the page for a list the server does not order (`tables.ts`):
 * its columns sort once every row is loaded, and not before. In the
 * server's order until a header is chosen.
 */
export function pageSort<T, C extends string>(
  columns: Record<C, { label: MessageId; kind: Kind; compare: (a: T, b: T) => number }>,
) {
  const [sort, setSort] = createSignal<Sort<C> | null>(null);
  const keys = Object.keys(columns) as C[];
  const compare = Object.fromEntries(keys.map((k) => [k, columns[k].compare])) as Record<
    C,
    (a: T, b: T) => number
  >;
  return {
    header: (column: C, whole: boolean) => (
      <SortHeader
        label={msg(columns[column].label)}
        column={column}
        by={sortBy(column, { server: [], page: keys, complete: whole })}
        sort={whole ? sort() : null}
        first={columns[column].kind === "text" ? "ascending" : "descending"}
        onSort={setSort}
      />
    ),
    rows: (rows: T[], whole: boolean): T[] => (whole ? sorted(rows, sort(), compare) : rows),
    note: (whole: boolean) => {
      const s = sort();
      return (
        <SortNote
          said={
            whole && s !== null
              ? msg("table.sorted", {
                  column: msg(columns[s.column].label),
                  order: orderText(columns[s.column].kind, s.direction),
                })
              : msg("table.unsorted")
          }
          partial={whole ? undefined : msg("table.sortPartial")}
        />
      );
    },
  };
}

/** Shown while a list's first page loads. */
export function ListLoading(props: { label: string; rows?: number }) {
  return (
    <>
      <p role="status" class="sr-only">
        {props.label}
      </p>
      <div class="mt-4 space-y-2" aria-hidden="true">
        <For each={Array.from({ length: props.rows ?? 4 }, (_, i) => i)}>
          {() => <div class="h-10 animate-pulse rounded-md bg-hover" />}
        </For>
      </div>
    </>
  );
}

/** A failed load: what happened, the request id, and Retry. */
export function ListError(props: { title: string; error: ApiError; onRetry: () => void }) {
  return (
    <div role="alert" class="mt-4 max-w-xl rounded-lg border border-line p-4">
      <p class="text-base text-fg">{props.title}</p>
      <p class="mt-1 text-base text-muted">{describe(props.error)}</p>
      <Show when={reference(props.error)}>
        {(r) => <p class="mt-1 text-sm text-faint">{r()}</p>}
      </Show>
      <Button class="mt-3 h-8" onClick={() => props.onRetry()}>
        {msg("action.retry")}
      </Button>
    </div>
  );
}

/** An empty list: what it is for, and the first action. */
export function Empty(props: {
  icon: IconName;
  title: string;
  body: string;
  children?: JSX.Element;
}) {
  return (
    <div class="mt-6 flex max-w-xl flex-col items-start gap-3 rounded-lg border border-dashed border-line-strong p-8">
      <Icon name={props.icon} class="size-8 text-faint" />
      <h2 class="text-lg font-semibold text-fg">{props.title}</h2>
      <p class="text-base text-muted">{props.body}</p>
      {props.children}
    </div>
  );
}

/** The data of a paged list once loaded, else its loading or error state. */
export function Paged<T>(props: {
  pages: Pages<T>;
  loading: string;
  failed: string;
  /** Given the rows as an accessor: they change with every page loaded. */
  children: (rows: Accessor<T[]>) => JSX.Element;
}) {
  const state = () => props.pages.state();
  return (
    <Show
      when={(() => {
        const s = state();
        return s.status === "ready" ? s.data.data : null;
      })()}
      fallback={
        <Show
          when={(() => {
            const s = state();
            return s.status === "error" ? s.error : null;
          })()}
          fallback={<ListLoading label={props.loading} />}
        >
          {(e) => <ListError title={props.failed} error={e()} onRetry={props.pages.refetch} />}
        </Show>
      }
    >
      {(rows) => props.children(rows)}
    </Show>
  );
}

/** "Load more" under a paged list while the API has more pages. */
export function LoadMore<T>(props: { pages: Pages<T> }) {
  const next = () => {
    const s = props.pages.state();
    return s.status === "ready" && s.data.next !== null;
  };
  const failed = () => {
    const m = props.pages.moreState();
    return typeof m === "object" ? m : null;
  };
  return (
    <Show when={next()}>
      <div class="mt-3 flex items-center gap-3">
        <Button
          class="h-8"
          disabled={props.pages.moreState() === "loading"}
          onClick={() => props.pages.more()}
        >
          {props.pages.moreState() === "loading" ? msg("list.loadingMore") : msg("list.loadMore")}
        </Button>
        <Show when={failed()}>
          {(e) => (
            <p role="alert" class="text-sm text-danger">
              {describe(e())}
            </p>
          )}
        </Show>
      </div>
    </Show>
  );
}

/**
 * A button for an action the role may lack: enabled, or shown disabled and
 * still focusable with the reason in its name and a tooltip. One element
 * either way, so focus stays on it when it becomes unavailable (the
 * manager's Block, once the selection it took is cleared).
 */
export function Action(props: {
  label: string;
  icon?: IconName;
  variant?: Variant;
  /** Why it cannot be used, or `null` when it can. */
  blocked: string | null;
  onClick: () => void;
  class?: string;
}) {
  return (
    <Tip label={props.blocked ?? props.label} open={props.blocked === null ? false : undefined}>
      <Tooltip.Trigger
        as="button"
        type="button"
        aria-disabled={props.blocked === null ? undefined : "true"}
        aria-label={props.blocked === null ? undefined : `${props.label}: ${props.blocked}`}
        class={buttonClass(props.variant ?? "secondary", props.class ?? "h-8")}
        onClick={() => {
          if (props.blocked === null) props.onClick();
        }}
      >
        <Show when={props.icon}>{(name) => <Icon name={name()} />}</Show>
        {props.label}
      </Tooltip.Trigger>
    </Tip>
  );
}

/** Labels, hints and errors for a control, as `ui/form.tsx`'s `Field`. */
function Labelled(props: {
  id: string;
  label: string;
  hint?: string | undefined;
  error?: string | null | undefined;
  children: JSX.Element;
}) {
  return (
    <div class="space-y-1">
      <label for={props.id} class="block text-sm font-medium text-fg">
        {props.label}
      </label>
      {props.children}
      <Show when={props.hint}>
        <p id={`${props.id}-hint`} class="text-sm text-muted">
          {props.hint}
        </p>
      </Show>
      <Show when={props.error}>
        <p id={`${props.id}-error`} class="text-sm text-danger">
          {props.error}
        </p>
      </Show>
    </div>
  );
}

const describedBy = (id: string, hint: unknown, error: unknown) =>
  [hint === undefined ? null : `${id}-hint`, error ? `${id}-error` : null]
    .filter((x) => x !== null)
    .join(" ") || undefined;

/** A whole-number field with its unit in the hint. The text is kept as
 * typed; the form reads and checks it. */
export function NumberField(props: {
  label: string;
  value: string;
  onInput: (value: string) => void;
  onBlur?: () => void;
  min: number;
  max: number;
  hint: string;
  error?: string | null;
}) {
  const id = createUniqueId();
  return (
    <Labelled id={id} label={props.label} hint={props.hint} error={props.error}>
      <input
        id={id}
        type="number"
        inputmode="numeric"
        step="1"
        min={props.min}
        max={props.max}
        class={`${fieldInputClass} w-40`}
        value={props.value}
        aria-invalid={props.error ? true : undefined}
        aria-describedby={describedBy(id, props.hint, props.error)}
        onInput={(e) => props.onInput(e.currentTarget.value)}
        onBlur={() => props.onBlur?.()}
      />
    </Labelled>
  );
}

export function SelectField<V extends string>(props: {
  label: string;
  value: V | "";
  options: { value: V; label: string }[];
  onChange: (value: V) => void;
  placeholder?: string;
  hint?: string;
  error?: string | null;
  disabled?: boolean;
}) {
  const id = createUniqueId();
  return (
    <Labelled id={id} label={props.label} hint={props.hint} error={props.error}>
      <select
        id={id}
        class={fieldInputClass}
        value={props.value}
        disabled={props.disabled}
        aria-invalid={props.error ? true : undefined}
        aria-describedby={describedBy(id, props.hint, props.error)}
        onChange={(e) => {
          const v = props.options.find((o) => o.value === e.currentTarget.value);
          if (v !== undefined) props.onChange(v.value);
        }}
      >
        <Show when={props.placeholder !== undefined && props.value === ""}>
          <option value="" disabled>
            {props.placeholder}
          </option>
        </Show>
        <For each={props.options}>{(o) => <option value={o.value}>{o.label}</option>}</For>
      </select>
    </Labelled>
  );
}

export function TextAreaField(props: {
  label: string;
  value: string;
  onInput: (value: string) => void;
  onBlur?: () => void;
  hint?: string;
  error?: string | null;
  placeholder?: string;
  rows?: number;
}) {
  const id = createUniqueId();
  return (
    <Labelled id={id} label={props.label} hint={props.hint} error={props.error}>
      <textarea
        id={id}
        rows={props.rows ?? 3}
        spellcheck={false}
        placeholder={props.placeholder}
        class={`${fieldInputClass} h-auto py-1.5 font-mono`}
        value={props.value}
        aria-invalid={props.error ? true : undefined}
        aria-describedby={describedBy(id, props.hint, props.error)}
        onInput={(e) => props.onInput(e.currentTarget.value)}
        onBlur={() => props.onBlur?.()}
      />
    </Labelled>
  );
}

export function CheckField(props: {
  label: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
  hint?: string;
}) {
  const id = createUniqueId();
  return (
    <div>
      <label class="flex items-center gap-2 text-base text-fg">
        <input
          type="checkbox"
          class="size-4 accent-accent-fill"
          checked={props.checked}
          aria-describedby={props.hint === undefined ? undefined : `${id}-hint`}
          onChange={(e) => props.onChange(e.currentTarget.checked)}
        />
        {props.label}
      </label>
      <Show when={props.hint}>
        <p id={`${id}-hint`} class="mt-1 pl-6 text-sm text-muted">
          {props.hint}
        </p>
      </Show>
    </div>
  );
}

/**
 * A value to copy: read-only, selected on focus, with a Copy button. When
 * the clipboard is refused (an insecure origin, a denied permission) the
 * text is selected for the person to copy themselves.
 */
export function CopyField(props: { label: string; value: string; mono?: boolean }) {
  const id = createUniqueId();
  const [copied, setCopied] = createSignal<"no" | "yes" | "failed">("no");
  let input: HTMLInputElement | undefined;
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(props.value);
      setCopied("yes");
    } catch {
      input?.focus();
      input?.select();
      setCopied("failed");
    }
  };
  return (
    <div class="space-y-1">
      <label for={id} class="block text-sm font-medium text-fg">
        {props.label}
      </label>
      <div class="flex gap-2">
        <input
          id={id}
          ref={(el) => (input = el)}
          readOnly
          class={`${fieldInputClass} ${props.mono === false ? "" : "font-mono"}`}
          value={props.value}
          spellcheck={false}
          onFocus={(e) => e.currentTarget.select()}
        />
        <Button
          class="h-8"
          icon={copied() === "yes" ? "check" : "file"}
          onClick={() => void copy()}
        >
          {copied() === "yes" ? msg("copy.copied") : msg("copy.copy")}
        </Button>
      </div>
      <p role="status" class="text-sm text-muted">
        {copied() === "failed" ? msg("copy.failed") : ""}
      </p>
    </div>
  );
}

/** A short identifier with a copy button beside it (a chart's key). */
export function CopyText(props: { value: string; label: string }) {
  const [copied, setCopied] = createSignal(false);
  return (
    <span class="inline-flex items-center gap-1">
      <code class="font-mono text-base text-muted">{props.value}</code>
      <Tip label={copied() ? msg("copy.copied") : props.label}>
        <Tooltip.Trigger
          as="button"
          type="button"
          aria-label={props.label}
          class="inline-flex size-6 items-center justify-center rounded-md text-muted hover:bg-hover hover:text-fg"
          onClick={() =>
            void navigator.clipboard.writeText(props.value).then(
              () => setCopied(true),
              () => setCopied(false),
            )
          }
        >
          <Icon name={copied() ? "check" : "file"} />
        </Tooltip.Trigger>
      </Tip>
    </span>
  );
}

/** Tabs: arrow keys move between them (Kobalte). */
export function Tabs<V extends string>(props: {
  label: string;
  value: V;
  onChange: (value: V) => void;
  tabs: { value: V; label: string; content: () => JSX.Element }[];
}) {
  return (
    <KTabs
      value={props.value}
      onChange={(v) => {
        const tab = props.tabs.find((t) => t.value === v);
        if (tab !== undefined) props.onChange(tab.value);
      }}
      class="mt-5"
    >
      <KTabs.List aria-label={props.label} class="flex gap-1 border-b border-line">
        <For each={props.tabs}>
          {(t) => (
            <KTabs.Trigger
              value={t.value}
              class="-mb-px h-9 border-b-2 border-transparent px-3 text-base text-muted hover:text-fg data-selected:border-accent data-selected:font-medium data-selected:text-fg"
            >
              {t.label}
            </KTabs.Trigger>
          )}
        </For>
      </KTabs.List>
      <For each={props.tabs}>
        {(t) => (
          <KTabs.Content value={t.value} class="pt-5">
            {t.content()}
          </KTabs.Content>
        )}
      </For>
    </KTabs>
  );
}
