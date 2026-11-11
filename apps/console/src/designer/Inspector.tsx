// The inspector (07 §5.3, §5.5): fields generated from `inspector()` by
// kind, "Mixed" where the selection differs, every change one `setField`
// (07 §2.3); number fields scrub. The chart's settings when nothing is
// selected.
import type { Curve, Field, Num, Shared } from "@cavea/designer-core";
import { Show, type JSX } from "solid-js";
import { formatNumber, msg, msgOf, type MessageId } from "../i18n/messages.ts";
import { CheckField, NumberField, SelectField, TextField, type Option } from "../ui/fields.tsx";
import type { Editor } from "./editor.ts";

const same = <T,>(s: Shared<T>): T | null => (s === "mixed" ? null : s.same);
const options = (prefix: string, values: readonly string[]): Option[] =>
  values.map((value) => ({ value, label: msgOf(prefix, value) ?? value }));
type Name = Field["field"];

function Group(props: { title: string; children: JSX.Element }) {
  return (
    <section class="border-b border-line py-2 last:border-b-0">
      <h3 class="px-3 pb-1 text-xs font-semibold tracking-wide text-faint uppercase">
        {props.title}
      </h3>
      {props.children}
    </section>
  );
}

export function Inspector(props: { editor: Editor }) {
  const editor = props.editor;
  const v = () => editor.views.inspector;
  const set = (field: Field): string | null => {
    const r = editor.attempt((s) => s.setField(field));
    return "error" in r ? r.error : null;
  };
  const scrub = (field: Name) => ({
    begin: () => "ok" in editor.attempt((s) => s.beginScrub()),
    // A refused value keeps the last good one (07 §2.3): nothing to show.
    set: (value: number) => void editor.attempt((s) => s.setField({ field, value } as Field)),
    end: () => editor.run((s) => s.endScrub()),
    cancel: () => editor.run((s) => s.cancelScrub()),
  });

  /** A number field of the view, set as `field`. */
  const num = (field: Name, n: Num, label: MessageId = `field.${field}` as MessageId, step = 1) => (
    <NumberField
      label={msg(label)}
      value={same(n.value)}
      min={n.min}
      max={n.max}
      step={step}
      unit={msg(`unit.${n.unit}`)}
      onCommit={(value) => set({ field, value } as Field)}
      scrub={scrub(field)}
    />
  );
  const choice = (
    field: Name,
    prefix: string,
    values: readonly string[],
    value: string | null,
    label?: MessageId,
  ) => (
    <SelectField
      label={msg(label ?? (`field.${field}` as MessageId))}
      options={options(prefix, values)}
      value={value}
      onChange={(picked) => void set({ field, value: picked } as Field)}
    />
  );
  const text = (field: Name, value: Shared<string | null>, nullable: boolean) => (
    <TextField
      label={msg(`field.${field}` as MessageId)}
      value={same(value)}
      mixed={value === "mixed"}
      onCommit={(raw) => set({ field, value: nullable && raw.trim() === "" ? null : raw } as Field)}
    />
  );
  const curve = (c: Shared<Curve>) => {
    const cur = same(c);
    return (
      <>
        <SelectField
          label={msg("field.curve")}
          options={options("curve", ["none", "arc", "bulge"])}
          value={cur?.kind ?? null}
          onChange={(kind) => {
            const value: Curve =
              kind === "arc"
                ? { kind, radius: 2000 }
                : kind === "bulge"
                  ? { kind, amount: 100 }
                  : { kind: "none" };
            editor.run((s) => s.setField({ field: "curve", value }));
          }}
        />
        <Show when={cur?.kind === "arc" && cur}>
          {(arc) => (
            <NumberField
              label={msg("field.curveRadius")}
              value={"radius" in arc() ? (arc() as { radius: number }).radius : null}
              min={1}
              max={1e7}
              unit={msg("unit.cu")}
              onCommit={(radius) => set({ field: "curve", value: { kind: "arc", radius } })}
            />
          )}
        </Show>
        <Show when={cur?.kind === "bulge" && cur}>
          {(b) => (
            <NumberField
              label={msg("field.curveAmount")}
              value={"amount" in b() ? (b() as { amount: number }).amount : null}
              min={-1e7}
              max={1e7}
              unit={msg("unit.cu")}
              onCommit={(amount) => set({ field: "curve", value: { kind: "bulge", amount } })}
            />
          )}
        </Show>
      </>
    );
  };

  const heading = () => {
    const sel = editor.views.selection;
    if (v().chart !== null) return msg("inspector.settings");
    if (sel.level.kind === "rows") return msg("inspector.level.rows");
    if (sel.level.kind === "seats") return msg("inspector.level.seats");
    const [only] = sel.kinds;
    if (sel.kinds.length === 1 && only !== undefined) {
      return only.count === 1
        ? msg(`kind.${only.kind}`)
        : msg("inspector.many", { n: only.count, kinds: msg(`kinds.${only.kind}`) });
    }
    return msg("inspector.selected", { n: sel.count });
  };
  const categories = (): Option[] => [
    { value: "", label: msg("inspector.inherit") },
    ...editor.views.categories.categories.map((c) => ({ value: c.key, label: c.label })),
  ];
  const floors = (): Option[] =>
    editor.views.floors.floors.map((f) => ({ value: String(f.id), label: f.name }));

  return (
    <div class="min-h-0 flex-1 overflow-y-auto">
      <p class="px-3 pt-2 pb-1 text-base font-semibold text-fg" aria-live="polite">
        {heading()}
      </p>
      <Show
        when={
          editor.views.selection.level.kind === "rows" ||
          editor.views.selection.level.kind === "seats"
        }
      >
        <p class="px-3 pb-2 text-sm text-muted">{msg("inspector.levelHint")}</p>
      </Show>

      <Show when={v().chart}>
        {(c) => (
          <Group title={msg("inspector.chart")}>
            <TextField
              label={msg("field.chartName")}
              value={c().name}
              onCommit={(value) => set({ field: "chartName", value })}
            />
            {num("seatSize", c().seatSize)}
            {choice("seatShape", "seatShape", ["circle", "rounded", "chair"], c().seatShape)}
            <TextField
              label={msg("field.labelTemplate")}
              value={c().labelTemplate}
              onCommit={(value) => set({ field: "labelTemplate", value })}
            />
            <TextField
              label={msg("field.tableLabelTemplate")}
              value={c().tableLabelTemplate}
              onCommit={(value) => set({ field: "tableLabelTemplate", value })}
            />
            {choice(
              "rowLabelDisplay",
              "rowLabelDisplay",
              ["none", "start", "end", "both"],
              c().rowLabelDisplay,
            )}
            {choice("alphaOverflow", "alphaOverflow", ["double", "spreadsheet"], c().alphaOverflow)}
          </Group>
        )}
      </Show>

      <Show when={v().common}>
        {(c) => (
          <Group title={msg("inspector.position")}>
            {text("name", c().name, true)}
            {num("x", c().x)}
            {num("y", c().y)}
            {num("rotation", c().rotation)}
            <Show when={c().floor !== null && floors().length > 0}>
              <SelectField
                label={msg("field.floor")}
                options={floors()}
                value={(() => {
                  const f = c().floor;
                  return f === null ? null : (same(f)?.toString() ?? null);
                })()}
                onChange={(id) => void set({ field: "floor", value: Number(id) })}
              />
            </Show>
            <Show when={c().category}>
              {(cat) => (
                <SelectField
                  label={msg("field.category")}
                  options={categories()}
                  value={cat() === "mixed" ? null : (same(cat()) ?? "")}
                  onChange={(key) =>
                    void set({ field: "category", value: key === "" ? null : key })
                  }
                />
              )}
            </Show>
            <Show when={c().label}>{(l) => text("label", l(), true)}</Show>
            <CheckField
              label={msg("field.locked")}
              checked={same(c().locked)}
              onChange={(on) => void set({ field: "locked", value: on })}
            />
            <CheckField
              label={msg("field.hidden")}
              checked={same(c().hidden)}
              onChange={(on) => void set({ field: "hidden", value: on })}
            />
          </Group>
        )}
      </Show>

      <Show when={v().rowBlock}>
        {(b) => (
          <Group title={msg("kind.rowBlock")}>
            {num("rows", b().rows)}
            {num("seats", b().seats)}
            {num("seatPitch", b().seatPitch)}
            {num("rowPitch", b().rowPitch)}
            {curve(b().curve)}
            {choice("align", "align", ["left", "center", "right"], same(b().align))}
            {num("stagger", b().stagger, "field.stagger", 0.5)}
            {num("skew", b().skew)}
            <p class="px-3 pt-1 text-sm text-faint">{msg("inspector.numberingPending")}</p>
          </Group>
        )}
      </Show>

      <Show when={v().row}>
        {(r) => (
          <Group title={msg("kind.row")}>
            {num("seatPitch", r().seatPitch)}
            <div
              class="grid grid-cols-[88px_minmax(0,1fr)] items-center gap-x-2 px-3 py-1"
              title={msg("inspector.readOnly")}
            >
              <span class="text-sm text-muted">{msg("field.seatsInRow")}</span>
              <span class="text-base text-fg tabular-nums">
                {(() => {
                  const n = same(r().seats);
                  return n === null ? msg("inspector.mixed") : formatNumber(n);
                })()}
              </span>
            </div>
          </Group>
        )}
      </Show>

      <Show when={v().section}>
        {(sec) => (
          <Group title={msg("kind.section")}>
            {num("priority", sec().priority)}
            {text("entrance", sec().entrance, true)}
            <div class="flex items-center gap-2 px-3 py-0.5">
              <span class="w-[88px] text-sm text-muted">{msg("field.autoOutline")}</span>
              <span class="text-sm text-fg">
                {same(sec().autoOutline) === false
                  ? msg("inspector.outlineExplicit")
                  : msg("inspector.outlineAuto")}
              </span>
            </div>
          </Group>
        )}
      </Show>

      <Show when={v().shape}>
        {(sh) => (
          <Group title={msg("kind.shape")}>
            {choice("layer", "layer", ["below", "above"], same(sh().layer))}
            <Show when={sh().width}>{(w) => num("width", w())}</Show>
            <Show when={sh().height}>{(h) => num("height", h())}</Show>
            <Show when={sh().cornerRadius}>{(r) => num("cornerRadius", r())}</Show>
            <Show when={sh().fill}>{(f) => text("fill", f(), true)}</Show>
            {text("stroke", sh().stroke, true)}
            {num("strokeWidth", sh().strokeWidth)}
            {num("opacity", sh().opacity, "field.opacity", 0.05)}
          </Group>
        )}
      </Show>

      <Show when={v().text}>
        {(t) => (
          <Group title={msg("kind.text")}>
            {text("text", t().text, false)}
            {num("textSize", t().size)}
            {num("weight", t().weight, "field.weight", 100)}
            {text("color", t().color, false)}
            {choice("textAlign", "align", ["left", "center", "right"], same(t().align))}
            {num("minZoom", t().minZoom, "field.minZoom", 0.1)}
          </Group>
        )}
      </Show>

      <Show when={v().image}>
        {(img) => (
          <Group title={msg("kind.image")}>
            {num("width", img().width)}
            {num("height", img().height)}
            {num("opacity", img().opacity, "field.opacity", 0.05)}
            {choice("imageRole", "imageRole", ["reference", "background"], same(img().role))}
          </Group>
        )}
      </Show>
    </div>
  );
}
