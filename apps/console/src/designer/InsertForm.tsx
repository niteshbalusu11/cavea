// The "Insert row block…" form (07 §15): rows, seats, pitches, position,
// rotation, curve, section and category, checked as the edit core checks
// them; one undo entry; the new block selected and revealed in the layers
// panel. Opened from the tool rail's row-block tool by the keyboard, the
// command palette and the menu.
import { createSignal, createUniqueId, For, Show } from "solid-js";
import { formatNumber, msg, type MessageId } from "../i18n/messages.ts";
import { Button } from "../ui/controls.tsx";
import { inputClass, Row, SelectField } from "../ui/fields.tsx";
import { Dialog } from "../ui/overlays.tsx";
import { selectedSection } from "./commands.ts";
import type { Editor } from "./editor.ts";
import {
  check,
  fieldOf,
  insertBlock,
  MAX_SEATS_PER_ROW,
  MIN_RADIUS,
  type BlockSpec,
  type SpecField,
} from "./insert.ts";

const round = (n: number) => String(Math.round(n));
const num = (s: string) => (s.trim() === "" ? Number.NaN : Number(s.replaceAll(",", "")));

type Text = Record<
  "rows" | "seats" | "seatPitch" | "rowPitch" | "radius" | "x" | "y" | "rotation",
  string
>;

/** The section the form offers: the one selected, or the one whose
 * children are selected. */
function sectionOf(editor: Editor): number | null {
  const level = editor.views.selection.level;
  return level.kind === "section" ? level.id : selectedSection(editor);
}

export function InsertForm(props: { editor: Editor; open: boolean; onClose: () => void }) {
  return (
    <Dialog
      open={props.open}
      title={msg("insert.title")}
      heading
      onClose={props.onClose}
      class="max-w-md"
      initial={() => document.querySelector<HTMLElement>("[data-field=rows]") ?? undefined}
    >
      <Show when={props.open}>
        <Form editor={props.editor} onClose={props.onClose} />
      </Show>
    </Dialog>
  );
}

