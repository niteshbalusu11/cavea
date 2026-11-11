// One open chart: the session, the stores the frame loop fills, the
// shell's camera, and the actions the menus, panels and shortcuts share.
import {
  fitView,
  pan as panView,
  zoomAbout,
  type DesignerFailure,
  type Input,
  type Key,
  type Modifiers,
  type Rect,
  type Session,
  type ToolId,
  type View,
} from "@cavea/designer-core";
import { createSignal, type Accessor } from "solid-js";
import { createStore, reconcile, type SetStoreFunction } from "solid-js/store";
import { msg } from "../i18n/messages.ts";
import { notify } from "../ui/overlays.tsx";
import { failureMessage, failureOf } from "./errors.ts";
import { browserScheduler, FrameLoop, type Views } from "./loop.ts";

/** The view: the world point at the canvas's centre and the zoom, in CSS
 * pixels per chart unit (07 §3: the shell owns the camera). */
export interface Camera {
  x: number;
  y: number;
  zoom: number;
}
/** The zoom range until the canvas knows the chart's (`Canvas.zoomLimits`). */
export const ZOOM = { min: 0.002, max: 64, step: 1.25 } as const;
/** CSS px left around a box framed by zoom to fit or to selection. */
export const FRAME_PADDING = 24;

/** The canvas's view of a camera at a size and pixel ratio. */
export function viewOf(camera: Camera, size: [number, number], dpr: number): View {
  return {
    centre: [camera.x, camera.y],
    zoom: camera.zoom,
    width: size[0],
    height: size[1],
    dpr,
  };
}

const cameraOf = (view: View): Camera => ({
  x: view.centre[0],
  y: view.centre[1],
  zoom: view.zoom,
});

/** A camera the canvas can draw: finite, with a positive zoom. */
export const drawable = (c: Camera): boolean =>
  Number.isFinite(c.x) && Number.isFinite(c.y) && Number.isFinite(c.zoom) && c.zoom > 0;

/** What happened to a call: its value, or the refusal in words and as data. */
export type Outcome<T> = { ok: T } | { error: string; failure: DesignerFailure };

export interface Editor {
  session: Session;
  views: Views;
  loop: FrameLoop;
  camera: Accessor<Camera>;
  setCamera: (c: Camera) => void;
  /** The canvas region's size in CSS px, set by the canvas host. */
  size: Accessor<[number, number]>;
  setSize: (size: [number, number]) => void;
  /** The device pixel ratio, set by the canvas host. */
  dpr: Accessor<number>;
  setDpr: (dpr: number) => void;
  /** The canvas's zoom range (CSS px per chart unit), once it knows it. */
  limits: Accessor<[number, number] | null>;
  setLimits: (limits: [number, number] | null) => void;
  /** The camera, size and pixel ratio as the canvas's `View`: the one view
   * the canvas draws and pointer positions are read in. */
  view: Accessor<View>;
  /** The pointer over the canvas region, in chart units, for the status bar. */
  pointer: Accessor<[number, number] | null>;
  setPointer: (at: [number, number] | null) => void;
  /** The layers row to bring into view, set by `reveal`. */
  revealed: Accessor<{ row: number } | null>;
  /** The revision last saved (or opened): differs when there is something to save. */
  saved: Accessor<{ revision: number; at: Date | null }>;
  /** Notes that the document as it was at `revision` is stored. */
  markSaved: (revision: number) => void;
  /** Runs a call; a refusal becomes a toast and `undefined`. */
  run<T>(call: (s: Session) => T): T | undefined;
  /** Runs a call; a refusal is returned in words, for an inline error. */
  attempt<T>(call: (s: Session) => T): Outcome<T>;
  reveal(id: number | undefined): void;
  input(input: Input): void;
  key(kind: "keyDown" | "keyUp", key: Key, mods: Modifiers): void;
  /** Frames a box (zoom to fit, zoom to selection, a finding). */
  frame(box: Rect | null): void;
  /** Zooms by `factor` about a point of the canvas (CSS px), its centre
   * by default, within the zoom range. */
  zoomBy(factor: number, at?: [number, number]): void;
  /** Moves the chart by `by` CSS px on the canvas (it follows a drag). */
  pan(by: [number, number]): void;
  /** Runs `dispose` when the editor is disposed, before its session. */
  attach(dispose: () => void): void;
  setTool(tool: ToolId): void;
  copy(cut: boolean): Promise<void>;
  paste(): Promise<void>;
  dispose(): void;
}

/** The clipboard text last copied here, for a browser that refuses the
 * async Clipboard API (its permission prompt, an insecure origin). */
let memory: string | null = null;

async function writeClipboard(text: string): Promise<void> {
  memory = text;
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    // Kept in `memory`: paste in this page still works.
  }
}

