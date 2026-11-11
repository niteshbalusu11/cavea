// The keyboard shortcut help (`?`, 07 §8), generated from the shortcut
// table, for this platform or the other one.
import { createSignal, For, Show } from "solid-js";
import { msg } from "../i18n/messages.ts";
import { Dialog } from "../ui/overlays.tsx";
import { helpSections, isMac } from "./shortcuts.ts";

export function Help(props: { open: boolean; onClose: () => void }) {
  const [mac, setMac] = createSignal(isMac());
  return (
    <Dialog
      open={props.open}
      title={msg("help.title")}
      heading
      onClose={props.onClose}
      class="max-w-3xl"
    >
      <div class="flex shrink-0 items-center justify-between px-5 pb-3">
        <p class="text-sm text-muted">{msg("help.body")}</p>
        <div
          role="radiogroup"
          aria-label={msg("help.platform")}
          class="flex rounded-md border border-line bg-surface p-0.5"
        >
          <For each={[false, true]}>
            {(m) => (
              <button
                type="button"
                role="radio"
                aria-checked={mac() === m}
                class="h-6 rounded-sm px-2 text-sm text-muted hover:text-fg aria-checked:bg-active aria-checked:text-fg"
                onClick={() => setMac(m)}
              >
                {msg(m ? "help.mac" : "help.pc")}
              </button>
            )}
          </For>
        </div>
      </div>
      <div class="min-h-0 overflow-y-auto border-t border-line px-5 py-3">
        <div class="columns-2 gap-8">
          <For each={helpSections(mac())}>
            {(section) => (
              <section class="mb-4 break-inside-avoid">
                <h3 class="mb-1 text-xs font-semibold tracking-wide text-faint uppercase">
                  {msg(`group.${section.group}`)}
                </h3>
                <dl>
                  <For each={section.rows}>
                    {(row) => (
                      <div
                        class="flex h-7 items-center justify-between gap-4 border-b border-line text-base last:border-b-0"
                        classList={{ "text-faint": row.pending !== undefined }}
                      >
                        <dt class="min-w-0 truncate">
                          {msg(row.label)}
                          <Show when={row.pending}>
                            <span class="ml-1.5 text-xs">{msg("help.pending")}</span>
                          </Show>
                        </dt>
                        <dd class="flex shrink-0 gap-1">
                          <For each={row.keys}>
                            {(k) => (
                              <kbd class="rounded-sm border border-line-strong bg-control px-1.5 font-sans text-sm leading-5 text-fg">
                                {k}
                              </kbd>
                            )}
                          </For>
                        </dd>
                      </div>
                    )}
                  </For>
                </dl>
              </section>
            )}
          </For>
        </div>
      </div>
    </Dialog>
  );
}