function Form(props: { editor: Editor; onClose: () => void }) {
  const editor = props.editor;
  const id = createUniqueId();
  const cam = editor.camera();
  const [text, setText] = createSignal<Text>({
    rows: "10",
    seats: "20",
    seatPitch: "55",
    rowPitch: "95",
    radius: "1500",
    x: round(cam.x),
    y: round(cam.y),
    rotation: "0",
  });
  const [curve, setCurve] = createSignal<BlockSpec["curve"]>("straight");
  const section = sectionOf(editor);
  const [parent, setParent] = createSignal<number | null>(section);
  const [category, setCategory] = createSignal(
    editor.views.tool.category ?? editor.views.categories.categories[0]?.key ?? "",
  );
  const [errors, setErrors] = createSignal<Partial<Record<SpecField, string>>>({});
  const [general, setGeneral] = createSignal<string | null>(null);
  const form: HTMLFormElement[] = [];

  const spec = (): BlockSpec => {
    const t = text();
    return {
      curve: curve(),
      rows: num(t.rows),
      seats: num(t.seats),
      seatPitch: num(t.seatPitch),
      rowPitch: num(t.rowPitch),
      radius: num(t.radius),
      x: num(t.x),
      y: num(t.y),
      rotation: num(t.rotation),
      parent: parent(),
      category: category(),
    };
  };
  const words: Record<string, (f: SpecField) => string> = {
    whole: () => msg("insert.whole"),
    positive: () => msg("insert.positive"),
    number: () => msg("inspector.notNumber"),
    range: (f) =>
      f === "radius"
        ? msg("insert.radius", { min: MIN_RADIUS })
        : msg("insert.max", { max: f === "seats" ? MAX_SEATS_PER_ROW : 65_535 }),
  };

  const submit = (e: Event) => {
    e.preventDefault();
    const s = spec();
    const local = check(s);
    const found = Object.fromEntries(
      Object.entries(local).map(([f, kind]) => [f, words[kind]?.(f as SpecField) ?? kind]),
    ) as Partial<Record<SpecField, string>>;
    setErrors(found);
    setGeneral(null);
    const firstBad = Object.keys(found)[0];
    if (firstBad !== undefined) return focusField(firstBad);
    const r = editor.attempt((session) => insertBlock(session, s));
    if ("ok" in r) {
      editor.reveal(r.ok);
      props.onClose();
      return;
    }
    // A refusal names a field when it is about one (`draft.rs`).
    const f = r.failure.kind === "command" ? fieldOf(r.failure.error) : null;
    if (f === null) return setGeneral(r.error);
    setErrors({ [f]: r.error });
    focusField(f);
  };
  const focusField = (f: string) =>
    form[0]?.querySelector<HTMLElement>(`[data-field="${f}"]`)?.focus();

  const field = (f: keyof Text, label: MessageId, unit = "") => {
    const fid = `${id}-${f}`;
    return (
      <Row label={msg(label)} for={fid} error={errors()[f] ?? null} errorId={`${fid}-error`}>
        <div class="relative">
          <input
            id={fid}
            data-field={f}
            inputmode="decimal"
            class={`${inputClass} tabular-nums ${unit === "" ? "" : "pr-8"}`}
            value={text()[f]}
            aria-invalid={errors()[f] !== undefined}
            aria-describedby={errors()[f] !== undefined ? `${fid}-error` : undefined}
            onInput={(e) => {
              const value = e.currentTarget.value;
              setText((t) => ({ ...t, [f]: value }));
              setErrors(({ [f]: _gone, ...rest }) => rest);
            }}
          />
          <Show when={unit !== ""}>
            <span class="pointer-events-none absolute inset-y-0 right-1.5 flex items-center text-xs text-faint">
              {unit}
            </span>
          </Show>
        </div>
      </Row>
    );
  };

  const seats = () => {
    const s = spec();
    if (!Number.isInteger(s.rows) || !Number.isInteger(s.seats)) return null;
    return s.curve === "straight" ? s.rows * s.seats : null;
  };

  return (
    <form
      ref={(el) => form.push(el)}
      class="flex min-h-0 flex-col"
      onSubmit={submit}
      noValidate
      aria-describedby={`${id}-hint`}
    >
      <div class="min-h-0 overflow-y-auto py-2">
        <p id={`${id}-hint`} class="px-5 pb-2 text-sm text-muted">
          {msg("insert.body")}
        </p>
        <div
          role="radiogroup"
          aria-label={msg("field.curve")}
          class="flex gap-1 px-3 pb-2 pl-[108px]"
        >
          <For each={["straight", "arc"] as const}>
            {(c) => (
              <button
                type="button"
                role="radio"
                aria-checked={curve() === c}
                class="h-6 rounded-sm border border-control-line px-2 text-sm text-muted hover:bg-hover hover:text-fg aria-checked:border-accent-fill aria-checked:bg-accent-soft aria-checked:text-accent"
                onClick={() => setCurve(c)}
              >
                {msg(c === "straight" ? "curve.none" : "curve.arc")}
              </button>
            )}
          </For>
        </div>
        {field("rows", "field.rows")}
        {field("seats", curve() === "arc" ? "insert.frontSeats" : "field.seats")}
        {field("seatPitch", "field.seatPitch", msg("unit.cu"))}
        {field("rowPitch", "field.rowPitch", msg("unit.cu"))}
        <Show when={curve() === "arc"}>
          {field("radius", "insert.radiusField", msg("unit.cu"))}
        </Show>
        {field("x", "field.x", msg("unit.cu"))}
        {field("y", "field.y", msg("unit.cu"))}
        {field("rotation", "field.rotation", msg("unit.degrees"))}
        <p class="px-3 pb-1 pl-[108px] text-xs text-faint">{msg("insert.positionHint")}</p>
        <SelectField
          label={msg("insert.parent")}
          options={[
            { value: "", label: msg("insert.topLevel") },
            ...(section === null
              ? []
              : [
                  {
                    value: String(section),
                    label: msg("layers.unnamed", {
                      kind: msg("kind.section"),
                      id: String(section),
                    }),
                  },
                ]),
          ]}
          value={parent() === null ? "" : String(parent())}
          onChange={(v) => setParent(v === "" ? null : Number(v))}
        />
        <SelectField
          label={msg("field.category")}
          options={editor.views.categories.categories.map((c) => ({
            value: c.key,
            label: c.label,
          }))}
          value={category()}
          onChange={setCategory}
        />
      </div>
      <div class="flex shrink-0 items-center gap-2 border-t border-line px-5 py-3">
        <p class="min-w-0 flex-1 text-sm" role="status">
          <Show
            when={general()}
            fallback={
              <span class="text-muted">
                {/* An expression, not a <Show> child: a child that returns a
                    string is read once, and the total would never follow
                    the fields. */}
                {seats() === null ? "" : msg("insert.total", { n: formatNumber(seats() ?? 0) })}
              </span>
            }
          >
            {(g) => (
              <span class="text-danger" role="alert">
                {g()}
              </span>
            )}
          </Show>
        </p>
        <Button onClick={props.onClose}>{msg("action.cancel")}</Button>
        <Button type="submit" variant="primary">
          {msg("insert.submit")}
        </Button>
      </div>
    </form>
  );
}
