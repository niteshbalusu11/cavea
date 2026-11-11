// A test harness for the designer's canvas (not product interface): opens a
// preset's draft, owns a minimal view (wheel zoom, pan through
// `session.input`'s `pan`), forwards pointer and key input to the session,
// and has a few buttons. Served by ./serve.ts. Query parameters: `preset`
// (default `theatre`), `theme` (`light`, `dark`), `bar=0` (no buttons), and
// `camera=x:y:width:height` (the view that fits that box, as the buyer demo's
// parameter, so the two pages can be compared).
import {
  createCanvas,
  fitView,
  loadDesigner,
  pan,
  toChart,
  zoomAbout,
  type Button,
  type Input,
  type Key,
  type Modifiers,
  type PointerKind,
  type ToolId,
  type View,
} from "../src/index.ts";

const params = new URLSearchParams(location.search);
const preset = params.get("preset") ?? "theatre";
const element = document.querySelector<HTMLCanvasElement>("#canvas");
const bar = document.querySelector<HTMLDivElement>("#bar");
if (element === null || bar === null) {
  throw new Error("page is missing its canvas");
}

const designer = await loadDesigner({
  bindings: new URL("/wasm/cavea_designer.js", location.href),
  wasm: new URL("/wasm/cavea_designer_bg.wasm", location.href),
});
const session = designer.open(await (await fetch(`/draft/${preset}`)).text());
let theme: "light" | "dark" = params.get("theme") === "dark" ? "dark" : "light";
let wanted = true;
const canvas = createCanvas(session, element, {
  theme,
  glyphs: "/glyphs.png",
  onFrame: () => (wanted = true),
  onError: (e) => console.error("canvas", e),
});

const size = (): Pick<View, "width" | "height" | "dpr"> => ({
  width: element.clientWidth,
  height: element.clientHeight,
  dpr: devicePixelRatio,
});
let view: View = { centre: [0, 0], zoom: 1, ...size() };
const camera = params.get("camera")?.split(":").map(Number);
const fit = session.fit();
if (camera !== undefined && camera.length === 4) {
  const [x = 0, y = 0, w = 1, h = 1] = camera;
  view = fitView(view, { min: [x, y], max: [x + w, y + h] });
} else if (fit !== null) {
  view = fitView(view, fit);
}
if (params.get("bar") === "0") {
  bar.style.display = "none";
}
const setView = (next: View) => {
  view = next;
  canvas.setView(view);
  wanted = true;
};
setView(view);
new ResizeObserver(() => setView({ ...view, ...size() })).observe(element);

// The frame loop: draw only when the session or the view changed, or the
// canvas asked for another frame; count the draws for the idle check.
const proof = {
  ready: false,
  draws: 0,
  /** Milliseconds of each `canvas.draw()` call, the last 240. */
  drawMs: [] as number[],
  get view() {
    return view;
  },
  setView,
  session,
  canvas,
  glError: () => element.getContext("webgl2")?.getError() ?? -1,
};
(window as unknown as { proof: typeof proof }).proof = proof;
const stats = document.createElement("span");
stats.id = "stats";
function frame(time: number) {
  const changed = session.changed();
  if (changed.length > 0 || wanted) {
    const t0 = performance.now();
    wanted = canvas.draw(time);
    proof.drawMs.push(performance.now() - t0);
    proof.drawMs.splice(0, proof.drawMs.length - 240);
    proof.draws += 1;
    const s = canvas.stats();
    stats.textContent = `${s.level ?? "-"} · ${s.drawCalls} draws · ${s.seatsDrawn} seats · ${s.compiled} compiled`;
  }
  // Validation in what is left of the frame, as the shell would in idle time.
  if (!session.validate(20_000).done) {
    wanted = true;
  }
  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);

