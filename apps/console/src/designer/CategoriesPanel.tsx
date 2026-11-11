// The categories panel (07 §7): the list with colour, label, key,
// accessible marker and object count; the active category; add, edit,
// reorder, merge and delete with reassignment. The colour field checks the
// colour live with the compiler's W107 rules (`contrast`, `tooSimilar`).
import type { Category, CategoryView } from "@cavea/designer-core";
import { DropdownMenu } from "@kobalte/core/dropdown-menu";
import { createSignal, For, Show } from "solid-js";
import { formatNumber, msg } from "../i18n/messages.ts";
import { Button, IconButton } from "../ui/controls.tsx";
import { CheckField, SelectField, TextField } from "../ui/fields.tsx";
import { Icon } from "../ui/Icon.tsx";
import { afterMenu, Confirm, menuContent, menuItem, menuSeparator } from "../ui/overlays.tsx";
import type { Editor } from "./editor.ts";

/** Colours offered to a new category, in turn. */
const PALETTE = [
  "#2563EB",
  "#B45309",
  "#047857",
  "#7C3AED",
  "#BE123C",
  "#0E7490",
  "#4D7C0F",
  "#C026D3",
];
const HEX = /^#[0-9a-fA-F]{6}$/;
const ratio = (n: number) => formatNumber(Math.round(n * 10) / 10);

