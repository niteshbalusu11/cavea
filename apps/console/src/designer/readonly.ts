// A chart opened by a role that may read it but not change it (09 §2):
// the session itself refuses every call that could change the document,
// so no panel, shortcut or tool can, whatever it offers. What stays is
// looking: the views, selection, floors, focus, copy, validation, and the
// hand tool to move around (the shell owns zoom).
import { Session, type Input, type InputResult, type ToolId } from "@cavea/designer-core";

/** Thrown by a refused call; the editor shows it as one message. */
export class ReadOnlyRefusal extends Error {
  constructor() {
    super("read-only");
    this.name = "ReadOnlyRefusal";
  }
}

/** Calls that read, or change only what is shown, never the document.
 * User guides are editor state beside the draft (07 §5.2), not the
 * document: a viewer may keep their own. */
const LOOKING = new Set([
  "changed",
  "export",
  "history",
  "title",
  "floors",
  "status",
  "selectionView",
  "fit",
  "toolView",
  "inspector",
  "layers",
  "categories",
  "numbering",
  "numberingPreview",
  "validation",
  "overlay",
  "cursor",
  "guides",
  "savedGuides",
  "addGuide",
  "moveGuide",
  "removeGuide",
  "clearGuides",
  "restoreGuides",
  "select",
  "selectAll",
  "stepIn",
  "stepOut",
  "setFloor",
  "focus",
  "copy",
  "sample",
  "seatUnder",
  "contrast",
  "tooSimilar",
  "validate",
  "expandLayer",
  "revealLayer",
  "clickLayer",
]);

const NOTHING: InputResult = { pan: null };

/** Makes `session` read-only, on the instance: the hand tool, pointer
 * input only, and every other call that is not looking refused. */
export function makeReadOnly(session: Session): void {
  const calls = Object.getPrototypeOf(Session.prototype) as object;
  for (const name of Object.getOwnPropertyNames(calls)) {
    if (name === "constructor" || name === "setTool" || name === "input" || LOOKING.has(name)) {
      continue;
    }
    Object.defineProperty(session, name, {
      value: () => {
        throw new ReadOnlyRefusal();
      },
    });
  }
  const setTool = Session.prototype.setTool;
  const input = Session.prototype.input;
  setTool.call(session, "hand");
  Object.defineProperty(session, "setTool", {
    value: (tool: ToolId) => {
      if (tool !== "hand") throw new ReadOnlyRefusal();
    },
  });
  Object.defineProperty(session, "input", {
    // Keys go nowhere: a tool's keys edit (Delete, numeric entry).
    value: (i: Input): InputResult =>
      i.kind === "keyDown" || i.kind === "keyUp" ? NOTHING : input.call(session, i),
  });
}
