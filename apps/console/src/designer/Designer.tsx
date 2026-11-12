// The designer page (07 §3): loads the module, opens the source's chart (or
// a file, or an empty chart), or a server chart's draft (`ServerChart.tsx`),
// and lays out the shell around it.
import { loadDesigner, type Designer, type Session } from "@cavea/designer-core";
import { useNavigate } from "@solidjs/router";
import { Tabs } from "@kobalte/core/tabs";
import {
  createEffect,
  createResource,
  createRoot,
  createSignal,
  lazy,
  Match,
  on,
  onCleanup,
  onMount,
  Show,
  Suspense,
  Switch,
  type JSX,
} from "solid-js";
import { msg } from "../i18n/messages.ts";
import { load, save } from "../storage.ts";
import { Button, IconButton } from "../ui/controls.tsx";
import { Confirm, notify } from "../ui/overlays.tsx";
import { Splitter } from "../ui/Splitter.tsx";
import { CanvasHost } from "./CanvasHost.tsx";
import { CategoriesPanel } from "./CategoriesPanel.tsx";
import { createEditor, type Editor } from "./editor.ts";
import { guidesKey, parseGuides, throttledWriter } from "./guides.ts";
import { createImageHost } from "./imageHost.ts";
import type { Checked } from "./images.ts";
import { failureMessage, failureOf } from "./errors.ts";
import { refused } from "./findings.ts";
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
import { PublishDialog } from "./Publish.tsx";
import { serverChart, type ServerChart, type ServerSetup } from "./ServerChart.tsx";
import { emptyChart, fileName, localSource, type ChartSource } from "./source.ts";
import { offer, templateCommand } from "./startingPoints.ts";
import type { Template } from "./templates.ts";
import { TopBar } from "./TopBar.tsx";
import { Tour } from "./Tour.tsx";
import { TOUR_KEY } from "./tour.ts";
import { ValidationPanel } from "./ValidationPanel.tsx";

/** The numbering panel's share of the right column at least, while it is
 * open in the inspector's place: it has more fields than the inspector. */
const NUMBERING_SPLIT = 0.7;

/** 09 §8: the designer needs a window 1,100 px wide (the viewport: a 1,280 px
 * laptop's browser that is not full-screen). Laid out down to 1,024 px. */
const MIN_WIDTH = 1100;

/** The buyer's renderer, loaded when the preview first opens. */
const Preview = lazy(() => import("./Preview.tsx"));

