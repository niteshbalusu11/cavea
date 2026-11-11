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

A `panic` failure means a bug in the edit core: the module is unusable until the page is reloaded (wasm-bindgen keeps one instance per page).
