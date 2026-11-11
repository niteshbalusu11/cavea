// The numbering panel (07 §6): every control of the row and seat rules for
// the selected blocks and rows, "Mixed" where they differ, a live preview
// (`previewNumbering`, debounced) with a sample of the labels it gives, and
// Apply as one undo entry. It opens from the inspector in the inspector's
// place, and reads the `numbering` view only while it is open. The preview
// ends when it closes, on Escape, when the tool changes (it closes) and
// when the selection changes (the edit core ends it too; the panel starts
// again from the new selection).
import type { AlphaOverflow, NumberedNode, Preview } from "@cavea/designer-core";
import {
  createEffect,
  createMemo,
  createSignal,
  For,
  on,
  onCleanup,
  Show,
  untrack,
} from "solid-js";
import { msg, type MessageId } from "../i18n/messages.ts";
import { Button, IconButton } from "../ui/controls.tsx";
import { CheckField, NumberField, SelectField, TextField, type Option } from "../ui/fields.tsx";
import { Icon } from "../ui/Icon.tsx";
import type { Editor } from "./editor.ts";
import { countOf } from "./Inspector.tsx";
import {
  EMPTY,
  reorder,
  ROW_DIRECTIONS,
  ROW_SCHEMES,
  rowsFrom,
  SEAT_DIRECTIONS,
  SEAT_SCHEMES,
  seatsFrom,
  toNumbering,
  touched,
  type Draft,
  type RowControls,
  type SeatControls,
} from "./numbering.ts";
import { display, isMac, LAYER_MOVE } from "./shortcuts.ts";

/** Typing settles this long before the preview runs: 16 ms in the edit core
 * for 100,000 seats (07 §6), too much for every keystroke. */
const DEBOUNCE_MS = 150;
const SAMPLE_ROWS = 3;
const SAMPLE_SEATS = 6;

const options = (prefix: string, values: readonly string[]): Option[] =>
  values.map((value) => ({ value, label: msg(`${prefix}.${value}` as MessageId) }));
const same = <T,>(s: { same: T } | "mixed" | undefined): T | null =>
  s === undefined || s === "mixed" ? null : s.same;

const listOf = (s: { same: string[] } | "mixed" | undefined) => same(s)?.join(", ") ?? null;
const startOf = (s: { same: number | string | null } | "mixed" | undefined) => {
  const x = same(s);
  return x === null ? (s === "mixed" ? null : "") : String(x);
};

export const nodeName = (n: NumberedNode): string =>
  msg("layers.unnamed", { kind: msg(n.block ? "kind.rowBlock" : "kind.row"), id: String(n.id) });

