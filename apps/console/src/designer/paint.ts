// When the canvas draws (07 §14): in the frame after the session changed
// (any view `changed()` names, the overlay and the preview included), the
// view or theme changed, or the canvas asked for another frame (labels
// still arriving, moved pieces still baking, the glyph atlas or a lost
// context back). Otherwise it draws nothing: an idle designer draws no frame.
import type { ViewName } from "@cavea/designer-core";
import type { Paint } from "./loop.ts";

/** The canvas's draw: whether it wants another frame. */
export interface Drawable {
  draw(time: number): boolean;
}

export class Painter implements Paint {
  readonly #canvas: Drawable;
  readonly #onChanged: (names: ReadonlySet<ViewName>) => void;
  #wanted = true;
  /** Draws made, for the idle check. */
  draws = 0;

  constructor(canvas: Drawable, onChanged: (names: ReadonlySet<ViewName>) => void = () => {}) {
    this.#canvas = canvas;
    this.#onChanged = onChanged;
  }

  /** The session changed: draw at the next frame. */
  changed(names: ReadonlySet<ViewName>): void {
    this.#wanted = true;
    this.#onChanged(names);
  }

  /** Something outside the session changed the picture: draw at the next frame. */
  invalidate(): void {
    this.#wanted = true;
  }

  frame(time: number): void {
    if (!this.#wanted) return;
    this.#wanted = false;
    this.draws += 1;
    if (this.#canvas.draw(time)) this.#wanted = true;
  }
}
