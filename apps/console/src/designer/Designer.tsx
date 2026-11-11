// The designer page (07 §3): loads the module, opens the source's chart (or
// a file, or an empty chart), and lays out the shell around it.
import { loadDesigner, type Session } from "@cavea/designer-core";
import { Tabs } from "@kobalte/core/tabs";
import {
  createResource,
  createSignal,
  Match,
  onCleanup,
  onMount,
  Show,
  Switch,
  type JSX,
} from "solid-js";
import { msg } from "../i18n/messages.ts";
import { load, save } from "../storage.ts";
import { IconButton } from "../ui/controls.tsx";
import { Confirm, notify } from "../ui/overlays.tsx";
import { Splitter } from "../ui/Splitter.tsx";
import { CanvasHost } from "./CanvasHost.tsx";
import { CategoriesPanel } from "./CategoriesPanel.tsx";
import { createEditor, type Editor } from "./editor.ts";
import { failureMessage, failureOf } from "./errors.ts";
import { commands } from "./commands.ts";
import { Help } from "./Help.tsx";
import { InsertForm } from "./InsertForm.tsx";
import { Inspector } from "./Inspector.tsx";
import { actions, bindKeys } from "./keys.ts";
import { LayersPanel } from "./LayersPanel.tsx";
import { NewChart, NewChartDialog, type Choices } from "./NewChart.tsx";
import { NumberingPanel } from "./NumberingPanel.tsx";
import { Palette } from "./Palette.tsx";
import { LAYOUT_KEY, parseLayout, SIDE, SPLIT, type Layout, type PanelTab } from "./layout.ts";
import { StatusBar, ToolRail } from "./Rails.tsx";
import { emptyChart, fileName, type ChartSource } from "./source.ts";
import { TopBar } from "./TopBar.tsx";
import { ValidationPanel } from "./ValidationPanel.tsx";

/** 09 §8: the designer needs 1,280 px. */
const MIN_WIDTH = 1280;