export function CategoriesPanel(props: { editor: Editor }) {
  const editor = props.editor;
  const list = () => editor.views.categories.categories;
  const [editing, setEditing] = createSignal<string | null>(null);
  const [removing, setRemoving] = createSignal<{ cat: CategoryView; merge: boolean } | null>(null);
  const [target, setTarget] = createSignal<string | null>(null);

  const update = (c: CategoryView, change: Partial<Category>): string | null => {
    const next: Category = {
      key: c.key,
      label: c.label,
      color: c.color,
      accessible: c.accessible,
      ...change,
    };
    const r = editor.attempt((s) => s.updateCategory(c.key, next));
    if ("error" in r) return r.error;
    if (change.key !== undefined) setEditing(change.key);
    return null;
  };
  const add = () => {
    const keys = new Set(list().map((c) => c.key));
    let n = list().length + 1;
    while (keys.has(`category-${n}`)) n++;
    const key = `category-${n}`;
    const color = PALETTE[list().length % PALETTE.length] ?? "#2563EB";
    if (
      editor.run((s) =>
        s.addCategory(null, { key, label: msg("categories.newLabel"), color, accessible: false }),
      )
    ) {
      setEditing(key);
    }
  };
  const openRemove = (cat: CategoryView, merge: boolean) => {
    setTarget(list().find((c) => c.key !== cat.key)?.key ?? null);
    setRemoving({ cat, merge });
  };

  return (
    <div class="flex min-h-0 flex-1 flex-col">
      <div class="flex h-9 shrink-0 items-center justify-between border-b border-line px-2">
        <span class="text-sm text-muted">{msg("categories.count", { n: list().length })}</span>
        <Button icon="plus" variant="ghost" class="h-6 px-2 text-sm" onClick={add}>
          {msg("categories.add")}
        </Button>
      </div>
      <Show when={list().length === 0}>
        <p class="p-3 text-sm text-faint">{msg("categories.empty")}</p>
      </Show>
      <ul class="min-h-0 flex-1 overflow-y-auto" aria-label={msg("categories.label")}>
        <For each={list()}>
          {(cat, i) => (
            <li class="border-b border-line">
              <div
                class="group flex h-9 items-center gap-2 pr-1 pl-2"
                classList={{ "bg-accent-soft": cat.active }}
              >
                <button
                  type="button"
                  aria-pressed={cat.active}
                  aria-label={`${cat.label}: ${cat.active ? msg("categories.active") : msg("categories.makeActive")}`}
                  class="flex min-w-0 flex-1 items-center gap-2 rounded-sm py-1 text-left"
                  onClick={() => editor.run((s) => s.setCategory(cat.key))}
                >
                  <span
                    class="size-3.5 shrink-0 rounded-full border border-line-strong"
                    style={{ background: cat.color }}
                  />
                  <span class="min-w-0 truncate text-base text-fg">{cat.label}</span>
                  <span class="min-w-0 truncate font-mono text-xs text-faint">{cat.key}</span>
                  <Show when={cat.accessible}>
                    <span class="text-accent" title={msg("categories.accessibleHint")}>
                      <Icon name="check" class="size-3.5" />
                      <span class="sr-only">{msg("categories.accessible")}</span>
                    </span>
                  </Show>
                  <Show when={cat.contrast?.weak === true || cat.similar.length > 0}>
                    <Icon name="warning" class="size-3.5 text-warning" />
                  </Show>
                </button>
                <span class="text-xs text-faint tabular-nums">
                  {msg("categories.objects", { n: cat.objects })}
                </span>
                <IconButton
                  icon={editing() === cat.key ? "chevronDown" : "chevronRight"}
                  label={msg("categories.edit", { label: cat.label })}
                  class="size-6"
                  onClick={() => setEditing(editing() === cat.key ? null : cat.key)}
                />
                <DropdownMenu gutter={4} placement="bottom-end">
                  <DropdownMenu.Trigger
                    class="inline-flex size-6 items-center justify-center rounded-md text-muted hover:bg-hover hover:text-fg"
                    aria-label={msg("categories.edit", { label: cat.label })}
                  >
                    <Icon name="more" />
                  </DropdownMenu.Trigger>
                  <DropdownMenu.Portal>
                    <DropdownMenu.Content class={menuContent}>
                      <DropdownMenu.Item
                        class={menuItem}
                        disabled={i() === 0}
                        onSelect={() => editor.run((s) => s.moveCategory(cat.key, i() - 1))}
                      >
                        {msg("categories.moveUp")}
                      </DropdownMenu.Item>
                      <DropdownMenu.Item
                        class={menuItem}
                        disabled={i() === list().length - 1}
                        onSelect={() => editor.run((s) => s.moveCategory(cat.key, i() + 1))}
                      >
                        {msg("categories.moveDown")}
                      </DropdownMenu.Item>
                      <DropdownMenu.Separator class={menuSeparator} />
                      <DropdownMenu.Item
                        class={menuItem}
                        disabled={list().length < 2}
                        onSelect={afterMenu(() => openRemove(cat, true))}
                      >
                        {msg("categories.merge")}
                      </DropdownMenu.Item>
                      <DropdownMenu.Item
                        class={`${menuItem} text-danger`}
                        onSelect={afterMenu(() => openRemove(cat, false))}
                      >
                        {msg("categories.delete")}
                      </DropdownMenu.Item>
                    </DropdownMenu.Content>
                  </DropdownMenu.Portal>
                </DropdownMenu>
              </div>
              <Show when={editing() === cat.key}>
                <Edit
                  editor={editor}
                  cat={cat}
                  others={list().filter((c) => c.key !== cat.key)}
                  update={(ch) => update(cat, ch)}
                />
              </Show>
            </li>
          )}
        </For>
      </ul>

      <Show when={removing()}>
        {(r) => {
          const inUse = () => r().cat.references > 0 || r().cat.objects > 0;
          const others = () => list().filter((c) => c.key !== r().cat.key);
          return (
            <Confirm
              open
              title={
                r().merge
                  ? msg("categories.mergeTitle", { label: r().cat.label })
                  : msg("categories.deleteTitle", { label: r().cat.label })
              }
              description={
                r().merge
                  ? msg("categories.mergeBody", { label: r().cat.label, n: r().cat.objects })
                  : inUse()
                    ? msg("categories.deleteBody", { label: r().cat.label, n: r().cat.objects })
                    : msg("categories.deleteUnused", { label: r().cat.label })
              }
              confirm={r().merge ? msg("action.merge") : msg("action.delete")}
              variant={r().merge ? "primary" : "danger"}
              onConfirm={() => {
                const into = target();
                const ok = r().merge
                  ? into !== null &&
                    editor.run((s) => s.mergeCategory(r().cat.key, into)) !== undefined
                  : editor.run((s) => s.deleteCategory(r().cat.key, inUse() ? into : null)) !==
                    undefined;
                if (ok && editing() === r().cat.key) setEditing(null);
                return ok;
              }}
              onClose={() => setRemoving(null)}
            >
              <Show when={r().merge || inUse()}>
                <div class="-mx-3 mt-3">
                  <SelectField
                    label={r().merge ? msg("categories.mergeInto") : msg("categories.replacement")}
                    options={others().map((c) => ({ value: c.key, label: c.label }))}
                    value={target()}
                    onChange={setTarget}
                  />
                </div>
              </Show>
            </Confirm>
          );
        }}
      </Show>
    </div>
  );
}