export function DesignerPage(props: { source: ChartSource; server?: ServerSetup | undefined }) {
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
  const [previewing, setPreviewing] = createSignal<Designer | null>(null);
  const [publishing, setPublishing] = createSignal(false);
  const navigate = useNavigate();
  let picker: HTMLInputElement | undefined;
  let imagePicker: HTMLInputElement | undefined;
  /** An image picked (and checked) to start a new chart, placed once that
   * chart opens. */
  let imageForNew: Checked | null = null;
  let newFromImage = false;

  const resize = () => setWidth(window.innerWidth);
  window.addEventListener("resize", resize);
  /** The page is gone: an open still in flight must not install a session. */
  let gone = false;
  onCleanup(() => {
    gone = true;
    window.removeEventListener("resize", resize);
    // `server` is set below, before any cleanup can run.
    server?.dispose();
    editor()?.dispose();
    pending()?.dispose();
    images.dispose();
  });

  const images = createImageHost({
    designer: loading,
    server: props.server?.ref ?? null,
    editor,
  });
  /** Where a server chart's guides are kept (07 §5.2); a local chart's
   * last as long as the page. */
  const guideKey =
    props.server === undefined
      ? null
      : guidesKey(props.server.user, props.server.ref.workspace, props.server.ref.key);
  const install = (session: Session): Editor => {
    editor()?.dispose();
    const e = createEditor(session, setPanic, images);
    if (guideKey !== null) keepGuides(e, guideKey);
    // For measuring and debugging in the browser's console; not in builds.
    if (import.meta.env.DEV) Object.assign(window, { caveaEditor: e });
    setEditor(e);
    return e;
  };
  const server: ServerChart | null =
    props.server === undefined
      ? null
      : serverChart({
          setup: props.server,
          install,
          editor,
          onPanic: setPanic,
          uploads: images,
        });
  /** Whether replacing the open chart asks first: the starting points'
   * rule (`offer`), for a file opened too. */
  const dirty = () => {
    const e = editor();
    if (e === null) return false;
    const unsaved = server === null && e.views.status.revision !== e.saved().revision;
    return offer(true, { nodes: e.views.status.nodes, unsaved }) === "ask";
  };
  const replace = (session: Session) => {
    if (server === null) {
      const e = install(session);
      const file = imageForNew;
      imageForNew = null;
      if (file !== null) void images.place(e, file);
    } else {
      void server.adopt(session).then(() => {
        const file = imageForNew;
        imageForNew = null;
        const e = editor();
        if (file !== null && e !== null && e.session === session) void images.place(e, file);
      });
    }
  };
  /** Places a picked image file (07 §4): in a new chart when it was picked
   * from the new-chart screen, else in the open one. */
  const placeImage = async (file: File) => {
    if (newFromImage) {
      newFromImage = false;
      // Checked before anything is replaced: a refusal changes nothing.
      const checked = await images.check(file);
      if (checked === null) return;
      imageForNew = checked;
      await request(emptyChart(msg("designer.untitled")), "");
      return;
    }
    const e = editor();
    if (e !== null) await images.place(e, file);
  };

  /** Opens chart document JSON (07 §12): a refusal is explained and the
   * open chart kept; a chart that opens replaces the open one, after asking
   * if that has unsaved changes. A file dropped while the module loads
   * waits for it. */
  const request = async (text: string, file: string) => {
    // A read-only chart takes no file over it (dropped, or opened).
    if (server?.readOnly() === true) return notify(server.whyReadOnly(), "error");
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
    if (!dirty()) return replace(session);
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
  const exportNow = () => {
    const e = editor();
    const draft = e?.run((s) => s.export());
    if (e === null || draft === undefined) return;
    const name = e.views.title;
    void localSource
      .save(draft, name)
      .then(() => notify(msg("export.done", { file: fileName(name) })));
  };
  const saveNow = () => {
    if (server !== null) return server.saveNow();
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
    if (server !== null) {
      void loading.then(
        (d) => (gone ? undefined : server.load(d)),
        () => undefined,
      );
      return;
    }
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
    if (isImage(file) && editor() !== null) void placeImage(file);
    else void readFile(file);
  };

  const choices: Choices = {
    empty: () => void request(emptyChart(msg("designer.untitled")), ""),
    template: (t) =>
      void t.load().then(
        (text) => request(text, msg(`template.${t.id}`)),
        () => setFailed({ file: msg(`template.${t.id}`), message: msg("new.loadFailed") }),
      ),
    open: () => picker?.click(),
    image: () => {
      newFromImage = true;
      imagePicker?.click();
    },
  };
  /** Read-only by role, or while another holds the editing lock. */
  const readOnly = () => server?.readOnly() === true;

  // A server chart's starting points (07 §16): a template is one edit of
  // the open draft (saved by autosave, undone by undo), asking first when
  // the draft has something in it; the reference image is placed in it.
  /** A template waiting for the person to agree to replace the chart. */
  const [replacing, setReplacing] = createSignal<{ template: Template; text: string } | null>(null);
  /** The empty chart's panel was closed, or the person started drawing. */
  const [startDone, setStartDone] = createSignal(false);
  const applyTemplate = (template: Template, text: string) => {
    const e = editor();
    if (e === null || readOnly()) return;
    const result = e.attempt((s) => s.apply(templateCommand(s.export(), text)));
    if ("error" in result) {
      return notify(msg("start.failed", { message: result.error }), "error");
    }
    setStartDone(true);
    e.frame(e.run((s) => s.fit()) ?? null);
    notify(msg("start.applied", { template: msg(`template.${template.id}`) }));
  };
  const serverChoices: Choices = {
    empty: () => setStartDone(true),
    template: (t) =>
      void t.load().then(
        (text) => {
          const e = editor();
          if (e === null || readOnly()) return;
          const nodes = e.views.status.nodes;
          const rule = offer(true, { nodes, unsaved: false });
          if (rule === "ask") setReplacing({ template: t, text });
          else applyTemplate(t, text);
        },
        () => setFailed({ file: msg(`template.${t.id}`), message: msg("new.loadFailed") }),
      ),
    open: () => picker?.click(),
    image: () => imagePicker?.click(),
  };
  /** The empty server chart's panel: while it is empty, editable, and the
   * person has not closed it or picked a drawing tool. */
  const startShown = () => {
    const e = editor();
    if (server === null || e === null || readOnly() || startDone()) return false;
    return offer(true, { nodes: e.views.status.nodes, unsaved: false }) === "apply";
  };
  createEffect(() => {
    const e = editor();
    if (e === null || startDone()) return;
    if (e.views.status.nodes > 0 || e.views.tool.tool !== "select") setStartDone(true);
  });

  const ui = {
    get onNew() {
      if (server === null) return () => setNewOpen(true);
      return readOnly() ? undefined : () => setNewOpen(true);
    },
    newLabel: server === null ? msg("menu.new") : msg("start.menu"),
    starting: {
      label: server === null ? msg("menu.new") : msg("start.menu"),
      run: () => setNewOpen(true),
      reason: () => (readOnly() ? (server?.whyReadOnly() ?? null) : null),
    },
    get onOpen() {
      return readOnly() ? undefined : () => picker?.click();
    },
    onSave: saveNow,
    onExport: server === null ? saveNow : exportNow,
    status: server?.indicator,
    get readOnly() {
      return readOnly();
    },
    onPreview: () => void loading.then(setPreviewing, () => undefined),
    get onImage() {
      return readOnly() ? undefined : () => imagePicker?.click();
    },
  };
  const publishFor = (e: Editor) => {
    const v = e.views.validation;
    const reason =
      server === null
        ? msg("topbar.publishLocal")
        : props.server?.canPublish !== true
          ? (props.server?.closed ?? msg("topbar.publishRole", { role: props.server?.role ?? "" }))
          : readOnly()
            ? msg("topbar.publishReadOnly")
            : !v.complete
              ? msg("topbar.publishChecking")
              : !v.publishable
                ? msg("topbar.publishBlocked")
                : null;
    return { reason, run: () => setPublishing(true) };
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
      <input
        ref={(e) => (imagePicker = e)}
        type="file"
        accept="image/png,image/jpeg,image/webp"
        class="hidden"
        onChange={(e) => {
          const file = e.currentTarget.files?.[0];
          e.currentTarget.value = "";
          if (file !== undefined) void placeImage(file);
        }}
        onCancel={() => (newFromImage = false)}
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
        <Match
          when={(() => {
            const p = server?.phase();
            return p?.kind === "failed" ? p : null;
          })()}
        >
          {(p) => (
            <Centre title={p().title} body={p().detail}>
              <div class="mt-4 flex gap-2">
                <Button onClick={() => navigate("/charts")}>{msg("serverChart.back")}</Button>
                <Show when={p().retry}>
                  <Button
                    variant="primary"
                    onClick={() => void loading.then((d) => server?.load(d))}
                  >
                    {msg("action.retry")}
                  </Button>
                </Show>
              </div>
            </Centre>
          )}
        </Match>
        <Match when={server?.phase().kind === "loading"}>
          <Centre title={msg("serverChart.loading")} />
        </Match>
        <Match when={editor()} keyed>
          {(e) => (
            <Shell
              editor={e}
              save={saveNow}
              ui={ui}
              publish={publishFor(e)}
              start={startShown() ? serverChoices : undefined}
            />
          )}
        </Match>
        <Match when={server !== null}>
          <Centre title={msg("serverChart.loading")} />
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

      <NewChartDialog
        open={newOpen()}
        title={server === null ? msg("new.title") : msg("start.title")}
        choices={server === null ? choices : serverChoices}
        onClose={() => setNewOpen(false)}
      />
      <Confirm
        open={replacing() !== null}
        title={msg("start.replaceTitle", {
          name: editor()?.views.title ?? "",
          template: msg(`template.${replacing()?.template.id ?? "theatre"}`),
        })}
        description={msg("start.replaceBody")}
        confirm={msg("start.replace")}
        variant="danger"
        onConfirm={() => {
          const r = replacing();
          setReplacing(null);
          if (r !== null) applyTemplate(r.template, r.text);
        }}
        onClose={() => setReplacing(null)}
      />
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
          if (session !== null) replace(session);
        }}
        onClose={() => {
          pending()?.dispose();
          setPending(null);
          imageForNew = null;
        }}
      />
      {server?.dialogs()}
      <Show when={previewing() !== null && editor()} keyed>
        {(e) => (
          <Suspense fallback={<Centre title={msg("preview.loading")} />}>
            <Preview
              designer={previewing() as Designer}
              session={e.session}
              images={(hash) => images.source(hash)}
              onClose={() => setPreviewing(null)}
            />
          </Suspense>
        )}
      </Show>
      <Show when={server !== null && editor()} keyed>
        {(e) => (
          <PublishDialog
            open={publishing()}
            name={e.views.title}
            chart={server?.chart() ?? null}
            seats={e.views.status.seats}
            run={() => server?.publish() ?? Promise.resolve({ kind: "notSaved" })}
            say={(findings) => e.run((s) => refused(findings, s)) ?? refused(findings, null)}
            onClose={() => setPublishing(false)}
          />
        )}
      </Show>
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

/** Restores the guides stored for a chart into its editor and stores them
 * again whenever they change (at most twice a second, and as the editor
 * closes). A record the session refuses is dropped. */
function keepGuides(e: Editor, key: string): void {
  const saved = load(key, parseGuides, null);
  const restored = saved === null ? null : e.attempt((s) => s.restoreGuides(saved));
  // Dropped only when the session judges the record itself (a guide out of
  // its limits), never for another refusal.
  if (restored !== null && "failure" in restored && restored.failure.kind === "command") {
    save(key, null);
  }
  const writer = throttledWriter((g) => save(key, g));
  createRoot((dispose) => {
    createEffect(
      on(
        () => e.views.guides.map((g) => [g.axis, g.at]),
        () => writer.put(() => e.session.savedGuides()),
        { defer: true },
      ),
    );
    e.attach(() => {
      writer.flush();
      dispose();
    });
  });
}

/** A file dropped that should be placed as an image rather than opened. */
const isImage = (file: File) =>
  file.type.startsWith("image/") || /\.(png|jpe?g|webp)$/i.test(file.name);

function Centre(props: { title: string; body?: string | undefined; children?: JSX.Element }) {
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
  ui: {
    onNew: (() => void) | undefined;
    newLabel: string;
    /** The palette's New chart, or a server chart's starting points. */
    starting: { label: string; run: () => void; reason: () => string | null };
    onOpen: (() => void) | undefined;
    onSave: () => void;
    onExport: () => void;
    status: (() => JSX.Element) | undefined;
    readOnly: boolean;
    onPreview: () => void;
    onImage: (() => void) | undefined;
  };
  publish: { reason: string | null; run: () => void };
  /** An empty server chart's starting points, while they are offered. */
  start?: Choices | undefined;
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
  // The first-run tour: once per browser, then from the palette.
  const [tour, setTour] = createSignal(load(TOUR_KEY, (v) => v !== "done", true));
  const endTour = () => {
    setTour(false);
    save(TOUR_KEY, "done");
  };
  const openNumbering = () => {
    setLayout({ inspectorOpen: true });
    setNumbering(true);
  };
  const act = actions(editor, {
    save: props.save,
    palette: () => setPalette(true),
    help: () => setHelp(true),
    image: () => props.ui.onImage?.(),
    rulers: () => setLayout({ rulers: !layout().rulers }),
  });
  onCleanup(bindKeys(editor, act));
  const all = commands(editor, act, {
    help: () => setHelp(true),
    insert: () => setInsert(true),
    numbering: openNumbering,
    tab: (tab) => setLayout({ tab, panelsOpen: true }),
    inspector: () => setLayout({ inspectorOpen: !layout().inspectorOpen }),
    newChart: props.ui.starting,
    open: props.ui.onOpen,
    tour: () => setTour(true),
  });
  let column: HTMLElement | undefined;

  const v = () => editor.views.validation;

  return (
    <div class="flex h-full flex-col">
      <TopBar
        editor={editor}
        {...props.ui}
        publish={props.publish}
        onInsert={() => setInsert(true)}
        onPalette={() => setPalette(true)}
        onHelp={() => setHelp(true)}
      />
      <div class="flex min-h-0 flex-1">
        <ToolRail editor={editor} onInsert={() => setInsert(true)} onImage={props.ui.onImage} />
        <div class="relative flex min-w-0 flex-1">
          <CanvasHost editor={editor} rulers={layout().rulers} />
          <Show when={props.start}>{(choices) => <StartPanel choices={choices()} />}</Show>
        </div>
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
                  ? `0 0 ${(numbering() ? Math.max(layout().split, NUMBERING_SPLIT) : layout().split) * 100}%`
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
      <Tour editor={editor} open={tour()} onClose={endTour} />
    </div>
  );
}

/** The starting points over an empty server chart's canvas (07 §16): not
 * modal, so the tools and the tour stay usable; closed by its button,
 * by choosing a drawing tool, or by the first thing drawn. */
function StartPanel(props: { choices: Choices }) {
  return (
    <section
      aria-labelledby="start-title"
      class="absolute inset-x-0 top-10 z-10 mx-auto w-[min(42rem,calc(100%-2rem))] rounded-lg border border-line bg-raised p-5 shadow-menu"
    >
      <div class="mb-3 flex items-start gap-3">
        <div class="min-w-0 flex-1">
          <h2 id="start-title" class="text-lg font-semibold text-fg">
            {msg("start.title")}
          </h2>
          <p class="text-base text-muted">{msg("start.body")}</p>
        </div>
        <IconButton
          icon="close"
          label={msg("start.close")}
          class="size-7"
          onClick={() => props.choices.empty()}
        />
      </div>
      <NewChart choices={props.choices} />
    </section>
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