export function DesignerPage(props: { source: ChartSource }) {
  const loading = loadDesigner();
  const [designer] = createResource(() => loading);
  const [editor, setEditor] = createSignal<Editor | null>(null);
  const [panic, setPanic] = createSignal<string | null>(null);
  const [failed, setFailed] = createSignal<{ file: string; message: string } | null>(null);
  /** A chart opened from a file, waiting for the user to discard the open one. */
  const [pending, setPending] = createSignal<Session | null>(null);
  const [width, setWidth] = createSignal(window.innerWidth);
  const [dragging, setDragging] = createSignal(false);
  const [newOpen, setNewOpen] = createSignal(false);
  let picker: HTMLInputElement | undefined;

  const resize = () => setWidth(window.innerWidth);
  window.addEventListener("resize", resize);
  /** The page is gone: an open still in flight must not install a session. */
  let gone = false;
  onCleanup(() => {
    gone = true;
    window.removeEventListener("resize", resize);
    editor()?.dispose();
    pending()?.dispose();
  });

  const dirty = () => {
    const e = editor();
    return e !== null && e.views.status.revision !== e.saved().revision;
  };
  const install = (session: Session) => {
    editor()?.dispose();
    const e = createEditor(session, setPanic);
    // For measuring and debugging in the browser's console; not in builds.
    if (import.meta.env.DEV) Object.assign(window, { caveaEditor: e });
    setEditor(e);
  };
  /** Opens chart document JSON (07 §12): a refusal is explained and the
   * open chart kept; a chart that opens replaces the open one, after asking
   * if that has unsaved changes. A file dropped while the module loads
   * waits for it. */
  const request = async (text: string, file: string) => {
    const d = await loading.catch(() => undefined);
    if (d === undefined || gone || panic() !== null) return;
    let session: Session;
    try {
      session = d.open(text);
    } catch (error) {
      const f = failureOf(error);
      if (f?.kind === "panic") return setPanic(f.message);
      if (f === null) throw error;
      return setFailed({ file, message: failureMessage(f) });
    }
    if (gone) return session.dispose();
    if (!dirty()) return install(session);
    pending()?.dispose();
    setPending(session);
  };
  const readFile = async (file: File) => {
    if (!file.name.toLowerCase().endsWith(".json") && file.type !== "application/json") {
      return setFailed({ file: file.name, message: msg("import.notJson") });
    }
    let text: string;
    try {
      text = await file.text();
    } catch {
      return setFailed({ file: file.name, message: msg("import.unreadable") });
    }
    await request(text, file.name);
  };
  const saveNow = () => {
    const e = editor();
    if (e === null) return;
    const draft = e.run((s) => s.export());
    if (draft === undefined) return;
    const name = e.views.title;
    const revision = e.views.status.revision;
    void props.source.save(draft, name).then(() => {
      e.markSaved(revision);
      notify(msg("export.done", { file: fileName(name) }));
    });
  };

  onMount(() => {
    void props.source.open().then((text) => {
      if (text !== null) void request(text, msg("designer.untitled"));
    });
  });

  // Drop a file anywhere on the window to open it.
  const onDragOver = (e: DragEvent) => {
    if (e.dataTransfer?.types.includes("Files") !== true) return;
    e.preventDefault();
    setDragging(true);
  };
  const onDrop = (e: DragEvent) => {
    const file = e.dataTransfer?.files[0];
    setDragging(false);
    if (file === undefined) return;
    e.preventDefault();
    void readFile(file);
  };

  const choices: Choices = {
    empty: () => void request(emptyChart(msg("designer.untitled")), ""),
    template: (t) =>
      void t.load().then(
        (text) => request(text, msg(`template.${t.id}`)),
        () => setFailed({ file: msg(`template.${t.id}`), message: msg("new.loadFailed") }),
      ),
    open: () => picker?.click(),
  };
  const ui = {
    onNew: () => setNewOpen(true),
    onOpen: () => picker?.click(),
    onSave: saveNow,
  };

  return (
    <div
      class="relative h-full"
      onDragOver={onDragOver}
      onDragLeave={(e) => e.relatedTarget === null && setDragging(false)}
      onDrop={onDrop}
    >
      <input
        ref={(e) => (picker = e)}
        type="file"
        accept=".json,application/json"
        class="hidden"
        onChange={(e) => {
          const file = e.currentTarget.files?.[0];
          e.currentTarget.value = "";
          if (file !== undefined) void readFile(file);
        }}
      />
      <Switch>
        <Match when={width() < MIN_WIDTH}>
          <Centre
            title={msg("designer.narrow", { width: MIN_WIDTH })}
            body={msg("designer.narrowBody")}
          />
        </Match>
        <Match when={designer.error !== undefined}>
          <Centre title={msg("designer.loadFailed", { message: String(designer.error) })} />
        </Match>
        <Match when={designer.loading}>
          <Centre title={msg("designer.loading")} />
        </Match>
        <Match when={editor()} keyed>
          {(e) => <Shell editor={e} save={saveNow} ui={ui} />}
        </Match>
        <Match when={true}>
          <Centre title={msg("new.title")} body={msg("new.body")}>
            <div class="mt-6 w-full max-w-2xl">
              <NewChart choices={choices} />
            </div>
          </Centre>
        </Match>
      </Switch>

      <Show when={dragging()}>
        <div class="pointer-events-none absolute inset-2 z-40 flex items-center justify-center rounded-lg border-2 border-dashed border-accent-fill bg-scrim">
          <p class="rounded-md bg-raised px-4 py-2 text-lg text-fg shadow-menu">
            {msg("designer.dropHint")}
          </p>
        </div>
      </Show>

      <NewChartDialog open={newOpen()} choices={choices} onClose={() => setNewOpen(false)} />
      <Confirm
        open={failed() !== null}
        title={msg("import.failed", { file: failed()?.file || msg("designer.empty") })}
        description={failed()?.message ?? ""}
        onClose={() => setFailed(null)}
      />
      <Confirm
        open={pending() !== null}
        title={msg("confirm.replaceTitle")}
        description={msg("confirm.replaceBody", { name: editor()?.views.title ?? "" })}
        confirm={msg("action.discard")}
        variant="danger"
        onConfirm={() => {
          const session = pending();
          setPending(null);
          if (session !== null) install(session);
        }}
        onClose={() => {
          pending()?.dispose();
          setPending(null);
        }}
      />
      <Confirm
        open={panic() !== null}
        blocking
        title={msg("panic.title")}
        description={msg("panic.body")}
        confirm={msg("action.reload")}
        onConfirm={() => location.reload()}
        onClose={() => {}}
      >
        <p class="mt-3 font-mono text-xs break-all text-faint">
          {msg("panic.detail", { message: panic() ?? "" })}
        </p>
      </Confirm>
    </div>
  );
}