/** A category's fields, with the colour checked as it is typed or picked. */
function Edit(props: {
  editor: Editor;
  cat: CategoryView;
  others: CategoryView[];
  update: (c: Partial<Category>) => string | null;
}) {
  const [colour, setColour] = createSignal(props.cat.color);
  const valid = () => HEX.test(colour());
  const contrast = () => (valid() ? (props.editor.run((s) => s.contrast(colour())) ?? null) : null);
  const similar = () =>
    valid()
      ? props.others.filter(
          (o) => props.editor.run((s) => s.tooSimilar(colour(), o.color)) === true,
        )
      : [];
  const commitColour = (value: string) => {
    if (!HEX.test(value)) return msg("categories.badHex");
    return value.toUpperCase() === props.cat.color.toUpperCase()
      ? null
      : props.update({ color: value.toUpperCase() });
  };
  return (
    <div class="bg-app pt-1 pb-2">
      <TextField
        label={msg("categories.labelField")}
        value={props.cat.label}
        onCommit={(label) => props.update({ label })}
      />
      <TextField
        label={msg("categories.key")}
        value={props.cat.key}
        onCommit={(key) => props.update({ key })}
      />
      <div class="grid grid-cols-[88px_minmax(0,1fr)] items-center gap-x-2 px-3 py-0.5">
        <span class="text-sm text-muted">{msg("categories.colour")}</span>
        <div class="flex items-center gap-1.5">
          <input
            type="color"
            aria-label={msg("categories.colour")}
            class="h-6 w-8 shrink-0 cursor-pointer rounded-sm border border-control-line bg-control p-0.5"
            value={valid() ? colour().toLowerCase() : "#000000"}
            onInput={(e) => setColour(e.currentTarget.value.toUpperCase())}
            onChange={(e) => commitColour(e.currentTarget.value)}
          />
          <input
            aria-label={msg("categories.hex")}
            class="h-6 w-full min-w-0 rounded-sm border border-control-line bg-control px-1.5 font-mono text-sm text-fg aria-invalid:border-danger"
            value={colour()}
            aria-invalid={!valid()}
            onInput={(e) => setColour(e.currentTarget.value.trim())}
            onKeyDown={(e) => e.key === "Enter" && commitColour(colour())}
            onBlur={() => {
              if (commitColour(colour()) !== null) setColour(props.cat.color);
            }}
          />
        </div>
      </div>
      <div class="space-y-0.5 py-1 pr-3 pl-[108px] text-xs" aria-live="polite">
        <Show when={!valid()}>
          <p class="text-danger">{msg("categories.badHex")}</p>
        </Show>
        <Show when={contrast()?.weak === true && contrast()}>
          {(c) => (
            <p class="flex gap-1 text-warning">
              <Icon name="warning" class="mt-px size-3.5" />
              {msg("categories.weak", { light: ratio(c().light), dark: ratio(c().dark) })}
            </p>
          )}
        </Show>
        <Show when={similar().length > 0}>
          <p class="flex gap-1 text-warning">
            <Icon name="warning" class="mt-px size-3.5" />
            {msg("categories.similar", {
              others: similar()
                .map((o) => o.label)
                .join(", "),
            })}
          </p>
        </Show>
      </div>
      <CheckField
        label={msg("categories.accessible")}
        checked={props.cat.accessible}
        onChange={(accessible) => void props.update({ accessible })}
      />
    </div>
  );
}
