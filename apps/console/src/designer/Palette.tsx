// The command palette (07 §8, Ctrl or Cmd+K): a combobox over every action,
// fuzzy-filtered as you type, run with Enter or a click. An action that
// cannot run now stays listed, dimmed, with the reason. Searching charts
// and events (09 §8) needs the server: it is another list of `Command`s
// handed to the same palette.
import { createEffect, createMemo, createSignal, createUniqueId, For, Show } from "solid-js";
import { msg } from "../i18n/messages.ts";
import { Dialog } from "../ui/overlays.tsx";
import type { Command } from "./commands.ts";
import { match, rank } from "./match.ts";

/** Highlights the matched characters of `text`. */
function Marked(props: { text: string; at: number[] }) {
  const parts = () => {
    const hit = new Set(props.at);
    const out: { s: string; on: boolean }[] = [];
    for (const [i, ch] of [...props.text].entries()) {
      const on = hit.has(i);
      const last = out.at(-1);
      if (last !== undefined && last.on === on) last.s += ch;
      else out.push({ s: ch, on });
    }
    return out;
  };
  return (
    <For each={parts()}>
      {(p) => (p.on ? <mark class="bg-transparent font-semibold text-fg">{p.s}</mark> : p.s)}
    </For>
  );
}

export function Palette(props: {
  open: boolean;
  commands: () => Command[];
  onClose: () => void;
  /** The console's search of charts and events (`console/Keys.tsx`):
   * told what is typed, its results listed after the commands, its state
   * said. */
  search?: {
    onQuery: (query: string) => void;
    results: () => Command[];
    status: () => string | null;
  };
  placeholder?: string;
}) {
  const id = createUniqueId();
  const [query, setQuery] = createSignal("");
  const [active, setActive] = createSignal(0);
  let input: HTMLInputElement | undefined;
  let list: HTMLUListElement | undefined;

  const shown = createMemo(() => {
    if (!props.open) return [];
    const found = (props.search?.results() ?? []).map((item) => ({
      item,
      match: match(query(), item.label) ?? { score: 0, at: [] },
    }));
    return [...rank(props.commands(), query(), (c) => c.label), ...found];
  });
  // Results can change under the active row (a search answering): the
  // active index stays within them, so what is highlighted, announced and
  // run is one row.
  createEffect(() => {
    const n = shown().length;
    if (active() > Math.max(0, n - 1)) setActive(Math.max(0, n - 1));
  });
  const current = () => shown()[active()];
  const optionId = (i: number) => `${id}-${i}`;

  const close = () => {
    setQuery("");
    props.search?.onQuery("");
    setActive(0);
    props.onClose();
  };
  const run = (c: Command) => {
    if (c.reason() !== null) return;
    close();
    // After the dialog has closed and returned focus, so that an action
    // opening a dialog of its own takes focus from there.
    setTimeout(c.run, 0);
  };
  const move = (to: number) => {
    const n = shown().length;
    if (n === 0) return;
    const i = (to + n) % n;
    setActive(i);
    list?.querySelector(`#${CSS.escape(optionId(i))}`)?.scrollIntoView({ block: "nearest" });
  };
  const key = (e: KeyboardEvent) => {
    const page = 8;
    const go: Record<string, () => void> = {
      ArrowDown: () => move(active() + 1),
      ArrowUp: () => move(active() - 1),
      PageDown: () => move(Math.min(shown().length - 1, active() + page)),
      PageUp: () => move(Math.max(0, active() - page)),
      Enter: () => {
        const c = current();
        if (c !== undefined) run(c.item);
      },
    };
    const action = go[e.key];
    if (action === undefined) return;
    e.preventDefault();
    action();
  };

  return (
    <Dialog
      open={props.open}
      title={msg("palette.title")}
      onClose={close}
      top
      class="max-w-xl"
      initial={() => input}
    >
      <div class="flex h-11 shrink-0 items-center border-b border-line px-3">
        <input
          ref={(e) => (input = e)}
          role="combobox"
          aria-expanded="true"
          aria-controls={`${id}-list`}
          aria-autocomplete="list"
          aria-activedescendant={current() ? optionId(active()) : undefined}
          aria-label={msg("palette.title")}
          placeholder={props.placeholder ?? msg("palette.placeholder")}
          class="h-full w-full bg-transparent text-lg text-fg outline-none placeholder:text-faint"
          value={query()}
          onInput={(e) => {
            setQuery(e.currentTarget.value);
            props.search?.onQuery(e.currentTarget.value);
            setActive(0);
            list?.scrollTo({ top: 0 });
          }}
          onKeyDown={key}
        />
      </div>
      <ul
        ref={(e) => (list = e)}
        id={`${id}-list`}
        role="listbox"
        aria-label={msg("palette.results")}
        class="max-h-[min(420px,60vh)] overflow-y-auto p-1"
      >
        <For each={shown()}>
          {(row, i) => {
            const reason = () => row.item.reason();
            return (
              <li
                id={optionId(i())}
                role="option"
                aria-selected={i() === active()}
                aria-disabled={reason() !== null}
                class="flex h-8 cursor-default items-center gap-3 rounded-sm px-2 text-base select-none aria-selected:bg-accent-soft"
                onPointerMove={() => i() !== active() && setActive(i())}
                onClick={() => run(row.item)}
              >
                <span
                  class="min-w-0 truncate"
                  classList={{ "text-fg": reason() === null, "text-faint": reason() !== null }}
                >
                  <Marked text={row.item.label} at={row.match.at} />
                </span>
                <Show when={reason()}>
                  {(r) => <span class="min-w-0 truncate text-sm text-faint">{r()}</span>}
                </Show>
                <Show when={row.item.detail}>
                  {(d) => <span class="min-w-0 truncate font-mono text-sm text-muted">{d()}</span>}
                </Show>
                <span class="ml-auto flex shrink-0 items-center gap-3">
                  <span class="text-xs text-faint">{msg(`group.${row.item.group}`)}</span>
                  <Show when={row.item.keys}>
                    {(k) => (
                      <kbd class="min-w-12 text-right font-sans text-sm text-muted">{k()}</kbd>
                    )}
                  </Show>
                </span>
              </li>
            );
          }}
        </For>
      </ul>
      <p class="px-4 pt-1 pb-4 text-base text-muted empty:hidden" role="status">
        {props.search?.status() ??
          (shown().length === 0 ? msg("palette.none", { query: query() }) : "")}
      </p>
    </Dialog>
  );
}
