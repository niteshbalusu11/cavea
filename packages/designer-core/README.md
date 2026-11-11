# @cavea/designer-core

The designer's edit core (`crates/cavea-edit`) as WebAssembly, behind a typed `Session` (`docs/07-designer.md` §2.4). No framework, no DOM, no dependencies.

Build the module first: `cargo xtask build-designer` (into `wasm/`, not committed). The types in `src/generated/` come from the Rust definitions: `cargo xtask types`.

```ts
import { DesignerError, loadDesigner } from "@cavea/designer-core";

const designer = await loadDesigner(); // or { wasm, bindings } for other URLs
const session = designer.open(draftJson); // throws DesignerError: { kind: "import", error }

// Once per animation frame: re-read only the views that changed.
function frame() {
  for (const view of session.changed()) {
    if (view === "history") setHistory(session.history());
    else if (view === "overlay") drawOverlay(session.overlay());
    // … one branch per ViewName
  }
  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);

// In idle time: validation, a piece at a time.
requestIdleCallback(function idle(deadline) {
  while (deadline.timeRemaining() > 1 && !session.validate(20_000).done);
  requestIdleCallback(idle);
});

// A refused call throws and changes nothing.
try {
  session.setField({ field: "rowPitch", value: 95 });
} catch (error) {
  if (error instanceof DesignerError && error.failure.kind === "command") {
    showRefusal(error.failure.error.code);
  } else throw error;
}

session.dispose(); // when the chart closes
```

## The canvas

`createCanvas(session, element)` draws the session's chart and overlay on a WebGL2 canvas (`docs/04-renderer.md` §25, `docs/07-designer.md` §2.4). The host owns the view (plain data) and every gesture; the canvas draws the view it is given.

```ts
import { createCanvas, fitView, pan, toChart, zoomAbout, type View } from "@cavea/designer-core";

const canvas = createCanvas(session, element, { theme: "light", onFrame: () => (dirty = true) });
let view: View = { centre: [0, 0], zoom: 1, width: element.clientWidth, height: element.clientHeight, dpr: devicePixelRatio };
view = fitView(view, session.fit() ?? { min: [0, 0], max: [1, 1] }, 24, null);
canvas.setView(view); // again after every pan, zoom or resize
// input: session.input({ kind: "pointerMove", data: { world: toChart(view, at), screen: at, zoom: view.zoom, … } })
// its `pan` result: view = pan(view, result.pan); the wheel: view = zoomAbout(view, f, at, canvas.zoomLimits())
requestAnimationFrame(function frame(t) {
  if (session.changed().length > 0 || dirty) dirty = canvas.draw(t); // true: draw again next frame
  requestAnimationFrame(frame);
});
canvas.dispose(); // when the chart closes, before session.dispose()
```

The canvas handles a lost WebGL context itself and calls `onFrame` when it needs a frame the host cannot know about (the glyph atlas arrived, the context came back). `packages/designer-core/proof` is a page that drives it (`AGENTS.md`).

A `panic` failure means a bug in the edit core: the module is unusable until the page is reloaded (wasm-bindgen keeps one instance per page).
