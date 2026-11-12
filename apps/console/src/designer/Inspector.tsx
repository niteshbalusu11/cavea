// The inspector (07 §5.3, §5.5): fields generated from `inspector()` by
// kind, "Mixed" where the selection differs, every change one `setField`
// (07 §2.3); number fields scrub. The chart's settings when nothing is
// selected.
import type { Curve, Field, ImageRole, NodeKind, Num, Shared } from "@cavea/designer-core";
import { For, Show, type JSX } from "solid-js";
import { formatNumber, msg, msgOf, type MessageId } from "../i18n/messages.ts";
import { Button } from "../ui/controls.tsx";
import {
  CheckField,
  ColorField,
  NumberField,
  SelectField,
  TextField,
  type Option,
} from "../ui/fields.tsx";
import { groupSelection, groupReason, selectedSection } from "./commands.ts";
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

/** "1 row block", "2 row blocks". */
export const countOf = (k: { kind: NodeKind; count: number }): string =>
  k.count === 1
    ? msg("inspector.one", { kind: msg(`kindOne.${k.kind}`) })
    : msg("inspector.many", { n: k.count, kinds: msg(`kinds.${k.kind}`) });

/** "Row block", "2 row blocks", "3 objects: 2 row blocks, 1 shape". */
export function selectionName(kinds: { kind: NodeKind; count: number }[], count: number): string {
  const each = countOf;
  const [only] = kinds;
  if (kinds.length === 1 && only !== undefined) {
    return only.count === 1 ? msg(`kind.${only.kind}`) : each(only);
  }
  return msg("inspector.objects", { n: count, list: kinds.map(each).join(", ") });
}

export function Inspector(props: { editor: Editor; onNumbering: () => void }) {
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
        {/* Read from `cur`, the view's own object, not a <Show> accessor:
            the view changes that object in place, so a field's effect can
            run after its <Show> has closed and before it is disposed, and
            an accessor read then throws ("Stale read from <Show>"), which
            stopped the designer following edits after an Insert. */}
        <Show when={cur?.kind === "arc"}>
          <NumberField
            label={msg("field.curveRadius")}
            value={cur?.kind === "arc" ? cur.radius : null}
            min={1}
            max={1e7}
            unit={msg("unit.cu")}
            onCommit={(radius) => set({ field: "curve", value: { kind: "arc", radius } })}
          />
        </Show>
        <Show when={cur?.kind === "bulge"}>
          <NumberField
            label={msg("field.curveAmount")}
            value={cur?.kind === "bulge" ? cur.amount : null}
            min={-1e7}
            max={1e7}
            unit={msg("unit.cu")}
            onCommit={(amount) => set({ field: "curve", value: { kind: "bulge", amount } })}
          />
        </Show>
      </>
    );
  };

  const heading = () => {
    const sel = editor.views.selection;
    if (v().chart !== null) return msg("inspector.settings");
    if (sel.level.kind === "rows") return msg("inspector.level.rows");
    if (sel.level.kind === "seats") return msg("inspector.level.seats");
    return selectionName(sel.kinds, sel.count);
  };
  const colour = (field: Name, value: Shared<string | null>, nullable: boolean) => (
    <ColorField
      label={msg(`field.${field}` as MessageId)}
      value={same(value)}
      mixed={value === "mixed"}
      nullable={nullable}
      onCommit={(picked) => set({ field, value: picked } as Field)}
    />
  );
  const numbering = () => (
    <div class="px-3 pt-1.5 pl-[108px]">
      <Button class="h-6 px-2 text-sm" onClick={() => props.onNumbering()}>
        {msg("inspector.numbering")}
      </Button>
    </div>
  );
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
                  // A node without a floor field is on the first floor.
                  const f = c().floor;
                  if (f === null || f === "mixed") return null;
                  return String(f.same ?? editor.views.floors.floors[0]?.id ?? "");
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
            <Show when={editor.views.selection.count > 1 && groupReason(editor) === null}>
              <div class="px-3 pt-1.5 pl-[108px]">
                <Button
                  class="h-6 px-2 text-sm"
                  icon="section"
                  onClick={() => groupSelection(editor)}
                >
                  {msg("layers.group")}
                </Button>
              </div>
            </Show>
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
            {numbering()}
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
            {numbering()}
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
            <Show when={selectedSection(editor)}>
              {(id) => (
                <div class="flex flex-wrap gap-1.5 px-3 pt-1.5 pl-[108px]">
                  <Show when={same(sec().autoOutline) === true}>
                    <Button
                      class="h-6 px-2 text-sm"
                      onClick={() => editor.run((s) => s.convertOutline(id()))}
                    >
                      {msg("inspector.convertOutline")}
                    </Button>
                  </Show>
                  <Button
                    class="h-6 px-2 text-sm"
                    onClick={() => editor.run((s) => s.dissolve(id()))}
                  >
                    {msg("inspector.dissolve")}
                  </Button>
                </div>
              )}
            </Show>
            {numbering()}
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
            <Show when={sh().fill}>{(f) => colour("fill", f(), true)}</Show>
            {colour("stroke", sh().stroke, true)}
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
            {colour("color", t().color, false)}
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
            <ImageStatus editor={editor} role={same(img().role)} asset={same(img().asset)} />
          </Group>
        )}
      </Show>
    </div>
  );
}

/** What the person should know about the selected picture (07 §4): who
 * sees it, whether it is uploaded and loaded, and that a local chart keeps
 * it for this page only. */
function ImageStatus(props: { editor: Editor; role: ImageRole | null; asset: string | null }) {
  const host = () => props.editor.images;
  const notes = (): { text: string; tone: "muted" | "warning" }[] => {
    const out: { text: string; tone: "muted" | "warning" }[] = [];
    if (props.role === "reference") out.push({ text: msg("image.referenceNote"), tone: "muted" });
    if (props.role === "background") out.push({ text: msg("image.backgroundNote"), tone: "muted" });
    const asset = props.asset;
    const h = host();
    if (h === null || asset === null) return out;
    if (h.failed().has(asset)) {
      out.push({ text: msg(h.local ? "image.missingLocal" : "image.missing"), tone: "warning" });
    } else if (h.pending(asset)) {
      // Read so that the note follows the uploads.
      const u = h.uploads();
      out.push({
        text: msg(u !== null && u.waiting > 0 ? "image.waiting" : "image.uploading"),
        tone: "muted",
      });
    } else if (h.local) {
      out.push({ text: msg("image.sessionNote"), tone: "warning" });
    }
    return out;
  };
  return (
    <For each={notes()}>
      {(n) => (
        <p
          class="px-3 py-1 text-sm"
          classList={{ "text-muted": n.tone === "muted", "text-warning": n.tone === "warning" }}
        >
          {n.text}
        </p>
      )}
    </For>
  );
}
