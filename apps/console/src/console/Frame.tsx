// The console's frame (09 §1): left navigation and a content column of at
// most 1,280 px (09 §8). Its screens arrive with later slices; each section
// says so rather than pretending.
import { A, useNavigate } from "@solidjs/router";
import { For, type ParentProps } from "solid-js";
import { msg, type MessageId } from "../i18n/messages.ts";
import { setThemeChoice, themeChoice, type ThemeChoice } from "../theme.ts";
import { Button } from "../ui/controls.tsx";
import { Icon, type IconName } from "../ui/Icon.tsx";
import { notify } from "../ui/overlays.tsx";
import { NewChart } from "../designer/NewChart.tsx";
import { emptyChart, handOff } from "../designer/source.ts";

export const SECTIONS: { path: string; label: MessageId; icon: IconName }[] = [
  { path: "/charts", label: "nav.charts", icon: "chart" },
  { path: "/events", label: "nav.events", icon: "calendar" },
  { path: "/reports", label: "nav.reports", icon: "report" },
  { path: "/developers", label: "nav.developers", icon: "code" },
  { path: "/settings", label: "nav.settings", icon: "settings" },
];
const THEMES: { value: ThemeChoice; icon: IconName }[] = [
  { value: "system", icon: "monitor" },
  { value: "light", icon: "sun" },
  { value: "dark", icon: "moon" },
];

export function Frame(props: ParentProps) {
  return (
    <div class="flex h-full">
      <nav
        aria-label={msg("nav.label")}
        class="flex w-56 shrink-0 flex-col border-r border-line bg-app p-2"
      >
        <div class="flex h-9 items-center gap-2 px-2 text-lg font-semibold text-fg">
          <span class="flex size-6 items-center justify-center rounded-md bg-accent-fill text-on-accent">
            <Icon name="section" class="size-3.5" />
          </span>
          {msg("app.name")}
        </div>
        <ul class="mt-3 space-y-0.5">
          <For each={SECTIONS}>
            {(s) => (
              <li>
                <A
                  href={s.path}
                  class="flex h-8 items-center gap-2 rounded-md px-2 text-base text-muted hover:bg-hover hover:text-fg"
                  activeClass="bg-active text-fg font-medium"
                >
                  <Icon name={s.icon} />
                  {msg(s.label)}
                </A>
              </li>
            )}
          </For>
        </ul>
        <div class="mt-auto space-y-2 px-2 pb-1">
          <div
            role="radiogroup"
            aria-label={msg("nav.theme")}
            class="flex rounded-md border border-line bg-surface p-0.5"
          >
            <For each={THEMES}>
              {(t) => (
                <button
                  type="button"
                  role="radio"
                  aria-checked={themeChoice() === t.value}
                  aria-label={msg(`theme.${t.value}`)}
                  title={msg(`theme.${t.value}`)}
                  class="flex h-6 flex-1 items-center justify-center rounded-sm text-muted hover:text-fg aria-checked:bg-active aria-checked:text-fg"
                  onClick={() => setThemeChoice(t.value)}
                >
                  <Icon name={t.icon} />
                </button>
              )}
            </For>
          </div>
        </div>
      </nav>
      <main class="min-w-0 flex-1 overflow-y-auto bg-surface">
        <div class="mx-auto max-w-[1280px] px-8 py-6">{props.children}</div>
      </main>
    </div>
  );
}

/** The charts screen. Its list needs the server; until then it is the
 * list's empty state (09 §3.1): the three starting points of 07 §16, each
 * handed to the designer. */
export function Charts() {
  const navigate = useNavigate();
  let picker: HTMLInputElement | undefined;
  const start = (text: Promise<string>) =>
    void text.then(
      (t) => {
        handOff(t);
        navigate("/designer/local");
      },
      () => notify(msg("new.loadFailed"), "error"),
    );
  return (
    <>
      <div class="flex items-center justify-between">
        <h1 class="text-xl font-semibold text-fg">{msg("charts.title")}</h1>
        <Button variant="primary" icon="plus" onClick={() => navigate("/designer/local")}>
          {msg("charts.new")}
        </Button>
      </div>
      <p class="mt-2 max-w-xl text-base text-muted">{msg("charts.body")}</p>
      <h2 class="mt-8 mb-3 text-lg font-semibold text-fg">{msg("charts.first")}</h2>
      <input
        ref={(e) => (picker = e)}
        type="file"
        accept=".json,application/json"
        class="hidden"
        onChange={(e) => {
          const file = e.currentTarget.files?.[0];
          e.currentTarget.value = "";
          if (file !== undefined) start(file.text());
        }}
      />
      <NewChart
        choices={{
          empty: () => start(Promise.resolve(emptyChart(msg("designer.untitled")))),
          template: (t) => start(t.load()),
          open: () => picker?.click(),
        }}
      />
    </>
  );
}

export function NotBuilt(props: { section: MessageId }) {
  return (
    <>
      <h1 class="text-xl font-semibold text-fg">
        {msg("notBuilt.title", { section: msg(props.section) })}
      </h1>
      <p class="mt-2 text-base text-muted">{msg("notBuilt.body")}</p>
    </>
  );
}

export function NotFound() {
  return (
    <>
      <h1 class="text-xl font-semibold text-fg">{msg("notFound.title")}</h1>
      <A href="/charts" class="mt-2 inline-block text-base text-accent underline">
        {msg("notFound.back")}
      </A>
    </>
  );
}
