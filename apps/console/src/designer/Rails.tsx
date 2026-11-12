// The tool rail and the status bar (07 §3).
import type { Readout, ToolId } from "@cavea/designer-core";
import { For, Show } from "solid-js";
import { formatNumber, msg } from "../i18n/messages.ts";
import { IconButton } from "../ui/controls.tsx";
import { Icon } from "../ui/Icon.tsx";
import type { Editor } from "./editor.ts";
import { nodeName } from "./findings.ts";
import { keyOf } from "./shortcuts.ts";

const GROUPS: ToolId[][] = [
  ["select", "hand"],
  ["rowBlock", "arcBlock", "row"],
  ["rect", "ellipse", "polygon", "line", "text"],
  ["section", "brush", "focal"],
];

const r1 = (n: number) => Math.round(n * 10) / 10;

/** The live readout of the gesture in progress (07 §4), in words. */
function readoutText(r: Readout | null): string {
  if (r === null) return "";
  switch (r.kind) {
    case "block":
      return msg("readout.block", { rows: r.data.rows, seats: r.data.seats });
    case "arc":
      return msg("readout.arc", {
        radius: r1(r.data.radius),
        span: r1(r.data.span),
        rows: r.data.rows,
        seats: r.data.seats,
      });
    case "row":
      return msg("readout.row", { length: r1(r.data.length), seats: r.data.seats });
    case "size":
      return msg("readout.size", { width: r1(r.data.width), height: r1(r.data.height) });
    case "line":
      return msg("readout.line", { length: r1(r.data.length), angle: r1(r.data.angle) });
    case "points":
      return msg("readout.points", { n: r.data });
    case "rotate":
      return msg("readout.rotate", { angle: r1(r.data.angle) });
    case "curve":
      return r.data.radius === null
        ? msg("readout.curve", { depth: r1(r.data.depth) })
        : msg("readout.curveArc", { depth: r1(r.data.depth), radius: r1(r.data.radius) });
  }
}

/** Moves focus among a toolbar's buttons with the arrow keys, Home and End. */
export function rove(e: KeyboardEvent, vertical: boolean): void {
  const keys = vertical ? ["ArrowUp", "ArrowDown"] : ["ArrowLeft", "ArrowRight"];
  if (![...keys, "Home", "End"].includes(e.key)) return;
  const bar = e.currentTarget as HTMLElement;
  const buttons = [...bar.querySelectorAll<HTMLElement>("button")];
  const at = buttons.indexOf(document.activeElement as HTMLElement);
  const next =
    e.key === "Home" ? 0 : e.key === "End" ? buttons.length - 1 : at + (e.key === keys[0] ? -1 : 1);
  const target = buttons[Math.max(0, Math.min(buttons.length - 1, next))];
  if (target === undefined) return;
  e.preventDefault();
  target.focus();
}

/** The tool rail. The row-block tool chosen with the keyboard (Enter or
 * Space on its button) also opens "Insert row block…" (07 §15): drawing
 * needs a pointer, the form does not. */
export function ToolRail(props: {
  editor: Editor;
  onInsert: () => void;
  /** Picks an image file to place; absent when the chart is read-only. */
  onImage: (() => void) | undefined;
}) {
  const active = () => props.editor.views.tool.tool;
  return (
    <nav
      role="toolbar"
      aria-label={msg("rail.label")}
      aria-orientation="vertical"
      class="flex w-10 shrink-0 flex-col items-center gap-0.5 border-r border-line bg-app py-1.5"
      onKeyDown={(e) => rove(e, true)}
    >
      <For each={GROUPS}>
        {(group, i) => (
          <>
            <Show when={i() > 0}>
              <div class="my-1 h-px w-5 bg-line" />
            </Show>
            <For each={group}>
              {(tool) => (
                <IconButton
                  icon={tool}
                  label={msg(`tool.${tool}`)}
                  keys={keyOf(`tool.${tool}`)}
                  placement="right"
                  pressed={active() === tool}
                  tabIndex={active() === tool ? 0 : -1}
                  onClick={(e) => {
                    props.editor.setTool(tool);
                    // A click from the keyboard has no pointer position (detail 0).
                    if (tool === "rowBlock" && e.detail === 0) props.onInsert();
                  }}
                />
              )}
            </For>
          </>
        )}
      </For>
      <div class="my-1 h-px w-5 bg-line" />
      <IconButton
        icon="image"
        label={msg("shortcut.image")}
        keys={keyOf("image")}
        placement="right"
        tabIndex={-1}
        reason={props.onImage === undefined ? msg("readOnly.refused") : null}
        onClick={() => props.onImage?.()}
      />
    </nav>
  );
}

function Toggle(props: { label: string; keys: string | null; on: boolean; onClick: () => void }) {
  return (
    <button
      type="button"
      aria-pressed={props.on}
      title={props.keys === null ? props.label : `${props.label} (${props.keys})`}
      class="inline-flex h-5 items-center gap-1 rounded-sm px-1.5 text-sm text-muted hover:bg-hover hover:text-fg aria-pressed:text-fg"
      onClick={() => props.onClick()}
    >
      <span
        class={`size-1.5 rounded-full ${props.on ? "bg-accent-fill" : "border border-control-line"}`}
      />
      {props.label}
    </button>
  );
}