function Centre(props: { title: string; body?: string; children?: JSX.Element }) {
  return (
    <div class="flex h-full flex-col items-center justify-center bg-canvas p-6 text-center">
      <h1 class="text-lg font-semibold text-fg">{props.title}</h1>
      <Show when={props.body}>
        <p class="mt-1 max-w-md text-base text-muted">{props.body}</p>
      </Show>
      {props.children}
    </div>
  );
}

function Shell(props: {
  editor: Editor;
  save: () => void;
  ui: { onNew: () => void; onOpen: () => void; onSave: () => void };
}) {
  const editor = props.editor;
  const [layout, setLayoutSignal] = createSignal<Layout>(
    load(LAYOUT_KEY, parseLayout, parseLayout(null)),
  );
  const setLayout = (change: Partial<Layout>) => {
    const next = { ...layout(), ...change };
    setLayoutSignal(next);
    save(LAYOUT_KEY, next);
  };
  const [palette, setPalette] = createSignal(false);
  const [help, setHelp] = createSignal(false);
  const [insert, setInsert] = createSignal(false);
  const [numbering, setNumbering] = createSignal(false);
  const openNumbering = () => {
    setLayout({ inspectorOpen: true });
    setNumbering(true);
  };
  const act = actions(editor, {
    save: props.save,
    palette: () => setPalette(true),
    help: () => setHelp(true),
  });
  onCleanup(bindKeys(editor, act));
  const all = commands(editor, act, {
    help: () => setHelp(true),
    insert: () => setInsert(true),
    numbering: openNumbering,
    tab: (tab) => setLayout({ tab, panelsOpen: true }),
    inspector: () => setLayout({ inspectorOpen: !layout().inspectorOpen }),
    newChart: props.ui.onNew,
    open: props.ui.onOpen,
  });
  let column: HTMLElement | undefined;

  const v = () => editor.views.validation;

  return (
    <div class="flex h-full flex-col">
      <TopBar
        editor={editor}
        {...props.ui}
        onInsert={() => setInsert(true)}
        onPalette={() => setPalette(true)}
        onHelp={() => setHelp(true)}
      />
      <div class="flex min-h-0 flex-1">
        <ToolRail editor={editor} onInsert={() => setInsert(true)} />
        <CanvasHost editor={editor} />
        <Splitter
          orientation="vertical"
          label={msg("panel.resize", { panel: msg("panel.panels") })}
          value={layout().side}
          min={SIDE.min}
          max={SIDE.max}
          step={10}
          fromDrag={(start, delta) => start - delta}
          onChange={(side) => setLayout({ side })}
        />
        <aside
          ref={(e) => (column = e)}
          class="flex shrink-0 flex-col border-l border-line bg-surface"
          style={{ width: `${layout().side}px` }}
        >
          <section
            aria-label={msg("panel.inspector")}
            class="flex min-h-0 flex-col"
            style={{
              flex: !layout().inspectorOpen
                ? "0 0 auto"
                : layout().panelsOpen
                  ? `0 0 ${layout().split * 100}%`
                  : "1 1 0",
            }}
          >
            <PanelHeader
              title={msg(numbering() ? "panel.numbering" : "panel.inspector")}
              open={layout().inspectorOpen}
              onToggle={() => setLayout({ inspectorOpen: !layout().inspectorOpen })}
              onBack={numbering() ? () => setNumbering(false) : undefined}
            />
            <Show when={layout().inspectorOpen}>
              <Show
                when={numbering()}
                fallback={<Inspector editor={editor} onNumbering={openNumbering} />}
              >
                <NumberingPanel editor={editor} onClose={() => setNumbering(false)} />
              </Show>
            </Show>
          </section>
          <Show when={layout().inspectorOpen && layout().panelsOpen}>
            <Splitter
              orientation="horizontal"
              label={msg("panel.resize", { panel: msg("panel.inspector") })}
              value={layout().split}
              min={SPLIT.min}
              max={SPLIT.max}
              step={0.02}
              fromDrag={(start, delta) => start + delta / (column?.clientHeight ?? 1)}
              onChange={(split) => setLayout({ split })}
            />
          </Show>
          <Tabs
            value={layout().tab}
            onChange={(tab) => setLayout({ tab: tab as PanelTab, panelsOpen: true })}
            class="flex min-h-0 flex-col border-t border-line"
            style={{ flex: layout().panelsOpen ? "1 1 0" : "0 0 auto" }}
          >
            <div class="flex h-8 shrink-0 items-center pr-1">
              <Tabs.List
                class="relative flex h-full flex-1 items-stretch gap-1 px-2"
                aria-label={msg("panel.panels")}
              >
                {(["layers", "categories", "validation"] as const).map((tab) => (
                  <Tabs.Trigger
                    value={tab}
                    class="relative inline-flex items-center gap-1.5 px-1.5 text-sm font-medium text-muted hover:text-fg data-selected:text-fg"
                  >
                    {msg(`panel.${tab}`)}
                    <Show when={tab === "validation" && v().errors + v().warnings > 0}>
                      <span
                        class="rounded-full px-1.5 text-2xs leading-4 tabular-nums"
                        classList={{
                          "bg-danger-soft text-danger": v().errors > 0,
                          "bg-warning-soft text-warning": v().errors === 0,
                        }}
                      >
                        {v().errors + v().warnings}
                      </span>
                    </Show>
                  </Tabs.Trigger>
                ))}
                <Tabs.Indicator class="absolute bottom-0 h-0.5 bg-accent-fill transition-all duration-150" />
              </Tabs.List>
              <IconButton
                icon={layout().panelsOpen ? "chevronDown" : "chevronRight"}
                label={msg(layout().panelsOpen ? "panel.collapse" : "panel.expand", {
                  panel: msg("panel.panels"),
                })}
                class="size-6"
                onClick={() => setLayout({ panelsOpen: !layout().panelsOpen })}
              />
            </div>
            <Show when={layout().panelsOpen}>
              <Tabs.Content value="layers" class="flex min-h-0 flex-1 flex-col outline-none">
                <LayersPanel editor={editor} />
              </Tabs.Content>
              <Tabs.Content value="categories" class="flex min-h-0 flex-1 flex-col outline-none">
                <CategoriesPanel editor={editor} />
              </Tabs.Content>
              <Tabs.Content value="validation" class="flex min-h-0 flex-1 flex-col outline-none">
                <ValidationPanel editor={editor} />
              </Tabs.Content>
            </Show>
          </Tabs>
        </aside>
      </div>
      <StatusBar
        editor={editor}
        act={act}
        onProblems={() => setLayout({ tab: "validation", panelsOpen: true })}
      />
      <Palette open={palette()} commands={() => all} onClose={() => setPalette(false)} />
      <Help open={help()} onClose={() => setHelp(false)} />
      <InsertForm editor={editor} open={insert()} onClose={() => setInsert(false)} />
    </div>
  );
}

function PanelHeader(props: {
  title: string;
  open: boolean;
  onToggle: () => void;
  /** A panel shown in this one's place: the way back. */
  onBack?: (() => void) | undefined;
}) {
  return (
    <div class="flex h-8 shrink-0 items-center justify-between border-b border-line pr-1 pl-3">
      <h2 class="flex items-center gap-1 text-sm font-medium text-fg">
        <Show when={props.onBack}>
          {(back) => (
            <IconButton
              icon="chevronLeft"
              label={msg("panel.back")}
              class="-ml-2 size-6"
              onClick={() => back()()}
            />
          )}
        </Show>
        {props.title}
      </h2>
      <IconButton
        icon={props.open ? "chevronDown" : "chevronRight"}
        label={msg(props.open ? "panel.collapse" : "panel.expand", { panel: props.title })}
        class="size-6"
        onClick={() => props.onToggle()}
      />
    </div>
  );
}
