// The text tool's field on the canvas (07 §4, Text). The edit core owns no
// text field: while text is typed it asks for one (`Session::text_field`,
// the overlay's `text`), placed at the text's anchor, which this draws over
// the canvas, sized and turned with the text at the current zoom as far as
// a field can follow (a font size the person can still read), and aligned
// as the text is. Every change goes to the session (`setText`), which shows
// it live on the canvas; Enter, or leaving the field for anywhere but the
// canvas, commits it as one undo entry; Escape cancels. A click on the
// canvas commits by itself (the tool's own rule), so the field leaves that
// to the click. With the text tool and nothing typed, Enter on the canvas
// places a text in the middle of the view, for the keyboard.
import { toScreen, type TextField } from "@cavea/designer-core";
import { Show } from "solid-js";
import { msg } from "../i18n/messages.ts";
import type { Editor } from "./editor.ts";

const NONE = { shift: false, alt: false, ctrl: false };
/** The field's font size, in CSS px, whatever the zoom: readable, and no
 * larger than a field the canvas can hold. */
const MIN_PX = 13;
const MAX_PX = 160;

/** Where the field's box lies along its x axis from the anchor, as the
 * text does (03 §2.7). */
const SHIFT = { left: "0", center: "-50%", right: "-100%" } as const;

/** The field's place on screen: anchor, font size, width, rotation. */
export function fieldBox(
  field: TextField,
  view: Parameters<typeof toScreen>[0],
): { left: number; top: number; px: number; width: number; angle: number } {
  const [left, top] = toScreen(view, field.at);
  const px = Math.min(MAX_PX, Math.max(MIN_PX, field.size * view.zoom));
  // The text's box model (03 §2.7): 0.6 em per character, and room to type.
  const width = Math.max(4, [...field.text].length + 2) * 0.6 * px + 12;
  return { left, top, px, width, angle: field.angle };
}

export function TextOnCanvas(props: {
  editor: Editor;
  /** The field the session asks for now, or `null`. */
  field: TextField | null;
  /** The canvas, where a click commits by itself. */
  canvas: () => HTMLCanvasElement;
}) {
  const editor = props.editor;
  /** Set while the field closes by Enter or Escape: its blur then does nothing. */
  let closing = false;
  const box = () => (props.field === null ? null : fieldBox(props.field, editor.view()));
  /** Commits (Enter) or cancels (Escape) through the tool; back to the
   * canvas unless focus went elsewhere. */
  const close = (k: "enter" | "escape", refocus: boolean) => {
    closing = true;
    editor.key("keyDown", k, NONE);
    editor.key("keyUp", k, NONE);
    if (refocus) props.canvas().focus({ preventScroll: true });
  };
  // Not keyed: the input lives while the field stays open, and is its own
  // value's source then, so the caret never jumps; a new field is a new
  // input, holding its text and focused.
  return (
    <Show when={box()}>
      {(b) => (
        <input
          ref={(e) => {
            closing = false;
            e.value = props.field?.text ?? "";
            queueMicrotask(() => {
              e.focus({ preventScroll: true });
              e.select();
            });
          }}
          type="text"
          spellcheck={false}
          autocomplete="off"
          aria-label={msg("canvas.textField")}
          title={msg("canvas.textFieldHint")}
          class="absolute z-10 rounded-sm border border-accent bg-raised px-1 text-fg shadow-menu outline-none"
          style={{
            left: `${b().left}px`,
            top: `${b().top}px`,
            width: `${b().width}px`,
            "font-size": `${b().px}px`,
            "line-height": "1.2",
            "font-weight": String(props.field?.weight ?? 400),
            "text-align": props.field?.align ?? "center",
            "transform-origin": "0 0",
            transform: `rotate(${b().angle}deg) translate(${SHIFT[props.field?.align ?? "center"]}, -50%)`,
          }}
          onInput={(e) => {
            const text = e.currentTarget.value;
            editor.run((s) => s.setText(text));
          }}
          onKeyDown={(e) => {
            if (e.isComposing) return;
            if (e.key === "Enter") {
              e.preventDefault();
              e.stopPropagation();
              close("enter", true);
            } else if (e.key === "Escape") {
              e.preventDefault();
              e.stopPropagation();
              close("escape", true);
            }
          }}
          onBlur={(e) => {
            // A press on the canvas commits by itself; Enter and Escape
            // have already acted.
            if (closing || e.relatedTarget === props.canvas()) return;
            close("enter", false);
          }}
        />
      )}
    </Show>
  );
}