// Input: positions in chart units through the view; the hand tool, Space
// and the middle button pan by what `input` returns.
const mods = (e: MouseEvent | KeyboardEvent): Modifiers => ({
  shift: e.shiftKey,
  alt: e.altKey,
  ctrl: e.ctrlKey || e.metaKey,
});
const buttons: Button[] = ["primary", "middle", "secondary"];
const send = (input: Input) => {
  const result = session.input(input);
  if (result.pan !== null) {
    setView(pan(view, result.pan));
  }
};
const pointer = (kind: "pointerDown" | "pointerMove" | "pointerUp") => (e: PointerEvent) => {
  const rect = element.getBoundingClientRect();
  const screen: [number, number] = [e.clientX - rect.left, e.clientY - rect.top];
  if (kind === "pointerDown") {
    element.setPointerCapture(e.pointerId);
    element.focus();
  }
  send({
    kind,
    data: {
      world: toChart(view, screen),
      screen,
      zoom: view.zoom,
      button: buttons[e.button] ?? "primary",
      kind: (e.pointerType === "touch" || e.pointerType === "pen"
        ? e.pointerType
        : "mouse") as PointerKind,
      mods: mods(e),
      time: e.timeStamp,
    },
  });
};
element.addEventListener("pointerdown", pointer("pointerDown"));
element.addEventListener("pointermove", pointer("pointerMove"));
element.addEventListener("pointerup", pointer("pointerUp"));
element.addEventListener("pointercancel", () => send({ kind: "pointerCancel" }));
element.addEventListener(
  "wheel",
  (e) => {
    e.preventDefault();
    const rect = element.getBoundingClientRect();
    const at: [number, number] = [e.clientX - rect.left, e.clientY - rect.top];
    setView(zoomAbout(view, 2 ** (-e.deltaY / 300), at, canvas.zoomLimits()));
  },
  { passive: false },
);
const KEYS: Record<string, Key> = {
  Escape: "escape",
  Enter: "enter",
  ArrowLeft: "arrowLeft",
  ArrowRight: "arrowRight",
  ArrowUp: "arrowUp",
  ArrowDown: "arrowDown",
  Backspace: "backspace",
  Delete: "delete",
};
const key = (kind: "keyDown" | "keyUp") => (e: KeyboardEvent) => {
  const k: Key | undefined =
    KEYS[e.key] ?? (e.key.length === 1 ? { char: e.key.toLowerCase() } : undefined);
  if (k !== undefined) {
    e.preventDefault();
    send({ kind, data: [k, mods(e)] });
  }
};
element.addEventListener("keydown", key("keyDown"));
element.addEventListener("keyup", key("keyUp"));

// Buttons.
const button = (label: string, run: () => void) => {
  const b = document.createElement("button");
  b.textContent = label;
  b.addEventListener("click", () => {
    run();
    wanted = true;
  });
  bar.append(b);
  return b;
};
const tools: [string, ToolId][] = [
  ["Select", "select"],
  ["Hand", "hand"],
  ["Row block", "rowBlock"],
  ["Arc block", "arcBlock"],
  ["Section", "section"],
  ["Focal", "focal"],
];
for (const [label, id] of tools) {
  button(label, () => session.setTool(id));
}
button("Select all", () => session.selectAll());
button("Undo", () => session.undo());
button("Redo", () => session.redo());
button("Number 101…", () => {
  session.previewNumbering({
    rows: null,
    seats: { scheme: "numeric", start: 101, step: 1, direction: "ltr" },
    afterZ: null,
    across: null,
    keepOverrides: false,
  });
});
button("Fit", () => {
  const box = session.fit();
  if (box !== null) setView(fitView(view, box, 24, canvas.zoomLimits()));
});
button("Theme", () => {
  theme = theme === "light" ? "dark" : "light";
  canvas.setTheme(theme);
  document.body.style.background = theme === "dark" ? "#1C1917" : "#FAFAF9";
});
const lose = element.getContext("webgl2")?.getExtension("WEBGL_lose_context") ?? null;
button("Lose context", () => lose?.loseContext());
button("Restore context", () => lose?.restoreContext());
bar.append(stats);
document.body.style.background = theme === "dark" ? "#1C1917" : "#FAFAF9";
proof.ready = true;
