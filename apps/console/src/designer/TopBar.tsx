// The top bar (07 §3): menu, chart name, undo and redo, save state, floor
// switcher, Preview and Publish.
import { DropdownMenu } from "@kobalte/core/dropdown-menu";
import { Tooltip } from "@kobalte/core/tooltip";
import { useNavigate } from "@solidjs/router";
import { For, Show } from "solid-js";
import { formatTime, historyName, msg } from "../i18n/messages.ts";
import { setThemeChoice, themeChoice, type ThemeChoice } from "../theme.ts";
import { buttonClass, IconButton, Tip } from "../ui/controls.tsx";
import { Icon } from "../ui/Icon.tsx";
import { afterMenu, menuContent, menuItem, menuKeys, menuSeparator } from "../ui/overlays.tsx";
import type { Editor } from "./editor.ts";
import { keyOf } from "./shortcuts.ts";

export function TopBar(props: {
  editor: Editor;
  onNew: () => void;
  onOpen: () => void;
  onSave: () => void;
  onPalette: () => void;
  onHelp: () => void;
}) {
  const views = props.editor.views;
  const navigate = useNavigate();
  const dirty = () => views.status.revision !== props.editor.saved().revision;
  const saveState = () => {
    if (dirty()) return msg("topbar.unsaved");
    const at = props.editor.saved().at;
    return at === null ? "" : msg("topbar.saved", { time: formatTime(at) });
  };
  const publishReason = () => {
    const v = views.validation;
    if (!v.complete) return msg("topbar.publishChecking");
    return v.publishable ? msg("topbar.publishPending") : msg("topbar.publishBlocked");
  };
  const shown = () => views.floors.floors.find((f) => f.id === views.floors.shown);

  return (
    <header class="flex h-10 shrink-0 items-center gap-1 border-b border-line bg-app px-1.5">
      <DropdownMenu gutter={4}>
        <DropdownMenu.Trigger class={buttonClass("ghost", "px-1.5")} aria-label={msg("menu.label")}>
          <Icon name="menu" />
        </DropdownMenu.Trigger>
        <DropdownMenu.Portal>
          <DropdownMenu.Content class={menuContent}>
            <DropdownMenu.Item class={menuItem} onSelect={afterMenu(props.onNew)}>
              <Icon name="plus" />
              {msg("menu.new")}
            </DropdownMenu.Item>
            <DropdownMenu.Item class={menuItem} onSelect={afterMenu(props.onOpen)}>
              <Icon name="file" />
              {msg("menu.open")}
            </DropdownMenu.Item>
            <DropdownMenu.Item class={menuItem} onSelect={props.onSave}>
              <Icon name="download" />
              {msg("menu.export")}
              <span class={menuKeys}>{keyOf("save")}</span>
            </DropdownMenu.Item>
            <DropdownMenu.Separator class={menuSeparator} />
            <DropdownMenu.Item class={menuItem} onSelect={afterMenu(props.onPalette)}>
              <Icon name="search" />
              {msg("shortcut.palette")}
              <span class={menuKeys}>{keyOf("palette")}</span>
            </DropdownMenu.Item>
            <DropdownMenu.Item class={menuItem} onSelect={afterMenu(props.onHelp)}>
              <Icon name="keyboard" />
              {msg("shortcut.help")}
              <span class={menuKeys}>{keyOf("help")}</span>
            </DropdownMenu.Item>
            <DropdownMenu.Separator class={menuSeparator} />
            <DropdownMenu.Sub overlap gutter={4}>
              <DropdownMenu.SubTrigger class={menuItem}>
                <Icon name="sun" />
                {msg("menu.theme")}
                <Icon name="chevronRight" class="ml-auto text-faint" />
              </DropdownMenu.SubTrigger>
              <DropdownMenu.Portal>
                <DropdownMenu.SubContent class={menuContent}>
                  <DropdownMenu.RadioGroup
                    value={themeChoice()}
                    onChange={(v) => setThemeChoice(v as ThemeChoice)}
                  >
                    <For each={["system", "light", "dark"] as const}>
                      {(c) => (
                        <DropdownMenu.RadioItem value={c} class={menuItem}>
                          <span class="w-4">
                            <DropdownMenu.ItemIndicator>
                              <Icon name="check" />
                            </DropdownMenu.ItemIndicator>
                          </span>
                          {msg(`theme.${c}`)}
                        </DropdownMenu.RadioItem>
                      )}
                    </For>
                  </DropdownMenu.RadioGroup>
                </DropdownMenu.SubContent>
              </DropdownMenu.Portal>
            </DropdownMenu.Sub>
            <DropdownMenu.Separator class={menuSeparator} />
            <DropdownMenu.Item class={menuItem} onSelect={() => navigate("/charts")}>
              <Icon name="chart" />
              {msg("menu.console")}
            </DropdownMenu.Item>
          </DropdownMenu.Content>
        </DropdownMenu.Portal>
      </DropdownMenu>

      <input
        aria-label={msg("topbar.chartName")}
        class="h-7 w-64 min-w-0 truncate rounded-md border border-transparent bg-transparent px-1.5 text-base font-semibold text-fg hover:border-line-strong focus:border-control-line focus:bg-control"
        value={views.title}
        onKeyDown={(e) => {
          if (e.key === "Enter") e.currentTarget.blur();
          if (e.key === "Escape") {
            e.currentTarget.value = views.title;
            e.currentTarget.blur();
          }
        }}
        onBlur={(e) => {
          const value = e.currentTarget.value.trim();
          if (value !== views.title && value !== "") {
            props.editor.run((s) => s.setField({ field: "chartName", value }));
          }
          e.currentTarget.value = views.title;
        }}
      />

      <div class="ml-1 flex items-center">
        <IconButton
          icon="undo"
          label={
            views.history.undo === null
              ? msg("topbar.nothingToUndo")
              : msg("topbar.undoWhat", { what: historyName(views.history.undo) })
          }
          keys={keyOf("undo")}
          reason={views.history.undo === null || views.history.gesture ? "" : null}
          onClick={() => props.editor.run((s) => s.undo())}
        />
        <IconButton
          icon="redo"
          label={
            views.history.redo === null
              ? msg("topbar.nothingToRedo")
              : msg("topbar.redoWhat", { what: historyName(views.history.redo) })
          }
          keys={keyOf("redo")}
          reason={views.history.redo === null || views.history.gesture ? "" : null}
          onClick={() => props.editor.run((s) => s.redo())}
        />
      </div>
      <span class="ml-2 min-w-28 text-sm text-faint" aria-live="polite">
        {saveState()}
      </span>

      <div class="ml-auto flex items-center gap-2">
        <Show
          when={views.floors.floors.length > 0}
          fallback={<span class="px-2 text-sm text-faint">{msg("topbar.singleFloor")}</span>}
        >
          <DropdownMenu gutter={4}>
            <DropdownMenu.Trigger class={buttonClass("ghost")}>
              <span class="text-muted">{msg("topbar.floor")}</span>
              {shown()?.name}
              <Icon name="chevronDown" class="text-faint" />
            </DropdownMenu.Trigger>
            <DropdownMenu.Portal>
              <DropdownMenu.Content class={menuContent}>
                <DropdownMenu.RadioGroup
                  value={String(views.floors.shown)}
                  onChange={(v) => props.editor.run((s) => s.setFloor(Number(v)))}
                >
                  <For each={views.floors.floors}>
                    {(f) => (
                      <DropdownMenu.RadioItem value={String(f.id)} class={menuItem}>
                        <span class="w-4">
                          <DropdownMenu.ItemIndicator>
                            <Icon name="check" />
                          </DropdownMenu.ItemIndicator>
                        </span>
                        {f.name}
                        <span class={menuKeys}>{msg("status.nodes", { n: f.nodes })}</span>
                      </DropdownMenu.RadioItem>
                    )}
                  </For>
                </DropdownMenu.RadioGroup>
              </DropdownMenu.Content>
            </DropdownMenu.Portal>
          </DropdownMenu>
        </Show>
        <div class="mx-1 h-5 w-px bg-line" />
        <Pending label={msg("topbar.preview")} reason={msg("topbar.previewPending")} />
        <Pending label={msg("topbar.publish")} reason={publishReason()} primary />
      </div>
    </header>
  );
}

/** A button that cannot act yet, focusable so its tooltip says why. */
function Pending(props: { label: string; reason: string; primary?: boolean }) {
  return (
    <Tip label={props.reason}>
      <Tooltip.Trigger
        as="button"
        type="button"
        aria-disabled="true"
        class={buttonClass(props.primary === true ? "primary" : "secondary")}
      >
        {props.label}
      </Tooltip.Trigger>
    </Tip>
  );
}