export function NumberingPanel(props: { editor: Editor; onClose: () => void }) {
  const editor = props.editor;
  const view = () => editor.views.numbering;
  const [draft, setDraft] = createSignal<Draft>(EMPTY);
  const [preview, setPreview] = createSignal<Preview | null>(null);
  const [pending, setPending] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  editor.loop.watch("numbering", true);
  onCleanup(() => {
    editor.loop.watch("numbering", false);
    editor.run((s) => s.cancelNumbering());
  });

  const nodes = () => view()?.nodes ?? [];
  const blocks = () => nodes().filter((n) => n.block).length;
  // The selection changed: start again from the new targets.
  createEffect(
    on(
      () =>
        nodes()
          .map((n) => n.id)
          .join(),
      () => {
        setDraft(EMPTY);
        setError(null);
      },
      { defer: true },
    ),
  );
  // A tool chosen closes the panel: drawing comes next (07 §2.3, reset).
  createEffect(on(() => editor.views.tool.tool, props.onClose, { defer: true }));

  // The preview: debounced after every change of the draft, the targets or
  // the document. With nothing touched it shows the labels as they are.
  let timer: ReturnType<typeof setTimeout> | undefined;
  createEffect(() => {
    const d = draft();
    const targets = nodes().length;
    void editor.views.status.revision;
    clearTimeout(timer);
    if (targets === 0) {
      setPreview(null);
      return;
    }
    setPending(true);
    timer = setTimeout(() => untrack(() => run(d)), DEBOUNCE_MS);
  });
  onCleanup(() => clearTimeout(timer));
  const run = (d: Draft) => {
    setPending(false);
    const r = editor.attempt((s) => s.previewNumbering(toNumbering(d)));
    if ("error" in r) {
      setError(r.error);
      setPreview(null);
      return;
    }
    setError(null);
    setPreview(r.ok);
  };

  const v = () => view();
  // "Mixed" shows while a group is left alone and its blocks differ. As
  // calls, not inline conditions, so that a field reading its props in an
  // event handler creates no computation.
  const rowMixed = (k: "custom" | "start" | "skip") =>
    rows() === null && v()?.rows?.[k] === "mixed";
  const seatMixed = (k: "custom" | "start") => seats() === null && v()?.seats?.[k] === "mixed";
  const rows = (): RowControls | null => draft().rows;
  const seats = (): SeatControls | null => draft().seats;
  const editRows = (change: Partial<RowControls>) => {
    const cur = v();
    if (cur === null) return;
    setDraft((d) => ({ ...d, rows: { ...(d.rows ?? rowsFrom(cur)), ...change } }));
  };
  const editSeats = (change: Partial<SeatControls>, across?: number[] | null) => {
    const cur = v();
    if (cur === null) return;
    setDraft((d) => ({
      ...d,
      seats: { ...(d.seats ?? seatsFrom(cur)), ...change },
      across: across === undefined ? d.across : across,
    }));
  };
  const apply = () => {
    const r = editor.attempt((s) => s.number(toNumbering(draft())));
    if ("error" in r) return setError(r.error);
    setDraft(EMPTY);
  };

  /** A row field: the draft's value once the group is touched, else the view's. */
  const rv = <K extends keyof RowControls>(k: K, fromView: () => RowControls[K] | null) =>
    rows()?.[k] ?? fromView();
  const sv = <K extends keyof SeatControls>(k: K, fromView: () => SeatControls[K] | null) =>
    seats()?.[k] ?? fromView();
  const notes = () =>
    nodes().filter(
      (n) => n.facing !== "forward" || n.mirrored || n.rowOverrides + n.seatOverrides > 0,
    );
  const overrides = () =>
    nodes().reduce(
      (t, n) => ({ rows: t.rows + n.rowOverrides, seats: t.seats + n.seatOverrides }),
      { rows: 0, seats: 0 },
    );
  const order = () => draft().across ?? nodes().map((n) => n.id);
  /** The preview's first rows, from the blocks in the order they are
   * numbered (`across`), else in document order. */
  const sampleRows = () => {
    const all = preview()?.rows ?? [];
    const across = draft().across;
    if (across === null) return all.slice(0, SAMPLE_ROWS);
    const first = across.find((id) => all.some((r) => r.node === id));
    return all.filter((r) => r.node === first).slice(0, SAMPLE_ROWS);
  };
  /** The first seat's full label under the chart's template (07 §6). */
  const full = createMemo(() => {
    const row = sampleRows()[0];
    const seat = row?.seats[0];
    if (row === undefined || seat === undefined) return null;
    return untrack(() => editor.run((s) => s.sample(row.node, seat.oid))) ?? null;
  });
  const byId = (id: number) => nodes().find((n) => n.id === id);
  let dragged: number | null = null;

  return (
    <div
      class="flex min-h-0 flex-1 flex-col"
      onKeyDown={(e) => {
        const t = e.target as HTMLElement;
        if (e.key === "Escape" && !e.defaultPrevented && !t.matches("input,select,textarea")) {
          e.preventDefault();
          props.onClose();
        }
      }}
    >
      <Show
        when={nodes().length > 0}
        fallback={<p class="p-3 text-sm text-muted">{msg("numbering.empty")}</p>}
      >
        <div class="min-h-0 flex-1 overflow-y-auto">
          <div class="space-y-1 border-b border-line px-3 py-2 text-sm">
            <p class="text-fg">
              {msg("numbering.targets", {
                list: [
                  { kind: "rowBlock" as const, count: blocks() },
                  { kind: "row" as const, count: nodes().length - blocks() },
                ]
                  .filter((k) => k.count > 0)
                  .map(countOf)
                  .join(", "),
              })}
            </p>
            <p class="flex gap-1.5 text-muted">
              <Icon name="focal" class="mt-px size-3.5 shrink-0" />
              {msg("numbering.facing")}
            </p>
            <Show when={notes().length > 0}>
              <ul class="space-y-0.5 pt-1 text-muted" aria-label={msg("numbering.notes")}>
                <For each={notes().slice(0, 6)}>
                  {(n) => (
                    <li class="flex gap-1.5">
                      <Icon
                        name={n.facing === "forward" && !n.mirrored ? "check" : "warning"}
                        class={`mt-px size-3.5 shrink-0 ${n.facing === "forward" && !n.mirrored ? "text-faint" : "text-warning"}`}
                      />
                      <span>
                        <span class="text-fg">{nodeName(n)}</span>
                        {": "}
                        {[
                          n.facing === "reversed" ? msg("numbering.reversed") : null,
                          n.facing === "unknown" ? msg("numbering.unknown") : null,
                          n.mirrored ? msg("numbering.mirrored") : null,
                          n.rowOverrides + n.seatOverrides > 0
                            ? msg("numbering.overridesOf", {
                                rows: n.rowOverrides,
                                seats: n.seatOverrides,
                              })
                            : null,
                        ]
                          .filter((x) => x !== null)
                          .join(" ")}
                      </span>
                    </li>
                  )}
                </For>
                <Show when={notes().length > 6}>
                  <li class="pl-5">{msg("numbering.more", { n: notes().length - 6 })}</li>
                </Show>
              </ul>
            </Show>
          </div>

          <Show when={v()?.rows}>
            {(r) => (
              <Group
                title={msg("numbering.rows")}
                set={rows() !== null}
                onReset={() => setDraft((d) => ({ ...d, rows: null }))}
              >
                <SelectField
                  label={msg("numbering.scheme")}
                  options={options("scheme", ROW_SCHEMES)}
                  value={rv("scheme", () => same(r().scheme))}
                  onChange={(scheme) => editRows({ scheme: scheme as RowControls["scheme"] })}
                />
                <Show when={rv("scheme", () => same(r().scheme)) === "custom"}>
                  <TextField
                    label={msg("numbering.custom")}
                    value={rv("custom", () => listOf(r().custom))}
                    mixed={rowMixed("custom")}
                    placeholder={msg("numbering.listHint")}
                    onInput={(custom) => editRows({ custom })}
                    onCommit={() => null}
                  />
                </Show>
                <TextField
                  label={msg("numbering.start")}
                  value={rv("start", () => startOf(r().start))}
                  mixed={rowMixed("start")}
                  placeholder={msg("numbering.startHint")}
                  onInput={(start) => editRows({ start })}
                  onCommit={() => null}
                />
                <NumberField
                  label={msg("numbering.step")}
                  value={rv("step", () => same(r().step))}
                  min={-1000}
                  max={1000}
                  unit=""
                  onCommit={(step) => (editRows({ step }), null)}
                />
                <TextField
                  label={msg("numbering.skip")}
                  value={rv("skip", () => listOf(r().skip))}
                  mixed={rowMixed("skip")}
                  placeholder={msg("numbering.skipHint")}
                  onInput={(skip) => editRows({ skip })}
                  onCommit={() => null}
                />
                <SelectField
                  label={msg("numbering.direction")}
                  options={options("rowDirection", ROW_DIRECTIONS)}
                  value={rv("direction", () => same(r().direction))}
                  onChange={(d) => editRows({ direction: d as RowControls["direction"] })}
                />
                <SelectField
                  label={msg("field.alphaOverflow")}
                  options={options("alphaOverflow", ["double", "spreadsheet"])}
                  value={draft().afterZ ?? v()?.afterZ ?? null}
                  onChange={(a) => setDraft((d) => ({ ...d, afterZ: a as AlphaOverflow }))}
                />
                <p class="px-3 pb-1 pl-[108px] text-xs text-faint">{msg("numbering.afterZHint")}</p>
                <Affixes
                  prefix={rv("prefix", () => same(r().prefix))}
                  suffix={rv("suffix", () => same(r().suffix))}
                  mixed={rows() === null}
                  onPrefix={(prefix) => editRows({ prefix })}
                  onSuffix={(suffix) => editRows({ suffix })}
                />
              </Group>
            )}
          </Show>
          <Show when={v() !== null && v()?.rows === null}>
            <p class="border-b border-line px-3 py-2 text-sm text-muted">
              {msg("numbering.noRowRule")}
            </p>
          </Show>

          <Show when={v()?.seats}>
            {(st) => (
              <Group
                title={msg("numbering.seats")}
                set={seats() !== null}
                onReset={() => setDraft((d) => ({ ...d, seats: null, across: null }))}
              >
                <SelectField
                  label={msg("numbering.scheme")}
                  options={options("scheme", SEAT_SCHEMES)}
                  value={sv("scheme", () => same(st().scheme))}
                  onChange={(scheme) => editSeats({ scheme: scheme as SeatControls["scheme"] })}
                />
                <Show when={sv("scheme", () => same(st().scheme)) === "custom"}>
                  <TextField
                    label={msg("numbering.custom")}
                    value={sv("custom", () => listOf(st().custom))}
                    mixed={seatMixed("custom")}
                    placeholder={msg("numbering.listHint")}
                    onInput={(custom) => editSeats({ custom })}
                    onCommit={() => null}
                  />
                </Show>
                <TextField
                  label={msg("numbering.start")}
                  value={sv("start", () => startOf(st().start))}
                  mixed={seatMixed("start")}
                  placeholder={msg("numbering.startHint")}
                  onInput={(start) => editSeats({ start })}
                  onCommit={() => null}
                />
                <NumberField
                  label={msg("numbering.step")}
                  value={sv("step", () => same(st().step))}
                  min={-1000}
                  max={1000}
                  unit=""
                  onCommit={(step) => (editSeats({ step }), null)}
                />
                <SelectField
                  label={msg("numbering.direction")}
                  options={options("seatDirection", SEAT_DIRECTIONS)}
                  value={sv("direction", () => same(st().direction))}
                  onChange={(d) => editSeats({ direction: d as SeatControls["direction"] })}
                />
                <CheckField
                  label={msg("numbering.continuous")}
                  checked={sv("continuous", () => same(st().continuous))}
                  onChange={(continuous) => editSeats({ continuous })}
                />
                <Affixes
                  prefix={sv("prefix", () => same(st().prefix))}
                  suffix={sv("suffix", () => same(st().suffix))}
                  mixed={seats() === null}
                  onPrefix={(prefix) => editSeats({ prefix })}
                  onSuffix={(suffix) => editSeats({ suffix })}
                />
                <Show when={nodes().length > 1}>
                  <CheckField
                    label={msg("numbering.across")}
                    checked={draft().across !== null}
                    onChange={(onOff) => editSeats({}, onOff ? nodes().map((n) => n.id) : null)}
                  />
                  <Show when={draft().across !== null}>
                    <p class="px-3 pl-[108px] text-xs text-faint">
                      {msg("numbering.acrossHint", {
                        keys: `${display(LAYER_MOVE.up, isMac())} ${display(LAYER_MOVE.down, isMac())}`,
                      })}
                    </p>
                    <ol
                      class="mx-3 mt-1 ml-[108px] overflow-hidden rounded-sm border border-line"
                      aria-label={msg("numbering.order")}
                    >
                      <For each={order()}>
                        {(id, i) => (
                          <li
                            tabIndex={0}
                            draggable="true"
                            aria-label={msg("numbering.orderItem", {
                              n: i() + 1,
                              name: byId(id) ? nodeName(byId(id) as NumberedNode) : String(id),
                            })}
                            class="flex h-6 items-center gap-1.5 border-b border-line bg-control px-1.5 text-sm text-fg last:border-b-0 focus-visible:-outline-offset-2"
                            onDragStart={(e) => {
                              dragged = id;
                              if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
                            }}
                            onDragOver={(e) => dragged !== null && e.preventDefault()}
                            onDrop={(e) => {
                              e.preventDefault();
                              if (dragged === null) return;
                              const from = order().indexOf(dragged);
                              editSeats({}, reorder(order(), dragged, i() - from));
                              dragged = null;
                            }}
                            onKeyDown={(e) => {
                              if (!e.altKey || (e.key !== "ArrowUp" && e.key !== "ArrowDown"))
                                return;
                              e.preventDefault();
                              const el = e.currentTarget;
                              editSeats({}, reorder(order(), id, e.key === "ArrowUp" ? -1 : 1));
                              // The item moved: keep focus on it at its new place.
                              queueMicrotask(() => {
                                const list = el.parentElement;
                                const at = order().indexOf(id);
                                (list?.children[at] as HTMLElement | undefined)?.focus();
                              });
                            }}
                          >
                            <Icon name="grip" class="size-3.5 text-faint" />
                            <span class="w-4 text-faint tabular-nums">{i() + 1}</span>
                            <span class="truncate">
                              {byId(id) ? nodeName(byId(id) as NumberedNode) : id}
                            </span>
                          </li>
                        )}
                      </For>
                    </ol>
                  </Show>
                </Show>
              </Group>
            )}
          </Show>

          <Show when={overrides().rows + overrides().seats > 0}>
            <section class="border-b border-line py-2">
              <CheckField
                label={msg("numbering.keep")}
                checked={draft().keepOverrides}
                onChange={(keepOverrides) => setDraft((d) => ({ ...d, keepOverrides }))}
              />
              <p class="px-3 pl-[108px] text-xs text-muted">
                {draft().keepOverrides
                  ? msg("numbering.keepOn")
                  : msg("numbering.keepOff", {
                      rows: overrides().rows,
                      seats: overrides().seats,
                    })}
              </p>
            </section>
          </Show>

          <section class="px-3 py-2" aria-live="polite" aria-busy={pending()}>
            <h3 class="flex items-center justify-between pb-1 text-xs font-semibold tracking-wide text-faint uppercase">
              {touched(draft()) ? msg("numbering.preview") : msg("numbering.current")}
              <Show when={pending()}>
                <span class="font-normal tracking-normal normal-case">
                  {msg("numbering.updating")}
                </span>
              </Show>
            </h3>
            <Show
              when={preview()?.rows.length}
              fallback={<p class="text-sm text-faint">{msg("numbering.noPreview")}</p>}
            >
              <div class="space-y-0.5 rounded-sm border border-line bg-control p-2 font-mono text-sm">
                <For each={sampleRows()}>
                  {(row) => (
                    <div class="flex gap-2">
                      <span class="w-8 shrink-0 truncate text-fg">{row.label}</span>
                      <span class="min-w-0 truncate text-muted">
                        {row.seats.length > SAMPLE_SEATS
                          ? `${row.seats
                              .slice(0, SAMPLE_SEATS - 1)
                              .map((s) => s.label)
                              .join(" ")} … ${row.seats.at(-1)?.label ?? ""}`
                          : row.seats.map((s) => s.label).join(" ")}
                      </span>
                    </div>
                  )}
                </For>
                <Show when={(preview()?.rows.length ?? 0) > SAMPLE_ROWS}>
                  <p class="text-faint">
                    {msg("numbering.moreRows", { n: (preview()?.rows.length ?? 0) - SAMPLE_ROWS })}
                  </p>
                </Show>
              </div>
              <Show when={full()}>
                {(f) => (
                  <p class="pt-1.5 text-sm text-muted">
                    {msg("numbering.fullLabel")} <span class="font-mono text-fg">{f()}</span>
                  </p>
                )}
              </Show>
            </Show>
          </section>
        </div>
        <div class="flex shrink-0 items-center gap-2 border-t border-line px-3 py-2">
          <Show when={error()}>
            {(e) => (
              <p class="min-w-0 flex-1 text-xs text-danger" role="alert">
                {e()}
              </p>
            )}
          </Show>
          <Show when={error() === null}>
            <p class="min-w-0 flex-1 text-xs text-faint">
              {touched(draft()) ? msg("numbering.oneStep") : msg("numbering.untouched")}
            </p>
          </Show>
          <Button disabled={!touched(draft())} onClick={() => setDraft(EMPTY)}>
            {msg("numbering.reset")}
          </Button>
          <Button variant="primary" disabled={!touched(draft())} onClick={apply}>
            {msg("numbering.apply")}
          </Button>
        </div>
      </Show>
    </div>
  );
}

