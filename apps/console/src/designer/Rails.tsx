// The tool rail and the status bar (07 §3).
import type { ToolId } from "@cavea/designer-core";
import { For, Show } from "solid-js";
import { formatNumber, msg } from "../i18n/messages.ts";
import { IconButton } from "../ui/controls.tsx";
import { Icon } from "../ui/Icon.tsx";
import type { Editor } from "./editor.ts";
import { keyOf } from "./shortcuts.ts";

const GROUPS: ToolId[][] = [
  ["select", "hand"],
  ["rowBlock", "arcBlock", "row"],
  ["rect", "ellipse", "polygon", "line", "text"],
  ["section", "brush", "focal"],
];

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

export function ToolRail(props: { editor: Editor }) {
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
                  onClick={() => props.editor.setTool(tool)}
                />
              )}
            </For>
          </>
        )}
      </For>
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
        <span class="tabular-nums text-fg">{msg("status.selected", { n: v.selection.count })}</span>
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
