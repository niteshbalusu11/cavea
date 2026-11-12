// The preview (07 §11, DSN-039): the buyer's renderer, `@cavea/renderer`
// as a customer uses it, on the chart compiled in the designer's module,
// at a phone's, a tablet's or a desktop's width, with sample states from a
// `customState` adapter playing a host. Loaded only when opened (a dynamic
// import), so the renderer and its WebAssembly cost the console nothing
// until then.
import type { Designer, ImageAsset, Session } from "@cavea/designer-core";
import { createSeatMap, customState, type AdapterContext, type SeatMap } from "@cavea/renderer";
import { consoleMap } from "@cavea/renderer/licence";
import { createSignal, For, onCleanup, onMount, Show, type JSX } from "solid-js";
import { msg } from "../i18n/messages.ts";
import { Dialog } from "../ui/overlays.tsx";

interface Device {
  id: "phone" | "tablet" | "desktop";
  width: number;
}
const DESKTOP: Device = { id: "desktop", width: 1280 };
const DEVICES: Device[] = [{ id: "phone", width: 390 }, { id: "tablet", width: 820 }, DESKTOP];
const STATES: { id: "empty" | "half" | "nearly"; sold: number }[] = [
  { id: "empty", sold: 0 },
  { id: "half", sold: 0.5 },
  { id: "nearly", sold: 0.95 },
];

export default function Preview(props: {
  designer: Designer;
  session: Session;
  /** Where background images are (the page's image host); buyers never
   * see a reference image, which the compiled chart does not hold. */
  images: ((hash: string) => ImageAsset | null) | undefined;
  onClose: () => void;
}) {
  const [device, setDevice] = createSignal<Device>(DESKTOP);
  const [sample, setSample] = createSignal(0);
  const [problem, setProblem] = createSignal<string | null>(null);
  const [ready, setReady] = createSignal(false);
  let host: HTMLDivElement | undefined;
  let map: SeatMap | null = null;
  let context: AdapterContext | null = null;
  let seq = 1;
  const controller = new AbortController();
  onCleanup(() => {
    controller.abort();
    map?.destroy();
  });

  // Compiled once, as the chart is when the preview opens.
  let chart: Uint8Array | null = null;
  const frame = (sold: number) =>
    chart === null ? null : props.designer.sampleState(chart, sold, seq++);
  const show = (index: number) => {
    setSample(index);
    const bytes = frame(STATES[index]?.sold ?? 0);
    if (bytes !== null) context?.applySnapshot({ kind: "binary", bytes });
  };

  /** A buyer's device opens the chart fitted: so does a width change,
   * once the map has seen its new size (two frames). */
  const pick = (d: Device) => {
    setDevice(d);
    requestAnimationFrame(() =>
      requestAnimationFrame(() => map?.zoomTo("fit", { animate: false })),
    );
  };

  onMount(() => {
    try {
      chart = props.session.preview();
    } catch (e) {
      setProblem(msg("preview.failed", { message: String(e) }));
      return;
    }
    const first = frame(STATES[sample()]?.sold ?? 0);
    if (chart === null || first === null || host === undefined) {
      setProblem(msg("preview.noChart"));
      return;
    }
    const dark = document.documentElement.dataset["theme"] === "dark";
    createSeatMap(host, {
      chart: { bytes: chart },
      // The designer's own preview: unmarked (the console shows the licence).
      state: consoleMap(
        customState({
          connect: (ctx) => {
            context = ctx;
            return Promise.resolve({ kind: "binary", bytes: first });
          },
          disconnect: () => {
            context = null;
          },
        }),
      ),
      theme: dark ? "dark" : "light",
      assets: {
        image: (hash) =>
          props.images?.(hash) ?? Promise.reject(new Error(`image ${hash}: not in this page`)),
      },
      signal: controller.signal,
      onReady: () => setReady(true),
    }).then(
      (m) => {
        map = m;
      },
      (e: unknown) => {
        if (!controller.signal.aborted) {
          setProblem(
            msg("preview.failed", { message: e instanceof Error ? e.message : String(e) }),
          );
        }
      },
    );
  });

  const radio =
    "rounded-sm px-2 h-6 text-sm text-muted hover:text-fg aria-checked:bg-active aria-checked:text-fg";

  return (
    <Dialog open title={msg("preview.title")} onClose={props.onClose} class="h-full max-w-none">
      <header class="flex shrink-0 flex-wrap items-center gap-4 border-b border-line px-4 py-2">
        <h2 class="text-lg font-semibold text-fg">{msg("preview.title")}</h2>
        <Choice label={msg("preview.device")}>
          <For each={DEVICES}>
            {(d) => (
              <button
                type="button"
                role="radio"
                aria-checked={device().id === d.id}
                class={radio}
                onClick={() => pick(d)}
              >
                {msg(`preview.${d.id}`)} · {d.width}
              </button>
            )}
          </For>
        </Choice>
        <Choice label={msg("preview.state")}>
          <For each={STATES}>
            {(s, i) => (
              <button
                type="button"
                role="radio"
                aria-checked={sample() === i()}
                class={radio}
                onClick={() => show(i())}
              >
                {msg(`preview.${s.id}`)}
              </button>
            )}
          </For>
        </Choice>
        <p class="text-sm text-faint">{msg("preview.note")}</p>
        <button
          type="button"
          class="ml-auto rounded-md px-2 h-7 text-base text-fg hover:bg-hover"
          onClick={() => props.onClose()}
        >
          {msg("action.close")}
        </button>
      </header>
      <div class="flex min-h-0 flex-1 justify-center overflow-auto bg-canvas p-4">
        <div
          class="relative h-full max-w-full overflow-hidden rounded-lg border border-line bg-surface"
          style={{ width: `${device().width}px` }}
        >
          <div ref={(e) => (host = e)} class="absolute inset-0" />
          <Show when={problem() ?? (ready() ? null : msg("preview.loading"))}>
            {(text) => (
              <p
                role={problem() === null ? "status" : "alert"}
                class="absolute inset-0 flex items-center justify-center p-6 text-center text-base text-muted"
              >
                {text()}
              </p>
            )}
          </Show>
        </div>
      </div>
    </Dialog>
  );
}

function Choice(props: { label: string; children: JSX.Element }) {
  return (
    <div
      role="radiogroup"
      aria-label={props.label}
      class="flex rounded-md border border-line bg-surface p-0.5"
    >
      {props.children}
    </div>
  );
}