export function StatusBar(props: {
  editor: Editor;
  act: Record<string, () => void>;
  onProblems: () => void;
}) {
  const v = props.editor.views;
  const snapOn = () => {
    const s = v.tool.snap;
    return s.objects || s.spacing || s.rows || s.angles || s.guides;
  };
  const zoom = () => `${formatNumber(Math.round(props.editor.camera().zoom * 1000) / 10)}%`;
  const call = (id: string) => () => props.act[id]?.();
  return (
    <footer class="flex h-7 shrink-0 items-center gap-3 border-t border-line bg-app px-3 text-sm text-muted">
      <span class="tabular-nums">{msg("status.seats", { n: v.status.seats })}</span>
      <span class="tabular-nums">{msg("status.nodes", { n: v.status.nodes })}</span>
      <Show when={v.selection.count > 0}>
        <span class="truncate tabular-nums text-fg">
          {v.selection.level.kind === "section"
            ? // A new object drawn inside a section goes into it: say so.
              msg("status.selectedIn", {
                n: v.selection.count,
                section: nodeName(v.selection.within, "section", v.selection.level.id),
              })
            : msg("status.selected", { n: v.selection.count })}
        </span>
      </Show>
      <button
        type="button"
        class="inline-flex h-5 items-center gap-1 rounded-sm px-1.5 hover:bg-hover hover:text-fg"
        onClick={() => props.onProblems()}
      >
        <Show
          when={v.validation.errors + v.validation.warnings > 0}
          fallback={
            <span>{v.validation.complete ? msg("status.noProblems") : msg("status.checking")}</span>
          }
        >
          <Show when={v.validation.errors > 0}>
            <Icon name="error" class="size-3.5 text-danger" />
          </Show>
          <Show when={v.validation.errors === 0}>
            <Icon name="warning" class="size-3.5 text-warning" />
          </Show>
          <span class="tabular-nums">
            {msg("status.problems", {
              errors: v.validation.errors,
              warnings: v.validation.warnings,
            })}
          </span>
        </Show>
      </button>
      <Show
        when={(() => {
          const u = props.editor.images?.uploads() ?? null;
          return u !== null && u.sending + u.waiting > 0 ? u : null;
        })()}
      >
        {(u) => (
          <span class="inline-flex items-center gap-1" role="status">
            <span classList={{ "text-warning": u().waiting > 0 }}>
              {u().waiting > 0
                ? msg("image.waitingCount", { n: u().waiting })
                : msg("image.uploadingCount", { n: u().sending })}
            </span>
            <Show when={u().waiting > 0}>
              <button
                type="button"
                class="h-5 rounded-sm px-1.5 text-accent hover:bg-hover"
                onClick={() => props.editor.images?.retry()}
              >
                {msg("action.retry")}
              </button>
            </Show>
          </span>
        )}
      </Show>
      <Show when={props.editor.readout()}>
        {(r) => (
          <span class="font-mono text-xs text-fg tabular-nums" aria-live="polite">
            {readoutText(r().readout)}
            <Show when={r().entry !== ""}>
              <span class="ml-2 text-accent">{msg("readout.entry", { entry: r().entry })}</span>
            </Show>
          </span>
        )}
      </Show>
      <Show when={props.editor.pointer()}>
        {(p) => (
          <span class="ml-auto font-mono text-xs tabular-nums">
            {msg("status.cursor", {
              x: formatNumber(Math.round(p()[0])),
              y: formatNumber(Math.round(p()[1])),
            })}
          </span>
        )}
      </Show>
      <div class="ml-auto flex items-center gap-1">
        <Toggle
          label={msg("status.snap")}
          keys={keyOf("snap")}
          on={snapOn()}
          onClick={call("snap")}
        />
        <Toggle
          label={msg("status.grid")}
          keys={keyOf("grid")}
          on={v.tool.snap.grid}
          onClick={call("grid")}
        />
        <div class="mx-2 h-4 w-px bg-line" />
        <IconButton
          icon="minus"
          label={msg("status.zoomOut")}
          keys={keyOf("zoomOut")}
          placement="top"
          class="size-5"
          onClick={call("zoomOut")}
        />
        <button
          type="button"
          class="h-5 w-14 rounded-sm text-center tabular-nums hover:bg-hover hover:text-fg"
          aria-label={`${msg("status.zoomFit")} (${zoom()})`}
          title={`${msg("status.zoomFit")} (${keyOf("zoomFit") ?? ""})`}
          onClick={call("zoomFit")}
        >
          {zoom()}
        </button>
        <IconButton
          icon="plus"
          label={msg("status.zoomIn")}
          keys={keyOf("zoomIn")}
          placement="top"
          class="size-5"
          onClick={call("zoomIn")}
        />
      </div>
    </footer>
  );
}
