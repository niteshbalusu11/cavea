// The first-run tour's coach marks (07 §16, `tour.ts`): a card beside the
// part of the shell each step is about, not modal, so the user does the
// step itself; Next, Back and Skip by keyboard, Escape skips.
import { createEffect, createSignal, on, onCleanup, Show } from "solid-js";
import { Portal } from "solid-js/web";
import { msg } from "../i18n/messages.ts";
import { Button } from "../ui/controls.tsx";
import type { Editor } from "./editor.ts";
import { next, STEPS, type Anchor } from "./tour.ts";

const WIDTH = 288;
const GAP = 12;

function anchorOf(anchor: Anchor): HTMLElement | null {
  switch (anchor) {
    case "rail":
      return document.querySelector("nav[role=toolbar]");
    case "canvas":
      return document.querySelector("[data-canvas]");
    case "inspector":
      return document.querySelector("aside > section");
    case "panels":
      return document.querySelector("aside [role=tablist]");
    case "publish":
      return (
        [...document.querySelectorAll<HTMLElement>("header button")].find(
          (b) => b.textContent?.trim() === msg("topbar.publish"),
        ) ?? null
      );
  }
}

/** Where the card goes: right of the rail, inside the canvas's top left,
 * left of the right column, under Publish; kept in the window. */
function place(anchor: Anchor, r: DOMRect): { left: number; top: number } {
  const at =
    anchor === "rail"
      ? { left: r.right + GAP, top: r.top + 72 }
      : anchor === "canvas"
        ? { left: r.left + GAP, top: r.top + GAP }
        : anchor === "publish"
          ? { left: r.right - WIDTH, top: r.bottom + GAP }
          : { left: r.left - WIDTH - GAP, top: r.top + GAP };
  return {
    left: Math.max(GAP, Math.min(at.left, window.innerWidth - WIDTH - GAP)),
    top: Math.max(GAP, Math.min(at.top, window.innerHeight - 220)),
  };
}

export function Tour(props: { editor: Editor; open: boolean; onClose: () => void }) {
  const editor = props.editor;
  const [at, setAt] = createSignal(0);
  let since = editor.views.status.revision;
  const go = (i: number) => {
    since = editor.views.status.revision;
    setAt(i);
  };
  // Opened again: from the start.
  createEffect(
    on(
      () => props.open,
      (open) => open && go(0),
    ),
  );
  // A step done by the user moves on.
  createEffect(() => {
    if (!props.open) return;
    const v = editor.views;
    const i = next(at(), since, v.status.revision, v.history, v.selection);
    if (i !== at()) go(i);
  });
  const step = () => STEPS[at()];

  const [pos, setPos] = createSignal({ left: 0, top: 0 });
  const measure = () => {
    const s = step();
    const el = s === undefined ? null : anchorOf(s.anchor);
    if (s !== undefined && el !== null) setPos(place(s.anchor, el.getBoundingClientRect()));
  };
  createEffect(on(at, measure));
  window.addEventListener("resize", measure);
  onCleanup(() => window.removeEventListener("resize", measure));

  return (
    <Show when={props.open && step()}>
      {(s) => (
        <Portal>
          <section
            role="dialog"
            aria-modal="false"
            aria-labelledby="tour-title"
            class="fixed z-30 rounded-lg border border-line bg-raised p-4 text-fg shadow-menu"
            style={{ left: `${pos().left}px`, top: `${pos().top}px`, width: `${WIDTH}px` }}
            onKeyDown={(e) => {
              if (e.key !== "Escape") return;
              e.stopPropagation();
              props.onClose();
            }}
          >
            <p class="text-xs text-muted" aria-live="polite">
              {msg("tour.step", { n: at() + 1, of: STEPS.length })}
            </p>
            <h2 id="tour-title" class="mt-0.5 text-base font-semibold">
              {msg(s().title)}
            </h2>
            <p class="mt-1 text-sm text-muted">{msg(s().body)}</p>
            <div class="mt-3 flex items-center gap-2">
              <Button variant="ghost" onClick={() => props.onClose()}>
                {msg("tour.skip")}
              </Button>
              <span class="flex-1" />
              <Show when={at() > 0}>
                <Button onClick={() => go(at() - 1)}>{msg("tour.back")}</Button>
              </Show>
              <Button
                variant="primary"
                onClick={() => (at() + 1 < STEPS.length ? go(at() + 1) : props.onClose())}
              >
                {at() + 1 < STEPS.length ? msg("tour.next") : msg("tour.done")}
              </Button>
            </div>
          </section>
        </Portal>
      )}
    </Show>
  );
}
