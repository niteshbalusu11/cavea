// The validation panel (07 §10): errors, then warnings (the view's order),
// each under a heading with its count; stale findings marked, "checking"
// while the full pass is pending. The list is virtual: a chart can have
// thousands of findings. A finding selects what it names (`focus`), the
// layers panel reveals it and the camera frames it. A finding's text is
// the compiler's English under the catalogue's title for its code: the view
// carries no data to compose the sentence from.
import type { FindingView } from "@cavea/designer-core";
import { createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { msg, msgOf } from "../i18n/messages.ts";
import { Icon } from "../ui/Icon.tsx";
import { windowFor } from "../ui/window.ts";
import type { Editor } from "./editor.ts";

const ROW = 44;
const HEAD = 28;
const OVERSCAN = 6;
type Severity = "error" | "warning";

export function ValidationPanel(props: { editor: Editor }) {
  const editor = props.editor;
  const v = () => editor.views.validation;
  const [scrollTop, setScrollTop] = createSignal(0);
  const [height, setHeight] = createSignal(0);
  let scroller: HTMLDivElement | undefined;
  onMount(() => {
    const ro = new ResizeObserver(([e]) => setHeight(e?.contentRect.height ?? 0));
    if (scroller !== undefined) ro.observe(scroller);
    onCleanup(() => ro.disconnect());
  });
  /** The two groups, each a heading and its rows, laid out one after the
   * other: the view lists errors first. */
  const groups = () => {
    const errors = v().findings.filter((f) => f.severity === "error").length;
    const n = v().findings.length;
    const out: { severity: Severity; top: number; first: number; count: number }[] = [];
    let top = 0;
    for (const [severity, first, count] of [
      ["error", 0, errors],
      ["warning", errors, n - errors],
    ] as const) {
      if (count === 0) continue;
      out.push({ severity, top, first, count });
      top += HEAD + count * ROW;
    }
    return { out, height: top };
  };
  /** The rows each group shows at this scroll position. */
  const shown = () =>
    groups().out.flatMap((g) => {
      const w = windowFor(scrollTop() - g.top - HEAD, height(), ROW, g.count, OVERSCAN);
      return v()
        .findings.slice(g.first + w.start, g.first + w.start + w.count)
        .map((f, i) => ({ f, top: g.top + HEAD + (w.start + i) * ROW }));
    });
  const go = (f: FindingView) => {
    const node = f.node;
    if (node === null) return;
    const box = editor.run((s) => s.focus(node, f.row, f.seat));
    editor.reveal(node);
    if (box !== undefined) editor.frame(box);
  };

  return (
    <div class="flex min-h-0 flex-1 flex-col" aria-busy={!v().complete}>
      <div
        class="flex h-8 shrink-0 items-center gap-3 border-b border-line px-3 text-sm"
        aria-live="polite"
      >
        <Show
          when={v().complete}
          fallback={
            <span class="flex items-center gap-2 text-muted">
              <span class="size-2 animate-pulse rounded-full bg-accent-fill" />
              {msg("validation.checking")}
            </span>
          }
        >
          <Show
            when={v().findings.length > 0}
            fallback={<span class="text-muted">{msg("validation.passed")}</span>}
          >
            <span class="flex items-center gap-1 text-fg">
              <Icon name="error" class="size-3.5 text-danger" />
              {msg("validation.errors")} <span class="tabular-nums">{v().errors}</span>
            </span>
            <span class="flex items-center gap-1 text-fg">
              <Icon name="warning" class="size-3.5 text-warning" />
              {msg("validation.warnings")} <span class="tabular-nums">{v().warnings}</span>
            </span>
          </Show>
        </Show>
      </div>
      <Show when={v().encode}>
        {(m) => (
          <p class="border-b border-line bg-danger-soft px-3 py-1.5 text-sm text-danger">
            {msg("validation.encode", { message: m() })}
          </p>
        )}
      </Show>
      <div
        ref={(e) => (scroller = e)}
        class="relative min-h-0 flex-1 overflow-y-auto"
        onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
      >
        <Show when={v().complete && v().findings.length === 0}>
          <div class="flex flex-col items-center gap-1 px-6 py-8 text-center">
            <span class="flex size-8 items-center justify-center rounded-full bg-hover text-success">
              <Icon name="check" />
            </span>
            <p class="text-base font-medium text-fg">{msg("validation.valid")}</p>
            <p class="text-sm text-muted">{msg("validation.validBody")}</p>
          </div>
        </Show>
        <Show when={!v().complete && v().findings.length === 0}>
          <p class="p-3 text-sm text-muted">{msg("validation.checkingBody")}</p>
        </Show>
        <div class="relative" style={{ height: `${groups().height}px` }}>
          <For each={groups().out}>
            {(g) => (
              <h3
                class="absolute inset-x-0 flex items-center gap-1.5 border-b border-line bg-surface px-3 text-xs font-semibold tracking-wide text-faint uppercase"
                style={{ top: `${g.top}px`, height: `${HEAD}px` }}
              >
                {msg(g.severity === "error" ? "validation.errors" : "validation.warnings")}
                <span class="tabular-nums">{g.count}</span>
              </h3>
            )}
          </For>
          <ul aria-label={msg("validation.label")}>
            <For each={shown()}>
              {({ f, top }) => {
                const title = msgOf("validation", f.code) ?? f.code;
                return (
                  <li class="absolute inset-x-0" style={{ top: `${top}px`, height: `${ROW}px` }}>
                    <button
                      type="button"
                      disabled={f.node === null}
                      title={f.message}
                      class="flex h-full w-full items-start gap-2 px-3 py-1.5 text-left enabled:hover:bg-hover"
                      classList={{ "opacity-70": f.stale }}
                      onClick={() => go(f)}
                    >
                      <Icon
                        name={f.severity === "error" ? "error" : "warning"}
                        class={`mt-px size-3.5 shrink-0 ${f.severity === "error" ? "text-danger" : "text-warning"}`}
                      />
                      <span class="min-w-0 flex-1">
                        <span class="flex items-baseline gap-1.5">
                          <span class="truncate font-medium text-fg">{title}</span>
                          <span class="font-mono text-2xs text-faint">{f.code}</span>
                          <Show when={f.stale}>
                            <span class="ml-auto shrink-0 text-xs text-faint italic">
                              {msg("validation.stale")}
                            </span>
                          </Show>
                        </span>
                        <span class="block truncate text-sm text-muted">{f.message}</span>
                      </span>
                    </button>
                  </li>
                );
              }}
            </For>
          </ul>
        </div>
      </div>
    </div>
  );
}
