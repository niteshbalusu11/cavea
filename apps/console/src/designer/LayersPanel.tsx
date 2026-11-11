// The layers panel (07 §3, §5.5): the floor shown's tree, virtualised and
// fed page by page from `layers(start, count)` through the frame loop. A
// tree for the keyboard (arrows, Home, End, Enter or Space to select, Alt
// with an arrow to move), drag and drop to reorder and reparent, and a
// context menu with the arrange actions.
import type { Drop, LayerRow, NodeKind, Order } from "@cavea/designer-core";
import { ContextMenu } from "@kobalte/core/context-menu";
import { createEffect, createSignal, For, on, onCleanup, onMount, Show } from "solid-js";
import { msg } from "../i18n/messages.ts";
import { Icon, type IconName } from "../ui/Icon.tsx";
import { menuContent, menuItem, menuKeys, menuSeparator } from "../ui/overlays.tsx";
import { scrollToShow, windowFor } from "../ui/window.ts";
import type { Editor } from "./editor.ts";
import { display, isMac, keyOf, LAYER_MOVE } from "./shortcuts.ts";

const ROW = 26;
const OVERSCAN = 10;
const ICONS: Record<NodeKind, IconName> = {
  section: "section",
  rowBlock: "rowBlock",
  row: "row",
  table: "ellipse",
  booth: "rect",
  area: "polygon",
  shape: "rect",
  text: "text",
  icon: "focal",
  image: "file",
};

/** Where a drop on `row` lands: into a section by its middle half, else
 * before or after by the row's half. */
function zone(e: DragEvent, row: LayerRow): Drop {
  const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
  const y = (e.clientY - r.top) / r.height;
  if (row.kind === "section" && y > 0.25 && y < 0.75) return { kind: "into", id: row.id };
  return { kind: y < 0.5 ? "before" : "after", id: row.id };
}

export const rowName = (r: LayerRow): string =>
  r.name ?? msg("layers.unnamed", { kind: msg(`kind.${r.kind}`), id: String(r.id) });

