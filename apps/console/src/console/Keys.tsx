// The console's own keys (09 §8): Ctrl+K (Cmd+K) opens a command palette
// that goes to a section, changes the theme, signs out, and finds charts
// and events by name or key; `?` shows the shortcuts. Mounted by the
// console frame and the event page; the designer has its own of both
// (07 §8) and is never inside them. Neither key fires while a text field
// has focus, nor over a dialog or menu (`consoleKey`).
//
// Searching stays in the workspace selected when the palette opened and
// within what the role may read: charts only with `charts.read` (the
// charts list, read once per opening, matched here: the route has no
// search), events only with `events.read` (the server's own search,
// `GET /v1/events?q=`, sent once typing pauses, the request before
// cancelled).
import { useNavigate } from "@solidjs/router";
import { createSignal, onCleanup, type JSX } from "solid-js";
import { checkChartList, checkEventList } from "../api/check.ts";
import { call } from "../api/client.ts";
import type { Chart, Event } from "../api/generated.ts";
import { loadAll } from "../api/pages.ts";
import { asApiError } from "../api/query.ts";
import type { Command } from "../designer/commands.ts";
import { Help } from "../designer/Help.tsx";
import { match } from "../designer/match.ts";
import { Palette } from "../designer/Palette.tsx";
import { display, isMac, SHORTCUTS } from "../designer/shortcuts.ts";
import { msg, type MessageId } from "../i18n/messages.ts";
import { signOut, workspace } from "../session.ts";
import { setThemeChoice } from "../theme.ts";
import { describe, report } from "./errors.ts";
import { may, SECTIONS } from "./Frame.tsx";
import { consoleKey, eventsQuery, RESULTS, Search, type SearchState } from "./paletteSearch.ts";

/** The charts searched: up to this many pages of 100. */
const CHART_PAGES = 10;

const GO: Record<string, MessageId> = {
  "/charts": "palette.goCharts",
  "/events": "palette.goEvents",
  "/reports": "palette.goReports",
  "/developers": "palette.goDevelopers",
  "/settings": "palette.goSettings",
};

const chord = (id: string, mac: boolean): string | null => {
  const c = SHORTCUTS.find((s) => s.id === id)?.chords[0];
  return c === undefined ? null : display(c, mac);
};

/** The help's sections for the console. */
function sections(mac: boolean) {
  return [
    {
      group: "general",
      rows: [
        {
          id: "palette",
          label: "help.console.palette" as const,
          keys: [chord("palette", mac) ?? ""],
        },
        { id: "help", label: "help.console.help" as const, keys: ["?"] },
      ],
    },
    {
      group: "tables",
      rows: [
        { id: "table.move", label: "help.table.move" as const, keys: ["↑", "↓"] },
        { id: "table.ends", label: "help.table.ends" as const, keys: ["Home", "End"] },
        { id: "table.enter", label: "help.table.enter" as const, keys: ["Enter"] },
      ],
    },
    {
      group: "palette",
      rows: [
        { id: "palette.move", label: "help.palette.move" as const, keys: ["↑", "↓"] },
        { id: "palette.run", label: "help.palette.run" as const, keys: ["Enter"] },
        { id: "palette.close", label: "help.palette.close" as const, keys: ["Esc"] },
      ],
    },
  ];
}

interface Found {
  /** The workspace searched, so a switch never shows another's results. */
  workspace: string;
  charts: Chart[];
  events: Event[];
}