function Group(props: { title: string; set: boolean; onReset: () => void; children: unknown }) {
  return (
    <section class="border-b border-line py-2">
      <h3 class="flex h-5 items-center justify-between px-3 pb-1 text-xs font-semibold tracking-wide text-faint uppercase">
        {props.title}
        <Show
          when={props.set}
          fallback={
            <span class="font-normal tracking-normal normal-case">{msg("numbering.keepEach")}</span>
          }
        >
          <span class="flex items-center gap-1 font-normal tracking-normal normal-case">
            <span class="text-accent">{msg("numbering.willSet")}</span>
            <IconButton
              icon="undo"
              label={msg("numbering.resetGroup")}
              class="size-5"
              onClick={props.onReset}
            />
          </span>
        </Show>
      </h3>
      {props.children as never}
    </section>
  );
}

const unset = (mixed: boolean, value: string | null) => mixed && value === null;

function Affixes(props: {
  prefix: string | null;
  suffix: string | null;
  mixed: boolean;
  onPrefix: (v: string) => void;
  onSuffix: (v: string) => void;
}) {
  return (
    <>
      <TextField
        label={msg("numbering.prefix")}
        value={props.prefix}
        mixed={unset(props.mixed, props.prefix)}
        onInput={props.onPrefix}
        onCommit={() => null}
      />
      <TextField
        label={msg("numbering.suffix")}
        value={props.suffix}
        mixed={unset(props.mixed, props.suffix)}
        onInput={props.onSuffix}
        onCommit={() => null}
      />
    </>
  );
}