export function LayersPanel(props: { editor: Editor }) {
  const editor = props.editor;
  const page = () => editor.views.layers;
  const [scrollTop, setScrollTop] = createSignal(0);
  const [height, setHeight] = createSignal(0);
  const [cursor, setCursor] = createSignal(0);
  const [focused, setFocused] = createSignal(false);
  const [drop, setDrop] = createSignal<Drop | null>(null);
  const [menuRow, setMenuRow] = createSignal<LayerRow | null>(null);
  let scroller = document.createElement("div");
  let tree = document.createElement("div");
  let dragged: number[] = [];

  onMount(() => {
    const ro = new ResizeObserver(([e]) => setHeight(e?.contentRect.height ?? 0));
    ro.observe(scroller);
    onCleanup(() => ro.disconnect());
  });
  createEffect(() => {
    const w = windowFor(scrollTop(), height(), ROW, page().total, OVERSCAN);
    // Before the first measure, a page's worth: the total comes with it.
    editor.loop.setLayersWindow(w.start, height() > 0 ? w.count : 50);
  });

  const rowAt = (index: number) => page().rows.find((r) => r.index === index);
  const current = () => rowAt(cursor());
  const show = (index: number) => {
    const top = scrollToShow(index, scroller.scrollTop, scroller.clientHeight, ROW);
    if (top !== scroller.scrollTop) scroller.scrollTop = top;
  };
  const moveCursor = (index: number) => {
    const i = Math.max(0, Math.min(page().total - 1, index));
    setCursor(i);
    show(i);
  };
  createEffect(on(editor.revealed, (r) => r !== null && moveCursor(r.row), { defer: true }));
  createEffect(
    on(
      () => page().total,
      (total) => cursor() >= total && setCursor(Math.max(0, total - 1)),
      { defer: true },
    ),
  );

  /** What a drag or a move takes: the selected rows shown, if `row` is one. */
  const subjects = (row: LayerRow) =>
    row.selected
      ? page()
          .rows.filter((r) => r.selected)
          .map((r) => r.id)
      : [row.id];
  const move = (ids: number[], to: Drop) => {
    if (editor.run((s) => s.moveLayers(ids, to)) !== undefined) editor.reveal(ids[0]);
  };
  /** One row up or down, into and out of sections on the way (Alt+arrow). */
  const step = (row: LayerRow, dir: -1 | 1) => {
    const ids = subjects(row);
    if (dir < 0) {
      let prev = rowAt(row.index - 1);
      if (row.kind === "section")
        while (prev !== undefined && prev.depth > 0) prev = rowAt(prev.index - 1);
      if (prev === undefined) return;
      return move(
        ids,
        prev.depth > row.depth ? { kind: "after", id: prev.id } : { kind: "before", id: prev.id },
      );
    }
    const next = rowAt(row.index + 1 + (row.expanded === true ? row.children : 0));
    if (next === undefined) return;
    if (row.kind !== "section" && next.depth < row.depth)
      return move(ids, { kind: "before", id: next.id });
    if (row.kind !== "section" && next.kind === "section" && next.expanded === true) {
      const child = rowAt(next.index + 1);
      return move(
        ids,
        child !== undefined && child.depth > 0
          ? { kind: "before", id: child.id }
          : { kind: "into", id: next.id },
      );
    }
    move(ids, { kind: "after", id: next.id });
  };
  const click = (row: LayerRow, e: { ctrlKey: boolean; metaKey: boolean; shiftKey: boolean }) => {
    const how = e.shiftKey ? "range" : e.ctrlKey || e.metaKey ? "toggle" : "replace";
    editor.run((s) => s.clickLayer(row.id, how));
    setCursor(row.index);
  };
  const expand = (row: LayerRow, open: boolean) => editor.run((s) => s.expandLayer(row.id, open));

  const key = (e: KeyboardEvent) => {
    // Navigate on the tree as it is, not as the last frame read it.
    editor.loop.flush();
    const row = current();
    const screen = Math.max(1, Math.floor(scroller.clientHeight / ROW) - 1);
    const go: Record<string, () => void> = {
      ArrowDown: () => (e.altKey && row ? step(row, 1) : moveCursor(cursor() + 1)),
      ArrowUp: () => (e.altKey && row ? step(row, -1) : moveCursor(cursor() - 1)),
      Home: () => moveCursor(0),
      End: () => moveCursor(Infinity),
      PageDown: () => moveCursor(cursor() + screen),
      PageUp: () => moveCursor(cursor() - screen),
      ArrowRight: () => {
        if (row?.expanded === false) expand(row, true);
        else if (row?.expanded === true && row.children > 0) moveCursor(cursor() + 1);
      },
      ArrowLeft: () => {
        if (row?.expanded === true) return void expand(row, false);
        if (row !== undefined && row.depth > 0) {
          let i = row.index - 1;
          while (i > 0 && (rowAt(i)?.depth ?? 0) > 0) i--;
          moveCursor(i);
        }
      },
      Enter: () => row && click(row, e),
      " ": () => row && click(row, e),
      Delete: () => editor.run((s) => s.delete()),
      Backspace: () => editor.run((s) => s.delete()),
      Escape: () => editor.run((s) => s.stepOut()),
    };
    const action = go[e.key];
    if (action === undefined || e.ctrlKey || e.metaKey) return;
    e.preventDefault();
    setFocused(true);
    action();
  };

  const order = (o: Order) => () => editor.run((s) => s.order(o));
  const sel = () => editor.views.selection;

  return (
    <ContextMenu onOpenChange={(open) => !open && setMenuRow(null)}>
      <ContextMenu.Trigger class="flex min-h-0 flex-1 flex-col">
        <div
          ref={(e) => (scroller = e)}
          class="relative min-h-0 flex-1 overflow-y-auto"
          onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
        >
          <Show when={page().total === 0}>
            <p class="p-3 text-sm text-faint">{msg("layers.empty")}</p>
          </Show>
          <div
            ref={(e) => (tree = e)}
            role="tree"
            tabIndex={0}
            aria-label={msg("layers.label")}
            aria-multiselectable="true"
            aria-activedescendant={current() ? `layer-${current()?.id}` : undefined}
            class="relative outline-none"
            style={{ height: `${page().total * ROW}px` }}
            onKeyDown={key}
            onFocus={() => setFocused(tree.matches(":focus-visible"))}
            onBlur={() => setFocused(false)}
          >
            <For each={page().rows}>
              {(row) => (
                <div
                  id={`layer-${row.id}`}
                  role="treeitem"
                  aria-level={row.depth + 1}
                  aria-selected={row.selected}
                  aria-expanded={row.expanded ?? undefined}
                  draggable="true"
                  class="group absolute inset-x-0 flex items-center gap-1 pr-1 text-base select-none"
                  classList={{
                    "bg-accent-soft": row.selected,
                    "hover:bg-hover": !row.selected,
                    "text-faint": row.hidden,
                    "text-fg": !row.hidden,
                    "outline-2 -outline-offset-2 outline-focus":
                      focused() && cursor() === row.index,
                    "bg-accent-soft ring-1 ring-inset ring-accent-fill":
                      drop()?.kind === "into" && drop()?.id === row.id,
                  }}
                  style={{
                    top: `${row.index * ROW}px`,
                    height: `${ROW}px`,
                    "padding-left": `${6 + row.depth * 16}px`,
                  }}
                  onClick={(e) => {
                    click(row, e);
                    tree.focus();
                  }}
                  onContextMenu={() => {
                    if (!row.selected)
                      click(row, { ctrlKey: false, metaKey: false, shiftKey: false });
                    setMenuRow(row);
                  }}
                  onDragStart={(e) => {
                    dragged = subjects(row);
                    e.dataTransfer?.setData("text/plain", dragged.join(","));
                    if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
                  }}
                  onDragOver={(e) => {
                    if (dragged.length === 0) return;
                    e.preventDefault();
                    setDrop(zone(e, row));
                  }}
                  onDragLeave={() => setDrop(null)}
                  onDragEnd={() => {
                    dragged = [];
                    setDrop(null);
                  }}
                  onDrop={(e) => {
                    e.preventDefault();
                    const to = zone(e, row);
                    const ids = dragged;
                    dragged = [];
                    setDrop(null);
                    if (ids.length > 0 && !ids.includes(row.id)) move(ids, to);
                  }}
                >
                  <Show when={drop()?.id === row.id && drop()?.kind !== "into"}>
                    <div
                      class="pointer-events-none absolute right-1 h-0.5 rounded-full bg-accent-fill"
                      style={{
                        left: `${6 + row.depth * 16}px`,
                        [drop()?.kind === "before" ? "top" : "bottom"]: "-1px",
                      }}
                    />
                  </Show>
                  <span class="flex w-4 justify-center">
                    <Show when={row.expanded !== null}>
                      <button
                        type="button"
                        tabIndex={-1}
                        aria-hidden="true"
                        class="text-faint hover:text-fg"
                        onClick={(e) => {
                          e.stopPropagation();
                          expand(row, row.expanded !== true);
                        }}
                      >
                        <Icon
                          name={row.expanded === true ? "chevronDown" : "chevronRight"}
                          class="size-3.5"
                        />
                      </button>
                    </Show>
                  </span>
                  <Icon name={ICONS[row.kind]} class="size-3.5 shrink-0 text-faint" />
                  <span
                    class="min-w-0 flex-1 truncate"
                    classList={{ "italic text-muted": row.name === null }}
                  >
                    {rowName(row)}
                  </span>
                  <Show when={row.expanded !== null}>
                    <span class="text-xs text-faint tabular-nums group-hover:hidden">
                      {row.children}
                    </span>
                  </Show>
                  <RowToggle
                    on={row.locked}
                    icon={row.locked ? "lock" : "unlock"}
                    label={row.locked ? msg("layers.unlock") : msg("layers.lock")}
                    onClick={() => editor.run((s) => s.lockLayer(row.id, !row.locked))}
                  />
                  <RowToggle
                    on={row.hidden}
                    icon={row.hidden ? "eyeOff" : "eye"}
                    label={row.hidden ? msg("layers.show") : msg("layers.hide")}
                    onClick={() => editor.run((s) => s.hideLayer(row.id, !row.hidden))}
                  />
                </div>
              )}
            </For>
          </div>
        </div>
      </ContextMenu.Trigger>
      <ContextMenu.Portal>
        <ContextMenu.Content class={menuContent}>
          <ContextMenu.Item class={menuItem} onSelect={order("forward")}>
            {msg("layers.forward")}
          </ContextMenu.Item>
          <ContextMenu.Item class={menuItem} onSelect={order("backward")}>
            {msg("layers.backward")}
          </ContextMenu.Item>
          <ContextMenu.Item class={menuItem} onSelect={order("front")}>
            {msg("layers.front")}
          </ContextMenu.Item>
          <ContextMenu.Item class={menuItem} onSelect={order("back")}>
            {msg("layers.back")}
          </ContextMenu.Item>
          <ContextMenu.Separator class={menuSeparator} />
          <ContextMenu.Item
            class={menuItem}
            disabled={menuRow() === null}
            onSelect={() => menuRow() && step(menuRow() as LayerRow, -1)}
          >
            {msg("layers.moveUp")}
            <span class={menuKeys}>{display(LAYER_MOVE.up, isMac())}</span>
          </ContextMenu.Item>
          <ContextMenu.Item
            class={menuItem}
            disabled={menuRow() === null}
            onSelect={() => menuRow() && step(menuRow() as LayerRow, 1)}
          >
            {msg("layers.moveDown")}
            <span class={menuKeys}>{display(LAYER_MOVE.down, isMac())}</span>
          </ContextMenu.Item>
          <ContextMenu.Separator class={menuSeparator} />
          <ContextMenu.Item
            class={menuItem}
            onSelect={() => editor.run((s) => s.setLocked(sel().locked < sel().count))}
          >
            {sel().locked === sel().count && sel().count > 0
              ? msg("layers.unlock")
              : msg("layers.lock")}
            <span class={menuKeys}>{keyOf("lock")}</span>
          </ContextMenu.Item>
          <ContextMenu.Item
            class={menuItem}
            onSelect={() => editor.run((s) => s.setHidden(sel().hidden < sel().count))}
          >
            {sel().hidden === sel().count && sel().count > 0
              ? msg("layers.show")
              : msg("layers.hide")}
            <span class={menuKeys}>{keyOf("hide")}</span>
          </ContextMenu.Item>
          <ContextMenu.Item
            class={menuItem}
            onSelect={() => editor.reveal(editor.run((s) => s.duplicate([50, 50]))?.created[0])}
          >
            {msg("layers.duplicate")}
            <span class={menuKeys}>{keyOf("duplicate")}</span>
          </ContextMenu.Item>
          <ContextMenu.Item
            class={menuItem}
            disabled={sel().level.kind !== "top" || menuRow()?.kind === "section"}
            onSelect={() => {
              const ids = page()
                .rows.filter((r) => r.selected && r.depth === 0)
                .map((r) => r.id);
              const id = editor.run((s) => s.group(ids, null));
              if (id !== undefined) editor.reveal(id);
            }}
          >
            {msg("layers.group")}
          </ContextMenu.Item>
          <Show when={menuRow()?.kind === "section"}>
            <ContextMenu.Item
              class={menuItem}
              onSelect={() => editor.run((s) => s.convertOutline(menuRow()?.id ?? -1))}
            >
              {msg("inspector.convertOutline")}
            </ContextMenu.Item>
            <ContextMenu.Item
              class={menuItem}
              onSelect={() => editor.run((s) => s.dissolve(menuRow()?.id ?? -1))}
            >
              {msg("layers.dissolve")}
            </ContextMenu.Item>
          </Show>
          <ContextMenu.Separator class={menuSeparator} />
          <ContextMenu.Item
            class={`${menuItem} text-danger`}
            onSelect={() => editor.run((s) => s.delete())}
          >
            {msg("layers.delete")}
            <span class={menuKeys}>{keyOf("delete")}</span>
          </ContextMenu.Item>
        </ContextMenu.Content>
      </ContextMenu.Portal>
    </ContextMenu>
  );
}

function RowToggle(props: { on: boolean; icon: IconName; label: string; onClick: () => void }) {
  return (
    <button
      type="button"
      tabIndex={-1}
      aria-label={props.label}
      title={props.label}
      class="flex size-5 items-center justify-center rounded-sm text-faint hover:bg-active hover:text-fg"
      classList={{ "invisible group-hover:visible": !props.on }}
      onClick={(e) => {
        e.stopPropagation();
        props.onClick();
      }}
    >
      <Icon name={props.icon} class="size-3.5" />
    </button>
  );
}