export function ConsoleKeys(): JSX.Element {
  const navigate = useNavigate();
  const mac = isMac();
  const [open, setOpen] = createSignal(false);
  const [help, setHelp] = createSignal(false);
  const [state, setState] = createSignal<SearchState<Found>>({ kind: "idle" });
  /** The last answer, kept on screen while the next search runs. */
  const [found, setFound] = createSignal<Found | null>(null);
  const reported = (s: SearchState<Found>) => {
    setState(s);
    if (s.kind === "done") setFound(s.value);
    if (s.kind === "idle") setFound(null);
  };
  /** The charts of the workspace the palette opened in, read once per
   * opening; closing the palette cancels the read. */
  let charts: { workspace: string; list: Promise<{ data: Chart[]; complete: boolean }> } | null =
    null;
  let opening = new AbortController();
  let chartsComplete = true;

  const chartsOf = (ws: string) => {
    if (charts?.workspace !== ws) {
      const signal = opening.signal;
      const list = loadAll(
        async (after) =>
          checkChartList(
            await call(
              "listCharts",
              { query: after === undefined ? { limit: 100 } : { limit: 100, after } },
              // Not a search's signal: the list serves every search of the opening.
              signal,
              { workspace: ws },
            ),
          ),
        CHART_PAGES,
      );
      charts = { workspace: ws, list };
      // A failure is the search's to say, and the next one tries again.
      list.catch(() => {
        if (charts?.list === list) charts = null;
      });
    }
    return charts.list;
  };

  const search = new Search<Found>(async (typed, signal) => {
    const ws = workspace()?.id;
    if (ws === undefined) return { workspace: "", charts: [], events: [] };
    const q = eventsQuery(typed);
    const [chartList, events] = await Promise.all([
      may("charts.read") ? chartsOf(ws) : { data: [], complete: true },
      may("events.read") && q !== null
        ? call("listEvents", { query: q }, signal, { workspace: ws }).then(
            (r) => checkEventList(r).data,
          )
        : Promise.resolve([]),
    ]);
    chartsComplete = chartList.complete;
    const scored = chartList.data
      .filter((c) => c.archived_at === null)
      .map((c) => ({
        c,
        m: Math.max(match(typed, c.name)?.score ?? -1, match(typed, c.key)?.score ?? -1),
      }))
      .filter((x) => x.m >= 0);
    // oxlint-disable-next-line unicorn/no-array-sort -- a new array of our own; `toSorted` is ES2023, past the lib (ES2022).
    scored.sort((a, b) => b.m - a.m);
    return { workspace: ws, charts: scored.slice(0, RESULTS).map((x) => x.c), events };
  }, reported);
  onCleanup(() => {
    search.dispose();
    opening.abort();
  });

  const close = () => {
    setOpen(false);
    search.set("");
    opening.abort();
    opening = new AbortController();
    charts = null;
  };

  const go = (path: string) => () => navigate(path);
  const results = (): Command[] => {
    const f = found();
    if (f === null || f.workspace !== workspace()?.id) return [];
    const s = { value: f };
    return [
      ...s.value.charts.map((c): Command => ({
        id: `chart:${c.key}`,
        label: c.name,
        detail: c.key,
        group: "charts",
        keys: null,
        reason: () => null,
        run: go(`/charts/${encodeURIComponent(c.key)}`),
      })),
      ...s.value.events.map((e): Command => ({
        id: `event:${e.key}`,
        label: e.name ?? e.key,
        detail: e.key,
        group: "events",
        keys: null,
        reason: () => null,
        run: go(`/events/${encodeURIComponent(e.key)}`),
      })),
    ];
  };

  const status = (): string | null => {
    const s = state();
    if (s.kind === "searching") return msg("palette.searching");
    if (s.kind === "failed") {
      return msg("palette.searchFailed", { message: describe(asApiError(s.error)) });
    }
    if (s.kind === "done" && !chartsComplete) {
      return msg("palette.chartsPartial", { n: CHART_PAGES * 100 });
    }
    return null;
  };

  const commands = (): Command[] => {
    const out: Command[] = [];
    const add = (id: string, label: string, group: Command["group"], run: () => void) =>
      out.push({ id, label, group, keys: null, reason: () => null, run });
    for (const s of SECTIONS) {
      const label = GO[s.path];
      if (label !== undefined && may(s.permission)) {
        add(`go${s.path}`, msg(label), "navigate", () => navigate(s.path));
      }
    }
    add("go/account", msg("palette.goAccount"), "navigate", () => navigate("/account"));
    add("designer", msg("palette.designer"), "navigate", () => navigate("/designer/local"));
    add("help", msg("palette.shortcuts"), "general", () => setHelp(true));
    for (const theme of ["system", "light", "dark"] as const) {
      add(`theme.${theme}`, msg(`command.theme.${theme}`), "theme", () => setThemeChoice(theme));
    }
    add("signOut", msg("palette.signOut"), "general", () => {
      signOut(new AbortController().signal).catch(report);
    });
    const palette = out.find((c) => c.id === "help");
    if (palette !== undefined) palette.keys = "?";
    return out;
  };

  const down = (e: KeyboardEvent) => {
    const which = consoleKey(e, mac);
    if (which === null || open() || help()) return;
    e.preventDefault();
    if (which === "palette") setOpen(true);
    else setHelp(true);
  };
  window.addEventListener("keydown", down);
  onCleanup(() => window.removeEventListener("keydown", down));

  return (
    <>
      <Palette
        open={open()}
        commands={commands}
        onClose={close}
        placeholder={msg("palette.consolePlaceholder")}
        search={{ onQuery: (q) => search.set(q), results, status }}
      />
      <Help
        open={help()}
        onClose={() => setHelp(false)}
        sections={sections}
        body={msg("help.consoleBody")}
      />
    </>
  );
}