async function readClipboard(): Promise<string | null> {
  try {
    const text = await navigator.clipboard.readText();
    if (text !== "") return text;
  } catch {
    // Refused or unavailable: this page's own copy.
  }
  return memory;
}

/**
 * Opens the editor on a session. `onPanic` is told when the module traps:
 * the session is unusable from then on, and the loop has stopped.
 */
export function createEditor(session: Session, onPanic: (message: string) => void): Editor {
  // The first frame is read before the stores exist, so that they start
  // full; every later one reconciles into them.
  const first: Partial<Views> = { numbering: null };
  let set: SetStoreFunction<Views> | null = null;
  const panic = (error: unknown): boolean => {
    const f = failureOf(error);
    if (f?.kind !== "panic") return false;
    loop.dispose();
    onPanic(f.message);
    return true;
  };
  const loop = new FrameLoop(
    session,
    {
      set(view, value) {
        if (set === null) first[view] = value;
        // Categories are known by their key; everything else by its id.
        else set(view, reconcile(value, { key: view === "categories" ? "key" : "id" }) as never);
      },
    },
    browserScheduler,
    (error) => {
      if (!panic(error)) throw error;
    },
  );
  loop.tick();
  const [views, setViews] = createStore<Views>(first as Views);
  set = setViews;

  const [camera, setCameraSignal] = createSignal<Camera>({ x: 0, y: 0, zoom: 0.1 });
  const setCamera = (c: Camera) => {
    if (drawable(c)) setCameraSignal(c);
  };
  const [size, setSize] = createSignal<[number, number]>([0, 0], {
    equals: (a, b) => a[0] === b[0] && a[1] === b[1],
  });
  const [dpr, setDpr] = createSignal(1);
  const [limits, setLimits] = createSignal<[number, number] | null>(null, {
    equals: (a, b) => a === b || (a !== null && b !== null && a[0] === b[0] && a[1] === b[1]),
  });
  const range = (): [number, number] => limits() ?? [ZOOM.min, ZOOM.max];
  const view = () => viewOf(camera(), size(), dpr());
  const attached: (() => void)[] = [];
  const [revealed, setRevealed] = createSignal<{ row: number } | null>(null);
  const [pointer, setPointer] = createSignal<[number, number] | null>(null);
  const [saved, setSaved] = createSignal({
    revision: views.status.revision,
    at: null as Date | null,
  });

  const attempt = <T>(call: (s: Session) => T): Outcome<T> => {
    try {
      return { ok: call(session) };
    } catch (error) {
      const f = failureOf(error);
      if (f !== null && panic(error)) return { error: msg("panic.title"), failure: f };
      if (f === null || f.kind === "panic") throw error;
      return { error: failureMessage(f), failure: f };
    }
  };
  const run = <T>(call: (s: Session) => T): T | undefined => {
    const r = attempt(call);
    if ("ok" in r) return r.ok;
    notify(r.error, "error");
    return undefined;
  };

  const editor: Editor = {
    session,
    views,
    loop,
    camera,
    setCamera,
    size,
    setSize,
    dpr,
    setDpr,
    limits,
    setLimits,
    view,
    revealed,
    pointer,
    setPointer,
    saved,
    markSaved: (revision) => setSaved({ revision, at: new Date() }),
    run,
    attempt,
    reveal(id) {
      if (id === undefined) return;
      const row = run((s) => s.revealLayer(id));
      if (row !== undefined && row !== null) setRevealed({ row });
    },
    input(input) {
      const by = run((s) => s.input(input))?.pan ?? null;
      if (by !== null) editor.pan(by);
    },
    key(kind, key, mods) {
      editor.input({ kind, data: [key, mods] });
    },
    frame(box) {
      const v = view();
      if (box === null || v.width <= 0 || v.height <= 0) return;
      setCamera(cameraOf(fitView(v, box, FRAME_PADDING, range())));
    },
    zoomBy(factor, at) {
      const v = view();
      setCamera(cameraOf(zoomAbout(v, factor, at ?? [v.width / 2, v.height / 2], range())));
    },
    pan(by) {
      setCamera(cameraOf(panView(view(), by)));
    },
    attach(dispose) {
      attached.push(dispose);
    },
    setTool(tool) {
      run((s) => s.setTool(tool));
    },
    async copy(cut) {
      const text = run((s) => (cut ? s.cut() : s.copy()));
      if (typeof text === "string") await writeClipboard(text);
    },
    async paste() {
      const text = await readClipboard();
      if (text === null) return notify(msg("clipboard.empty"));
      editor.reveal(run((s) => s.paste(text, null))?.created[0]);
    },
    dispose() {
      loop.dispose();
      for (const d of attached.splice(0)) d();
      session.dispose();
    },
  };
  loop.start();
  return editor;
}
